//! Service-side Export orchestration: bounded staging, local Photo
//! Development admission, validated publication, restart reconciliation,
//! and retention sweeping.
//!
//! The durable Export record lives in the serialized persistence owner; this
//! module owns the heavy work between acceptance and settlement. It never
//! starts a replacement attempt against a possibly live one and never
//! publishes an unvalidated artifact.

use crate::config::ProcessingConfig;
use crate::photo_executor::PhotoExecutor;
use slipstream_core::{
    ExportAttempt, ExportRecord, ExportSettlement, ExportSnapshot, ExportState, ExportTarget,
    ExportWorkspace, Library, LibraryRoot, OriginalCapability, OriginalKind, RelativeOriginalPath,
    StagedOriginal,
};
use slipstream_processing::local_photo::OutputIdentity;
#[path = "export_manager_composable.rs"]
mod composable;
pub(crate) use composable::{ProcessingExportExecution, ProcessingPreviewExecution};
use slipstream_processing::photo_profile;
#[path = "output_validation.rs"]
pub(crate) mod output_validation;
pub(crate) use output_validation::DevelopmentTiffFacts;
#[cfg(test)]
use output_validation::validate_development_tiff;
use output_validation::{
    OUTPUT_VALIDATION_FAILED, open_read_only, validate_output, verify_developed_output,
};
use std::{
    collections::HashMap,
    fs,
    future::Future,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

/// Resolves the published Library Location of one Photo. The closure keeps
/// ExportManager decoupled from the Application's published snapshot.
pub(crate) type SourceLocationResolver =
    Arc<dyn Fn(&str) -> Option<RelativeOriginalPath> + Send + Sync>;

/// The actionable reason an interrupted attempt settles failed with after a
/// restart that owns no recoverable publication.
const INTERRUPTED_ATTEMPT: &str = "the attempt was interrupted by a restart";

/// The shared local executor plus the filesystem seams one attempt owns.
pub(crate) struct ExportManager {
    library: Arc<Library>,
    library_root: PathBuf,
    resolver: SourceLocationResolver,
    workspace: ExportWorkspace,
    /// The one local Photo Development executor every heavy attempt runs
    /// through. It owns the fresh engine child, its private scratch, and
    /// its cleanup; this manager owns admission and settlement.
    executor: Arc<PhotoExecutor>,
    /// The random identity minted at server startup that names every
    /// attempt this process runs. A restart mints a fresh one, so a
    /// durable attempt from another process is never mistaken for live
    /// work of this one.
    incarnation: String,
    /// Orders the attempts of this server lifetime.
    next_sequence: AtomicU64,
    /// The cancellation tokens of the attempts this process is running,
    /// keyed by Export identity. HTTP cancellation settles the durable
    /// record first and then flips the live token so the engine child and
    /// its scratch are torn down before the slot is released; a queued
    /// Export owns no token and settles through its terminal state alone.
    running: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Finite retained-output allowance configured for this deployment.
    allowance: u64,
    /// The initial scheduler admits at most one heavy processing job at a
    /// time per instance; the slot also serializes restart reconciliation.
    admission: Arc<tokio::sync::Mutex<()>>,
    /// Publications not yet confirmed by the serialized Library owner.
    publications: Arc<Mutex<HashMap<String, ProcessingPublicationOwner>>>,
    /// Stops new lifecycle tasks before shutdown drains the existing ones.
    tasks: Mutex<TaskState>,
    shutting_down: AtomicBool,
    /// How often a live download stream renews its lease liveness anchor.
    /// Periodic retention work is cancelled before the Library closes.
    sweep_task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    lease_renewal_interval_millis: std::sync::atomic::AtomicU64,
}

struct TaskState {
    closing: bool,
    handles: Vec<tokio::task::JoinHandle<()>>,
}

struct ProcessingPublicationOwner {
    request_id: String,
    active: bool,
}

/// Dropping an unsettled execution retains its claim until owner reads can
/// establish a durable artifact or a terminal decision without publication.
pub(crate) struct ProcessingPublication {
    artifact_id: String,
    publications: Arc<Mutex<HashMap<String, ProcessingPublicationOwner>>>,
    admission: Arc<tokio::sync::Mutex<()>>,
}

impl ProcessingPublication {
    pub(crate) async fn release(self) {
        let _slot = self.admission.lock().await;
        self.clear();
    }

    fn clear(&self) {
        self.publications
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&self.artifact_id);
    }
}

impl Drop for ProcessingPublication {
    fn drop(&mut self) {
        if let Some(owner) = self
            .publications
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get_mut(&self.artifact_id)
        {
            owner.active = false;
        }
    }
}

/// A live download renews its lease well inside the staleness window.
const LEASE_RENEWAL_INTERVAL: u64 = 10 * 60 * 1000;

