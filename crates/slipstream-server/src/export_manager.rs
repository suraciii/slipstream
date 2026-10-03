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
    ExportRecord, ExportSettlement, ExportState, ExportTarget, ExportWorkspace, Library,
    LibraryRoot, OriginalCapability, OriginalKind, RelativeOriginalPath, StagedOriginal,
};
use slipstream_processing::local_photo::OutputIdentity;
#[path = "export_manager_composable.rs"]
mod composable;
pub(crate) use composable::{ProcessingExportExecution, ProcessingPreviewExecution};
use slipstream_processing::photo_profile;
#[path = "output_validation.rs"]
pub(crate) mod output_validation;
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

    /// Resolves historical unfinished work without launching retired workloads.
    /// Persisted attempts retain validated publication recovery; unattempted
    /// queued records settle as interrupted.
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
                    manager
                        .settle_failed(&record.id, INTERRUPTED_ATTEMPT.to_owned())
                        .await;
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
}

impl ExportManager {
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
