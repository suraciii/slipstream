//! SQLite state ownership for the production Library core.

mod admission;
mod owner;
mod schema;

pub use crate::domain::{
    PhotoSetMember, PhotoSetMutation, PhotoSetMutationResult, PhotoSetRecord, PhotoStateField,
    PhotoStateMutation, PhotoStateMutationResult, PhotoStateUndo, PhotoStateValue, PreviewSeed,
    PreviewSeedResult, ScanSnapshot, SelectionState,
};
pub use admission::{DatabaseName, StateDirectory, StateError, StateFileIdentity};
pub use owner::{MutationError, Persistence, PersistenceError};
pub use schema::{SchemaError, SchemaVersion, validate_canonical_schema};