/// The random attempt-incarnation identity of one server lifetime: 32
/// lowercase hex characters, exactly the width the persistence boundary
/// validates.
fn startup_incarnation() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| format!("startup randomness: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

impl ExportManager {
    /// Opens the application-owned Export workspace and the shared local
    /// Photo Development executor. Blocking filesystem work; the caller
    /// runs it inside `spawn_blocking` during startup.
    pub(crate) fn open(
        library: Arc<Library>,
        library_root: PathBuf,
        state_directory: &Path,
        resolver: SourceLocationResolver,
        processing: ProcessingConfig,
        allowance: u64,
    ) -> Result<Self, String> {
        let workspace_root = state_directory.join("exports");
        std::fs::create_dir_all(&workspace_root)
            .map_err(|error| format!("Export workspace is unavailable: {error}"))?;
        let workspace = ExportWorkspace::open(&workspace_root, &library_root)
            .map_err(|error| format!("Export workspace is unavailable: {error}"))?;
        let executor = Arc::new(PhotoExecutor::open(&processing, state_directory)?);
        Ok(Self {
            library,
            library_root,
            resolver,
            workspace,
            executor,
            incarnation: startup_incarnation()?,
            next_sequence: AtomicU64::new(1),
            running: Mutex::new(HashMap::new()),
            allowance,
            admission: Arc::new(tokio::sync::Mutex::new(())),
            publications: Arc::new(Mutex::new(HashMap::new())),
            tasks: Mutex::new(TaskState {
                closing: false,
                handles: Vec::new(),
            }),
            shutting_down: AtomicBool::new(false),
            lease_renewal_interval_millis: std::sync::atomic::AtomicU64::new(
                LEASE_RENEWAL_INTERVAL,
            ),
            sweep_task: Mutex::new(None),
        })
    }

    pub(crate) fn spawn_task<F>(&self, future: F) -> bool
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut tasks = self.tasks.lock().unwrap_or_else(|error| error.into_inner());
        if tasks.closing {
            return false;
        }
        tasks.handles.retain(|handle| !handle.is_finished());
        tasks.handles.push(tokio::spawn(future));
        true
    }

    /// Closes lifecycle admission synchronously and signals every live engine
    /// attempt before any asynchronous drain begins.
    pub(crate) fn begin_shutdown(&self) {
        self.shutting_down.store(true, Ordering::Release);
        self.tasks
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .closing = true;
        for token in self
            .running
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
        {
            token.store(true, Ordering::Release);
        }
    }

    async fn drain_tasks(&self) {
        loop {
            let handles = {
                let mut tasks = self.tasks.lock().unwrap_or_else(|error| error.into_inner());
                tasks.handles.retain(|handle| !handle.is_finished());
                std::mem::take(&mut tasks.handles)
            };
            if handles.is_empty() {
                return;
            }
            for handle in handles {
                let _ = handle.await;
            }
        }
    }

    /// The interval at which a live download stream renews its lease.
    pub(crate) fn lease_renewal_interval(&self) -> Duration {
        Duration::from_millis(
            self.lease_renewal_interval_millis
                .load(std::sync::atomic::Ordering::Relaxed),
        )
    }

    /// Overrides the download lease renewal interval; tests shorten it.
    #[cfg(test)]
    pub(crate) fn set_lease_renewal_interval(&self, interval: Duration) {
        self.lease_renewal_interval_millis.store(
            interval.as_millis() as u64,
            std::sync::atomic::Ordering::Relaxed,
        );
    }

    pub(crate) fn allowance(&self) -> u64 {
        self.allowance
    }

    /// Stops lifecycle admission, cancels periodic retention work and live
    /// engine attempts, then drains every task before the Library closes.
    pub(crate) async fn shutdown_processing(&self) {
        self.begin_shutdown();
        let task = self
            .sweep_task
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
        self.executor.shutdown().await;
        self.drain_tasks().await;
    }

    /// The retained artifact file of one Export identity. The path stays
    /// private to the service; responses carry identity facts only.
    pub(crate) fn artifact_path(&self, export_id: &str) -> Option<PathBuf> {
        self.artifact_path_for_workload(export_id, "development-tiff")
    }

    pub(crate) fn artifact_path_for_workload(
        &self,
        export_id: &str,
        workload: &str,
    ) -> Option<PathBuf> {
        let target = match workload {
            "development-tiff" => ExportTarget::DevelopmentTiff,
            "film-jpeg" => ExportTarget::FilmJpeg,
            _ => return None,
        };
        valid_artifact_stem(export_id).map(|stem| {
            self.workspace
                .root()
                .join("artifacts")
                .join(format!("{stem}.{}", target.extension()))
        })
    }

    /// Deletes one ephemeral preview output after failure, cancellation,
    /// supersession, or retention expiry.
    pub(crate) fn delete_preview_output(&self, attempt_key: &str) {
        let _ = self.workspace.delete_preview_artifact(attempt_key);
    }

    /// The single heavy-work admission shared by durable Exports and
    /// preview-class renders: one serialized processing slot per instance.
    /// The caller holds the returned guard for exactly one attempt.
    pub(crate) async fn acquire_heavy_slot(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.admission.lock().await
    }

    /// Begins one private ephemeral preview output below the workspace's
    /// preview namespace. The rendering executor owns the temporary bytes
    /// until it publishes or discards them.
    pub(crate) fn begin_preview_output(
        &self,
        attempt_key: &str,
        target: ExportTarget,
    ) -> Result<slipstream_core::ArtifactWriter, slipstream_core::ExportError> {
        self.workspace.begin_preview_artifact(attempt_key, target)
    }

    /// The published Library Location of one Photo, or `None` before the
    /// first publication.
    pub(crate) fn resolve_source_location(&self, photo_id: &str) -> Option<RelativeOriginalPath> {
        (self.resolver)(photo_id)
    }

    /// The shared serialized Library instance this manager was opened
    /// with. The preview render executor reads render-time facts through
    /// the same single instance; this is a resource reference, not a copy.
    pub(crate) fn library(&self) -> &Arc<Library> {
        &self.library
    }

    /// The bundle identity every attempt of this deployment executes
    /// under. Startup configuration cannot change while the service runs.
    pub(crate) fn bundle_sha256(&self) -> &str {
        self.executor.bundle_sha256()
    }

    /// Pre-acceptance admission: the local Photo Development executor must
    /// be available before an Export is accepted, so an unusable bundle
    /// never consumes a request identity or a capacity reservation.
    pub(crate) async fn ensure_admissible(&self) -> Result<(), String> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err("Photo Development is shutting down".to_owned());
        }
        if self.executor.available() {
            Ok(())
        } else {
            Err("Photo Development is unavailable".to_owned())
        }
    }

    /// Runs one serialized local development through the shared executor.
    /// The caller owns the staging paths and the validation of the result;
    /// this is the same engine boundary the durable Export lifecycle uses.
    pub(crate) async fn develop(
        &self,
        input: PathBuf,
        output: PathBuf,
        exposure_milli_ev: i64,
        cancellation: Arc<AtomicBool>,
    ) -> Result<OutputIdentity, String> {
        self.executor
            .develop(input, output, exposure_milli_ev, cancellation)
            .await
    }

    /// The retained Development TIFF of one Photo whose captured snapshot
    /// matches the current Edit identity and whose retention has not expired,
    /// or `None` when no matching artifact is retained.
    ///
    /// This is the durable Development Result retention the Edit Preview
    /// derivation resolves against: a published Development TIFF artifact is
    /// the retained result, and its disclosed expiry is its retention. A
    /// Library read failure resolves as "not retained" so the caller refuses
    /// fail-closed instead of serving a result it cannot vouch for.
    pub(crate) async fn retained_development_result(
        &self,
        photo_id: &str,
        identity: &RetainedDevelopmentIdentity<'_>,
    ) -> Option<RetainedDevelopmentTiff> {
        let records = self.library.photo_exports(photo_id).await.ok().flatten()?;
        retained_development_tiff_of(&records, identity, unix_seconds(), |export_id| {
            self.artifact_path(export_id)
        })
    }

    /// Admits one accepted Export: the heavy attempt runs in the background
    /// and survives browser departure. Duplicate identities never reach this
    /// entry because the persistence owner deduplicates first.
    pub(crate) fn start(self: &Arc<Self>, export: ExportRecord) {
        let manager = Arc::clone(self);
        let _ = self.spawn_task(async move {
            if manager.shutting_down.load(Ordering::Acquire) || export.attempt.is_some() {
                // A record with a persisted attempt was started before; only
                // restart reconciliation may resolve it, never a new launch.
                return;
            }
            manager.execute(export).await;
        });
    }

    /// Resolves unfinished work after a restart. Queued work starts through
    /// the ordinary admission path; running work is resolved from the
    /// durable snapshot into a validated publication or a terminal
    /// failure, never a replacement attempt.
    pub(crate) fn reconcile_after_restart(self: &Arc<Self>) {
        let manager = Arc::clone(self);
        let _ = self.spawn_task(async move {
            let Ok(unfinished) = manager.library.unfinished_exports().await else {
                return;
            };
            for record in unfinished {
                if manager.shutting_down.load(Ordering::Acquire) {
                    break;
                }
                if record.attempt.is_some() {
                    manager.clone().resolve_interrupted(record).await;
                } else {
                    manager.start(record);
                }
            }
        });
    }
    /// Runs the retention sweep periodically for the life of the process.
    pub(crate) fn schedule_expiry_sweep(self: &Arc<Self>) {
        let manager = Arc::clone(self);
        let task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(3600));
            loop {
                interval.tick().await;
                manager.sweep_expiry().await;
            }
        });
        *self
            .sweep_task
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(task);
    }

    /// Removes expired artifacts and orphaned files, then lets the owner
    /// expire the corresponding rows and receipts.
    pub(crate) async fn sweep_expiry(&self) {
        let now = unix_seconds();
        let Ok(sweep) = self.library.sweep_export_expiry(now).await else {
            return;
        };
        for export_id in sweep
            .artifact_expiry_ids
            .iter()
            .chain(sweep.record_expiry_ids.iter())
        {
            for extension in ["tiff", "jpg"] {
                if let Some(stem) = valid_artifact_stem(export_id) {
                    let _ = fs::remove_file(
                        self.workspace
                            .root()
                            .join("artifacts")
                            .join(format!("{stem}.{extension}")),
                    );
                }
            }
        }
        // Composable artifacts share this namespace but are retired by their
        // own persistence owner. Remove bytes only after that owner has
        // durably expired the corresponding records and leases.
        if let Ok(processing) = self.library.sweep_processing_export_expiry(now).await {
            for artifact_id in processing {
                if let Some(stem) = valid_artifact_stem(&artifact_id) {
                    // A composable artifact's module owns its extension;
                    // removing both is idempotent for the one that does
                    // not exist.
                    for extension in ["tiff", "jpg"] {
                        let _ = fs::remove_file(
                            self.workspace
                                .root()
                                .join("artifacts")
                                .join(format!("{stem}.{extension}")),
                        );
                    }
                }
            }
        }
        self.remove_orphan_artifacts().await;
    }

    /// Deletes artifact files that no succeeded legacy Export or composable
    /// ProcessingArtifact record claims. A crash between file publication and
    /// state commit can leave one behind; uncertain commits are retained until
    /// persistence proves the record expired.
    async fn remove_orphan_artifacts(&self) {
        // Serialize orphan reads/deletion with publication and claim release,
        // so a read made before settlement cannot delete a newly claimed file.
        let _slot = self.admission.lock().await;
        let artifacts = self.workspace.root().join("artifacts");
        let Ok(entries) = fs::read_dir(artifacts) else {
            return;
        };
        for entry in entries.flatten() {
            let entry_path = entry.path();
            let stem = entry_path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default();
            let publication = self
                .publications
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(stem)
                .map(|owner| (owner.request_id.clone(), owner.active));
            if matches!(publication, Some((_, true))) {
                continue;
            }
            let terminal = if let Some((request_id, _)) = &publication {
                match self.library.processing_export_work(request_id).await {
                    Ok(Some(work)) if work.admission.request_id == *request_id => {
                        use slipstream_core::ProcessingExportWorkState;
                        matches!(
                            work.state,
                            ProcessingExportWorkState::Failed
                                | ProcessingExportWorkState::Cancelled
                        ) || (work.state == ProcessingExportWorkState::Succeeded
                            && work.artifact_id.as_ref().map(|id| id.as_str()) != Some(stem))
                    }
                    _ => false,
                }
            } else {
                false
            };
            let (Ok(legacy), Ok(processing)) = (
                self.library.export(stem).await,
                self.library.processing_artifact(stem).await,
            ) else {
                // An unavailable owner cannot prove a file unclaimed.
                continue;
            };
            let claimed =
                legacy.is_some_and(|record| record.artifact.is_some()) || processing.is_some();
            if publication.is_some() {
                if !claimed && !terminal {
                    continue;
                }
                self.publications
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .remove(stem);
            }
            if !claimed {
                let _ = fs::remove_file(entry_path);
            }
        }
    }

    async fn settle_failed(&self, export_id: &str, outcome: String) {
        let _ = self
            .library
            .settle_export(
                export_id,
                ExportSettlement::Failed {
                    outcome: bounded_outcome(&outcome),
                    settled_at: unix_seconds(),
                },
            )
            .await;
    }

    /// Resolves one running Export after a restart. This process never owns
    /// the crashed attempt and attaches to no engine process: only a
    /// validated artifact durably claimed by exactly that attempt is
    /// recovered from disk; anything else fails as interrupted.
    async fn resolve_interrupted(self: Arc<Self>, record: ExportRecord) {
        let _slot = self.admission.lock().await;
        let current = match self.library.export(&record.id).await {
            Ok(Some(current)) if !current.state.is_terminal() => current,
            _ => return,
        };
        let Some(attempt) = current.attempt.clone() else {
            return;
        };
        let Ok(target) = export_target(&current.snapshot.workload) else {
            self.settle_failed(
                &current.id,
                "export snapshot has an unsupported workload".to_owned(),
            )
            .await;
            return;
        };
        let claim = self
            .library
            .export_publication_claim(&current.id)
            .await
            .unwrap_or(None);
        if claim.as_ref() == Some(&(attempt.incarnation.clone(), attempt.sequence))
            && let Some(published_path) =
                self.artifact_path_for_workload(&current.id, &current.snapshot.workload)
            && tokio::fs::metadata(&published_path).await.is_ok()
        {
            // The crashed process durably claimed this attempt's
            // publication and renamed a validated file into place; the
            // recovery validates it again before adopting it.
            if let Err(outcome) = self
                .settle_from_published_file(&current.id, &published_path, target)
                .await
            {
                self.settle_failed(&current.id, outcome).await;
            }
            return;
        }
        self.settle_failed(&current.id, INTERRUPTED_ATTEMPT.to_owned())
            .await;
    }

    /// Registers the live cancellation token of one running Export attempt,
    /// superseding any token a zombie attempt of the same identity still
    /// holds so it can no longer publish.
    fn begin_running(&self, export_id: &str) -> Arc<AtomicBool> {
        let token = Arc::new(AtomicBool::new(false));
        let mut running = self
            .running
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(superseded) = running.insert(export_id.to_owned(), Arc::clone(&token)) {
            superseded.store(true, Ordering::Relaxed);
        }
        token
    }

    fn end_running(&self, export_id: &str, token: &Arc<AtomicBool>) {
        let mut running = self
            .running
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if running
            .get(export_id)
            .is_some_and(|current| Arc::ptr_eq(current, token))
        {
            running.remove(export_id);
        }
    }

    /// The live cancellation token of one running attempt, when this
    /// process owns it.
    fn running_token(&self, export_id: &str) -> Option<Arc<AtomicBool>> {
        self.running
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(export_id)
            .cloned()
    }

    /// Signals one live composable Export after its durable cancel decision.
    pub(crate) fn cancel_running(&self, export_id: &str) {
        if let Some(token) = self.running_token(export_id) {
            token.store(true, Ordering::Release);
        }
    }
    /// One bounded heavy attempt: stage, develop, validate, claim, publish.
    async fn execute(self: &Arc<Self>, export: ExportRecord) {
        if self.shutting_down.load(Ordering::Acquire) {
            return;
        }
        let export_id = export.id.clone();
        let _slot = self.admission.lock().await;
        if self.shutting_down.load(Ordering::Acquire) {
            return;
        }
        // The record may have settled (cancelled) while queued.
        let record = match self.library.export(&export_id).await {
            Ok(Some(record)) if !record.state.is_terminal() => record,
            Ok(_) => return,
            Err(_) => {
                return self
                    .settle_failed(&export_id, PERSISTENCE_UNAVAILABLE.to_owned())
                    .await;
            }
        };
        let snapshot = record.snapshot.clone();

        // The attempt identity is minted locally: the incarnation names this
        // server lifetime and the sequence orders its attempts.
        let attempt = ExportAttempt {
            incarnation: self.incarnation.clone(),
            sequence: self.next_sequence.fetch_add(1, Ordering::Relaxed),
        };
        let record = match self
            .library
            .begin_export_attempt(&export_id, attempt.clone())
            .await
        {
            Ok(Some(record)) if !record.state.is_terminal() => record,
            // Settled by cancellation or lost between admission and attempt
            // persistence.
            Ok(_) => return,
            Err(_) => {
                return self
                    .settle_failed(&export_id, PERSISTENCE_UNAVAILABLE.to_owned())
                    .await;
            }
        };

        // Stage the Original through the confined Library boundary and bind
        // the verified bytes to the record before any engine contact.
        let staged = match self.stage_original(&snapshot).await {
            Ok(staged) => staged,
            Err(error) => return self.settle_failed(&export_id, error).await,
        };
        let staged_facts = staged.facts();
        let record = match self
            .library
            .record_export_source(
                &record.id,
                staged_facts.source_facts.size,
                &staged_facts.sha256,
            )
            .await
        {
            Ok(Some(record)) if !record.state.is_terminal() => record,
            Ok(_) => return,
            Err(_) => {
                return self
                    .settle_failed(&export_id, PERSISTENCE_UNAVAILABLE.to_owned())
                    .await;
            }
        };

        // The staged source evidence the attempt runs against must be the
        // one the persistence boundary recorded.
        if record.source.is_none() {
            return self
                .settle_failed(
                    &export_id,
                    "staged source evidence was not recorded".to_owned(),
                )
                .await;
        }

        // Once an attempt identity is persisted, its outcome may only settle
        // while it is still the record's current attempt: a retry or
        // cancellation that superseded this task never steals the truth.
        if let Err(error) = self
            .drive_attempt(&record, &snapshot, staged, &attempt)
            .await
        {
            let still_current = match self.library.export(&export_id).await {
                Ok(Some(current)) => {
                    current.state == ExportState::Running
                        && current.attempt.as_ref() == Some(&attempt)
                }
                _ => false,
            };
            if still_current {
                self.settle_failed(&export_id, error).await;
            }
        }
    }
}

