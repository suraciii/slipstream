//! Production Photo Library and Preview core for Slipstream.
//!
//! This crate owns domain values, Linux read-only Original File confinement,
//! Library scanning and SQLite persistence, plus bounded capture inspection,
//! native Preview extraction, derivative processing, and cache publication.
//! Application configuration, startup and shutdown, HTTP protocol mapping, and
//! Web delivery live in `slipstream-server`.

pub mod cache;
pub mod capture;
pub mod confinement;
pub mod derivative;
pub mod domain;
pub mod export;
pub mod identity;
pub mod library;
mod native;
pub mod persistence;
pub mod preview;
pub mod reconcile;
mod recovery;

#[cfg(test)]
mod test_support;

pub use cache::{
    CacheDirectory, CacheError, CachedDerivative, DEFAULT_QUEUE_CAPACITY, DEFAULT_WAITER_CAPACITY,
    DEFAULT_WORKERS, DERIVATIVE_ALGORITHM_VERSION, DerivativeFailure, DerivativeFailureKind,
    DerivativeIdentity, DerivativePriority, DerivativeResult, DerivativeScheduler,
    DerivativeSchedulerOptions, DerivativeSource, NativeWorkBudget, NativeWorkPermit,
    derivative_cache_key, manifest_identity,
};
pub use capture::{
    CaptureFact, CaptureInspectionError, CaptureMetadataState, CaptureReviewMetadata,
    CaptureTimeField, MAXIMUM_CAPTURE_METADATA_BYTES, capture_source_revision,
    inspect_review_metadata,
};
pub use confinement::{LibraryRoot, OriginalCapability, ScanLimits};
pub use derivative::{
    DISPLAY_TRANSFORM_VERSION, Derivative, DerivativeError, DerivativeProfile, DerivativeTarget,
    process_development_tiff, process_jpeg,
};
pub use domain::{
    ALBUM_MEMBERSHIP_BATCH_MAX, AlbumBrowseMember, AlbumBrowseTarget, AlbumCreationResult,
    AlbumMember, AlbumMembershipMutation, AlbumMembershipResult, AlbumMutation,
    AlbumMutationResult, AlbumQueryFilter, AlbumRecord, AlbumSummary, CaptureTimeBound,
    CheckedAlbumMutation, CheckedAlbumMutationResult, CheckedPhotoDecisionCounts,
    CheckedPhotoDecisionItem, CheckedPhotoDecisionItemResult, CheckedPhotoDecisionMutation,
    CheckedPhotoDecisionOutcome, CheckedPhotoDecisionResult, DiscoveredOriginal,
    EXPORT_AS_SHOT_WHITE_BALANCE, EXPORT_DEVELOPMENT_TIFF_WORKLOAD, EXPORT_RETENTION_SECONDS,
    EditRecipe, EditRecipeRead, EditRecipeSettings, EditRecipeWriteOutcome, ExportArtifactFacts,
    ExportAttempt, ExportExposureRange, ExportLeaseOutcome, ExportRecipePayload, ExportRecord,
    ExportRetryOutcome, ExportSettingsError, ExportSettlement, ExportSnapshot,
    ExportSourceEvidence, ExportState, ExportSubmission, ExportSubmissionResolution,
    ExportSubmitOutcome, ExportSweepResult, MAXIMUM_FOLDER_ALBUM_PHOTOS, MAXIMUM_PHOTO_RATING,
    OriginalErrorCategory, OriginalFacts, OriginalFingerprint, OriginalKind, OriginalRecord,
    OriginalScanError, PHOTO_STATE_BATCH_MAX, PhotoAlbumMembership, PhotoDecisionFacts,
    PhotoDecisionSnapshot, PhotoQuery, PhotoQueryCandidate, PhotoQueryError, PhotoQueryOrder,
    PhotoQueryProjection, PhotoQuerySource, PhotoRead, PhotoRecord, PhotoStateBatchApplied,
    PhotoStateBatchChangedElsewhere, PhotoStateBatchItem, PhotoStateBatchMissing,
    PhotoStateBatchMutation, PhotoStateBatchResult, PhotoStateField, PhotoStateMutation,
    PhotoStateMutationResult, PhotoStateUndo, PhotoStateValue, PreviewSeed, PreviewSeedResult,
    PreviewSource, PreviewState, RebindEditRecipe, RelativeOriginalPath, SaveEditRecipe,
    ScanResult, ScanSnapshot, SelectionState, WhiteBalanceIntent, export_submission_payload_digest,
};
pub use export::{
    ArtifactWriter, ExportError, ExportTarget, ExportWorkspace, MAXIMUM_EXPORT_BYTES,
    PublishedArtifact, StagedOriginal, StagedOriginalFacts,
};
pub use identity::{InvalidModificationTime, original_id, source_revision, standalone_photo_id};
pub use library::{
    Library, LibraryConfig, LibraryError, ScanOutcome, ScanPhase, ScanProgress, expand_library,
};
pub use native::{
    InspectedPreview, InspectedPreviewSource, NativePreview, NativePreviewError, PreviewError,
    extract_embedded_jpeg, inspect_matching_jpeg, inspect_preview_source,
};
pub use persistence::{AlbumWriteError, MutationError, PhotoDecisionWriteError};
pub use preview::{
    DEFAULT_PREVIEW_QUEUE_CAPACITY, DEFAULT_PREVIEW_WAITER_CAPACITY, DEFAULT_PREVIEW_WORKERS,
    PreviewFacts, PreviewFailure, PreviewFailureKind, PreviewReady, PreviewRequestResult,
    PreviewService, PreviewServiceError, PreviewServiceOptions, PreviewUnavailable,
    PreviewUnavailableReason,
};
pub use reconcile::{ReconciledPhoto, preview_should_preserve, reconcile, selected_source};
pub use recovery::{
    AppliedRelocations, ManualOutcome, ManualProposal, RecoveryProgress, RecoverySurvey,
    RequestedRelocation, RetireSummary, UnavailablePhotoRecord, digest_bytes,
    evidence_original_ids, parse_location_prefix, plan_manual_relocations, plan_recovery,
    plan_single_relocation,
};
