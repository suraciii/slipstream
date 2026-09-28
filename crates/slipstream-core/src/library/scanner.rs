use super::*;

pub(super) type ScanReply = tokio::sync::oneshot::Sender<Result<Arc<ScanSnapshot>, LibraryError>>;

pub(super) enum ScanCommand {
    Scan,
    Stop,
}

pub(super) struct ScanState {
    pub(super) open: bool,
    pub(super) in_flight: Option<Vec<ScanReply>>,
}
pub(super) struct Scanner {
    pub(super) sender: std::sync::mpsc::SyncSender<ScanCommand>,
    pub(super) state: Arc<(Mutex<ScanState>, Condvar)>,
    pub(super) join: Mutex<Option<JoinHandle<()>>>,
}

/// Milliseconds since the epoch of the current instant, so Loading Status
/// can report the last observed progress advance without inventing a rate.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| u64::try_from(value.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

/// Marks one truthful progress advance.
fn touch(progress: &mut ScanProgress) {
    progress.updated_ms = now_ms();
}

pub(super) fn inspect_capture_facts(
    root: &LibraryRoot,
    native_work: &NativeWorkBudget,
    originals: &mut [crate::DiscoveredOriginal],
    previous: &[crate::OriginalRecord],
    progress: &Mutex<ScanProgress>,
) {
    let previous = previous
        .iter()
        .map(|original| (original.relative_path.as_str(), original))
        .collect::<std::collections::HashMap<_, _>>();
    for (index, original) in originals.iter_mut().enumerate() {
        {
            let mut progress = progress.lock().unwrap();
            progress.inspected = u64::try_from(index + 1).unwrap_or(u64::MAX);
            touch(&mut progress);
        }
        let prior = previous.get(original.path.as_str()).copied();
        if original.error_category.is_some() {
            if let Some(prior) = prior {
                original.capture = prior.capture.clone();
            }
            continue;
        }
        let revision = capture_source_revision(original.path.as_str(), original.facts);
        let Ok(revision) = revision else {
            original.capture = CaptureFact::failed(None);
            continue;
        };
        if let Some(prior) = prior
            && prior.capture.is_reusable_for(&revision)
        {
            original.capture = prior.capture.clone();
            continue;
        }
        original.capture = match root.original(original.path.clone()) {
            Ok(capability) => {
                let _permit = native_work.acquire();
                match crate::capture::inspect_capture(&capability, original.kind, original.facts) {
                    Ok(capture) => capture,
                    Err(crate::CaptureInspectionError::Confinement(
                        crate::confinement::ConfinementError::Changed,
                    )) => match crate::capture::inspect_capture_fresh(&capability, original.kind) {
                        Ok(observation) => {
                            original.facts = observation.facts;
                            observation.capture
                        }
                        Err(_) => CaptureFact::failed(None),
                    },
                    Err(_) => CaptureFact::failed(Some(revision)),
                }
            }
            Err(_) => CaptureFact::failed(Some(revision)),
        };
    }
}