/// The outcome of one cancellation request against an Export.
pub(crate) enum ExportCancelOutcome {
    /// The Export identity is unknown.
    Unknown,
    /// The settled Export, exactly once against the actual completion state.
    Settled(Box<ExportRecord>),
    /// The actual completion could not be proven; nothing was settled.
    Uncertain,
}

impl ExportManager {
    /// Cancels one Export exactly once against the actual completion state.
    /// The durable exactly-once cancellation settles the record first; a
    /// completion that raced it and already settled keeps its published
    /// artifact, and the live engine attempt is signalled afterwards so the
    /// child and its scratch are torn down before the slot is released.
    pub(crate) async fn cancel(&self, export_id: &str) -> ExportCancelOutcome {
        let record = match self.library.export(export_id).await {
            Ok(Some(record)) => record,
            Ok(None) => return ExportCancelOutcome::Unknown,
            Err(_) => return ExportCancelOutcome::Uncertain,
        };
        if record.state.is_terminal() {
            return ExportCancelOutcome::Settled(Box::new(record));
        }
        match self.library.cancel_export(export_id).await {
            Ok(Some(record)) => {
                if let Some(token) = self.running_token(export_id) {
                    token.store(true, Ordering::Release);
                }
                ExportCancelOutcome::Settled(Box::new(record))
            }
            Ok(None) => ExportCancelOutcome::Unknown,
            Err(_) => ExportCancelOutcome::Uncertain,
        }
    }

