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
    DerivativeSchedulerOptions, DerivativeSource, NativeWorkBudget, derivative_cache_key,
    manifest_identity,
};
pub use capture::{
    CaptureFact, CaptureInspectionError, CaptureMetadataState, CaptureReviewMetadata,
    CaptureTimeField, MAXIMUM_CAPTURE_METADATA_BYTES, inspect_review_metadata,
};
pub use confinement::{LibraryRoot, OriginalCapability, ScanLimits};
pub use derivative::{
    Derivative, DerivativeError, DerivativeProfile, DerivativeTarget, process_jpeg,
};
pub use domain::{
    ALBUM_MEMBERSHIP_BATCH_MAX, AlbumBrowseMember, AlbumBrowseTarget, AlbumMember,
    AlbumMembershipMutation, AlbumMembershipResult, AlbumMutation, AlbumMutationResult,
    AlbumRecord, AlbumSummary, DiscoveredOriginal, MAXIMUM_FOLDER_ALBUM_PHOTOS,
    OriginalErrorCategory, OriginalFacts, OriginalFingerprint, OriginalKind, OriginalRecord,
    OriginalScanError, PHOTO_STATE_BATCH_MAX, PhotoAlbumMembership, PhotoRecord,
    PhotoStateBatchApplied, PhotoStateBatchChangedElsewhere, PhotoStateBatchItem,
    PhotoStateBatchMissing, PhotoStateBatchMutation, PhotoStateBatchResult, PhotoStateField,
    PhotoStateMutation, PhotoStateMutationResult, PhotoStateUndo, PhotoStateValue, PreviewSeed,
    PreviewSeedResult, PreviewSource, PreviewState, RelativeOriginalPath, ScanResult, ScanSnapshot,
    SelectionState,
};
pub use identity::{InvalidModificationTime, original_id, source_revision, standalone_photo_id};
pub use library::{
    Library, LibraryConfig, LibraryError, ScanOutcome, ScanPhase, ScanProgress, expand_library,
};
pub use native::{
    InspectedPreview, InspectedPreviewSource, NativePreview, NativePreviewError, PreviewError,
    extract_embedded_jpeg, inspect_matching_jpeg, inspect_preview_source,
};
pub use persistence::MutationError;
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
