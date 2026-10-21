//! Background enrollment of content fingerprints for available Originals.
//! One bounded worker reads each Original once; scans pause it so candidate
//! hashing and enrollment never compete for storage.

use super::*;

/// Shared state the scanner thread and the enrollment worker coordinate on.
/// `stopped` ends the worker; `scan_running` pauses it while a scan owns
/// storage access.
#[derive(Default)]
pub(super) struct EnrollmentState {
    pub(super) stopped: bool,
    pub(super) scan_running: bool,
}

pub(super) fn enrollment_main(
    root: LibraryRoot,
    native_work: NativeWorkBudget,
    persistence: Persistence,
    enrollment: Arc<(Mutex<EnrollmentState>, Condvar)>,
    fingerprint_counts: Arc<Mutex<crate::persistence::FingerprintCounts>>,
) {
    let refresh_counts = |persistence: &Persistence| {
        if let Ok(counts) = persistence.fingerprint_counts_blocking() {
            *fingerprint_counts.lock().unwrap() = counts;
        }
    };
    refresh_counts(&persistence);
    let mut deferred: std::collections::HashMap<String, std::time::Instant> =
        std::collections::HashMap::new();
    loop {
        {
            let mut state = enrollment.0.lock().unwrap();
            if state.stopped {
                return;
            }
            while state.scan_running && !state.stopped {
                state = enrollment
                    .1
                    .wait_timeout(state, std::time::Duration::from_secs(5))
                    .unwrap()
                    .0;
            }
            if state.stopped {
                return;
            }
        }
        let target = match persistence.next_fingerprint_target_blocking() {
            Ok(target) => target,
            Err(_) => {
                thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
        };
        let Some(target) = target else {
            let state = enrollment.0.lock().unwrap();
            if state.stopped {
                return;
            }
            drop(state);
            let (lock, signal) = &*enrollment;
            let state = lock.lock().unwrap();
            if state.stopped {
                return;
            }
            let _unused = signal
                .wait_timeout(state, std::time::Duration::from_secs(5))
                .unwrap()
                .0;
            continue;
        };
        if let Some(deferred_at) = deferred.get(&target.original_id)
            && deferred_at.elapsed() < std::time::Duration::from_secs(60)
        {
            thread::sleep(std::time::Duration::from_millis(250));
            continue;
        }
        let relative = match crate::RelativeOriginalPath::parse(target.relative_path.clone()) {
            Ok(relative) => relative,
            Err(_) => {
                deferred.insert(target.original_id.clone(), std::time::Instant::now());
                continue;
            }
        };
        let permit = native_work.acquire();
        let digest = root
            .original(relative)
            .and_then(|capability| capability.digest_file());
        drop(permit);
        match digest {
            Ok(checked)
                if checked.facts.size == target.size
                    && checked.facts.mtime_ms == target.mtime_ms =>
            {
                let fingerprint = crate::OriginalFingerprint {
                    original_id: target.original_id.clone(),
                    digest: checked.digest,
                    size: checked.facts.size,
                    mtime_ms: checked.facts.mtime_ms,
                };
                if persistence.store_fingerprint_blocking(fingerprint).is_ok() {
                    deferred.remove(&target.original_id);
                    refresh_counts(&persistence);
                } else {
                    deferred.insert(target.original_id.clone(), std::time::Instant::now());
                }
            }
            _ => {
                deferred.insert(target.original_id.clone(), std::time::Instant::now());
            }
        }
    }
}