    /// Resolves, copies, and verifies the Original. A changed source revision
    /// refuses the attempt before any engine contact.
    async fn stage_original(&self, snapshot: &ExportSnapshot) -> Result<StagedOriginal, String> {
        self.stage_original_for(&snapshot.photo_id, &snapshot.source_revision)
            .await
    }

    /// The common staging seam used by durable Exports and preview-class
    /// attempts. The expected source revision is supplied directly so the
    /// ephemeral path never needs an Export snapshot or persistence row.
    async fn stage_original_for(
        &self,
        photo_id: &str,
        source_revision: &str,
    ) -> Result<StagedOriginal, String> {
        let (staged, _) = self
            .stage_original_details(photo_id, source_revision, None)
            .await?;
        Ok(staged)
    }

    /// Stages one preview Original and classifies its approved processing
    /// profile from the same confined bytes. The profile is engine input,
    /// not part of the Edit identity facts.
    pub(crate) async fn stage_preview_original(
        &self,
        photo_id: &str,
        source_revision: &str,
        kind: OriginalKind,
        filename: &str,
    ) -> Result<(StagedOriginal, String), String> {
        self.stage_original_details(photo_id, source_revision, Some((kind, filename.to_owned())))
            .await
            .and_then(|(staged, profile)| {
                profile
                    .ok_or_else(|| "source class has no approved profile".to_owned())
                    .map(|profile| (staged, profile))
            })
    }

