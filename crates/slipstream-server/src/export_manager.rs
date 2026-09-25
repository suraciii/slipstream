//! Service-side Export orchestration: bounded staging, launcher admission,
//! validated publication, restart reconciliation, and retention sweeping.
//!
//! The durable Export record lives in the serialized persistence owner; this
//! module owns the heavy work between acceptance and settlement. It never
//! starts a replacement attempt against a possibly live one and never
//! publishes an unvalidated artifact.

use super::*;
use crate::config::ProcessingConfig;
use slipstream_core::{
    ExportAttempt, ExportError, ExportRecipePayload, ExportRecord, ExportSettlement,
    ExportSnapshot, ExportSourceEvidence, ExportState, ExportWorkspace, LibraryRoot,
    OriginalCapability, RelativeOriginalPath, StagedOriginal,
};
use slipstream_processing::{
    photo::{self, PhotoReceipt, Recipe, Request, ResultBody, Source},
    protocol::{Availability, PHOTO_MODE, PHOTO_PROTOCOL_VERSION, PHOTO_WORKLOAD},
};
use std::io::{Read, Seek, SeekFrom};

/// Resolves the published Library Location of one Photo. The closure keeps
/// ExportManager decoupled from the Application's published snapshot.
pub(crate) type SourceLocationResolver =
    Arc<dyn Fn(&str) -> Option<RelativeOriginalPath> + Send + Sync>;

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
        valid_artifact_stem(export_id).map(|stem| {
            self.workspace
                .root()
                .join("artifacts")
                .join(format!("{stem}.tiff"))
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
            if let Some(path) = self.artifact_path(export_id) {
                let _ = fs::remove_file(path);
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
enum AttemptCancel {
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
    async fn cancel_attempt(&self, export_id: &str, attempt: &ExportAttempt) -> AttemptCancel {
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

    async fn reconcile_slot(&self) -> Result<(String, u64), String> {
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
        let library_root = self.library_root.clone();
        let source_revision = snapshot.source_revision.clone();
        let relative_path = (self.resolver)(&snapshot.photo_id)
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
            Ok(staged)
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
        let staged_path = staged.path().to_path_buf();
        let socket = self.processing.socket_path();
        let instance = self.processing.instance.clone();
        let start_export_id = export_id.clone();
        let start_incarnation = incarnation.to_owned();
        let start_snapshot = snapshot.clone();
        let start_source = source.clone();
        tokio::task::spawn_blocking(move || -> Result<(), String> {
            let file = open_read_only(&staged_path)
                .map_err(|_| "staged source could not be opened".to_owned())?;
            let request = Request::Start {
                mode: PHOTO_MODE.to_owned(),
                version: PHOTO_PROTOCOL_VERSION,
                instance,
                export_id: start_export_id,
                incarnation: start_incarnation,
                sequence,
                policy: start_snapshot.policy_id.clone(),
                bundle: start_snapshot.bundle_id.clone(),
                workload: PHOTO_WORKLOAD.to_owned(),
                source: Source {
                    kind: "raw".to_owned(),
                    profile_id: start_snapshot.source_profile_id.clone(),
                    size: start_source.size,
                    sha256: start_source.sha256.clone(),
                },
                recipe: Recipe {
                    exposure_milli_ev: recipe.exposure_milli_ev,
                    white_balance_mode: recipe.white_balance_mode.to_owned(),
                },
                recipe_digest: start_snapshot.recipe_digest.clone(),
                manifest_sha256,
            };
            photo::request_with_descriptor(&socket, &request, file.as_raw_fd())
                .map(|_| ())
                .map_err(|_| "launcher refused the start".to_owned())
        })
        .await
        .map_err(|error| format!("launcher task failed: {error}"))??;
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
        let claim_is_current = record.attempt.as_ref().is_some_and(|attempt| {
            claim.as_ref() == Some(&(attempt.incarnation.clone(), attempt.sequence))
        });
        if claim_is_current {
            let published_path = self
                .artifact_path(&export_id)
                .ok_or_else(|| "the publication claim named no artifact directory".to_owned())?;
            if tokio::fs::metadata(&published_path).await.is_ok() {
                return self
                    .settle_from_published_file(&export_id, &published_path)
                    .await;
            }
            // The launcher transfer claim is spent; a second Output can
            // never arrive. The attempt fails, and a retry starts fresh.
            return Err("the claimed publication never produced its artifact".to_owned());
        }
        if let Some(published_path) = self.artifact_path(&export_id)
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
            .begin_development_tiff(&export_id)
            .map_err(|error| format!("output staging failed: {error}"))?;
        let output_path = writer.temporary_path().to_path_buf();
        let output_file = open_writable(&output_path)
            .map_err(|error| format!("output file could not be opened: {error}"))?;
        let socket = self.processing.socket_path();
        let instance = self.processing.instance.clone();
        let export_id_for_output = export_id.clone();
        let incarnation_for_output = incarnation.to_owned();
        let output_receipt = tokio::task::spawn_blocking(
            move || -> Result<slipstream_processing::photo::OutputReceipt, String> {
                let request = Request::Output {
                    mode: PHOTO_MODE.to_owned(),
                    version: PHOTO_PROTOCOL_VERSION,
                    instance,
                    export_id: export_id_for_output,
                    incarnation: incarnation_for_output,
                    sequence,
                    target: PHOTO_WORKLOAD.to_owned(),
                };
                let response =
                    photo::request_with_descriptor(&socket, &request, output_file.as_raw_fd())
                        .map_err(|_| "launcher could not transfer the output".to_owned())?;
                match response {
                    slipstream_processing::photo::Response::Result { result, .. } => {
                        match *result {
                            ResultBody::Output { receipt } => Ok(receipt),
                            _ => {
                                Err("launcher answered the output request unexpectedly".to_owned())
                            }
                        }
                    }
                    slipstream_processing::photo::Response::Error { .. } => {
                        Err("launcher refused the output transfer".to_owned())
                    }
                }
            },
        )
        .await
        .map_err(|error| format!("output task failed: {error}"))??;

        // Verify the received bytes against the launcher receipt and the
        // closed Development TIFF contract before any acknowledgement.
        let validation_path = output_path.clone();
        let validation_receipt = output_receipt.clone();
        let validation = tokio::task::spawn_blocking(move || {
            verify_received_output(&validation_path, &validation_receipt)
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
                let facts = validate_development_tiff(path)?;
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

    /// Resolves an Export from an artifact that a previous process already
    /// renamed into place before crashing: the file is validated through the
    /// same closed Development TIFF contract, hashed, and settled with its
    /// own publication time. No launcher transfer is requested.
    async fn settle_from_published_file(&self, export_id: &str, path: &Path) -> Result<(), String> {
        let hash_path = path.to_path_buf();
        let facts_and_hash = tokio::task::spawn_blocking(move || -> Result<_, String> {
            let facts = validate_development_tiff(&hash_path)
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

    async fn launcher_inspect(
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

    async fn acknowledge_output(
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

/// An attempt whose validated output waits for collection reports `settling`
/// with no outcome: the launcher records the service's acknowledgement before
/// it reports a terminal result, so waiting for `settled` here would deadlock
/// against the acknowledgement this service sends after collecting the output.
fn output_awaits_collection(receipt: &PhotoReceipt) -> bool {
    receipt.state == "settling" && receipt.outcome.is_none()
}

fn is_completed_receipt(receipt: &PhotoReceipt) -> bool {
    output_awaits_collection(receipt)
        || (receipt.state == "settled" && receipt.outcome.as_deref() == Some("completed"))
}

fn is_terminal_receipt(receipt: &PhotoReceipt) -> bool {
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

fn open_read_only(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
}

fn open_writable(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
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
    use sha2::{Digest, Sha256};
    let manifest = format!(
        "{{\"bundle\":\"{}\",\"policy\":\"{}\",\"recipe\":[{},\"{}\"],\"source\":{{\"kind\":\"raw\",\"profile_id\":\"{}\",\"sha256\":\"{}\",\"size\":{}}},\"target\":\"{}\",\"workload\":\"{}\"}}",
        snapshot.bundle_id,
        snapshot.policy_id,
        recipe.exposure_milli_ev,
        recipe.white_balance_mode,
        snapshot.source_profile_id,
        source.sha256,
        source.size,
        snapshot.workload,
        snapshot.workload,
    );
    format!("{:x}", Sha256::digest(manifest.as_bytes()))
}

/// The one actionable reason every output refusal carries. The launcher
/// retains its result for reconciliation either way.
const OUTPUT_VALIDATION_FAILED: &str = "received output failed Development TIFF validation";

/// Verifies the received output against the launcher receipt: byte length,
/// SHA-256 identity, and the closed Development TIFF contract (float32 RGB
/// scene-linear pixels with a matching embedded ICC profile). Returns the
/// validated facts on success.
fn verify_received_output(
    path: &Path,
    receipt: &slipstream_processing::photo::OutputReceipt,
) -> Result<DevelopmentTiffFacts, ExportError> {
    use sha2::{Digest, Sha256};
    let file = open_read_only(path).map_err(|_| ExportError::InvalidArtifact)?;
    let metadata = file.metadata().map_err(ExportError::Io)?;
    if metadata.len() != receipt.size || receipt.size == 0 {
        return Err(ExportError::InvalidArtifact);
    }
    let mut hasher = Sha256::new();
    let mut file = file;
    io::copy(&mut file, &mut hasher).map_err(ExportError::Io)?;
    if format!("{:x}", hasher.finalize()) != receipt.sha256 {
        return Err(ExportError::InvalidArtifact);
    }
    validate_development_tiff(path)
}

/// Validated Development TIFF facts the wire contract discloses with the
/// artifact: declared geometry and the embedded-profile identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DevelopmentTiffFacts {
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// SHA-256 of the embedded ICC profile bytes.
    pub(crate) profile_identity: String,
}

/// Validates the closed `development-tiff` output contract with a bounded
/// TIFF directory walk: positive geometry, three 32-bit IEEE-float samples per
/// pixel, RGB photometric interpretation, and an embedded RGB ICC profile.
/// Returns the validated facts on success.
pub(crate) fn validate_development_tiff(path: &Path) -> Result<DevelopmentTiffFacts, ExportError> {
    use std::os::fd::AsRawFd;
    macro_rules! invalid {
        () => {{ ExportError::Validation("Output is not a valid Development TIFF") }};
    }
    let mut file = open_read_only(path).map_err(|_| invalid!())?;
    use slipstream_core::derivative::DerivativeTarget;
    let mut header = [0_u8; 8];
    file.read_exact(&mut header).map_err(|_| invalid!())?;
    let little_endian = match &header[..4] {
        b"II\x2a\x00" => true,
        b"MM\x00\x2a" => false,
        _ => return Err(invalid!()),
    };
    let word = |bytes: [u8; 2]| {
        if little_endian {
            u16::from_le_bytes(bytes)
        } else {
            u16::from_be_bytes(bytes)
        }
    };
    let dword = |bytes: [u8; 4]| {
        if little_endian {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        }
    };
    let ifd_offset = dword([header[4], header[5], header[6], header[7]]);
    let mut entry = [0_u8; 2];
    file.seek(SeekFrom::Start(u64::from(ifd_offset)))
        .map_err(|_| invalid!())?;
    file.read_exact(&mut entry).map_err(|_| invalid!())?;
    let entry_count = word([entry[0], entry[1]]);
    if entry_count == 0 || entry_count > 512 {
        return Err(invalid!());
    }
    let mut width = 0_u32;
    let mut height = 0_u32;
    let mut bits_per_sample: Vec<u16> = Vec::new();
    let mut sample_format: Vec<u16> = Vec::new();
    let mut samples_per_pixel = 0_u16;
    let mut photometric = 0_u16;
    let mut compression = 0_u32;
    let mut strip_longs: Vec<u32> = Vec::new();
    let mut strip_counts: Vec<u32> = Vec::new();
    let mut icc: Option<(u32, u32)> = None;
    // Reads a LONG array either inline or at its external offset.
    fn long_array(
        file: &mut std::fs::File,
        little_endian: bool,
        count: u32,
        inline_value: [u8; 4],
        offset: u32,
    ) -> Option<Vec<u32>> {
        if count == 0 || count > 1_048_576 {
            return None;
        }
        if count == 1 {
            let raw = if little_endian {
                u32::from_le_bytes(inline_value)
            } else {
                u32::from_be_bytes(inline_value)
            };
            return Some(vec![raw]);
        }
        let resume = file.stream_position().ok()?;
        let mut bytes = vec![0_u8; count as usize * 4];
        file.seek(SeekFrom::Start(u64::from(offset))).ok()?;
        file.read_exact(&mut bytes).ok()?;
        file.seek(SeekFrom::Start(resume)).ok()?;
        let word = |chunk: &[u8]| {
            let raw: [u8; 4] = chunk.try_into().ok()?;
            Some(if little_endian {
                u32::from_le_bytes(raw)
            } else {
                u32::from_be_bytes(raw)
            })
        };
        bytes.chunks_exact(4).map(word).collect()
    }
    for _ in 0..entry_count {
        let mut raw = [0_u8; 12];
        file.read_exact(&mut raw).map_err(|_| invalid!())?;
        let tag = word([raw[0], raw[1]]);
        let kind = word([raw[2], raw[3]]);
        let count = dword([raw[4], raw[5], raw[6], raw[7]]);
        let type_size = match kind {
            1 | 2 | 6 | 7 => 1, // BYTE | ASCII | SBYTE | UNDEFINED
            3 | 8 => 2,         // SHORT | SSHORT
            4 | 9 => 4,         // LONG | SLONG
            // An engine artifact carries resolution, EXIF, and XMP entries
            // whose types this walk does not constrain. The contract is
            // defined by the tags read below, so an unread entry is skipped
            // rather than refusing a decodable image.
            _ => continue,
        };
        let total = count.checked_mul(type_size).ok_or(ExportError::Validation(
            "Output is not a valid Development TIFF",
        ))?;
        let inline = total <= 4;
        let mut value_bytes = [0_u8; 4];
        value_bytes.copy_from_slice(&raw[8..12]);
        let value_offset = dword(value_bytes);
        let shorts = |bytes: [u8; 4], index: usize| -> u16 {
            let pair = [bytes[index * 2], bytes[index * 2 + 1]];
            word(pair)
        };
        match tag {
            256 | 257 => {
                // LONG dimensions or one SHORT dimension.
                let dimension = if kind == 4 {
                    value_offset
                } else {
                    u32::from(shorts(value_bytes, 0))
                };
                if tag == 256 {
                    width = dimension;
                } else {
                    height = dimension;
                }
            }
            258 | 339 => {
                let values = if inline {
                    (0..count as usize)
                        .map(|index| shorts(value_bytes, index))
                        .collect::<Vec<_>>()
                } else {
                    let resume = file.stream_position().map_err(|_| invalid!())?;
                    let mut bytes = vec![0_u8; total.min(16) as usize];
                    file.seek(SeekFrom::Start(u64::from(value_offset)))
                        .map_err(|_| invalid!())?;
                    file.read_exact(&mut bytes).map_err(|_| invalid!())?;
                    file.seek(SeekFrom::Start(resume)).map_err(|_| invalid!())?;
                    (0..(bytes.len() / 2))
                        .map(|index| word([bytes[index * 2], bytes[index * 2 + 1]]))
                        .collect::<Vec<_>>()
                };
                if tag == 258 {
                    bits_per_sample = values;
                } else {
                    sample_format = values;
                }
            }
            259 => compression = value_offset,
            262 => photometric = shorts(value_bytes, 0),
            273 | 279 => {
                let Some(values) =
                    long_array(&mut file, little_endian, count, value_bytes, value_offset)
                else {
                    return Err(invalid!());
                };
                if tag == 273 {
                    strip_longs = values;
                } else {
                    strip_counts = values;
                }
            }
            277 => samples_per_pixel = shorts(value_bytes, 0),
            34675 => icc = Some((value_offset, count)),
            _ => {}
        }
    }
    if width == 0 || height == 0 {
        return Err(invalid!());
    }
    if samples_per_pixel != 3
        || bits_per_sample != vec![32, 32, 32]
        || sample_format != vec![3, 3, 3]
        || photometric != 2
        || compression != 8
    {
        return Err(invalid!());
    }
    // The embedded profile must describe RGB data; the pixels are scene-linear
    // ProPhoto RGB by the engine contract.
    let Some((offset, size)) = icc else {
        return Err(invalid!());
    };
    if !(128..=1024 * 1024).contains(&size) {
        return Err(invalid!());
    }
    let mut profile = [0_u8; 24];
    file.seek(SeekFrom::Start(u64::from(offset)))
        .map_err(|_| invalid!())?;
    file.read_exact(&mut profile).map_err(|_| invalid!())?;
    if &profile[16..20] != b"RGB " {
        return Err(invalid!());
    }
    // The declared strips must exist, agree on their lengths, and declare the
    // pinned Deflate payload.
    if strip_longs.is_empty() || strip_longs.len() != strip_counts.len() {
        return Err(invalid!());
    }
    if strip_counts.contains(&0) {
        return Err(invalid!());
    }
    // Publication additionally requires that the payload really decodes:
    // structural validity cannot prove the compressed strips inflate to the
    // declared geometry, so the artifact is read through the same bounded
    // Development TIFF reader the preview path uses before anything is
    // published.
    let derivative = slipstream_core::derivative::process_development_tiff(
        file.as_raw_fd(),
        DerivativeTarget::Thumbnail512,
    )
    .map_err(|_| invalid!())?;
    drop(derivative);
    // The profile identity is byte identity: the SHA-256 over the embedded
    // profile bytes, the same discipline the launcher pins at qualification.
    use sha2::{Digest, Sha256};
    file.seek(SeekFrom::Start(u64::from(offset)))
        .map_err(|_| invalid!())?;
    let mut hasher = Sha256::new();
    let mut remaining = usize::try_from(size).map_err(|_| invalid!())?;
    let mut buffer = [0_u8; 8192];
    while remaining > 0 {
        let chunk = remaining.min(buffer.len());
        file.read_exact(&mut buffer[..chunk])
            .map_err(|_| invalid!())?;
        hasher.update(&buffer[..chunk]);
        remaining -= chunk;
    }
    Ok(DevelopmentTiffFacts {
        width,
        height,
        profile_identity: format!("{:x}", hasher.finalize()),
    })
}

#[cfg(test)]
pub(crate) mod development_tiff_decode {
    use super::*;
    use sha2::{Digest, Sha256};

    /// Writes a structurally valid Development TIFF whose single Deflate
    /// strip carries `payload`, so only the decoded content can differ
    /// between a good and a corrupt artifact. The embedded profile is the
    /// pinned accepted source profile asset.
    pub(crate) fn write_development_tiff(path: &Path, payload: &[u8]) {
        let icc: &[u8] = include_bytes!("../../slipstream-core/assets/prophoto-linear-g10.icc");
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
        let icc: &[u8] = include_bytes!("../../slipstream-core/assets/prophoto-linear-g10.icc");
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
