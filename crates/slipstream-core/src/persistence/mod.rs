//! SQLite state ownership for the production Library core.

mod admission;
mod edit_recipe;
mod export;
mod migrations;
mod owner;
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
pub(crate) use owner::expand_library_binding;
pub use owner::{
    ActiveAssociation, AlbumWriteError, DiscoveredFingerprint, FingerprintCounts,
    FingerprintTarget, MetadataContext, MetadataRecord, MetadataStoreError, MutationError,
    ObservedSidecar, ObservedSidecarState, Persistence, PersistenceError, PhotoDecisionWriteError,
    RetainedOrphan, ScanApplication, ScanRecoveryPlan,
};
pub use schema::{SchemaError, SchemaVersion, validate_canonical_schema};
