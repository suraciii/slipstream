//! One application-owned native Photo Development executor.
//!
//! The executor deliberately owns no Library or HTTP state. It serializes fresh
//! darktable-mcp children, gives each child a private scratch directory, and
//! cancels the child through the shared request token before releasing the
//! processing slot.

use crate::ProcessingConfig;
use slipstream_processing::local_photo::{self, OutputIdentity};
use std::{
    collections::HashMap,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{Mutex as AsyncMutex, Notify};

const ENGINE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Clone)]
pub(crate) struct PhotoExecutor {
    config: ProcessingConfig,
    root: PathBuf,
    serial: Arc<AsyncMutex<()>>,
    active: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>,
    next_id: Arc<AtomicU64>,
    running: Arc<AtomicUsize>,
    idle: Arc<Notify>,
    shutting_down: Arc<AtomicBool>,
}

impl PhotoExecutor {
    pub(crate) fn open(config: &ProcessingConfig, state_directory: &Path) -> Result<Self, String> {
        let root = state_directory.join("photo-processing");
        match fs::symlink_metadata(&root) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err("Photo processing workspace is not a directory".to_owned());
            }
            Ok(_) => fs::remove_dir_all(&root)
                .map_err(|error| format!("Photo processing workspace is unavailable: {error}"))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "Photo processing workspace is unavailable: {error}"
                ));
            }
        }
        fs::create_dir_all(&root)
            .map_err(|error| format!("Photo processing workspace is unavailable: {error}"))?;
        let mut permissions = fs::metadata(&root)
            .map_err(|error| format!("Photo processing workspace is unavailable: {error}"))?
            .permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&root, permissions)
            .map_err(|error| format!("Photo processing workspace is unavailable: {error}"))?;
        Ok(Self {
            config: config.clone(),
            root,
            serial: Arc::new(AsyncMutex::new(())),
            active: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(0)),
            running: Arc::new(AtomicUsize::new(0)),
            idle: Arc::new(Notify::new()),
            shutting_down: Arc::new(AtomicBool::new(false)),
        })
    }

    pub(crate) fn available(&self) -> bool {
        self.config.failure.is_none()
    }

    pub(crate) fn bundle_sha256(&self) -> &str {
        &self.config.bundle_sha256
    }

    pub(crate) async fn develop(
        &self,
        input: PathBuf,
        output: PathBuf,
        exposure_milli_ev: i64,
        cancellation: Arc<AtomicBool>,
    ) -> Result<OutputIdentity, String> {
        if !self.available() {
            return Err("Photo Development bundle is unavailable".to_owned());
        }
        if self.shutting_down.load(Ordering::Acquire) {
            return Err("Photo Development is shutting down".to_owned());
        }
        let serial = Arc::clone(&self.serial).lock_owned().await;
        if cancellation.load(Ordering::Acquire) {
            return Err("Photo Development was cancelled".to_owned());
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        {
            let mut active = self
                .active
                .lock()
                .expect("Photo executor active set is not poisoned");
            if self.shutting_down.load(Ordering::Acquire) {
                return Err("Photo Development is shutting down".to_owned());
            }
            self.running.fetch_add(1, Ordering::AcqRel);
            active.insert(id, Arc::clone(&cancellation));
        }
        let work = self.root.join(format!("attempt-{id}"));
        let engine = self.config.bundle_root.join("darktable/bin/darktable-mcp");
        let metadata = self.config.bundle_root.join("engine-metadata.json");
        let profile = self.config.bundle_root.join("icc/LargeRGB-elle-V2-g10.icc");
        let active = Arc::clone(&self.active);
        let running = Arc::clone(&self.running);
        let idle = Arc::clone(&self.idle);
        tokio::task::spawn_blocking(move || {
            // A dropped async waiter cannot release the slot while its child
            // still runs; blocking execution owns it through cleanup.
            let _serial = serial;
            let result = (|| {
                fs::create_dir(&work)?;
                let mut permissions = fs::metadata(&work)?.permissions();
                permissions.set_mode(0o700);
                fs::set_permissions(&work, permissions)?;
                local_photo::develop(
                    &engine,
                    &metadata,
                    &profile,
                    &work,
                    &input,
                    &output,
                    exposure_milli_ev,
                    cancellation,
                    ENGINE_TIMEOUT,
                )
            })()
            .map_err(|error| error.to_string());
            let _ = fs::remove_dir_all(&work);
            active
                .lock()
                .expect("Photo executor active set is not poisoned")
                .remove(&id);
            if running.fetch_sub(1, Ordering::AcqRel) == 1 {
                idle.notify_waiters();
            }
            result
        })
        .await
        .map_err(|error| format!("Photo Development task failed: {error}"))?
    }

    pub(crate) async fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::Release);
        for token in self
            .active
            .lock()
            .expect("Photo executor active set is not poisoned")
            .values()
        {
            token.store(true, Ordering::Release);
        }
        // Register for the idle wakeup before re-checking the running count,
        // so a run that finishes between the check and the wait can never
        // lose its notification and stall the shutdown.
        let mut notified = std::pin::pin!(self.idle.notified());
        loop {
            notified.as_mut().enable();
            if self.running.load(Ordering::Acquire) == 0 {
                break;
            }
            notified.as_mut().await;
            notified.set(self.idle.notified());
        }
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU64;

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn open_replaces_stale_attempts_and_hardens_workspace() {
        let base = std::env::temp_dir().join(format!(
            "slipstream-photo-executor-{}-{}",
            std::process::id(),
            NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let state = base.join("state");
        let stale = state.join("photo-processing/attempt-0");
        fs::create_dir_all(&stale).unwrap();
        fs::write(stale.join("stale"), b"old").unwrap();

        let config = ProcessingConfig {
            policy_sha256: "a".repeat(64),
            bundle_sha256: "b".repeat(64),
            bundle_root: base.join("bundle"),
            failure: None,
        };
        let executor = PhotoExecutor::open(&config, &state).unwrap();
        let mode = fs::metadata(state.join("photo-processing"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;

        assert!(!stale.exists());
        assert_eq!(mode, 0o700);
        drop(executor);
        fs::remove_dir_all(base).unwrap();
    }
}
