//! Production Library and persistence core for Slipstream.
//!
//! Issue #21 provides domain identities, Linux read-only Original confinement,
//! and the SQLite persistence foundation. HTTP and Preview processing live elsewhere.

pub mod confinement;
pub mod domain;
pub mod identity;
pub mod library;
pub mod persistence;
pub mod reconcile;

pub use confinement::{LibraryRoot, OriginalCapability, ScanLimits};
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
pub use persistence::MutationError;
pub use reconcile::{ReconciledPhoto, preview_should_preserve, reconcile, selected_source};