    async fn stage_original_details(
        &self,
        photo_id: &str,
        source_revision: &str,
        profile: Option<(OriginalKind, String)>,
    ) -> Result<(StagedOriginal, Option<String>), String> {
        let library_root = self.library_root.clone();
        let source_revision = source_revision.to_owned();
        let relative_path = (self.resolver)(photo_id)
            .ok_or("published Library has no source Location for the Photo")?;
        let staged_location = relative_path.as_str().to_owned();
        let workspace = self.workspace.clone();
        tokio::task::spawn_blocking(move || {
            let root = LibraryRoot::open(&library_root)
                .map_err(|_| "source Library could not be opened".to_owned())?;
            let capability: OriginalCapability = root
                .original(relative_path)
                .map_err(|_| "source Location is unsafe".to_owned())?;
            let staged = workspace
                .stage_original(&capability)
                .map_err(|error| format!("source staging failed: {error}"))?;
            // Post-copy stability: the staged bytes must still describe the
            // captured source revision. The staged facts are the Original's
            // own stat, so the same path/size/mtime revision the snapshot
            // captured must re-derive from them.
            let facts = staged.facts();
            let observed = slipstream_core::source_revision(
                &staged_location,
                facts.source_facts.size,
                facts.source_facts.mtime_ms,
            )
            .map_err(|_| "source revision could not be captured".to_owned())?;
            if observed != source_revision {
                return Err("source changed after acceptance".to_owned());
            }
            let profile_id = profile.map(|(kind, filename)| {
                let metadata =
                    slipstream_core::inspect_review_metadata(&capability, kind, facts.source_facts)
                        .map_err(|_| "source metadata could not be inspected".to_owned())?;
                let container = photo_profile::container_of_filename(&filename)
                    .ok_or("source has no RAW container".to_owned())?;
                let (Some(make), Some(model)) = (metadata.make, metadata.model) else {
                    return Err("source camera identity could not be inspected".to_owned());
                };
                photo_profile::classify(&make, &model, &container)
                    .map(|profile| profile.profile_id.to_owned())
                    .ok_or("source class has no approved profile".to_owned())
            });
            let profile_id = profile_id.transpose()?;
            Ok((staged, profile_id))
        })
        .await
        .map_err(|error| format!("staging worker failed: {error}"))?
    }

