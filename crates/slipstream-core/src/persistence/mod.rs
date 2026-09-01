//! SQLite state ownership for the production Library core.

mod admission;
mod owner;
mod schema;

pub use crate::domain::{
    AlbumBrowseMember, AlbumBrowseTarget, AlbumMember, AlbumMutation, AlbumMutationResult,
    AlbumRecord, AlbumSummary, PhotoStateField, PhotoStateMutation, PhotoStateMutationResult,
    PhotoStateUndo, PhotoStateValue, PreviewSeed, PreviewSeedResult, ScanSnapshot, SelectionState,
};
pub use admission::{DatabaseName, StateDirectory, StateError, StateFileIdentity};
pub(crate) use owner::expand_library_binding;
pub use owner::{MutationError, Persistence, PersistenceError};
pub use schema::{SchemaError, SchemaVersion, validate_canonical_schema};
