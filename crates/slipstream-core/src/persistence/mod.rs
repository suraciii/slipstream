//! SQLite state ownership for the production Library core.

mod admission;
mod owner;
mod schema;

pub use crate::domain::{
    AlbumBrowseMember, AlbumBrowseTarget, AlbumMember, AlbumMembershipMutation,
    AlbumMembershipResult, AlbumMutation, AlbumMutationResult, AlbumQueryFilter, AlbumRecord,
    AlbumSummary, PhotoQuery, PhotoQueryError, PhotoRead, PhotoStateField, PhotoStateMutation,
    PhotoStateMutationResult, PhotoStateUndo, PhotoStateValue, PreviewSeed, PreviewSeedResult,
    ScanSnapshot, SelectionState,
};
pub use admission::{DatabaseName, StateDirectory, StateError, StateFileIdentity};
pub(crate) use owner::expand_library_binding;
pub use owner::{
    DiscoveredFingerprint, FingerprintCounts, FingerprintTarget, MutationError, Persistence,
    PersistenceError, ScanApplication, ScanRecoveryPlan,
};
pub use schema::{SchemaError, SchemaVersion, validate_canonical_schema};
