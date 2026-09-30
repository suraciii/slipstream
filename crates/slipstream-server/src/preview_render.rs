//! Ephemeral preview-class rendering through the same closed production
//! launcher workload an Export uses.
//!
//! This module owns the ephemeral attempt lifecycle — attempt identity,
//! cooperative cancellation, following, abandonment, and discard — and the
//! temporary staging bytes one attempt owns. It creates no durable Export
//! row, publication claim, or retained-output reservation: the durable
//! Export lifecycle keeps those, and the Preview registry keeps rendition
//! admission, retention deadlines, and the sweep that enforces them. The
//! execution resources stay single: one Library, one launcher admission
//! slot, and one confined workspace, reached through the ExportManager's
//! narrow operations.

use crate::export_manager::output_validation::{
    OUTPUT_VALIDATION_FAILED, open_read_only, validate_output, verify_received_output,
};
use crate::export_manager::{
    AttemptCancel, DevelopmentTiffFacts, ExportManager, OutputExchange, StartExchange,
    is_completed_receipt, is_terminal_receipt, manifest_digest_parts, open_writable, output_photo,
    start_photo,
};
use slipstream_core::{
    DEVELOPMENT_PREVIEW_LONG_EDGE, ExportExposureRange, ExportRecipePayload, ExportSourceEvidence,
    ExportTarget, Library,
};
use slipstream_processing::{
    photo::PhotoReceipt,
    photo_profile::{APPROVED_EXPOSURE_MILLI_EV_MAX, APPROVED_EXPOSURE_MILLI_EV_MIN},
    protocol::PHOTO_WORKLOAD_PROXY_FILM,
};
use std::{
    io::Write,
    os::{
        fd::AsRawFd,
        unix::fs::{OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// How many times one abandoned preview attempt retries a cancel whose answer
/// was lost. The cancel is idempotent, so a retry is safe and bounded.
const ABANDON_TOLERANCE: u32 = 3;

/// Consecutive launcher transport failures tolerated while a preview attempt
/// is followed before it settles failed with an actionable reason. A live
/// launcher-owned attempt cannot outlast this window of silence.
const LAUNCHER_FAILURE_TOLERANCE: u32 = 120;

/// A cooperative cancellation marker for one preview-class launcher attempt.
/// The gate flips it when a newer intent supersedes the attempt; the runner
/// checks it between blocking exchanges and settles the launcher receipt
/// before discarding any private output.
#[derive(Clone, Default)]
pub(crate) struct PreviewCancellation {
    cancelled: Arc<AtomicBool>,
}

impl PreviewCancellation {
    pub(crate) fn from_token(cancelled: Arc<AtomicBool>) -> Self {
        Self { cancelled }
    }
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

/// The render-time identity inputs of one Original preview attempt: the
/// selectors it was admitted under and the source, recipe, and bundle facts
/// the attempt re-derived while it ran. The caller maps these into its own
/// Preview identity; this evidence never carries that vocabulary.
pub(crate) struct RenderedIdentity {
    pub(crate) stage: &'static str,
    pub(crate) settings: &'static str,
    pub(crate) bundle_sha256: String,
    pub(crate) source_revision: String,
    pub(crate) recipe_revision: Option<String>,
    pub(crate) exposure_milli_ev: i64,
}

/// The typed rendered-output evidence of one Original preview attempt: the
/// validated output's private location and identity, the staged source
/// evidence it rendered from, and the identity inputs observed at render
/// time.
pub(crate) struct RenderedPreview {
    pub(crate) attempt_key: String,
    pub(crate) path: PathBuf,
    pub(crate) size: u64,
    pub(crate) sha256: String,
    pub(crate) identity: RenderedIdentity,
    pub(crate) output_facts: DevelopmentTiffFacts,
    pub(crate) source_size: u64,
    pub(crate) source_sha256: String,
    pub(crate) source_profile_id: String,
    pub(crate) source_relative_path: String,
}

/// The rendered output of one proxy Film attempt: the validated JPEG's
/// private location and identity. A proxy rendition derives its identity
/// from the proxy record it rendered, not from the render.
pub(crate) struct RenderedProxyFilm {
    pub(crate) attempt_key: String,
    pub(crate) path: PathBuf,
    pub(crate) sha256: String,
    pub(crate) output_facts: DevelopmentTiffFacts,
}

/// A service-minted preview attempt identity is opaque to the launcher and
/// uses only its closed lower-case identifier alphabet.
static PREVIEW_ATTEMPT_COUNTER: AtomicU64 = AtomicU64::new(0);

fn preview_attempt_key() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = PREVIEW_ATTEMPT_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("prev-{}-{nanos}-{sequence}", std::process::id())
}

/// The ephemeral preview render executor. It holds the same single
/// instances the durable Export lifecycle runs on — the serialized Library
/// and the ExportManager that owns the launcher admission and the confined
/// workspace — and owns nothing beyond the attempt lifecycle.
#[derive(Clone)]
pub(crate) struct PreviewRender {
    library: Arc<Library>,
    exports: Arc<ExportManager>,
}

impl PreviewRender {
    pub(crate) fn new(library: Arc<Library>, exports: Arc<ExportManager>) -> Self {
        Self { library, exports }
    }

    /// Runs one preview-class attempt through the same closed
    /// `development-tiff` workload as an Export. No persistence row,
    /// retained-output reservation, or artifact publication is touched.
    pub(crate) async fn render(
        &self,
        photo_id: &str,
        stage: &'static str,
        settings: &'static str,
        cancellation: PreviewCancellation,
    ) -> Result<RenderedPreview, String> {
        let _slot = self.exports.acquire_heavy_slot().await;
        if cancellation.is_cancelled() {
            return Err("preview render cancelled".to_owned());
        }
        self.exports.ensure_admissible().await?;
        if cancellation.is_cancelled() {
            return Err("preview render cancelled".to_owned());
        }
        let (incarnation, sequence) = self.exports.reconcile_slot().await?;
        if cancellation.is_cancelled() {
            return Err("preview render cancelled".to_owned());
        }
        let binding = self.exports.launcher_binding();

        let photo = self
            .library
            .photo(photo_id)
            .await
            .map_err(|_| "Photo facts could not be read".to_owned())?
            .ok_or_else(|| "Photo disappeared before preview admission".to_owned())?;
        let read = self
            .library
            .edit_recipe(photo_id)
            .await
            .map_err(|_| "Edit recipe could not be read".to_owned())?
            .ok_or_else(|| "Edit recipe facts disappeared before preview admission".to_owned())?;
        if !read.source_available || !photo.original_available {
            return Err("the Original is unavailable".to_owned());
        }
        let Some(source_revision) = read.current_source_revision.clone() else {
            return Err("source facts are pending publication".to_owned());
        };
        // The baseline selector names the processing baseline itself: 0 EV
        // against the documented baseline and as-shot white balance,
        // independently of the saved recipe. A Photo without a saved recipe
        // is that same baseline.
        let baseline = settings == "baseline";
        let target = if stage == "film" {
            ExportTarget::FilmJpeg
        } else {
            ExportTarget::DevelopmentTiff
        };
        let workload = target.workload();
        let recipe = match read.recipe.as_ref() {
            Some(recipe) if !baseline => ExportRecipePayload::capture(
                &recipe.settings,
                ExportExposureRange {
                    minimum_milli_ev: APPROVED_EXPOSURE_MILLI_EV_MIN,
                    maximum_milli_ev: APPROVED_EXPOSURE_MILLI_EV_MAX,
                },
            )
            .map_err(|_| "captured recipe is not representable by the execution payload")?,
            _ => ExportRecipePayload {
                exposure_milli_ev: 0,
                white_balance_mode: "as-shot",
            },
        };
        let recipe_revision = match read.recipe.as_ref() {
            Some(recipe) if !baseline => Some(recipe.revision.clone()),
            _ => None,
        };
        let exposure_milli_ev = recipe.exposure_milli_ev;
        let (staged, source_profile_id) = self
            .exports
            .stage_preview_original(
                photo_id,
                &source_revision,
                photo.original_kind,
                &photo.filename,
            )
            .await?;
        if cancellation.is_cancelled() {
            drop(staged);
            return Err("preview render cancelled".to_owned());
        }
        let staged_facts = staged.facts();
        let source_size = staged_facts.source_facts.size;
        let source_sha256 = staged_facts.sha256.clone();
        let source = ExportSourceEvidence {
            size: source_size,
            sha256: source_sha256.clone(),
        };
        let attempt_key = preview_attempt_key();
        let manifest_sha256 = manifest_digest_parts(
            &binding.policy_sha256,
            &binding.bundle_sha256,
            (&source_profile_id, "raw", &source),
            &recipe,
            workload,
            workload,
        );

        // Keep the staged descriptor open until Start returns, exactly like
        // the Export path. The launcher copies and hashes the source before
        // releasing its worker.
        let recipe_digest = recipe.digest();
        let start_result = start_photo(StartExchange {
            path: staged.path().to_path_buf(),
            socket: binding.socket_path.clone(),
            instance: binding.instance.clone(),
            export_id: attempt_key.clone(),
            incarnation: incarnation.clone(),
            sequence,
            policy: binding.policy_sha256.clone(),
            bundle: binding.bundle_sha256.clone(),
            workload: workload.to_owned(),
            source_kind: "raw".to_owned(),
            source_profile_id: source_profile_id.clone(),
            source,
            recipe,
            recipe_digest,
            manifest_sha256,
            task_error_prefix: "preview launcher task failed",
            open_error: "staged source could not be opened",
            refusal_error: "launcher refused the preview start",
        })
        .await;
        let start_result = match start_result {
            Ok(result) => result,
            Err(error) => {
                self.abandon_attempt(&attempt_key, &incarnation, sequence)
                    .await;
                drop(staged);
                return Err(error);
            }
        };
        if let Err(error) = start_result {
            self.abandon_attempt(&attempt_key, &incarnation, sequence)
                .await;
            drop(staged);
            return Err(error);
        }
        drop(staged);

        let receipt = match self
            .follow_attempt(&attempt_key, &incarnation, sequence, &cancellation)
            .await
        {
            Ok(receipt) => receipt,
            Err(error) => {
                self.abandon_attempt(&attempt_key, &incarnation, sequence)
                    .await;
                return Err(error);
            }
        };
        if !is_completed_receipt(&receipt) {
            return Err(format!(
                "preview processing attempt did not complete: {}",
                receipt.outcome.unwrap_or_else(|| "unknown".to_owned())
            ));
        }
        if cancellation.is_cancelled() {
            self.abandon_attempt(&attempt_key, &incarnation, sequence)
                .await;
            return Err("preview render cancelled".to_owned());
        }

        let writer = match self.exports.begin_preview_output(&attempt_key, target) {
            Ok(writer) => writer,
            Err(error) => {
                self.abandon_attempt(&attempt_key, &incarnation, sequence)
                    .await;
                return Err(format!("preview output staging failed: {error}"));
            }
        };
        let output_path = writer.temporary_path().to_path_buf();
        let output_file = match open_writable(&output_path) {
            Ok(file) => file,
            Err(error) => {
                self.abandon_attempt(&attempt_key, &incarnation, sequence)
                    .await;
                return Err(format!("preview output could not be opened: {error}"));
            }
        };
        let output_receipt_result = output_photo(OutputExchange {
            socket: binding.socket_path.clone(),
            instance: binding.instance.clone(),
            export_id: attempt_key.clone(),
            incarnation: incarnation.clone(),
            sequence,
            target: workload.to_owned(),
            output_file,
            task_error_prefix: "preview output task failed",
            transfer_error: "launcher could not transfer the preview output",
            unexpected_error: "launcher answered the preview output unexpectedly",
            refusal_error: "launcher refused the preview output transfer",
        })
        .await;
        let output_receipt = match output_receipt_result {
            Ok(receipt) => receipt,
            Err(error) => {
                self.abandon_attempt(&attempt_key, &incarnation, sequence)
                    .await;
                return Err(error);
            }
        };
        let validation_path = output_path.clone();
        let validation_receipt = output_receipt.clone();
        let validation_result = tokio::task::spawn_blocking(move || {
            verify_received_output(&validation_path, &validation_receipt, target)
        })
        .await;
        let output_facts = match validation_result {
            Ok(Ok(facts)) => facts,
            Ok(Err(_)) => {
                self.discard_attempt(&attempt_key, &incarnation, sequence, Some(&output_receipt))
                    .await;
                return Err(OUTPUT_VALIDATION_FAILED.to_owned());
            }
            Err(error) => {
                self.discard_attempt(&attempt_key, &incarnation, sequence, Some(&output_receipt))
                    .await;
                return Err(format!("preview validation task failed: {error}"));
            }
        };
        let published = match writer.publish(|path| validate_output(path, target).map(|_| ())) {
            Ok(published) => published,
            Err(error) => {
                self.discard_attempt(&attempt_key, &incarnation, sequence, Some(&output_receipt))
                    .await;
                return Err(format!("preview output publication failed: {error}"));
            }
        };
        if cancellation.is_cancelled() {
            self.discard_attempt(&attempt_key, &incarnation, sequence, Some(&output_receipt))
                .await;
            return Err("preview render cancelled".to_owned());
        }
        if !self
            .exports
            .acknowledge_output(
                &attempt_key,
                &incarnation,
                sequence,
                true,
                output_receipt.size,
                &output_receipt.sha256,
            )
            .await
        {
            self.discard_attempt(&attempt_key, &incarnation, sequence, Some(&output_receipt))
                .await;
            return Err("preview output acknowledgement failed".to_owned());
        }
        Ok(RenderedPreview {
            attempt_key,
            path: published.path,
            size: published.size,
            sha256: published.sha256,
            identity: RenderedIdentity {
                stage,
                settings,
                bundle_sha256: binding.bundle_sha256.clone(),
                source_revision,
                recipe_revision,
                exposure_milli_ev,
            },
            output_facts,
            source_size,
            source_sha256,
            source_profile_id,
            source_relative_path: self
                .exports
                .resolve_source_location(photo_id)
                .map(|path| path.as_str().to_owned())
                .unwrap_or_default(),
        })
    }

    /// Runs one qualified Film attempt over a Development Proxy frame. The
    /// frame is the only source descriptor the launcher sees; the workload's
    /// closed plan admits `development-proxy` and requires the zero exposure
    /// recipe because the transform happened before this crossing.
    pub(crate) async fn render_proxy_film(
        &self,
        exposure_milli_ev: i64,
        frame_path: &Path,
        source_profile_id: &str,
        cancellation: PreviewCancellation,
    ) -> Result<RenderedProxyFilm, String> {
        let _slot = self.exports.acquire_heavy_slot().await;
        if cancellation.is_cancelled() {
            return Err("preview render cancelled".to_owned());
        }
        self.exports.ensure_admissible().await?;
        let (incarnation, sequence) = self.exports.reconcile_slot().await?;
        let binding = self.exports.launcher_binding();
        let workload = PHOTO_WORKLOAD_PROXY_FILM;
        let target = ExportTarget::FilmJpeg;
        let recipe = ExportRecipePayload {
            // `proxy-film` deliberately has a zero-EV payload. Apply the
            // saved exposure to a private scene-linear handoff first.
            exposure_milli_ev: 0,
            white_balance_mode: "as-shot",
        };
        let attempt_key = preview_attempt_key();
        let input_writer = self
            .exports
            .begin_preview_output(
                &format!("{attempt_key}-input"),
                ExportTarget::DevelopmentTiff,
            )
            .map_err(|error| format!("proxy Film input staging failed: {error}"))?;
        let input_path = input_writer.temporary_path().to_path_buf();
        let input_output_path = input_path.clone();
        let source_path = frame_path.to_path_buf();
        let (transformed_size, transformed_sha256) =
            tokio::task::spawn_blocking(move || -> Result<(u64, String), String> {
                let file = open_read_only(&source_path)
                    .map_err(|_| "staged proxy frame could not be opened".to_owned())?;
                let frame = slipstream_core::decode_development_frame(
                    file.as_raw_fd(),
                    DEVELOPMENT_PREVIEW_LONG_EDGE,
                )
                .map_err(|_| "staged proxy frame could not be decoded".to_owned())?;
                let mut frame = frame;
                slipstream_core::apply_proxy_exposure(&mut frame, exposure_milli_ev)
                    .map_err(|_| "proxy exposure could not be applied".to_owned())?;
                let encoded = slipstream_core::encode_development_frame(&frame)
                    .map_err(|_| "proxy exposure handoff could not be encoded".to_owned())?;
                let size = encoded.len() as u64;
                let sha256 = {
                    use sha2::{Digest, Sha256};
                    format!("{:x}", Sha256::digest(&encoded))
                };
                stage_proxy_film_input(&input_output_path, &encoded)?;
                Ok((size, sha256))
            })
            .await
            .map_err(|error| format!("proxy Film input task failed: {error}"))??;
        let frame_path = input_path;
        let source = ExportSourceEvidence {
            size: transformed_size,
            sha256: transformed_sha256,
        };
        let manifest_sha256 = manifest_digest_parts(
            &binding.policy_sha256,
            &binding.bundle_sha256,
            (source_profile_id, "development-proxy", &source),
            &recipe,
            workload,
            workload,
        );
        let recipe_digest = recipe.digest();
        let start_result = start_photo(StartExchange {
            path: frame_path.to_path_buf(),
            socket: binding.socket_path.clone(),
            instance: binding.instance.clone(),
            export_id: attempt_key.clone(),
            incarnation: incarnation.clone(),
            sequence,
            policy: binding.policy_sha256.clone(),
            bundle: binding.bundle_sha256.clone(),
            workload: workload.to_owned(),
            source_kind: "development-proxy".to_owned(),
            source_profile_id: source_profile_id.to_owned(),
            source: source.clone(),
            recipe,
            recipe_digest,
            manifest_sha256,
            task_error_prefix: "proxy Film launcher task failed",
            open_error: "staged proxy frame could not be opened",
            refusal_error: "launcher refused the proxy Film start",
        })
        .await?;
        if let Err(error) = start_result {
            self.abandon_attempt(&attempt_key, &incarnation, sequence)
                .await;
            return Err(error);
        }
        // The launcher copied and hashed the sealed handoff during Start; the
        // private temporary has no further consumer and is removed now. Every
        // earlier return dropped the writer the same way.
        drop(input_writer);
        let _receipt = match self
            .follow_attempt(&attempt_key, &incarnation, sequence, &cancellation)
            .await
        {
            Ok(receipt) if is_completed_receipt(&receipt) => receipt,
            Ok(receipt) => {
                self.abandon_attempt(&attempt_key, &incarnation, sequence)
                    .await;
                return Err(format!(
                    "proxy Film attempt did not complete: {}",
                    receipt.outcome.unwrap_or_else(|| "unknown".to_owned())
                ));
            }
            Err(error) => {
                self.abandon_attempt(&attempt_key, &incarnation, sequence)
                    .await;
                return Err(error);
            }
        };
        let writer = match self.exports.begin_preview_output(&attempt_key, target) {
            Ok(writer) => writer,
            Err(error) => {
                self.discard_attempt(&attempt_key, &incarnation, sequence, None)
                    .await;
                return Err(format!("proxy Film output staging failed: {error}"));
            }
        };
        let output_path = writer.temporary_path().to_path_buf();
        let output_file = match open_writable(&output_path) {
            Ok(file) => file,
            Err(error) => {
                self.discard_attempt(&attempt_key, &incarnation, sequence, None)
                    .await;
                return Err(format!("proxy Film output could not be opened: {error}"));
            }
        };
        let output_receipt_result = output_photo(OutputExchange {
            socket: binding.socket_path.clone(),
            instance: binding.instance.clone(),
            export_id: attempt_key.clone(),
            incarnation: incarnation.clone(),
            sequence,
            target: workload.to_owned(),
            output_file,
            task_error_prefix: "proxy Film output task failed",
            transfer_error: "launcher could not transfer proxy Film output",
            unexpected_error: "launcher answered proxy Film output unexpectedly",
            refusal_error: "launcher refused proxy Film output transfer",
        })
        .await;
        let output_receipt = match output_receipt_result {
            Ok(receipt) => receipt,
            Err(error) => {
                self.discard_attempt(&attempt_key, &incarnation, sequence, None)
                    .await;
                return Err(error);
            }
        };
        let validation_path = output_path.clone();
        let validation_receipt = output_receipt.clone();
        let validation_result = tokio::task::spawn_blocking(move || {
            verify_received_output(&validation_path, &validation_receipt, target)
        })
        .await;
        let output_facts = match validation_result {
            Ok(Ok(facts)) => facts,
            Ok(Err(_)) => {
                self.discard_attempt(&attempt_key, &incarnation, sequence, Some(&output_receipt))
                    .await;
                return Err(OUTPUT_VALIDATION_FAILED.to_owned());
            }
            Err(error) => {
                self.discard_attempt(&attempt_key, &incarnation, sequence, Some(&output_receipt))
                    .await;
                return Err(format!("proxy Film validation task failed: {error}"));
            }
        };
        let published = match writer.publish(|path| validate_output(path, target).map(|_| ())) {
            Ok(published) => published,
            Err(error) => {
                self.discard_attempt(&attempt_key, &incarnation, sequence, Some(&output_receipt))
                    .await;
                return Err(format!("proxy Film publication failed: {error}"));
            }
        };
        if cancellation.is_cancelled() {
            self.discard_attempt(&attempt_key, &incarnation, sequence, Some(&output_receipt))
                .await;
            return Err("preview render cancelled".to_owned());
        }
        if !self
            .exports
            .acknowledge_output(
                &attempt_key,
                &incarnation,
                sequence,
                true,
                output_receipt.size,
                &output_receipt.sha256,
            )
            .await
        {
            self.discard_attempt(&attempt_key, &incarnation, sequence, Some(&output_receipt))
                .await;
            return Err("proxy Film output acknowledgement failed".to_owned());
        }
        Ok(RenderedProxyFilm {
            attempt_key,
            path: published.path,
            sha256: published.sha256,
            output_facts,
        })
    }

    /// Abandons one launcher attempt the service will never validate. The
    /// cancel is idempotent, so a lost answer is retried a bounded number of
    /// times instead of leaving the launcher holding an attempt its owner has
    /// given up on. An attempt that completed before the cancel arrived is
    /// left to the launcher's own reconciliation: the service holds no
    /// collected output to validate or reject, and a rejection without one is
    /// refused by the launcher anyway.
    async fn abandon_attempt(&self, export_id: &str, incarnation: &str, sequence: u64) {
        let attempt = slipstream_core::ExportAttempt {
            incarnation: incarnation.to_owned(),
            sequence,
        };
        for _ in 0..ABANDON_TOLERANCE {
            if !matches!(
                self.exports.cancel_attempt(export_id, &attempt).await,
                AttemptCancel::Uncertain
            ) {
                return;
            }
        }
    }

    /// Releases one preview attempt the service will not publish: the
    /// transferred output is rejected while the launcher still waits for an
    /// acknowledgement, the private output is deleted, and the attempt is
    /// abandoned. Ephemeral preview staging never outlives its admission.
    async fn discard_attempt(
        &self,
        attempt_key: &str,
        incarnation: &str,
        sequence: u64,
        receipt: Option<&slipstream_processing::photo::OutputReceipt>,
    ) {
        if let Some(receipt) = receipt {
            let _ = self
                .exports
                .acknowledge_output(
                    attempt_key,
                    incarnation,
                    sequence,
                    false,
                    receipt.size,
                    &receipt.sha256,
                )
                .await;
        }
        self.exports.delete_preview_output(attempt_key);
        self.abandon_attempt(attempt_key, incarnation, sequence)
            .await;
    }

    /// Polls one preview attempt while honoring supersession cancellation.
    /// A cancellation is resolved against the launcher receipt before the
    /// caller discards the private output.
    async fn follow_attempt(
        &self,
        export_id: &str,
        incarnation: &str,
        sequence: u64,
        cancellation: &PreviewCancellation,
    ) -> Result<PhotoReceipt, String> {
        let attempt = slipstream_core::ExportAttempt {
            incarnation: incarnation.to_owned(),
            sequence,
        };
        let mut failures = 0_u32;
        loop {
            if cancellation.is_cancelled() {
                self.abandon_attempt(export_id, incarnation, sequence).await;
                return Err("preview render cancelled".to_owned());
            }
            match self.exports.launcher_inspect(export_id, &attempt).await {
                Ok(receipt) => {
                    if is_terminal_receipt(&receipt) {
                        return Ok(receipt);
                    }
                    failures = 0;
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                Err(_) => {
                    failures += 1;
                    if failures >= LAUNCHER_FAILURE_TOLERANCE {
                        return Err(
                            "processing launcher became unreachable while the preview ran"
                                .to_owned(),
                        );
                    }
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }
}

/// Writes one proxy Film exposure handoff into the prepared temporary path
/// and seals it read-only. The launcher admits a source descriptor only when
/// the file carries no write permission bits, so the writable staging mode
/// the artifact writer created must be dropped before Start.
fn stage_proxy_film_input(path: &Path, encoded: &[u8]) -> Result<(), String> {
    let mut handoff = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
        .map_err(|_| "proxy exposure handoff could not be staged".to_owned())?;
    handoff
        .write_all(encoded)
        .and_then(|()| handoff.sync_all())
        .map_err(|_| "proxy exposure handoff could not be staged".to_owned())?;
    let mut permissions = handoff
        .metadata()
        .map_err(|_| "proxy exposure handoff could not be sealed".to_owned())?
        .permissions();
    permissions.set_mode(0o400);
    handoff
        .set_permissions(permissions)
        .map_err(|_| "proxy exposure handoff could not be sealed".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use slipstream_processing::photo::{
        DescriptorKind, DescriptorRequirement, validate_descriptor,
    };
    use std::fs;
    use std::os::unix::fs::OpenOptionsExt;

    /// The proxy Film exposure handoff is staged into a writable temporary
    /// but must satisfy the launcher's source descriptor contract before
    /// Start: sealed read-only with the exact declared size. The same bytes
    /// left in the writable staging mode are refused.
    #[test]
    fn proxy_film_input_is_sealed_read_only_for_the_launcher() {
        let base = std::env::temp_dir().join(format!(
            "proxy-film-input-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        fs::create_dir_all(&base).unwrap();
        let payload = b"proxy-film-handoff";
        let requirement = DescriptorRequirement {
            kind: DescriptorKind::Source,
            peer_uid: unsafe { libc::getuid() },
            declared_size: payload.len() as u64,
            max_bytes: 1 << 20,
        };

        // Mirror begin_preview_output: a fresh private, writable staging
        // file that the staging helper fills and seals.
        let sealed = base.join("sealed.tiff");
        fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .mode(0o600)
            .open(&sealed)
            .unwrap();
        stage_proxy_film_input(&sealed, payload).unwrap();
        let file = open_read_only(&sealed).unwrap();
        let metadata = validate_descriptor(file.as_raw_fd(), requirement).unwrap();
        assert_eq!(metadata.size, payload.len() as u64);
        assert_eq!(metadata.mode & 0o222, 0);
        drop(file);
        assert_eq!(fs::read(&sealed).unwrap(), payload);

        let unsealed = base.join("unsealed.tiff");
        fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .mode(0o600)
            .open(&unsealed)
            .unwrap();
        fs::write(&unsealed, payload).unwrap();
        let file = open_read_only(&unsealed).unwrap();
        assert!(validate_descriptor(file.as_raw_fd(), requirement).is_err());
        drop(file);

        let _ = fs::remove_dir_all(base);
    }
}
