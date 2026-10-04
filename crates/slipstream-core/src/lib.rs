//! Production Photo Library and Preview core for Slipstream.
//!
//! This crate owns domain values, Linux read-only Original File confinement,
//! Library scanning and SQLite persistence, plus bounded capture inspection,
//! native Preview extraction, derivative processing, and cache publication.
//! It also carries the durable vocabulary of composable photo-processing
//! modules, which is not yet wired into the admitted production surfaces.
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
pub mod metadata;
mod native;
pub mod native_work;
pub mod persistence;
pub mod preview;
pub mod processing;
pub mod reconcile;
mod recovery;
pub mod xmp;

#[cfg(test)]
mod test_support;

pub use cache::{
    CacheDirectory, CacheError, CachedDerivative, DEFAULT_QUEUE_CAPACITY, DEFAULT_WAITER_CAPACITY,
    DEFAULT_WORKERS, DERIVATIVE_ALGORITHM_VERSION, DerivativeFailure, DerivativeFailureKind,
    DerivativeIdentity, DerivativePriority, DerivativeResult, DerivativeScheduler,
    DerivativeSchedulerOptions, DerivativeSource, derivative_cache_key, manifest_identity,
};
pub use capture::{
    CameraIdentity, CaptureFact, CaptureInspectionError, CaptureMetadataState,
    CaptureReviewMetadata, CaptureTimeField, MAXIMUM_CAPTURE_METADATA_BYTES,
    capture_revision_matches_descriptor, capture_source_revision, inspect_review_metadata,
};
pub use confinement::{LibraryRoot, OriginalCapability, OriginalDeletionOutcome, ScanLimits};
pub use derivative::{
    DEVELOPMENT_PREVIEW_LONG_EDGE, DISPLAY_TRANSFORM_VERSION, Derivative, DerivativeError,
    DerivativeProfile, DerivativeTarget, LinearFrame, apply_proxy_exposure,
    decode_development_frame, develop_linear_frame, development_tiff_fixture,
    encode_development_frame, process_development_proxy, process_development_tiff, process_jpeg,
};
pub use domain::{
    ALBUM_MEMBERSHIP_BATCH_MAX, AlbumBrowseMember, AlbumBrowseTarget, AlbumCreationResult,
    AlbumMember, AlbumMembershipMutation, AlbumMembershipResult, AlbumMutation,
    AlbumMutationResult, AlbumQueryFilter, AlbumRecord, AlbumSummary, CaptureTimeBound,
    CheckedAlbumMutation, CheckedAlbumMutationResult, CheckedPhotoDecisionCounts,
    CheckedPhotoDecisionItem, CheckedPhotoDecisionItemResult, CheckedPhotoDecisionMutation,
    CheckedPhotoDecisionOutcome, CheckedPhotoDecisionResult, DEVELOPMENT_PROXY_LONG_EDGE,
    DEVELOPMENT_PROXY_PIPELINE_VERSION, DevelopmentProxyExpectation, DevelopmentProxyRecord,
    DiscoveredOriginal, EXPORT_AS_SHOT_WHITE_BALANCE, EXPORT_DEVELOPMENT_TIFF_WORKLOAD,
    EXPORT_FILM_JPEG_WORKLOAD, EXPORT_RETENTION_SECONDS, EditRecipe, EditRecipeRead,
    EditRecipeSettings, EditRecipeWriteOutcome, ExplicitPhotoRemovalMutation,
    ExplicitPhotoRestoreCounts, ExplicitPhotoRestoreMutation, ExplicitPhotoRestoreResult,
    ExportArtifactFacts, ExportAttempt, ExportExposureRange, ExportLeaseOutcome,
    ExportRecipePayload, ExportRecord, ExportRetryOutcome, ExportSettingsError, ExportSettlement,
    ExportSnapshot, ExportSourceEvidence, ExportState, ExportSubmission,
    ExportSubmissionResolution, ExportSubmitOutcome, ExportSweepResult,
    MAXIMUM_FOLDER_ALBUM_PHOTOS, MAXIMUM_PHOTO_RATING, OriginalErrorCategory, OriginalFacts,
    OriginalFingerprint, OriginalKind, OriginalRecord, OriginalScanError, PERMANENT_DELETION_MAX,
    PHOTO_REMOVAL_MAX, PHOTO_STATE_BATCH_MAX, PermanentDeletionItemResult,
    PermanentDeletionItemState, PermanentDeletionRejection, PermanentDeletionResult,
    PermanentDeletionReview, PermanentDeletionReviewItem, PermanentDeletionSelection,
    PermanentDeletionTarget, PermanentDeletionWorkItem, PhotoAlbumMembership, PhotoDecisionFacts,
    PhotoDecisionSnapshot, PhotoOperationRemainder, PhotoQuery, PhotoQueryCandidate,
    PhotoQueryError, PhotoQueryOrder, PhotoQueryProjection, PhotoQuerySource, PhotoRead,
    PhotoRecord, PhotoRemovalCounts, PhotoRemovalMarker, PhotoRemovalMutation, PhotoRemovalResult,
    PhotoRemovalTarget, PhotoRestoration, PhotoRestorationCounts, PhotoRestorationResult,
    PhotoStateBatchApplied, PhotoStateBatchChangedElsewhere, PhotoStateBatchItem,
    PhotoStateBatchMissing, PhotoStateBatchMutation, PhotoStateBatchResult, PhotoStateField,
    PhotoStateMutation, PhotoStateMutationResult, PhotoStateUndo, PhotoStateValue, PreviewSeed,
    PreviewSeedResult, PreviewSource, PreviewState, RebindEditRecipe, RelativeOriginalPath,
    RemovedPhotoRecord, SaveEditRecipe, ScanResult, ScanSnapshot, SelectionState,
    TrashPhotoCandidate, WhiteBalanceIntent, export_submission_payload_digest,
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
pub use native_work::{NativeWorkBudget, NativeWorkPermit};
pub use persistence::{AlbumWriteError, MutationError, PhotoDecisionWriteError};
pub use preview::{
    DEFAULT_PREVIEW_QUEUE_CAPACITY, DEFAULT_PREVIEW_WAITER_CAPACITY, DEFAULT_PREVIEW_WORKERS,
    PreviewFacts, PreviewFailure, PreviewFailureKind, PreviewReady, PreviewRequestResult,
    PreviewService, PreviewServiceError, PreviewServiceOptions, PreviewUnavailable,
    PreviewUnavailableReason,
};
pub use processing::{
    AutomaticAdjustment, ComposableEditRecipe, ComposableEditRecipeRead,
    ComposableEditRecipeWriteOutcome, ComposableRecipeRequestError, DIGEST_HEX_BYTES,
    MAXIMUM_ARTIFACT_ID_BYTES, MAXIMUM_CONTRACT_NAME_BYTES, MAXIMUM_EXPORT_REQUEST_ID_BYTES,
    MAXIMUM_GEOMETRY_EDGE, MAXIMUM_LIVE_PROCESSING_EXPORTS, MAXIMUM_MODULE_ID_BYTES,
    MAXIMUM_PARAMETER_SNAPSHOT_BYTES, MAXIMUM_PARAMETER_SNAPSHOT_DEPTH, MAXIMUM_PHOTO_ID_BYTES,
    MAXIMUM_RECIPE_STEPS, MAXIMUM_REVISION_BYTES, MAXIMUM_SOURCE_REVISION_BYTES,
    MAXIMUM_STEP_ID_BYTES, PROCESSING_ARTIFACT_RETENTION_SECONDS,
    PROCESSING_EXPORT_RECEIPT_RETENTION_SECONDS, ProcessingArtifact, ProcessingArtifactId,
    ProcessingArtifactLeaseOutcome, ProcessingArtifactPublication, ProcessingArtifactRetention,
    ProcessingContractError, ProcessingExportAdapterDecision, ProcessingExportAdmission,
    ProcessingExportAttempt, ProcessingExportAttemptOutcome, ProcessingExportCancelOutcome,
    ProcessingExportFailureOutcome, ProcessingExportIdentity, ProcessingExportList,
    ProcessingExportRefusal, ProcessingExportRequestError, ProcessingExportSettlement,
    ProcessingExportSubmitOutcome, ProcessingExportWork, ProcessingExportWorkState,
    ProcessingGeometry, ProcessingImageContract, ProcessingInput, ProcessingInputEvidence,
    ProcessingInputHandoffError, ProcessingModuleId, ProcessingParameterSnapshot,
    ProcessingPreviewIdentity, ProcessingStep, ProcessingStepId, RebindComposableEditRecipe,
    ReplayProcessingExport, RetryProcessingExport, SaveComposableEditRecipe,
    SubmitProcessingExport, validate_bounded_name, validate_digest, validate_parameter_tree,
    validate_revision, validate_source_revision,
};
pub use reconcile::{ReconciledPhoto, preview_should_preserve, reconcile, selected_source};
pub use recovery::{
    AppliedRelocations, ManualOutcome, ManualProposal, MappingBlock, RecoveryProgress,
    RecoveryRecord, RecoverySurvey, RelocationSet, RequestedRelocation, RetireSummary,
    count_prefix_scope, digest_bytes, evaluate_relocation, evidence_original_ids,
    parse_location_prefix, plan_manual_relocations, plan_recovery, plan_single_relocation,
};
pub use xmp::{XMP_CONTENT_TYPE, XMP_RETENTION_SECONDS, XmpCreateOutcome, XmpExportRecord};
