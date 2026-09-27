//! Shared write-path plumbing: the transaction wrapper every mutation domain runs
//! inside, plus the SQLite and persistence error translations it needs.

use super::admission::{DatabaseName, StateDirectory};
use super::owner::{MutationError, PersistenceError};
use rusqlite::{Connection, ErrorCode, Transaction, TransactionBehavior};

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

pub(super) fn mutation_transaction<T>(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    operation: impl FnOnce(&Transaction<'_>) -> Result<T, MutationError>,
) -> Result<T, MutationError> {
    state
        .admit_sidecars(database_name)
        .map_err(|_| MutationError::Persistence)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| MutationError::Persistence)?;
    let result = operation(&transaction)?;
    transaction
        .commit()
        .map_err(|_| MutationError::Persistence)?;
    Ok(result)
}
