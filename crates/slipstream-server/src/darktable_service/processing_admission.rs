//! Shared heavy-work admission for darktable and composable processing.
//!
//! The host state directory owns one `.processing-admission` slot. Blocking
//! commands and cancellable jobs use separate entry points so each caller
//! keeps its waiting and error semantics.
use super::{Result, ServiceError};
use photo_files::{Directory, FileLease, LeaseMode};
use std::{
    io,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

const DIRECTORY_NAME: &str = ".processing-admission";
const LEASE_TIMEOUT: Duration = Duration::from_secs(900);
const RETRY_INTERVAL: Duration = Duration::from_millis(100);

fn directory(state_directory: &Path) -> io::Result<Directory> {
    Directory::create(state_directory.join(DIRECTORY_NAME))
}

/// Acquires the processing lease using the file lease's ordinary bounded wait.
///
/// Independent commands use this path because they have no cancellation state
/// to observe while waiting for the shared heavy-work admission.
pub(crate) fn acquire_blocking(state_directory: &Path) -> Result<FileLease, String> {
    directory(state_directory)
        .and_then(|directory| directory.lease(LeaseMode::Exclusive, LEASE_TIMEOUT))
        .map_err(|error| error.to_string())
}

/// Acquires the processing lease while retaining job cancellation and shutdown
/// checks between short lease attempts.
pub(crate) fn acquire_cancellable(
    state_directory: &Path,
    cancel: &AtomicBool,
    stopping: &AtomicBool,
) -> Result<FileLease> {
    let directory = directory(state_directory).map_err(ServiceError::io)?;
    let start = std::time::Instant::now();
    loop {
        if cancel.load(Ordering::Acquire) || stopping.load(Ordering::Acquire) {
            return Err(ServiceError::new(
                "cancelled",
                "Cancelled before execution",
                409,
            ));
        }
        match directory.lease(LeaseMode::Exclusive, RETRY_INTERVAL) {
            Ok(lease) => return Ok(lease),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) && start.elapsed() < LEASE_TIMEOUT => {}
            Err(error) => return Err(ServiceError::io(error)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        sync::{Arc, atomic::AtomicBool},
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn a_cancelled_waiter_stops_before_the_lease_timeout() {
        let root = std::env::temp_dir().join(format!(
            "slipstream-processing-admission-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _host = Directory::create(&root).unwrap();
        let held = directory(&root)
            .unwrap()
            .lease(LeaseMode::Exclusive, Duration::from_secs(1))
            .unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let stopping = Arc::new(AtomicBool::new(false));
        let waiter_cancelled = Arc::clone(&cancelled);
        let waiter_stopping = Arc::clone(&stopping);
        let waiter_root = root.clone();
        let started = Instant::now();
        let waiter = thread::spawn(move || {
            acquire_cancellable(&waiter_root, &waiter_cancelled, &waiter_stopping)
        });

        thread::sleep(Duration::from_millis(20));
        cancelled.store(true, Ordering::Release);
        let result = waiter.join().unwrap();
        assert!(started.elapsed() < Duration::from_millis(500));
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("cancelled waiter acquired the lease"),
        };
        assert_eq!(error.value["error"]["code"], "cancelled");
        drop(held);
        fs::remove_dir_all(root).unwrap();
    }
}
