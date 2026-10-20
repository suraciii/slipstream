//! SQLite state ownership for the production Library core.

mod admission;
mod albums;
mod codecs;
mod composable_recipe;
mod decisions;
mod development_proxy;
mod edit_recipe;
mod expansion;
mod export;
mod metadata;
mod metadata_store;
mod migrations;
mod mutation;
mod owner;
mod owner_recovery;
mod permanent_deletion;
mod processing_export;
mod queries;
mod removal;
mod scan;
mod scan_recovery;
mod schema;
#[cfg(test)]
mod test_support;
mod trash;
mod xmp;

pub use crate::domain::{
    AlbumBrowseMember, AlbumBrowseTarget, AlbumCreationResult, AlbumMember,
    AlbumMembershipMutation, AlbumMembershipResult, AlbumMutation, AlbumMutationResult,
    AlbumQueryFilter, AlbumRecord, AlbumSummary, CheckedAlbumMutation, CheckedAlbumMutationResult,
    CheckedPhotoDecisionCounts, CheckedPhotoDecisionItem, CheckedPhotoDecisionItemResult,
    CheckedPhotoDecisionMutation, CheckedPhotoDecisionOutcome, EditRecipe, EditRecipeRead,
    EditRecipeSettings, PhotoDecisionFacts, PhotoDecisionSnapshot, PhotoQuery, PhotoQueryError,
    PhotoRead, PhotoStateField, PhotoStateMutation, PhotoStateMutationResult, PhotoStateUndo,
    PhotoStateValue, PreviewSeed, PreviewSeedResult, ScanSnapshot, SelectionState,
    WhiteBalanceIntent,
};
pub use admission::{
    DatabaseName, StateDatabaseLock, StateDirectory, StateError, StateFileIdentity,
};
pub use albums::AlbumWriteError;
pub use composable_recipe::{processing_recipe_decode, processing_recipe_encode};
pub(crate) use expansion::expand_library_binding;
pub use metadata::{
    ActiveAssociation, MetadataContext, MetadataRecord, MetadataStoreError, ObservedSidecar,
    ObservedSidecarState, RetainedOrphan,
};
pub use owner::{MutationError, Persistence, PersistenceError, PhotoDecisionWriteError};
pub use scan::ScanApplication;
pub use scan_recovery::{
    DiscoveredFingerprint, FingerprintCounts, FingerprintTarget, ScanRecoveryPlan,
    ScanRelocationSource,
};
pub use schema::{SchemaError, SchemaVersion, validate_canonical_schema};