/// Shared handles the scanner thread owns for the lifetime of the Library.
pub(super) struct ScannerShared {
    pub(super) state: Arc<(Mutex<ScanState>, Condvar)>,
    pub(super) progress: Arc<Mutex<ScanProgress>>,
    pub(super) outcome: Arc<Mutex<Option<ScanOutcome>>>,
    pub(super) enrollment: Arc<(Mutex<EnrollmentState>, Condvar)>,
    pub(super) fingerprint_counts: Arc<Mutex<crate::persistence::FingerprintCounts>>,
}
pub(super) fn scanner_main(
    root: LibraryRoot,
    native_work: NativeWorkBudget,
    persistence: Persistence,
    limits: ScanLimits,
    receiver: std::sync::mpsc::Receiver<ScanCommand>,
    shared: ScannerShared,
) {
    let ScannerShared {
        state,
        progress,
        outcome,
        enrollment,
        fingerprint_counts,
    } = shared;
    while let Ok(command) = receiver.recv() {
        match command {
            ScanCommand::Stop => break,
            ScanCommand::Scan => {
                {
                    enrollment.0.lock().unwrap().scan_running = true;
                    let mut progress = progress.lock().unwrap();
                    *progress = ScanProgress {
                        phase: ScanPhase::Discovering,
                        ..ScanProgress::default()
                    };
                    touch(&mut progress);
                }
                #[cfg(test)]
                scanner_test_hook(&root);
                let discovered = AtomicU64::new(0);
                let result = root
                    .scan_with_progress(limits, &discovered)
                    .map_err(LibraryError::from)
                    .and_then(|mut result: ScanResult| {
                        let previous = persistence
                            .snapshot_blocking()
                            .map_err(LibraryError::from)?;
                        {
                            let mut progress = progress.lock().unwrap();
                            progress.discovered =
                                discovered.load(std::sync::atomic::Ordering::Relaxed);
                            progress.inspected = 0;
                            progress.inspect_total =
                                Some(u64::try_from(result.originals.len()).unwrap_or(u64::MAX));
                            progress.phase = ScanPhase::Inspecting;
                            touch(&mut progress);
                        }
                        inspect_capture_facts(
                            &root,
                            &native_work,
                            &mut result.originals,
                            &previous.originals,
                            &progress,
                        );
                        let mut evidence_ids =
                            crate::recovery::evidence_original_ids(&result.originals, &previous);
                        // A permanently deleted Original keeps no relocation
                        // or identity evidence: its bytes are gone, and a file
                        // at its reviewed Location is a new Original.
                        let deleted_originals = persistence
                            .permanently_deleted_original_ids_blocking()
                            .map_err(LibraryError::from)?
                            .into_iter()
                            .collect::<std::collections::HashSet<_>>();
                        evidence_ids.retain(|id| !deleted_originals.contains(id));
                        let fingerprints = persistence
                            .recovery_facts_blocking(evidence_ids)
                            .map_err(LibraryError::from)?;
                        let mut recovery_progress = crate::recovery::RecoveryProgress::default();
                        {
                            let mut progress = progress.lock().unwrap();
                            progress.phase = ScanPhase::Recovering;
                            touch(&mut progress);
                        }
                        let recovery_report = {
                            let progress = Arc::clone(&progress);
                            move |hashed: u64, hash_total: u64| {
                                let mut progress = progress.lock().unwrap();
                                progress.hashed = hashed;
                                progress.hash_total = Some(hash_total);
                                touch(&mut progress);
                            }
                        };
                        let recovery = crate::recovery::plan_recovery(
                            crate::recovery::RecoveryContext {
                                root: &root,
                                native_work: &native_work,
                            },
                            &result.originals,
                            &previous,
                            &fingerprints,
                            &deleted_originals,
                            &mut recovery_progress,
                            &recovery_report,
                        );
                        {
                            let mut progress = progress.lock().unwrap();
                            progress.hashed = recovery_progress.hashed;
                            progress.hash_total = Some(recovery_progress.hash_total);
                        }
                        {
                            let mut progress = progress.lock().unwrap();
                            progress.phase = ScanPhase::Applying;
                            touch(&mut progress);
                        }
                        let applied = persistence
                            .apply_scan_recovered_blocking(
                                result.originals,
                                result.errors,
                                recovery,
                            )
                            .map_err(LibraryError::from)?;
                        let unavailable = applied
                            .snapshot
                            .photos
                            .iter()
                            .filter(|photo| !photo.available)
                            .count();
                        *outcome.lock().unwrap() = Some(ScanOutcome {
                            relocated_originals: applied.relocated_originals,
                            fingerprinted_originals: applied.fingerprinted_originals,
                            unavailable_photos: unavailable,
                        });
                        if let Ok(counts) = persistence.fingerprint_counts_blocking() {
                            *fingerprint_counts.lock().unwrap() = counts;
                        }
                        Ok(applied.snapshot)
                    })
                    .map(Arc::new);
                {
                    let mut enrollment_state = enrollment.0.lock().unwrap();
                    enrollment_state.scan_running = false;
                }
                enrollment.1.notify_all();
                {
                    let mut progress = progress.lock().unwrap();
                    progress.phase = ScanPhase::Idle;
                    touch(&mut progress);
                }
                let (lock, signal) = &*state;
                let mut guard = lock.lock().unwrap();
                if let Some(waiters) = guard.in_flight.take() {
                    for waiter in waiters {
                        let _ = waiter.send(result.clone());
                    }
                }
                signal.notify_all();
            }
        }
    }
    let (lock, signal) = &*state;
    let mut guard = lock.lock().unwrap();
    if let Some(waiters) = guard.in_flight.take() {
        for waiter in waiters {
            let _ = waiter.send(Err(LibraryError::ScannerStopped));
        }
    }
    signal.notify_all();
}
