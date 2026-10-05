//! One application-owned native Photo Development executor.
//!
//! The executor deliberately owns no Library or HTTP state. It serializes fresh
//! darktable-mcp children, gives each child a private scratch directory, and
//! cancels the child through the shared request token before releasing the
//! processing slot.

use crate::ProcessingConfig;
use crate::config::FilmConfig;
use slipstream_processing::local_film;
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

    /// The verified standalone SpektraFilm runtime of this deployment, or
    /// `None` while the module is disabled, missing, or unverified.
    pub(crate) fn film(&self) -> Option<&FilmConfig> {
        self.config.film.as_ref().filter(|film| film.ready())
    }

    /// Truthful availability of the standalone SpektraFilm peer. It never
    /// derives from the darktable stage — a `darktable-disabled`
    /// deployment still runs film work — and stays unavailable while the
    /// shared processing slot is shutting down.
    pub(crate) fn film_available(&self) -> Result<(), String> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err("Photo Development is shutting down".to_owned());
        }
        match self.film() {
            Some(_) => Ok(()),
            None => Err("standalone SpektraFilm runtime is unavailable".to_owned()),
        }
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

    /// Runs one serialized local development of a selected composable
    /// step's complete module-owned parameter snapshot through the same
    /// fresh-engine, private-scratch, cancel-and-deadline boundary as
    /// [`develop`](Self::develop).
    pub(crate) async fn develop_selected_step(
        &self,
        input: PathBuf,
        output: PathBuf,
        parameters: slipstream_processing::modules::Parameters,
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
                local_photo::develop_selected_step(
                    &engine,
                    &metadata,
                    &profile,
                    &work,
                    &input,
                    &output,
                    &parameters,
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

    /// Runs one engine-owned automatic adjustment through the shared
    /// serialized child boundary. The caller supplies the complete selected
    /// step parameters and the original request instruction.
    pub(crate) async fn auto_parameters(
        &self,
        input: PathBuf,
        parameters: slipstream_processing::modules::Parameters,
        operation: String,
        multi_priority: i64,
        instruction: serde_json::Value,
        cancellation: Arc<AtomicBool>,
    ) -> Result<serde_json::Value, String> {
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
        let active = Arc::clone(&self.active);
        let running = Arc::clone(&self.running);
        let idle = Arc::clone(&self.idle);
        tokio::task::spawn_blocking(move || {
            let _serial = serial;
            let result = (|| {
                fs::create_dir(&work)?;
                let mut permissions = fs::metadata(&work)?.permissions();
                permissions.set_mode(0o700);
                fs::set_permissions(&work, permissions)?;
                local_photo::auto_parameters(
                    &engine,
                    &metadata,
                    &work,
                    &input,
                    &parameters,
                    &operation,
                    multi_priority,
                    &instruction,
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

    /// Runs one bounded selected-step Preview through the same serialized
    /// native child boundary as Export, without creating a full-resolution
    /// handoff or touching the Original.
    pub(crate) async fn render_selected_step(
        &self,
        input: PathBuf,
        output: PathBuf,
        parameters: slipstream_processing::modules::Parameters,
        cancellation: Arc<AtomicBool>,
    ) -> Result<slipstream_processing::local_preview::PreviewIdentity, String> {
        if !self.available() {
            return Err("Photo Development bundle is unavailable".to_owned());
        }
        if self.shutting_down.load(Ordering::Acquire) {
            return Err("Photo Development is shutting down".to_owned());
        }
        let serial = Arc::clone(&self.serial).lock_owned().await;
        if cancellation.load(Ordering::Acquire) {
            return Err("Photo Preview was cancelled".to_owned());
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
        let active = Arc::clone(&self.active);
        let running = Arc::clone(&self.running);
        let idle = Arc::clone(&self.idle);
        tokio::task::spawn_blocking(move || {
            let _serial = serial;
            let result = (|| {
                fs::create_dir(&work)?;
                let mut permissions = fs::metadata(&work)?.permissions();
                permissions.set_mode(0o700);
                fs::set_permissions(&work, permissions)?;
                slipstream_processing::local_preview::render_selected_step(
                    &engine,
                    &metadata,
                    &work,
                    &input,
                    &output,
                    &parameters,
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
        .map_err(|error| format!("Photo Preview task failed: {error}"))?
    }

    /// Runs one serialized standalone SpektraFilm Export of an
    /// artifact-bound step's complete parameter snapshot through the same
    /// shared slot, fresh-engine, private-scratch, cancel-and-deadline
    /// boundary as darktable development. The input is the staged retained
    /// Development TIFF handoff; the output is the validated Finished JPEG.
    pub(crate) async fn develop_film_selected_step(
        &self,
        input: PathBuf,
        output: PathBuf,
        parameters: slipstream_processing::modules::Parameters,
        cancellation: Arc<AtomicBool>,
    ) -> Result<OutputIdentity, String> {
        self.film_available()?;
        let film = self.film().cloned().expect("film runtime verified");
        let film_profile = film.film_profile.clone();
        let print_profile = film.print_profile.clone();
        self.dispatch_film(
            film,
            cancellation,
            move |binary, data_root, work, cancellation| {
                local_film::develop_selected_step(
                    binary,
                    data_root,
                    &film_profile,
                    &print_profile,
                    work,
                    &input,
                    &output,
                    &parameters,
                    cancellation,
                    Duration::from_millis(
                        slipstream_processing::modules::SPEKTRAFILM_DEADLINE_MILLIS,
                    ),
                )
            },
        )
        .await
    }

    /// Runs one bounded standalone SpektraFilm Preview at the disclosed
    /// Preview long edge through the same serialized boundary; the
    /// simulation never receives a full-resolution frame and no handoff
    /// artifact is produced.
    pub(crate) async fn render_film_selected_step(
        &self,
        input: PathBuf,
        output: PathBuf,
        parameters: slipstream_processing::modules::Parameters,
        cancellation: Arc<AtomicBool>,
    ) -> Result<slipstream_processing::local_preview::PreviewIdentity, String> {
        self.film_available()?;
        let film = self.film().cloned().expect("film runtime verified");
        let film_profile = film.film_profile.clone();
        let print_profile = film.print_profile.clone();
        self.dispatch_film(
            film,
            cancellation,
            move |binary, data_root, work, cancellation| {
                local_film::render_selected_step(
                    binary,
                    data_root,
                    &film_profile,
                    &print_profile,
                    work,
                    &input,
                    &output,
                    &parameters,
                    slipstream_processing::local_preview::PREVIEW_LONG_EDGE,
                    cancellation,
                    Duration::from_millis(
                        slipstream_processing::modules::SPEKTRAFILM_DEADLINE_MILLIS,
                    ),
                )
            },
        )
        .await
    }

    /// The shared serialized film dispatch: one fresh private scratch
    /// directory, one supervised pinned-runtime attempt of `run`, and
    /// cleanup that releases the slot only after the child is gone.
    async fn dispatch_film<T, F>(
        &self,
        film: FilmConfig,
        cancellation: Arc<AtomicBool>,
        run: F,
    ) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(&Path, &Path, &Path, Arc<AtomicBool>) -> std::io::Result<T> + Send + 'static,
    {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err("Photo Development is shutting down".to_owned());
        }
        let serial = Arc::clone(&self.serial).lock_owned().await;
        if cancellation.load(Ordering::Acquire) {
            return Err("standalone Film execution was cancelled".to_owned());
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
        let binary = film.binary;
        let data_root = film.data_root;
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
                run(&binary, &data_root, &work, cancellation)
            })()
            .map_err(|error: std::io::Error| error.to_string());
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
        .map_err(|error| format!("standalone Film task failed: {error}"))?
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
            film: None,
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

    #[test]
    fn film_availability_is_independent_of_the_darktable_stage() {
        let base = std::env::temp_dir().join(format!("film-only-{}", std::process::id()));
        let state = base.join("state");
        std::fs::create_dir_all(&state).unwrap();
        let config = ProcessingConfig {
            policy_sha256: "a".repeat(64),
            bundle_sha256: String::new(),
            bundle_root: base.join("bundle"),
            film: Some(crate::config::FilmConfig {
                bundle_sha256: "c".repeat(64),
                bundle_root: base.join("film"),
                binary: base.join("film/spektrafilm"),
                data_root: base.join("film/data"),
                parameter_default: Default::default(),
                film_profile: "kodak_portra_400".to_owned(),
                print_profile: "kodak_portra_endura".to_owned(),
                failure: None,
            }),
            failure: Some("darktable-disabled"),
        };
        let executor = PhotoExecutor::open(&config, &state).unwrap();
        // The darktable stage is disabled, yet the film peer stays
        // admissible on its own.
        assert!(!executor.available());
        assert!(executor.film_available().is_ok());
        drop(executor);
        std::fs::remove_dir_all(base).unwrap();
    }
}