    /// Runs the local engine attempt of one admitted Export: develop into a
    /// private temporary, validate against the closed contract and the
    /// executor's report, then claim and publish atomically. `staged` is
    /// dropped once the engine consumed the source copy, removing the
    /// private file.
    async fn drive_attempt(
        &self,
        record: &ExportRecord,
        snapshot: &ExportSnapshot,
        staged: StagedOriginal,
        attempt: &ExportAttempt,
    ) -> Result<(), String> {
        let export_id = record.id.clone();
        let recipe = snapshot.recipe_payload().map_err(|_| {
            "captured recipe is not representable by the execution payload".to_owned()
        })?;
        let target = export_target(&snapshot.workload)?;

        // The private output the engine writes; validation gates any
        // publication, and dropping the writer discards a partial output.
        let writer = self
            .workspace
            .begin_artifact(&export_id, target)
            .map_err(|error| format!("output staging failed: {error}"))?;
        let output_path = writer.temporary_path().to_path_buf();

        // The live cancellation token: HTTP cancellation settles the record
        // and flips it; the executor kills the engine process group and
        // removes its scratch before the slot is released.
        let token = self.begin_running(&export_id);
        let developed = self
            .executor
            .develop(
                staged.path().to_path_buf(),
                output_path.clone(),
                recipe.exposure_milli_ev,
                Arc::clone(&token),
            )
            .await;
        drop(staged);
        self.end_running(&export_id, &token);
        let identity = developed?;

        // Verify the developed bytes against the executor's report and the
        // closed Development TIFF contract before anything is claimed or
        // renamed into place.
        let validation_path = output_path.clone();
        let facts = tokio::task::spawn_blocking(move || {
            verify_developed_output(&validation_path, &identity, target)
        })
        .await
        .map_err(|error| format!("validation task failed: {error}"))?
        .map_err(|_| OUTPUT_VALIDATION_FAILED.to_owned())?;

        // Cancellation must win the race up to this point: a settled record
        // is never published and the engine result is discarded.
        let current = self
            .library
            .export(&export_id)
            .await
            .map_err(|_| PERSISTENCE_UNAVAILABLE.to_owned())?
            .ok_or("export record disappeared")?;
        if current.state != ExportState::Running || current.attempt.as_ref() != Some(attempt) {
            return Err("export was settled by cancellation".to_owned());
        }

        // A publication claim from an earlier process names that process's
        // attempt, never this one. A claimed file this attempt did not
        // publish is a stale leftover to discard; only the claim this
        // attempt is about to take can adopt the rename below.
        let claim = self
            .library
            .export_publication_claim(&export_id)
            .await
            .unwrap_or(None);
        let claim_is_current =
            claim.as_ref() == Some(&(attempt.incarnation.clone(), attempt.sequence));
        if claim_is_current {
            let published_path = self
                .artifact_path_for_workload(&export_id, &snapshot.workload)
                .ok_or_else(|| "the publication claim named no artifact directory".to_owned())?;
            if tokio::fs::metadata(&published_path).await.is_ok() {
                return self
                    .settle_from_published_file(&export_id, &published_path, target)
                    .await;
            }
            return Err("the claimed publication never produced its artifact".to_owned());
        }
        if let Some(published_path) =
            self.artifact_path_for_workload(&export_id, &snapshot.workload)
            && tokio::fs::metadata(&published_path).await.is_ok()
        {
            // A file without a matching claim belongs to a superseded
            // attempt; it is never this attempt's output.
            let _ = fs::remove_file(&published_path);
        }

        // Claim the publication durably before the rename, so a crash around
        // it leaves recoverable evidence instead of an unattributed file.
        if self
            .library
            .claim_export_publication(&export_id, &attempt.incarnation, attempt.sequence)
            .await
            .is_err()
        {
            return Err("the publication could not be claimed durably".to_owned());
        }

        let published = writer
            .publish(|path| validate_output(path, target).map(|_| ()))
            .map_err(|error| format!("artifact publication failed: {error}"))?;
        let settled = self
            .library
            .settle_export(
                &export_id,
                ExportSettlement::Succeeded {
                    artifact_size: published.size,
                    artifact_sha256: published.sha256.clone(),
                    published_at: unix_seconds(),
                    artifact_width: facts.width,
                    artifact_height: facts.height,
                    artifact_profile_identity: facts.profile_identity,
                },
            )
            .await
            .map_err(|_| PERSISTENCE_UNAVAILABLE.to_owned())?;
        if let Some(settled) = settled
            && settled.state != ExportState::Succeeded
        {
            // Cancellation won the exactly-once settlement race; the renamed
            // file belongs to no record and must not leak.
            let _ = fs::remove_file(&published.path);
        }
        Ok(())
    }

