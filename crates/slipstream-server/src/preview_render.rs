//! Ephemeral preview-class rendering through the same local Photo
//! Development executor an Export uses.
//!
//! This module owns the ephemeral attempt lifecycle — attempt identity,
//! cooperative cancellation, staging, and discard — and the temporary bytes
//! one attempt owns. It creates no durable Export row, publication claim, or
//! retained-output reservation: the durable Export lifecycle keeps those,
//! and the Preview registry keeps rendition admission, retention deadlines,
//! and the sweep that enforces them. The execution resources stay single:
//! one Library, one serialized heavy-work admission slot, and one confined
//! workspace, reached through the ExportManager's narrow operations.

use crate::export_manager::output_validation::{
    OUTPUT_VALIDATION_FAILED, validate_output, verify_developed_output,
};
use crate::export_manager::{DevelopmentTiffFacts, ExportManager};
use slipstream_core::{ExportExposureRange, ExportRecipePayload, ExportTarget, Library};
use slipstream_processing::photo_profile::{
    APPROVED_EXPOSURE_MILLI_EV_MAX, APPROVED_EXPOSURE_MILLI_EV_MIN,
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

/// A cooperative cancellation marker for one preview-class attempt.
/// The gate flips it when a newer intent supersedes the attempt; the runner
/// passes it to the local executor so the engine child is torn down, and
/// rechecks it before the ephemeral publication is ever handed over.
#[derive(Clone, Default)]
pub(crate) struct PreviewCancellation {
    cancelled: Arc<AtomicBool>,
}

impl PreviewCancellation {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// The underlying cancellation flag the local executor takes.
    fn token(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancelled)
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

/// A service-minted preview attempt identity is opaque outside the service
/// and uses only its closed lower-case identifier alphabet.
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
/// and the ExportManager that owns the local executor admission and the
/// confined workspace — and owns nothing beyond the attempt lifecycle.
#[derive(Clone)]
pub(crate) struct PreviewRender {
    library: Arc<Library>,
    exports: Arc<ExportManager>,
}

impl PreviewRender {
    pub(crate) fn new(library: Arc<Library>, exports: Arc<ExportManager>) -> Self {
        Self { library, exports }
    }

    /// Runs one preview-class attempt through the same local
    /// `development-tiff` workload as an Export. No persistence row,
    /// retained-output reservation, or durable artifact publication is
    /// touched.
    pub(crate) async fn render(
        &self,
        photo_id: &str,
        stage: &'static str,
        settings: &'static str,
        cancellation: PreviewCancellation,
    ) -> Result<RenderedPreview, String> {
        if stage == "film" {
            // The fixed Film stage has no qualified local execution path;
            // the caller reports the stage unavailable instead of admitting
            // work that can never produce a rendition.
            return Err("the Film stage is not qualified for local development".to_owned());
        }
        let _slot = self.exports.acquire_heavy_slot().await;
        if cancellation.is_cancelled() {
            return Err("preview render cancelled".to_owned());
        }
        self.exports.ensure_admissible().await?;
        if cancellation.is_cancelled() {
            return Err("preview render cancelled".to_owned());
        }
        let bundle_sha256 = self.exports.bundle_sha256().to_owned();

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
        let target = ExportTarget::DevelopmentTiff;
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
        let attempt_key = preview_attempt_key();

        // The private output the engine writes; validation gates the
        // ephemeral publication, and dropping the writer discards a partial
        // output on every refusal below.
        let writer = self
            .exports
            .begin_preview_output(&attempt_key, target)
            .map_err(|error| format!("preview output staging failed: {error}"))?;
        let output_path = writer.temporary_path().to_path_buf();

        // The same fresh-engine local development an Export runs, bounded by
        // the supersession token; the staged copy is removed once the engine
        // consumed it.
        let developed = self
            .exports
            .develop(
                staged.path().to_path_buf(),
                output_path.clone(),
                exposure_milli_ev,
                cancellation.token(),
            )
            .await;
        drop(staged);
        let identity = developed?;

        // Verify the developed bytes against the executor's report and the
        // closed Development TIFF contract before the ephemeral publication.
        let validation_path = output_path.clone();
        let output_facts = match tokio::task::spawn_blocking(move || {
            verify_developed_output(&validation_path, &identity, target)
        })
        .await
        {
            Ok(Ok(facts)) => facts,
            Ok(Err(_)) => return Err(OUTPUT_VALIDATION_FAILED.to_owned()),
            Err(error) => return Err(format!("preview validation task failed: {error}")),
        };

        let published = writer
            .publish(|path| validate_output(path, target).map(|_| ()))
            .map_err(|error| format!("preview output publication failed: {error}"))?;

        // A superseded intent never serves its rendition even when the
        // engine finished: the ephemeral publication is discarded before the
        // evidence is handed over.
        if cancellation.is_cancelled() {
            self.exports.delete_preview_output(&attempt_key);
            return Err("preview render cancelled".to_owned());
        }
        Ok(RenderedPreview {
            attempt_key,
            path: published.path,
            size: published.size,
            sha256: published.sha256,
            identity: RenderedIdentity {
                stage,
                settings,
                bundle_sha256,
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
}
