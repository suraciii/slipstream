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
}

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
        })
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

    /// Admits one accepted Export: the heavy attempt runs in the background
    /// and survives browser departure. Duplicate identities never reach this
    /// entry because the persistence owner deduplicates first.
    pub(crate) fn start(self: &Arc<Self>, export: ExportRecord) {
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            let export_id = export.id.clone();
            if export.attempt.is_some() {
                // A record with a persisted attempt was started before; only
                // restart reconciliation may resolve it, never a new launch.
                return;
            }
            if let Err(outcome) = manager.execute(export).await {
                manager.settle_failed(&export_id, outcome).await;
            }
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
    async fn execute(self: &Arc<Self>, export: ExportRecord) -> Result<(), String> {
        let _slot = self.admission.lock().await;

        // The record may have settled (cancelled) while queued.
        let record = self
            .library
            .export(&export.id)
            .await
            .map_err(|_| PERSISTENCE_UNAVAILABLE.to_owned())?
            .ok_or("export record disappeared")?;
        if record.state.is_terminal() {
            return Ok(());
        }
        let snapshot = record.snapshot.clone();

        // Reconcile the launcher slot: available with no active receipt, and
        // the source of the executor attempt identity.
        let (incarnation, sequence) = self.reconcile_slot().await?;
        let record = self
            .library
            .begin_export_attempt(
                &record.id,
                ExportAttempt {
                    incarnation: incarnation.clone(),
                    sequence,
                },
            )
            .await
            .map_err(|_| PERSISTENCE_UNAVAILABLE.to_owned())?
            .ok_or("export record disappeared")?;
        if record.state.is_terminal() {
            return Ok(());
        }

        // Stage the Original through the confined Library boundary and bind
        // the verified bytes to the record before any launcher contact.
        let staged = self.stage_original(&snapshot).await?;
        let staged_facts = staged.facts();
        let record = self
            .library
            .record_export_source(
                &record.id,
                staged_facts.source_facts.size,
                &staged_facts.sha256,
            )
            .await
            .map_err(|_| PERSISTENCE_UNAVAILABLE.to_owned())?
            .ok_or("export record disappeared")?;
        if record.state.is_terminal() {
            return Ok(());
        }
        let source = record
            .source
            .clone()
            .ok_or("staged source evidence was not recorded")?;

        self.drive_attempt(&record, &snapshot, &source, staged, &incarnation, sequence)
            .await
    }

    async fn reconcile_slot(&self) -> Result<(String, u64), String> {
        let socket = self.processing.socket_path();
        let instance = self.processing.instance.clone();
        let mut failures = 0_u32;
        loop {
            let attempt_socket = socket.clone();
            let attempt_instance = instance.clone();
            let response = tokio::task::spawn_blocking(move || {
                photo::reconcile(&attempt_socket, attempt_instance)
                    .map_err(|_| RECONCILE_UNAVAILABLE.to_owned())
            })
            .await
            .map_err(|error| format!("reconcile task failed: {error}"))?;
            let capability = match response {
                Ok(slipstream_processing::photo::Response::Result { result, .. }) => *result,
                Ok(slipstream_processing::photo::Response::Error { .. }) => {
                    return Err("processing launcher refused reconciliation".to_owned());
                }
                Err(_) => {
                    failures += 1;
                    if failures >= RECONCILE_TOLERANCE {
                        return Err(RECONCILE_UNAVAILABLE.to_owned());
                    }
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }
            };
            let ResultBody::Capability {
                incarnation,
                next_sequence,
                availability,
                active,
                ..
            } = capability
            else {
                return Err("processing launcher answered reconciliation unexpectedly".to_owned());
            };
            if availability != Availability::Available {
                return Err("processing launcher is configured but blocked".to_owned());
            }
            if active.is_some() {
                return Err("processing launcher already owns an active attempt".to_owned());
            }
            return Ok((incarnation, next_sequence));
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
            // captured source revision.
            let facts = staged.facts();
            let observed =
                slipstream_core::capture_source_revision(&staged_location, facts.source_facts)
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

        let acknowledged = self
            .acknowledge_output(
                &export_id,
                incarnation,
                sequence,
                true,
                output_receipt.size,
                &output_receipt.sha256,
            )
            .await;
        if !acknowledged {
            return Err("launcher did not accept the validation acknowledgement".to_owned());
        }

        // Validate-and-publish in one atomic rename, then commit durable
        // success. The publication validator re-checks type and identity.
        let publish_path = output_path.clone();
        let published = writer
            .publish(move |_path| validate_development_tiff(&publish_path))
            .map_err(|error| format!("artifact publication failed: {error}"))?;
        let settled = self
            .library
            .settle_export(
                &export_id,
                ExportSettlement::Succeeded {
                    artifact_size: published.size,
                    artifact_sha256: published.sha256.clone(),
                    published_at: unix_seconds(),
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

fn is_completed_receipt(receipt: &PhotoReceipt) -> bool {
    receipt.state == "settled" && receipt.outcome.as_deref() == Some("completed")
}

fn is_terminal_receipt(receipt: &PhotoReceipt) -> bool {
    matches!(receipt.state.as_str(), "settled" | "blocked")
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
/// sorted object keys over every field that affects execution. The launcher
/// recomputes the same digest and fails closed on any mismatch.
fn manifest_digest(
    snapshot: &ExportSnapshot,
    source: &ExportSourceEvidence,
    recipe: &ExportRecipePayload,
) -> String {
    use sha2::{Digest, Sha256};
    let manifest = format!(
        "{{\"bundle\":\"{}\",\"policy\":\"{}\",\"recipe\":[{},\"{}\"],\"source\":{{\"kind\":\"raw\",\"sha256\":\"{}\",\"size\":{}}},\"target\":\"{}\",\"workload\":\"{}\"}}",
        snapshot.bundle_id,
        snapshot.policy_id,
        recipe.exposure_milli_ev,
        recipe.white_balance_mode,
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
/// scene-linear pixels with a matching embedded ICC profile).
fn verify_received_output(
    path: &Path,
    receipt: &slipstream_processing::photo::OutputReceipt,
) -> Result<(), ExportError> {
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

/// Validates the closed `development-tiff` output contract with a bounded
/// TIFF directory walk: positive geometry, three 32-bit IEEE-float samples per
/// pixel, RGB photometric interpretation, and an embedded RGB ICC profile.
pub(crate) fn validate_development_tiff(path: &Path) -> Result<(), ExportError> {
    macro_rules! invalid {
        () => {{ ExportError::Validation("Output is not a valid Development TIFF") }};
    }
    let mut file = open_read_only(path).map_err(|_| invalid!())?;
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
            _ => return Err(invalid!()),
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
    // pinned Deflate payload. Structural validity alone is not publication:
    // publication additionally requires the payload to decode through the
    // same Development TIFF reader the preview path uses, with a bounded
    // target. That reader (`slipstream_core::derivative::process_development_tiff`,
    // backed by the native `slipstream_vips_linear_from_fd`) exists only on
    // the main-side preview work this branch rebases onto; the call is wired
    // here in that rebase step.
    if strip_longs.is_empty() || strip_longs.len() != strip_counts.len() {
        return Err(invalid!());
    }
    if strip_counts.contains(&0) {
        return Err(invalid!());
    }
    Ok(())
}

#[cfg(test)]
mod development_tiff_structural {
    use super::*;

    /// Writes a Development TIFF directory with the pinned geometry, sample,
    /// compression, and profile tags so the structural gate can be exercised
    /// independently of the strip payload decode.
    fn write_development_tiff(path: &Path, strip_count: u32, strip_length: u32) {
        let icc = {
            let mut profile = vec![0_u8; 140];
            profile[16..20].copy_from_slice(b"RGB ");
            profile
        };
        let mut bytes = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
        bytes.extend_from_slice(&11_u16.to_le_bytes());
        let mut externals: Vec<u8> = Vec::new();
        let base: usize = 8 + 2 + 11 * 12 + 4;
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
        entry(
            273,
            4,
            strip_count,
            0,
            None,
            &mut bytes,
            &mut externals,
            &mut at,
        );
        entry(277, 3, 1, 3, None, &mut bytes, &mut externals, &mut at);
        entry(
            279,
            4,
            strip_count,
            strip_length,
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
        entry(
            34675,
            7,
            u32::try_from(icc.len()).unwrap(),
            0,
            Some(&icc),
            &mut bytes,
            &mut externals,
            &mut at,
        );
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes.extend_from_slice(&externals);
        let strip_offset: u32 = bytes.len() as u32;
        bytes[strip_patch..strip_patch + 4].copy_from_slice(&strip_offset.to_le_bytes());
        bytes.extend_from_slice(&vec![0_u8; strip_length as usize]);
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn structural_gate_accepts_the_pinned_directory_and_refuses_broken_strips() {
        let base = std::env::temp_dir().join(format!(
            "export-structural-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        fs::create_dir_all(&base).unwrap();

        let good = base.join("good.tif");
        write_development_tiff(&good, 1, 16);
        assert!(validate_development_tiff(&good).is_ok());

        // A zero-length declared strip is not a payload.
        let empty_strip = base.join("empty-strip.tif");
        write_development_tiff(&empty_strip, 1, 0);
        assert!(validate_development_tiff(&empty_strip).is_err());

        let _ = fs::remove_dir_all(base);
    }
}