    async fn settle_from_published_file(
        &self,
        export_id: &str,
        path: &Path,
        target: ExportTarget,
    ) -> Result<(), String> {
        let hash_path = path.to_path_buf();
        let facts_and_hash = tokio::task::spawn_blocking(move || -> Result<_, String> {
            let facts = validate_output(&hash_path, target)
                .map_err(|_| OUTPUT_VALIDATION_FAILED.to_owned())?;
            let file = open_read_only(&hash_path)
                .map_err(|_| "published artifact could not be reopened".to_owned())?;
            let metadata = file
                .metadata()
                .map_err(|_| "published artifact could not be stated".to_owned())?;
            use sha2::{Digest as _, Sha256};
            let mut hasher = Sha256::new();
            let mut file = file;
            std::io::copy(&mut file, &mut hasher)
                .map_err(|_| "published artifact could not be hashed".to_owned())?;
            let published_at = metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs())
                .unwrap_or_else(unix_seconds);
            Ok((
                facts,
                metadata.len(),
                format!("{:x}", hasher.finalize()),
                published_at,
            ))
        })
        .await
        .map_err(|error| format!("artifact recovery task failed: {error}"))??;
        let (facts, size, sha256, published_at) = facts_and_hash;
        let settled = self
            .library
            .settle_export(
                export_id,
                ExportSettlement::Succeeded {
                    artifact_size: size,
                    artifact_sha256: sha256,
                    published_at,
                    artifact_width: facts.width,
                    artifact_height: facts.height,
                    artifact_profile_identity: facts.profile_identity,
                },
            )
            .await
            .map_err(|_| PERSISTENCE_UNAVAILABLE.to_owned())?;
        if let Some(settled) = settled
            && settled.state != ExportState::Succeeded
        {
            let _ = fs::remove_file(path);
        }
        Ok(())
    }
}

