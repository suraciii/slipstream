//! SQLite state ownership for the production Library core.

mod admission;
mod owner;
mod schema;

pub use crate::domain::{
    AlbumBrowseMember, AlbumBrowseTarget, AlbumCreationResult, AlbumMember,
    AlbumMembershipMutation, AlbumMembershipResult, AlbumMutation, AlbumMutationResult,
    AlbumQueryFilter, AlbumRecord, AlbumSummary, CheckedAlbumMutation, CheckedAlbumMutationResult,
    CheckedPhotoDecisionCounts, CheckedPhotoDecisionItem, CheckedPhotoDecisionItemResult,
    CheckedPhotoDecisionMutation, CheckedPhotoDecisionOutcome, PhotoDecisionFacts,
    PhotoDecisionSnapshot, PhotoQuery, PhotoQueryError, PhotoRead, PhotoStateField,
    PhotoStateMutation, PhotoStateMutationResult, PhotoStateUndo, PhotoStateValue, PreviewSeed,
    PreviewSeedResult, ScanSnapshot, SelectionState,
};
pub use admission::{
    DatabaseName, StateDatabaseLock, StateDirectory, StateError, StateFileIdentity,
};
pub(crate) use owner::expand_library_binding;
pub use owner::{
    AlbumWriteError, DiscoveredFingerprint, FingerprintCounts, FingerprintTarget, MutationError,
    Persistence, PersistenceError, PhotoDecisionWriteError, ScanApplication, ScanRecoveryPlan,
};
pub use schema::{SchemaError, SchemaVersion, validate_canonical_schema};
