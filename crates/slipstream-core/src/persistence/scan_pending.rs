//! Persisted Pending Folder summaries and retained changed candidate Locations.

use super::metadata_store;
use super::owner::PersistenceError;
use rusqlite::{Connection, Transaction};

const PENDING_FOLDERS_KEY: &str = "pending_folders";
const PENDING_CHANGED_PATHS_KEY: &str = "pending_changed_paths";

type PendingState = (Vec<crate::PendingFolder>, Vec<String>);

pub(super) fn read(connection: &Connection) -> Result<PendingState, PersistenceError> {
    let folders = metadata_store::read_value(connection, PENDING_FOLDERS_KEY, |_| {
        PersistenceError::Storage
    })?
    .map(|value| serde_json::from_str::<Vec<crate::PendingFolder>>(&value))
    .transpose()
    .map_err(|_| PersistenceError::Storage)?
    .unwrap_or_default();
    let paths = metadata_store::read_value(connection, PENDING_CHANGED_PATHS_KEY, |_| {
        PersistenceError::Storage
    })?
    .map(|value| serde_json::from_str::<Vec<String>>(&value))
    .transpose()
    .map_err(|_| PersistenceError::Storage)?
    .unwrap_or_default();
    Ok((folders, paths))
}

pub(super) fn write(
    transaction: &Transaction<'_>,
    pending: &[crate::PendingFolder],
    pending_changed_paths: &[String],
) -> Result<(), PersistenceError> {
    let encoded = serde_json::to_string(pending).map_err(|_| PersistenceError::Storage)?;
    metadata_store::upsert(transaction, PENDING_FOLDERS_KEY, &encoded, |_| {
        PersistenceError::Storage
    })?;
    let encoded =
        serde_json::to_string(pending_changed_paths).map_err(|_| PersistenceError::Storage)?;
    metadata_store::upsert(transaction, PENDING_CHANGED_PATHS_KEY, &encoded, |_| {
        PersistenceError::Storage
    })
}