const PERSISTENCE_UNAVAILABLE: &str = "persistence is unavailable";

/// The current Edit identity facts a retained Development TIFF must have been
/// produced under to be current for one Edit Preview derivation: the exact
/// recipe revision and exposure, the source revision, and the bundle.
pub(crate) struct RetainedDevelopmentIdentity<'a> {
    /// Whether the request names the processing baseline rather than one
    /// saved recipe revision: a baseline request matches any captured
    /// snapshot produced under exactly the baseline settings, while every
    /// other request matches the captured revision exactly.
    pub(crate) matches_baseline: bool,
    pub(crate) recipe_revision: Option<&'a str>,
    pub(crate) exposure_milli_ev: i64,
    pub(crate) source_revision: &'a str,
    pub(crate) bundle_sha256: &'a str,
}

/// One retained Development TIFF: the published artifact of a succeeded
/// Development TIFF Export with the identity its publication captured. The
/// path stays private to the service; responses carry identity facts only.
pub(crate) struct RetainedDevelopmentTiff {
    pub(crate) path: PathBuf,
    pub(crate) sha256: String,
    pub(crate) byte_length: u64,
    pub(crate) recipe_revision: String,
    pub(crate) exposure_milli_ev: i64,
    pub(crate) source_revision: String,
    pub(crate) bundle_id: String,
}

/// The first retained Development TIFF in one Photo's Export records, in the
/// Library's retention order, that was produced under exactly the current
/// identity. The caller reads the records through the durable Export
/// lifecycle; this selection is pure so the ordering and identity rules are
/// testable without a Library.
pub(crate) fn retained_development_tiff_of(
    records: &[ExportRecord],
    identity: &RetainedDevelopmentIdentity<'_>,
    now_unix_seconds: u64,
    artifact_path: impl Fn(&str) -> Option<PathBuf>,
) -> Option<RetainedDevelopmentTiff> {
    records.iter().find_map(|record| {
        retained_development_tiff(record, identity, now_unix_seconds, &artifact_path)
    })
}

/// The retained Development TIFF of one Export record when it was produced
/// under exactly the current identity, its Export settled successfully, and
/// its artifact retention is still live. Any other identity is not current
/// and must never be served as one.
pub(crate) fn retained_development_tiff(
    record: &ExportRecord,
    identity: &RetainedDevelopmentIdentity<'_>,
    now_unix_seconds: u64,
    artifact_path: impl Fn(&str) -> Option<PathBuf>,
) -> Option<RetainedDevelopmentTiff> {
    if record.state != ExportState::Succeeded || record.snapshot.workload != "development-tiff" {
        return None;
    }
    // The captured snapshot's own execution payload is the identity the
    // attempt ran under: a snapshot that cannot produce one never ran.
    let payload = record.snapshot.recipe_payload().ok()?;
    let snapshot = &record.snapshot;
    // A baseline request names the processing baseline rather than a saved
    // recipe, so any snapshot whose captured settings are exactly that
    // baseline is the same development whatever revision captured them.
    // Every other request matches the captured revision exactly.
    let revision_matches = identity.matches_baseline
        || Some(snapshot.recipe_revision.as_str()) == identity.recipe_revision;
    let matches = revision_matches
        && payload.exposure_milli_ev == identity.exposure_milli_ev
        && snapshot.source_revision == identity.source_revision
        && snapshot.bundle_id == identity.bundle_sha256;
    if !matches {
        return None;
    }
    let artifact = record.artifact.as_ref()?;
    if artifact.expires_at <= now_unix_seconds {
        return None;
    }
    Some(RetainedDevelopmentTiff {
        path: artifact_path(&record.id)?,
        sha256: artifact.sha256.clone(),
        byte_length: artifact.size,
        recipe_revision: snapshot.recipe_revision.clone(),
        exposure_milli_ev: payload.exposure_milli_ev,
        source_revision: snapshot.source_revision.clone(),
        bundle_id: snapshot.bundle_id.clone(),
    })
}

fn bounded_outcome(outcome: &str) -> String {
    outcome.chars().take(200).collect()
}

fn valid_artifact_stem(value: &str) -> Option<&str> {
    let valid = (1..=128).contains(&value.len())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        });
    valid.then_some(value)
}

fn unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn export_target(workload: &str) -> Result<ExportTarget, String> {
    match workload {
        "development-tiff" => Ok(ExportTarget::DevelopmentTiff),
        "film-jpeg" => Ok(ExportTarget::FilmJpeg),
        _ => Err("export snapshot has an unsupported workload".to_owned()),
    }
}

#[cfg(test)]
#[path = "export_manager_development_tiff_tests.rs"]
pub(crate) mod development_tiff_decode;
