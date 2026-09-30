//! Service-side Export orchestration: bounded staging, launcher admission,
//! validated publication, restart reconciliation, and retention sweeping.
//!
//! The durable Export record lives in the serialized persistence owner; this
//! module owns the heavy work between acceptance and settlement. It never
//! starts a replacement attempt against a possibly live one and never
//! publishes an unvalidated artifact.

use crate::config::ProcessingConfig;
use slipstream_core::{
    ExportAttempt, ExportRecipePayload, ExportRecord, ExportSettlement, ExportSnapshot,
    ExportSourceEvidence, ExportState, ExportTarget, ExportWorkspace, Library, LibraryRoot,
    OriginalCapability, OriginalKind, RelativeOriginalPath, StagedOriginal,
};
use slipstream_processing::{
    photo::{self, PhotoReceipt, Recipe, Request, ResultBody, Source},
    photo_profile,
    protocol::{Availability, PHOTO_MODE, PHOTO_PROTOCOL_VERSION, PHOTO_WORKLOAD},
};
#[path = "output_validation.rs"]
pub(crate) mod output_validation;
pub(crate) use output_validation::DevelopmentTiffFacts;
#[cfg(test)]
use output_validation::validate_development_tiff;
use output_validation::{
    OUTPUT_VALIDATION_FAILED, open_read_only, validate_output, verify_received_output,
};
use std::{
    fs, io,
    os::unix::{fs::OpenOptionsExt, io::AsRawFd},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

/// Resolves the published Library Location of one Photo. The closure keeps
/// ExportManager decoupled from the Application's published snapshot.
pub(crate) type SourceLocationResolver =
    Arc<dyn Fn(&str) -> Option<RelativeOriginalPath> + Send + Sync>;

/// The immutable launcher identity one attempt binds to: the socket, the
/// instance, the qualified policy, and the bundle. A value naming the
/// launcher for one attempt's exchanges, never a handle to the manager's
/// whole configuration.
pub(crate) struct LauncherBinding {
    pub(crate) socket_path: PathBuf,
    pub(crate) instance: String,
    pub(crate) policy_sha256: String,
    pub(crate) bundle_sha256: String,
}

/// Consecutive launcher transport failures tolerated while an attempt is
/// followed or reconciled before the attempt settles failed with an
/// actionable reason. A live launcher-owned attempt cannot outlast this
/// window of silence.
const LAUNCHER_FAILURE_TOLERANCE: u32 = 120;

/// Server-side launcher identities plus the filesystem seams one attempt owns.
pub(crate) struct ExportManager {
    library: Arc<Library>,
    library_root: PathBuf,
    resolver: SourceLocationResolver,
    workspace: ExportWorkspace,
    processing: ProcessingConfig,
    /// Finite retained-output allowance configured for this deployment.
    allowance: u64,
    /// The initial scheduler admits at most one heavy processing job at a
    /// time per instance; the slot also serializes reconciliation output.
    admission: tokio::sync::Mutex<()>,
    /// How often a live download stream renews its lease liveness anchor.
    lease_renewal_interval_millis: std::sync::atomic::AtomicU64,
}

/// A live download renews its lease well inside the staleness window.
const LEASE_RENEWAL_INTERVAL: u64 = 10 * 60 * 1000;

impl ExportManager {
    /// Opens the application-owned Export workspace. Blocking filesystem
    /// work; the caller runs it inside `spawn_blocking` during startup.
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
        Ok(Self {
            library,
            library_root,
            resolver,
            workspace,
            processing,
            allowance,
            admission: tokio::sync::Mutex::new(()),
            lease_renewal_interval_millis: std::sync::atomic::AtomicU64::new(
                LEASE_RENEWAL_INTERVAL,
            ),
        })
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
    /// preview-class renders: one serialized launcher slot per instance.
    /// The caller holds the returned guard for exactly one attempt.
    pub(crate) async fn acquire_heavy_slot(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.admission.lock().await
    }

    /// The immutable launcher identity of this deployment: socket, instance,
    /// qualified policy, and bundle. Startup configuration cannot change
    /// while the service runs, so one snapshot names the launcher for a
    /// whole attempt.
    pub(crate) fn launcher_binding(&self) -> LauncherBinding {
        LauncherBinding {
            socket_path: self.processing.socket_path(),
            instance: self.processing.instance.clone(),
            policy_sha256: self.processing.policy_sha256.clone(),
            bundle_sha256: self.processing.bundle_sha256.clone(),
        }
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

    /// Verifies one launcher capability against this deployment's configured
    /// identity: capability kind, instance, qualified policy and bundle, and
    /// a well-formed attempt identity. Any mismatch is a fail-closed refusal,
    /// never an available slot.
    fn verify_capability(
        &self,
        capability: &slipstream_processing::photo::ResultBody,
    ) -> Result<(String, u64), String> {
        let slipstream_processing::photo::ResultBody::Capability {
            capability: kind,
            instance,
            incarnation,
            next_sequence,
            policy,
            bundle,
            availability,
            active,
        } = capability
        else {
            return Err("processing launcher answered reconciliation unexpectedly".to_owned());
        };
        if kind != slipstream_processing::protocol::PHOTO_CAPABILITY {
            return Err("processing launcher answered with a foreign capability".to_owned());
        }
        if instance.as_str() != self.processing.instance {
            return Err("processing launcher answered with a foreign instance".to_owned());
        }
        if policy.as_str() != self.processing.policy_sha256 {
            return Err("processing launcher qualified a foreign policy".to_owned());
        }
        if bundle.as_str() != self.processing.bundle_sha256 {
            return Err("processing launcher qualified a foreign bundle".to_owned());
        }
        let incarnation_valid = incarnation.len() == 32
            && incarnation
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if !incarnation_valid || *next_sequence == 0 {
            return Err("processing launcher reported an invalid attempt identity".to_owned());
        }
        if *availability != Availability::Available {
            return Err("processing launcher is configured but blocked".to_owned());
        }
        if active.is_some() {
            return Err("processing launcher already owns an active attempt".to_owned());
        }
        Ok((incarnation.clone(), *next_sequence))
    }

    /// One bounded reconcile exchange with the launcher.
    async fn reconcile_once(&self) -> Result<slipstream_processing::photo::ResultBody, String> {
        let socket = self.processing.socket_path();
        let instance = self.processing.instance.clone();
        tokio::task::spawn_blocking(move || {
            photo::reconcile(&socket, instance).map_err(|_| RECONCILE_UNAVAILABLE.to_owned())
        })
        .await
        .map_err(|error| format!("reconcile task failed: {error}"))?
        .map_err(|_| RECONCILE_UNAVAILABLE.to_owned())
        .map(|response| match response {
            slipstream_processing::photo::Response::Result { result, .. } => Ok(*result),
            slipstream_processing::photo::Response::Error { .. } => {
                Err("processing launcher refused reconciliation".to_owned())
            }
        })?
    }

    /// Pre-acceptance admission: the launcher must be reachable and its
    /// capability must match this deployment's configured identity before an
    /// Export is accepted, so a blocked deployment never consumes a request
    /// identity or a capacity reservation.
    pub(crate) async fn ensure_admissible(&self) -> Result<(), String> {
        let capability = self.reconcile_once().await?;
        self.verify_capability(&capability).map(|_| ())
    }

    /// Admits one accepted Export: the heavy attempt runs in the background
    /// and survives browser departure. Duplicate identities never reach this
    /// entry because the persistence owner deduplicates first.
    pub(crate) fn start(self: &Arc<Self>, export: ExportRecord) {
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            if export.attempt.is_some() {
                // A record with a persisted attempt was started before; only
                // restart reconciliation may resolve it, never a new launch.
                return;
            }
            manager.execute(export).await;
        });
    }

    /// Resolves unfinished work after a restart. Queued work starts through
    /// the ordinary admission path; running work is resolved from the durable
    /// snapshot and the launcher receipt into a validated publication or a
    /// terminal failure, never a replacement attempt.
    pub(crate) fn reconcile_after_restart(self: &Arc<Self>) {
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            let Ok(unfinished) = manager.library.unfinished_exports().await else {
                return;
            };
            for record in unfinished {
                if record.attempt.is_some() {
                    let manager = Arc::clone(&manager);
                    tokio::spawn(async move {
                        manager.resolve_interrupted(record).await;
                    });
                } else {
                    manager.start(record);
                }
            }
        });
    }

    /// Runs the retention sweep periodically for the life of the process.
    pub(crate) fn schedule_expiry_sweep(self: &Arc<Self>) {
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(3600));
            loop {
                interval.tick().await;
                manager.sweep_expiry().await;
            }
        });
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
        self.remove_orphan_artifacts().await;
    }

    /// Deletes artifact files that no succeeded Export record claims. A crash
    /// between file publication and state commit can leave one behind.
    async fn remove_orphan_artifacts(&self) {
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
            let claimed = valid_artifact_stem(stem).is_some()
                && matches!(
                    self.library.export(stem).await,
                    Ok(Some(ExportRecord {
                        artifact: Some(_),
                        ..
                    }))
                );
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

    /// Resolves one running Export after a restart. The launcher receipt is
    /// the only settlement evidence; an unreachable launcher leaves the
    /// Export truthfully running and keeps retrying within the tolerance.
    async fn resolve_interrupted(self: Arc<Self>, record: ExportRecord) {
        let mut failures = 0_u32;
        loop {
            let current = match self.library.export(&record.id).await {
                Ok(Some(current)) => current,
                Ok(None) => return,
                Err(_) => return,
            };
            if current.state.is_terminal() {
                return;
            }
            let Some(attempt) = current.attempt.clone() else {
                return;
            };
            match self.launcher_inspect(&current.id, &attempt).await {
                Ok(receipt) => {
                    if is_completed_receipt(&receipt) {
                        let _slot = self.admission.lock().await;
                        if let Err(outcome) = self
                            .collect_output_and_publish(
                                &current,
                                &attempt.incarnation,
                                attempt.sequence,
                            )
                            .await
                        {
                            self.settle_failed(&current.id, outcome).await;
                        }
                        return;
                    }
                    if is_terminal_receipt(&receipt) {
                        self.settle_failed(
                            &current.id,
                            format!(
                                "interrupted attempt settled: {}",
                                receipt.outcome.unwrap_or_else(|| "unknown".to_owned())
                            ),
                        )
                        .await;
                        return;
                    }
                    failures = 0;
                }
                Err(_) => {
                    failures += 1;
                    if failures >= LAUNCHER_FAILURE_TOLERANCE {
                        self.settle_failed(
                            &current.id,
                            "processing launcher never reconciled the interrupted attempt"
                                .to_owned(),
                        )
                        .await;
                        return;
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    }

    /// One bounded heavy attempt: stage, launch, collect, validate, publish.
    async fn execute(self: &Arc<Self>, export: ExportRecord) {
        let export_id = export.id.clone();
        let _slot = self.admission.lock().await;

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

        // Reconcile the launcher slot: available with no active receipt, and
        // the source of the executor attempt identity.
        let (incarnation, sequence) = match self.reconcile_slot().await {
            Ok(slot) => slot,
            Err(error) => return self.settle_failed(&export_id, error).await,
        };
        let attempt = ExportAttempt {
            incarnation: incarnation.clone(),
            sequence,
        };
        let record = match self
            .library
            .begin_export_attempt(&export_id, attempt.clone())
            .await
        {
            Ok(Some(record)) if !record.state.is_terminal() => record,
            // Settled by cancellation or lost between slot reconciliation and
            // attempt persistence; the launcher slot stays reconcilable.
            Ok(_) => return,
            Err(_) => {
                return self
                    .settle_failed(&export_id, PERSISTENCE_UNAVAILABLE.to_owned())
                    .await;
            }
        };

        // Stage the Original through the confined Library boundary and bind
        // the verified bytes to the record before any launcher contact.
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
        let source = record
            .source
            .clone()
            .ok_or_else(|| "staged source evidence was not recorded".to_owned());

        let source = match source {
            Ok(source) => source,
            Err(error) => return self.settle_failed(&export_id, error).await,
        };

        // Once an attempt identity is persisted, its outcome may only settle
        // while it is still the record's current attempt: a retry or
        // cancellation that superseded this task never steals the truth.
        if let Err(error) = self
            .drive_attempt(&record, &snapshot, &source, staged, &incarnation, sequence)
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

/// The launcher-side answer to one cancellation request.
pub(crate) enum AttemptCancel {
    /// The completion raced the cancellation and won; the output can still
    /// be collected and published.
    Completed,
    /// The launcher settled the attempt as cancelled.
    Cancelled,
    /// The launcher settled the attempt with a failed outcome.
    TerminalFailed(String),
    /// The launcher's answer was lost or refused; the settlement is unproven.
    Uncertain,
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
    /// Drives one launcher Cancel exchange to a terminal receipt.
    pub(crate) async fn cancel_attempt(
        &self,
        export_id: &str,
        attempt: &ExportAttempt,
    ) -> AttemptCancel {
        let socket = self.processing.socket_path();
        let instance = self.processing.instance.clone();
        let cancel_export_id = export_id.to_owned();
        let incarnation = attempt.incarnation.clone();
        let sequence = attempt.sequence;
        let response = tokio::task::spawn_blocking(move || {
            photo::request_socket(
                &socket,
                &Request::Cancel {
                    mode: PHOTO_MODE.to_owned(),
                    version: PHOTO_PROTOCOL_VERSION,
                    instance,
                    export_id: cancel_export_id,
                    incarnation,
                    sequence,
                },
            )
        })
        .await;
        let receipt = match response {
            Ok(Ok(slipstream_processing::photo::Response::Result { result, .. })) => {
                match *result {
                    slipstream_processing::photo::ResultBody::Receipt { receipt } => receipt,
                    _ => return AttemptCancel::Uncertain,
                }
            }
            // The launcher forgot the attempt, so no completion can ever be
            // reported later; cancellation is the only remaining resolution.
            Ok(Ok(slipstream_processing::photo::Response::Error { error, .. }))
                if error.code == slipstream_processing::protocol::ErrorCode::UnknownAttempt =>
            {
                return AttemptCancel::Cancelled;
            }
            // A refusal or a transport loss leaves the completion unproven.
            _ => return AttemptCancel::Uncertain,
        };
        if is_completed_receipt(&receipt) {
            return AttemptCancel::Completed;
        }
        if is_terminal_receipt(&receipt) {
            return match receipt.outcome {
                Some(outcome) if receipt.state == "settled" && outcome == "cancelled" => {
                    AttemptCancel::Cancelled
                }
                _ => AttemptCancel::TerminalFailed(
                    receipt.outcome.unwrap_or_else(|| "unknown".to_owned()),
                ),
            };
        }
        // Cancellation was requested but the attempt is still live; follow it
        // to the terminal receipt so the completion race is never guessed.
        match self
            .follow_attempt(export_id, &attempt.incarnation, attempt.sequence)
            .await
        {
            Ok(receipt) if is_completed_receipt(&receipt) => AttemptCancel::Completed,
            Ok(receipt) if is_terminal_receipt(&receipt) => match receipt.outcome {
                Some(outcome) if receipt.state == "settled" && outcome == "cancelled" => {
                    AttemptCancel::Cancelled
                }
                _ => AttemptCancel::TerminalFailed(
                    receipt.outcome.unwrap_or_else(|| "unknown".to_owned()),
                ),
            },
            _ => AttemptCancel::Uncertain,
        }
    }

    /// Cancels one Export exactly once against the actual completion state:
    /// the launcher attempt is cancelled first and a completion that races
    /// the cancellation is still collected and published.
    pub(crate) async fn cancel(&self, export_id: &str) -> ExportCancelOutcome {
        let record = match self.library.export(export_id).await {
            Ok(Some(record)) => record,
            Ok(None) => return ExportCancelOutcome::Unknown,
            Err(_) => return ExportCancelOutcome::Uncertain,
        };
        if record.state.is_terminal() {
            return ExportCancelOutcome::Settled(Box::new(record));
        }
        if let Some(attempt) = record.attempt.clone() {
            match self.cancel_attempt(export_id, &attempt).await {
                AttemptCancel::Completed => {
                    if let Err(outcome) = self
                        .collect_output_and_publish(&record, &attempt.incarnation, attempt.sequence)
                        .await
                    {
                        self.settle_failed(export_id, outcome).await;
                    }
                }
                AttemptCancel::TerminalFailed(outcome) => {
                    self.settle_failed(export_id, outcome).await;
                }
                AttemptCancel::Uncertain => return ExportCancelOutcome::Uncertain,
                AttemptCancel::Cancelled => {}
            }
        }
        match self.library.cancel_export(export_id).await {
            Ok(Some(record)) => ExportCancelOutcome::Settled(Box::new(record)),
            Ok(None) => ExportCancelOutcome::Unknown,
            Err(_) => ExportCancelOutcome::Uncertain,
        }
    }

    pub(crate) async fn reconcile_slot(&self) -> Result<(String, u64), String> {
        let mut failures = 0_u32;
        loop {
            match self
                .reconcile_once()
                .await
                .and_then(|capability| self.verify_capability(&capability))
            {
                Ok(slot) => return Ok(slot),
                Err(error) if error == RECONCILE_UNAVAILABLE => {
                    failures += 1;
                    if failures >= RECONCILE_TOLERANCE {
                        return Err(RECONCILE_UNAVAILABLE.to_owned());
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// Resolves, copies, and verifies the Original. A changed source revision
    /// refuses the attempt before any launcher contact.
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
    /// profile from the same confined bytes. The profile is launcher input,
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

    /// Runs Start, the Inspect loop, Output, validation, acknowledgement, and
    /// publication for one admitted attempt. `staged` is dropped after the
    /// launcher acknowledges the source copy, removing the private file.
    async fn drive_attempt(
        &self,
        record: &ExportRecord,
        snapshot: &ExportSnapshot,
        source: &ExportSourceEvidence,
        staged: StagedOriginal,
        incarnation: &str,
        sequence: u64,
    ) -> Result<(), String> {
        let export_id = record.id.clone();
        let recipe = snapshot.recipe_payload().map_err(|_| {
            "captured recipe is not representable by the execution payload".to_owned()
        })?;
        let manifest_sha256 = manifest_digest(snapshot, source, &recipe);

        // LauncherStart carries exactly one read-only source descriptor. The
        // launcher copies and hashes the bytes before releasing a worker, so
        // the descriptor must stay open until Start returns.
        start_photo(StartExchange {
            path: staged.path().to_path_buf(),
            socket: self.processing.socket_path(),
            instance: self.processing.instance.clone(),
            export_id: export_id.clone(),
            incarnation: incarnation.to_owned(),
            sequence,
            policy: snapshot.policy_id.clone(),
            bundle: snapshot.bundle_id.clone(),
            workload: snapshot.workload.clone(),
            source_kind: "raw".to_owned(),
            source_profile_id: snapshot.source_profile_id.clone(),
            source: source.clone(),
            recipe,
            recipe_digest: snapshot.recipe_digest.clone(),
            manifest_sha256,
            task_error_prefix: "launcher task failed",
            open_error: "staged source could not be opened",
            refusal_error: "launcher refused the start",
        })
        .await??;
        drop(staged);

        // Follow the attempt until the launcher reports terminal settlement.
        let receipt = self
            .follow_attempt(&export_id, incarnation, sequence)
            .await?;
        if !is_completed_receipt(&receipt) {
            return Err(format!(
                "processing attempt did not complete: {}",
                receipt.outcome.unwrap_or_else(|| "unknown".to_owned())
            ));
        }
        self.collect_output_and_publish(record, incarnation, sequence)
            .await
    }

    /// Collects a completed launcher result, validates it, acknowledges it,
    /// and publishes the artifact with one exactly-once settlement. Also the
    /// recovery path for interrupted work after a restart.
    async fn collect_output_and_publish(
        &self,
        record: &ExportRecord,
        incarnation: &str,
        sequence: u64,
    ) -> Result<(), String> {
        let export_id = record.id.clone();
        // A previous process may have claimed this attempt's publication and
        // crashed around the rename. The durable publication claim ties the
        // file to the attempt that produced it: only that attempt's restart
        // may adopt it, a claim without a file means the transfer result was
        // lost, and anything else is a stale leftover to discard.
        let claim = self
            .library
            .export_publication_claim(&export_id)
            .await
            .unwrap_or(None);
        let target = export_target(&record.snapshot.workload)?;
        let workload = record.snapshot.workload.clone();
        let claim_is_current = record.attempt.as_ref().is_some_and(|attempt| {
            claim.as_ref() == Some(&(attempt.incarnation.clone(), attempt.sequence))
        });
        if claim_is_current {
            let published_path = self
                .artifact_path_for_workload(&export_id, &workload)
                .ok_or_else(|| "the publication claim named no artifact directory".to_owned())?;
            if tokio::fs::metadata(&published_path).await.is_ok() {
                return self
                    .settle_from_published_file(&export_id, &published_path, target)
                    .await;
            }
            // The launcher transfer claim is spent; a second Output can
            // never arrive. The attempt fails, and a retry starts fresh.
            return Err("the claimed publication never produced its artifact".to_owned());
        }
        if let Some(published_path) = self.artifact_path_for_workload(&export_id, &workload)
            && tokio::fs::metadata(&published_path).await.is_ok()
        {
            // A file without a matching claim belongs to a superseded
            // attempt; it is never this attempt's output.
            let _ = fs::remove_file(&published_path);
        }
        // Collect the output into a private temporary file through the
        // workspace, then validate before any acknowledgement.
        let writer = self
            .workspace
            .begin_artifact(&export_id, target)
            .map_err(|error| format!("output staging failed: {error}"))?;
        let output_path = writer.temporary_path().to_path_buf();
        let output_file = open_writable(&output_path)
            .map_err(|error| format!("output file could not be opened: {error}"))?;
        let output_receipt = output_photo(OutputExchange {
            socket: self.processing.socket_path(),
            instance: self.processing.instance.clone(),
            export_id: export_id.clone(),
            incarnation: incarnation.to_owned(),
            sequence,
            target: workload.clone(),
            output_file,
            task_error_prefix: "output task failed",
            transfer_error: "launcher could not transfer the output",
            unexpected_error: "launcher answered the output request unexpectedly",
            refusal_error: "launcher refused the output transfer",
        })
        .await?;

        // Verify the received bytes against the launcher receipt and the
        // closed Development TIFF contract before any acknowledgement.
        let validation_path = output_path.clone();
        let validation_receipt = output_receipt.clone();
        let validation_target = target;
        let validation = tokio::task::spawn_blocking(move || {
            verify_received_output(&validation_path, &validation_receipt, validation_target)
        })
        .await
        .map_err(|error| format!("validation task failed: {error}"))?;
        if validation.is_err() {
            // Negative acknowledgement: the launcher retains its result for
            // reconciliation; the temporary file is discarded with the writer.
            let _ = self
                .acknowledge_output(&export_id, incarnation, sequence, false, 1, &"0".repeat(64))
                .await;
            return Err(OUTPUT_VALIDATION_FAILED.to_owned());
        }

        // Cancellation must win the race up to this point: a settled record
        // is never published and the attempt result is discarded.
        let current = self
            .library
            .export(&export_id)
            .await
            .map_err(|_| PERSISTENCE_UNAVAILABLE.to_owned())?
            .ok_or("export record disappeared")?;
        let same_attempt = current.attempt.as_ref().is_some_and(|attempt| {
            attempt.incarnation == incarnation && attempt.sequence == sequence
        });
        if current.state != ExportState::Running || !same_attempt {
            return Err("export was settled by cancellation".to_owned());
        }

        // Publish and commit durable success BEFORE the positive
        // acknowledgement: the launcher cleans its attempt as settled once
        // it is accepted, so the only valid result must already be renamed
        // into place and committed when it is released. A lost or refused
        // acknowledgement is benign afterwards; the launcher reconciles the
        // attempt by its own deadline and the Export is already settled.

        // Claim the publication durably before the rename, so a crash around
        // it leaves recoverable evidence instead of an unattributed file.
        if self
            .library
            .claim_export_publication(&export_id, incarnation, sequence)
            .await
            .is_err()
        {
            return Err("the publication could not be claimed durably".to_owned());
        }

        let facts_slot = std::cell::RefCell::new(None);
        let facts_ref = &facts_slot;
        let published = writer
            .publish(move |path| {
                let facts = validate_output(path, target)?;
                *facts_ref.borrow_mut() = Some(facts);
                Ok(())
            })
            .map_err(|error| format!("artifact publication failed: {error}"))?;
        let facts = facts_slot
            .borrow_mut()
            .take()
            .ok_or("artifact publication produced no validated facts")?;
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
            return Ok(());
        }
        // Best-effort: the Export is durably settled, so an unanswered
        // acknowledgement never loses the result.
        let _ = self
            .acknowledge_output(
                &export_id,
                incarnation,
                sequence,
                true,
                output_receipt.size,
                &output_receipt.sha256,
            )
            .await;
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

    /// Polls the launcher until the attempt reaches terminal settlement.
    async fn follow_attempt(
        &self,
        export_id: &str,
        incarnation: &str,
        sequence: u64,
    ) -> Result<PhotoReceipt, String> {
        let attempt = ExportAttempt {
            incarnation: incarnation.to_owned(),
            sequence,
        };
        let mut failures = 0_u32;
        loop {
            match self.launcher_inspect(export_id, &attempt).await {
                Ok(receipt) => {
                    if is_terminal_receipt(&receipt) {
                        return Ok(receipt);
                    }
                    failures = 0;
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                Err(_) => {
                    // A dropped response is not failure evidence; keep
                    // following the attempt within the bounded tolerance.
                    failures += 1;
                    if failures >= LAUNCHER_FAILURE_TOLERANCE {
                        return Err(
                            "processing launcher became unreachable while the attempt ran"
                                .to_owned(),
                        );
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    pub(crate) async fn launcher_inspect(
        &self,
        export_id: &str,
        attempt: &ExportAttempt,
    ) -> Result<PhotoReceipt, String> {
        let socket = self.processing.socket_path();
        let instance = self.processing.instance.clone();
        let export_id = export_id.to_owned();
        let incarnation = attempt.incarnation.clone();
        let sequence = attempt.sequence;
        let response = tokio::task::spawn_blocking(move || {
            photo::request_socket(
                &socket,
                &Request::Inspect {
                    mode: PHOTO_MODE.to_owned(),
                    version: PHOTO_PROTOCOL_VERSION,
                    instance,
                    export_id,
                    incarnation,
                    sequence,
                },
            )
            .map_err(|_| "launcher inspect failed".to_owned())
        })
        .await
        .map_err(|error| format!("inspect task failed: {error}"))??;
        match response {
            slipstream_processing::photo::Response::Result { result, .. } => {
                let ResultBody::Receipt { receipt } = *result else {
                    return Err("launcher answered the inspect unexpectedly".to_owned());
                };
                Ok(receipt)
            }
            slipstream_processing::photo::Response::Error { .. } => {
                Err("launcher refused the inspect".to_owned())
            }
        }
    }

    pub(crate) async fn acknowledge_output(
        &self,
        export_id: &str,
        incarnation: &str,
        sequence: u64,
        accepted: bool,
        size: u64,
        sha256: &str,
    ) -> bool {
        let socket = self.processing.socket_path();
        let instance = self.processing.instance.clone();
        let export_id = export_id.to_owned();
        let incarnation = incarnation.to_owned();
        let sha256 = sha256.to_owned();
        let outcome = tokio::task::spawn_blocking(move || {
            photo::request_socket(
                &socket,
                &Request::ValidateOutput {
                    mode: PHOTO_MODE.to_owned(),
                    version: PHOTO_PROTOCOL_VERSION,
                    instance,
                    export_id,
                    incarnation,
                    sequence,
                    target: PHOTO_WORKLOAD.to_owned(),
                    size,
                    sha256,
                    accepted,
                },
            )
            .map(|_| ())
            .map_err(|_| "validation acknowledgement failed".to_owned())
        })
        .await;
        matches!(outcome, Ok(Ok(())))
    }
}

const PERSISTENCE_UNAVAILABLE: &str = "persistence is unavailable";
const RECONCILE_UNAVAILABLE: &str = "processing launcher is unavailable";
/// Consecutive reconcile refusals tolerated before an attempt refuses to
/// start; admission stays fail-closed instead of guessing.
const RECONCILE_TOLERANCE: u32 = 5;

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

/// An attempt whose validated output waits for collection reports `settling`
/// with no outcome: the launcher records the service's acknowledgement before
/// it reports a terminal result, so waiting for `settled` here would deadlock
/// against the acknowledgement this service sends after collecting the output.
fn output_awaits_collection(receipt: &PhotoReceipt) -> bool {
    receipt.state == "settling" && receipt.outcome.is_none()
}

pub(crate) fn is_completed_receipt(receipt: &PhotoReceipt) -> bool {
    output_awaits_collection(receipt)
        || (receipt.state == "settled" && receipt.outcome.as_deref() == Some("completed"))
}

pub(crate) fn is_terminal_receipt(receipt: &PhotoReceipt) -> bool {
    output_awaits_collection(receipt) || matches!(receipt.state.as_str(), "settled" | "blocked")
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

pub(crate) fn open_writable(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
}
/// The shared descriptor-bearing Start exchange. Workload callers fill in the
/// facts that are part of their protocol identity; this helper owns the
/// blocking socket call and keeps the source descriptor alive through Start.
pub(crate) struct StartExchange {
    pub(crate) path: PathBuf,
    pub(crate) socket: PathBuf,
    pub(crate) instance: String,
    pub(crate) export_id: String,
    pub(crate) incarnation: String,
    pub(crate) sequence: u64,
    pub(crate) policy: String,
    pub(crate) bundle: String,
    pub(crate) workload: String,
    pub(crate) source_kind: String,
    pub(crate) source_profile_id: String,
    pub(crate) source: ExportSourceEvidence,
    pub(crate) recipe: ExportRecipePayload,
    pub(crate) recipe_digest: String,
    pub(crate) manifest_sha256: String,
    pub(crate) task_error_prefix: &'static str,
    pub(crate) open_error: &'static str,
    pub(crate) refusal_error: &'static str,
}

pub(crate) async fn start_photo(exchange: StartExchange) -> Result<Result<(), String>, String> {
    let StartExchange {
        path,
        socket,
        instance,
        export_id,
        incarnation,
        sequence,
        policy,
        bundle,
        workload,
        source_kind,
        source_profile_id,
        source,
        recipe,
        recipe_digest,
        manifest_sha256,
        task_error_prefix,
        open_error,
        refusal_error,
    } = exchange;
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let file = open_read_only(&path).map_err(|_| open_error.to_owned())?;
        let request = Request::Start {
            mode: PHOTO_MODE.to_owned(),
            version: PHOTO_PROTOCOL_VERSION,
            instance,
            export_id,
            incarnation,
            sequence,
            policy,
            bundle,
            workload,
            source: Source {
                kind: source_kind,
                profile_id: source_profile_id,
                size: source.size,
                sha256: source.sha256,
            },
            recipe: Recipe {
                exposure_milli_ev: recipe.exposure_milli_ev,
                white_balance_mode: recipe.white_balance_mode.to_owned(),
            },
            recipe_digest,
            manifest_sha256,
        };
        photo::request_with_descriptor(&socket, &request, file.as_raw_fd())
            .map(|_| ())
            .map_err(|_| refusal_error.to_owned())
    })
    .await
    .map_err(|error| format!("{task_error_prefix}: {error}"))
}

/// The shared descriptor-bearing Output exchange. The caller retains control
/// of acknowledgement/discard policy; this helper only transfers and decodes
/// the launcher receipt.
pub(crate) struct OutputExchange {
    pub(crate) socket: PathBuf,
    pub(crate) instance: String,
    pub(crate) export_id: String,
    pub(crate) incarnation: String,
    pub(crate) sequence: u64,
    pub(crate) target: String,
    pub(crate) output_file: fs::File,
    pub(crate) task_error_prefix: &'static str,
    pub(crate) transfer_error: &'static str,
    pub(crate) unexpected_error: &'static str,
    pub(crate) refusal_error: &'static str,
}

pub(crate) async fn output_photo(
    exchange: OutputExchange,
) -> Result<slipstream_processing::photo::OutputReceipt, String> {
    let OutputExchange {
        socket,
        instance,
        export_id,
        incarnation,
        sequence,
        target,
        output_file,
        task_error_prefix,
        transfer_error,
        unexpected_error,
        refusal_error,
    } = exchange;
    tokio::task::spawn_blocking(
        move || -> Result<slipstream_processing::photo::OutputReceipt, String> {
            let request = Request::Output {
                mode: PHOTO_MODE.to_owned(),
                version: PHOTO_PROTOCOL_VERSION,
                instance,
                export_id,
                incarnation,
                sequence,
                target,
            };
            let response =
                photo::request_with_descriptor(&socket, &request, output_file.as_raw_fd())
                    .map_err(|_| transfer_error.to_owned())?;
            match response {
                slipstream_processing::photo::Response::Result { result, .. } => match *result {
                    ResultBody::Output { receipt } => Ok(receipt),
                    _ => Err(unexpected_error.to_owned()),
                },
                slipstream_processing::photo::Response::Error { .. } => {
                    Err(refusal_error.to_owned())
                }
            }
        },
    )
    .await
    .map_err(|error| format!("{task_error_prefix}: {error}"))?
}

/// The canonical manifest digest of the frozen protocol: compact JSON with
/// sorted object keys over every field that affects execution, including the
/// qualified source profile. The launcher recomputes the same digest and
/// fails closed on any mismatch.
fn manifest_digest(
    snapshot: &ExportSnapshot,
    source: &ExportSourceEvidence,
    recipe: &ExportRecipePayload,
) -> String {
    manifest_digest_parts(
        &snapshot.policy_id,
        &snapshot.bundle_id,
        (&snapshot.source_profile_id, "raw", source),
        recipe,
        &snapshot.workload,
        &snapshot.workload,
    )
}

pub(crate) fn manifest_digest_parts(
    policy_id: &str,
    bundle_id: &str,
    input: (&str, &str, &ExportSourceEvidence),
    recipe: &ExportRecipePayload,
    target: &str,
    workload: &str,
) -> String {
    let (source_profile_id, source_kind, source) = input;
    use sha2::{Digest, Sha256};
    let manifest = format!(
        "{{\"bundle\":\"{}\",\"policy\":\"{}\",\"recipe\":[{},\"{}\"],\"source\":{{\"kind\":\"{}\",\"profile_id\":\"{}\",\"sha256\":\"{}\",\"size\":{}}},\"target\":\"{}\",\"workload\":\"{}\"}}",
        bundle_id,
        policy_id,
        recipe.exposure_milli_ev,
        recipe.white_balance_mode,
        source_kind,
        source_profile_id,
        source.sha256,
        source.size,
        target,
        workload,
    );
    format!("{:x}", Sha256::digest(manifest.as_bytes()))
}

fn export_target(workload: &str) -> Result<ExportTarget, String> {
    match workload {
        "development-tiff" => Ok(ExportTarget::DevelopmentTiff),
        "film-jpeg" => Ok(ExportTarget::FilmJpeg),
        _ => Err("export snapshot has an unsupported workload".to_owned()),
    }
}

#[cfg(test)]
pub(crate) mod development_tiff_decode {
    use super::*;
    use sha2::{Digest, Sha256};

    /// Writes a structurally valid Development TIFF whose single Deflate
    /// strip carries `payload`, so only the decoded content can differ
    /// between a good and a corrupt artifact. The embedded profile is the
    /// pinned accepted engine output profile.
    pub(crate) fn write_development_tiff(path: &Path, payload: &[u8]) {
        let icc: &[u8] =
            include_bytes!("../../slipstream-core/assets/prophoto-linear-g10-darktable.icc");
        let mut bytes = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
        bytes.extend_from_slice(&13_u16.to_le_bytes());
        let mut externals: Vec<u8> = Vec::new();
        let base: usize = 8 + 2 + 13 * 12 + 4;
        let mut at = base as u32;
        let entry = |tag: u16,
                     kind: u16,
                     count: u32,
                     value: u32,
                     extra: Option<&[u8]>,
                     out: &mut Vec<u8>,
                     externals: &mut Vec<u8>,
                     at: &mut u32| {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&kind.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            match extra {
                Some(blob) => {
                    out.extend_from_slice(&at.to_le_bytes());
                    externals.extend_from_slice(blob);
                    if blob.len() % 2 == 1 {
                        externals.push(0);
                    }
                    *at += u32::try_from(blob.len() + blob.len() % 2).unwrap();
                }
                None => out.extend_from_slice(&value.to_le_bytes()),
            }
        };
        let three_shorts =
            |a: u16, b: u16, c: u16| [a.to_le_bytes(), b.to_le_bytes(), c.to_le_bytes()].concat();
        entry(256, 4, 1, 2, None, &mut bytes, &mut externals, &mut at);
        entry(257, 4, 1, 1, None, &mut bytes, &mut externals, &mut at);
        entry(
            258,
            3,
            3,
            0,
            Some(&three_shorts(32, 32, 32)),
            &mut bytes,
            &mut externals,
            &mut at,
        );
        entry(259, 4, 1, 8, None, &mut bytes, &mut externals, &mut at);
        entry(262, 3, 1, 2, None, &mut bytes, &mut externals, &mut at);
        let strip_patch = bytes.len() + 8;
        entry(273, 4, 1, 0, None, &mut bytes, &mut externals, &mut at);
        entry(277, 3, 1, 3, None, &mut bytes, &mut externals, &mut at);
        entry(
            279,
            4,
            1,
            u32::try_from(payload.len()).unwrap(),
            None,
            &mut bytes,
            &mut externals,
            &mut at,
        );
        entry(284, 3, 1, 1, None, &mut bytes, &mut externals, &mut at);
        entry(
            339,
            3,
            3,
            0,
            Some(&three_shorts(3, 3, 3)),
            &mut bytes,
            &mut externals,
            &mut at,
        );
        // Resolution entries are RATIONAL, a type this walk does not read but
        // a real engine artifact always carries. A walk that refuses an unread
        // type refuses every development artifact.
        let resolution = [0x2c_u8, 0x01, 0, 0, 1, 0, 0, 0];
        entry(
            282,
            5,
            1,
            0,
            Some(&resolution),
            &mut bytes,
            &mut externals,
            &mut at,
        );
        entry(
            283,
            5,
            1,
            0,
            Some(&resolution),
            &mut bytes,
            &mut externals,
            &mut at,
        );
        entry(
            34675,
            7,
            u32::try_from(icc.len()).unwrap(),
            0,
            Some(icc),
            &mut bytes,
            &mut externals,
            &mut at,
        );
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes.extend_from_slice(&externals);
        let strip_offset: u32 = bytes.len() as u32;
        let patch_end = strip_patch + 4;
        bytes[strip_patch..patch_end].copy_from_slice(&strip_offset.to_le_bytes());
        bytes.extend_from_slice(payload);
        fs::write(path, bytes).unwrap();
    }

    /// One zlib stream made of stored deflate blocks, so the test does not
    /// need a compressor to produce a payload that must decode cleanly.
    pub(crate) fn stored_zlib(content: &[u8]) -> Vec<u8> {
        let mut stream = vec![0x78, 0x01];
        for chunk in content.chunks(65_535) {
            let length = chunk.len() as u16;
            stream.push(if chunk.len() == content.len() { 1 } else { 0 });
            stream.extend_from_slice(&length.to_le_bytes());
            stream.extend_from_slice(&(!length).to_le_bytes());
            stream.extend_from_slice(chunk);
        }
        let mut a: u32 = 1;
        let mut b: u32 = 0;
        for &byte in content {
            a = (a + u32::from(byte)) % 65_521;
            b = (b + a) % 65_521;
        }
        stream.extend_from_slice(&((b << 16) | a).to_be_bytes());
        stream
    }

    #[test]
    fn publication_requires_a_developed_payload_that_really_inflates() {
        let base = std::env::temp_dir().join(format!(
            "export-decode-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        fs::create_dir_all(&base).unwrap();

        // A structurally valid TIFF whose strip does not inflate: refused by
        // the real Development TIFF reader.
        // A stream whose stored-block header claims more bytes than follow.
        let mut corrupt = vec![0x78_u8, 0x01, 1];
        corrupt.extend_from_slice(&24_u16.to_le_bytes());
        corrupt.extend_from_slice(&(!24_u16).to_le_bytes());
        corrupt.extend_from_slice(&[0_u8; 6]);
        let corrupt_path = base.join("corrupt.tif");
        write_development_tiff(&corrupt_path, &corrupt);
        assert!(validate_development_tiff(&corrupt_path).is_err());

        // The publication path refuses before any artifact is published.
        let originals = base.join("originals");
        fs::create_dir_all(&originals).unwrap();
        fs::create_dir_all(base.join("exports")).unwrap();
        let workspace = ExportWorkspace::open(base.join("exports"), &originals).unwrap();
        let writer = workspace.begin_development_tiff("exp-corrupt").unwrap();
        fs::copy(&corrupt_path, writer.temporary_path()).unwrap();
        assert!(
            writer
                .publish(|path| validate_development_tiff(path).map(|_| ()))
                .is_err()
        );
        let artifacts = base.join("exports/artifacts");
        assert_eq!(fs::read_dir(&artifacts).unwrap().count(), 0);

        // A well-formed stored-block payload with the exact float32 RGB
        // geometry decodes through the reader and publishes; the validation
        // yields the disclosed geometry and the embedded-profile identity.
        let pixels = vec![0_u8; 2 * 3 * 4];
        let good_path = base.join("good.tif");
        write_development_tiff(&good_path, &stored_zlib(&pixels));
        let good_facts = validate_development_tiff(&good_path).unwrap();
        assert_eq!(good_facts.width, 2);
        assert_eq!(good_facts.height, 1);
        let icc: &[u8] =
            include_bytes!("../../slipstream-core/assets/prophoto-linear-g10-darktable.icc");
        assert_eq!(
            good_facts.profile_identity,
            format!("{:x}", Sha256::digest(icc))
        );
        let writer = workspace.begin_development_tiff("exp-good").unwrap();
        fs::copy(&good_path, writer.temporary_path()).unwrap();
        let published = writer
            .publish(|path| validate_development_tiff(path).map(|_| ()))
            .unwrap();
        assert_eq!(
            fs::read(&published.path).unwrap(),
            fs::read(&good_path).unwrap()
        );

        // A truncated stream that decodes to fewer samples is also refused.
        let short = stored_zlib(&pixels[..12]);
        let short_path = base.join("short.tif");
        write_development_tiff(&short_path, &short);
        assert!(validate_development_tiff(&short_path).is_err());

        let _ = fs::remove_dir_all(base);
    }
}
