//! Shared write-path plumbing: SQLite and persistence error translation.

use super::owner::{MutationError, PersistenceError};
use rusqlite::ErrorCode;

pub(super) fn mutation_error_from_persistence(error: PersistenceError) -> MutationError {
    match error {
        PersistenceError::Saturated => MutationError::Saturated,
        PersistenceError::Closed => MutationError::Closed,
        PersistenceError::OwnerStopped
        | PersistenceError::State(_)
        | PersistenceError::RecoveryRequired
        | PersistenceError::UnsupportedSchema
        | PersistenceError::NewerSchema
        | PersistenceError::RootMismatch
        | PersistenceError::InvalidLegacyData
        | PersistenceError::InvalidExpansion
        | PersistenceError::InvalidRecovery
        | PersistenceError::InvalidRecoveryMapping { .. }
        | PersistenceError::IdCollision
        | PersistenceError::Storage => MutationError::Persistence,
    }
}

pub(super) fn mutation_error_from_sqlite(error: rusqlite::Error) -> MutationError {
    if matches!(
        error,
        rusqlite::Error::SqliteFailure(ref failure, _)
            if failure.code == ErrorCode::ConstraintViolation
    ) {
        MutationError::Conflict
    } else {
        MutationError::Persistence
    }
}
