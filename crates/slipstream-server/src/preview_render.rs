//! Ephemeral baseline rendering for Development Proxy construction through
//! the shared local Photo Development executor.
//!
//! This module owns cooperative cancellation, staging, validation, and the
//! temporary bytes of one native render. It creates no durable Export row,
//! publication claim, or retained-output reservation. The Development Proxy
//! installs its own bounded artifact and discards these temporary bytes.
//! Execution uses the shared Library, heavy-work admission slot, and confined
//! workspace through the ExportManager's narrow operations.

use crate::export_manager::ExportManager;
use crate::export_manager::output_validation::{
    OUTPUT_VALIDATION_FAILED, validate_output, verify_developed_output,
};
use slipstream_core::{ExportTarget, Library};
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

/// One validated baseline render and the staged Original evidence used to
/// produce it. The Development Proxy owns the durable identity and retention.
pub(crate) struct RenderedPreview {
    pub(crate) attempt_key: String,
    pub(crate) path: PathBuf,
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

    /// Runs the baseline `development-tiff` workload through the shared local
    /// executor without creating a durable Export or reserving retention.
    pub(crate) async fn render(
        &self,
        photo_id: &str,
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
        let target = ExportTarget::DevelopmentTiff;
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
                0,
                cancellation.token(),
            )
            .await;
        drop(staged);
        let identity = developed?;

        // Verify the developed bytes against the executor's report and the
        // closed Development TIFF contract before the ephemeral publication.
        let validation_path = output_path.clone();
        match tokio::task::spawn_blocking(move || {
            verify_developed_output(&validation_path, &identity, target)
        })
        .await
        {
            Ok(Ok(_)) => (),
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
