//! Production Library and persistence core for Slipstream.
//!
//! Issue #21 provides domain identities, Linux read-only Original confinement,
//! and the SQLite persistence foundation. HTTP and Preview processing live elsewhere.

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

pub use cache::{
    CacheDirectory, CacheError, CachedDerivative, DEFAULT_QUEUE_CAPACITY, DEFAULT_WAITER_CAPACITY,
    DEFAULT_WORKERS, DERIVATIVE_ALGORITHM_VERSION, DerivativeFailure, DerivativeFailureKind,
    DerivativeIdentity, DerivativePriority, DerivativeResult, DerivativeScheduler,
    DerivativeSchedulerOptions, DerivativeSource, NativeWorkBudget, derivative_cache_key,
    manifest_identity,
};
pub use capture::{
    CaptureFact, CaptureInspectionError, CaptureMetadataState, CaptureTimeField,
    MAXIMUM_CAPTURE_METADATA_BYTES,
};
pub use confinement::{LibraryRoot, OriginalCapability, ScanLimits};
pub use derivative::{
    Derivative, DerivativeError, DerivativeProfile, DerivativeTarget, process_jpeg,
};
pub use domain::{
    DiscoveredOriginal, OriginalErrorCategory, OriginalFacts, OriginalKind, OriginalRecord,
    OriginalScanError, PhotoRecord, PhotoSetMember, PhotoSetMutation, PhotoSetMutationResult,
    PhotoSetRecord, PhotoStateField, PhotoStateMutation, PhotoStateMutationResult, PhotoStateUndo,
    PhotoStateValue, PreviewCandidate, PreviewSeed, PreviewSeedResult, PreviewState,
    RelativeOriginalPath, ScanResult, ScanSnapshot, SelectionState,
};
pub use identity::{
    InvalidModificationTime, original_id, paired_photo_id, source_revision, standalone_photo_id,
};
pub use library::{Library, LibraryConfig, LibraryError};
pub use native::{
    InspectedPreview, InspectedPreviewSource, NativePreview, NativePreviewError, PreviewError,
    extract_embedded_jpeg, inspect_matching_jpeg, inspect_preview_source,
};
pub use persistence::MutationError;
pub use preview::{
    DEFAULT_PREVIEW_QUEUE_CAPACITY, DEFAULT_PREVIEW_WAITER_CAPACITY, DEFAULT_PREVIEW_WORKERS,
    PreviewFailure, PreviewFailureKind, PreviewReady, PreviewRequestResult, PreviewService,
    PreviewServiceError, PreviewServiceOptions, PreviewUnavailable, PreviewUnavailableReason,
};
pub use reconcile::{ReconciledPhoto, preview_should_preserve, reconcile, selected_source};
