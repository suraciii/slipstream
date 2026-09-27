//! SQLite state ownership for the production Library core.

mod admission;
mod albums;
mod decisions;
mod edit_recipe;
mod expansion;
mod export;
mod metadata;
mod migrations;
mod mutation;
mod owner;
mod queries;
mod removal;
mod scan;
mod schema;

pub use crate::domain::{
    AlbumBrowseMember, AlbumBrowseTarget, AlbumCreationResult, AlbumMember,
    AlbumMembershipMutation, AlbumMembershipResult, AlbumMutation, AlbumMutationResult,
    AlbumQueryFilter, AlbumRecord, AlbumSummary, CheckedAlbumMutation, CheckedAlbumMutationResult,
    CheckedPhotoDecisionCounts, CheckedPhotoDecisionItem, CheckedPhotoDecisionItemResult,
    CheckedPhotoDecisionMutation, CheckedPhotoDecisionOutcome, EditRecipe, EditRecipeRead,
    EditRecipeSettings, EditRecipeWriteOutcome, PhotoDecisionFacts, PhotoDecisionSnapshot,
    PhotoQuery, PhotoQueryError, PhotoRead, PhotoStateField, PhotoStateMutation,
    PhotoStateMutationResult, PhotoStateUndo, PhotoStateValue, PreviewSeed, PreviewSeedResult,
    RebindEditRecipe, SaveEditRecipe, ScanSnapshot, SelectionState, WhiteBalanceIntent,
};
pub use admission::{
    DatabaseName, StateDatabaseLock, StateDirectory, StateError, StateFileIdentity,
};
pub use albums::AlbumWriteError;
pub(crate) use expansion::expand_library_binding;
pub use metadata::{
    ActiveAssociation, MetadataContext, MetadataRecord, MetadataStoreError, ObservedSidecar,
    ObservedSidecarState, RetainedOrphan,
};
pub use owner::{MutationError, Persistence, PersistenceError, PhotoDecisionWriteError};
pub use scan::{
    DiscoveredFingerprint, FingerprintCounts, FingerprintTarget, ScanApplication, ScanRecoveryPlan,
};
pub use schema::{SchemaError, SchemaVersion, validate_canonical_schema};
