//! The `library_metadata` key/value store: the shared internal helpers every
//! namespaced domain in this persistence layer uses to read, upsert, insert
//! only, delete, and scan its own keys.
//!
//! Every helper operates on a caller's existing `Connection` or `Transaction`
//! and never opens, commits, or rolls one back, so the caller keeps full
//! ownership of its transaction boundary. The three write forms are
//! deliberate and must stay distinct:
//!
//! - `insert_only` refuses to overwrite an existing key. A constraint
//!   violation surfaces as an error so the caller can decide what a clash
//!   means, never a silent replacement.
//! - `upsert` overwrites an existing value.
//! - `delete` removes one exact key.
//!
//! Reads distinguish "absent" (`None`) from a stored value, and namespace
//! scans are ordered by key so callers see a deterministic sequence.
//!
//! Error mapping is the caller's, so a helper called from the owner-facing
//! write path reports failures exactly like the SQL it replaces, and one
//! called from a mutation path maps them to that path's own error.

use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::ops::Deref;

/// The one table every helper addresses.
const TABLE: &str = "library_metadata";

/// A metadata access point: a `Connection`, or a `Transaction`, which derefs
/// to one. One bound serves both so the SQL lives in exactly one place per
/// operation.
pub(super) trait MetadataAccess {
    fn connection(&self) -> &Connection;
}

impl MetadataAccess for Connection {
    fn connection(&self) -> &Connection {
        self
    }
}

impl MetadataAccess for Transaction<'_> {
    fn connection(&self) -> &Connection {
        self.deref()
    }
}

/// Reads one stored value. `None` means the key is absent.
pub(super) fn read_value<A: MetadataAccess, M, E>(
    connection: &A,
    key: &str,
    map_error: M,
) -> Result<Option<String>, E>
where
    M: Fn(rusqlite::Error) -> E,
{
    connection
        .connection()
        .query_row(
            &format!("SELECT value FROM {TABLE} WHERE key=?"),
            [key],
            |row| row.get(0),
        )
        .optional()
        .map_err(map_error)
}

/// Whether one exact key exists.
pub(super) fn exists<A: MetadataAccess, M, E>(
    connection: &A,
    key: &str,
    map_error: M,
) -> Result<bool, E>
where
    M: Fn(rusqlite::Error) -> E,
{
    connection
        .connection()
        .query_row(&format!("SELECT 1 FROM {TABLE} WHERE key=?"), [key], |_| {
            Ok(())
        })
        .optional()
        .map(|found| found.is_some())
        .map_err(map_error)
}

/// Writes one value, refusing to overwrite an existing key.
pub(super) fn insert_only<M, E>(
    transaction: &Transaction<'_>,
    key: &str,
    value: &str,
    map_error: M,
) -> Result<(), E>
where
    M: Fn(rusqlite::Error) -> E,
{
    transaction
        .execute(
            &format!("INSERT INTO {TABLE}(key,value) VALUES(?,?)"),
            params![key, value],
        )
        .map_err(map_error)?;
    Ok(())
}

/// Writes one value, overwriting an existing value under the same key.
pub(super) fn upsert<M, E>(
    transaction: &Transaction<'_>,
    key: &str,
    value: &str,
    map_error: M,
) -> Result<(), E>
where
    M: Fn(rusqlite::Error) -> E,
{
    transaction
        .execute(
            &format!(
                "INSERT INTO {TABLE}(key,value) VALUES(?,?)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value"
            ),
            params![key, value],
        )
        .map_err(map_error)?;
    Ok(())
}

/// Compares and sets one existing key's value, returning the number of rows
/// changed: `1` when the stored value was the expected one, `0` otherwise.
/// The key is never inserted, so an absent key stays absent.
pub(super) fn compare_and_set<M, E>(
    transaction: &Transaction<'_>,
    key: &str,
    value: &str,
    expected_value: &str,
    map_error: M,
) -> Result<usize, E>
where
    M: Fn(rusqlite::Error) -> E,
{
    transaction
        .execute(
            &format!("UPDATE {TABLE} SET value=? WHERE key=? AND value=?"),
            params![value, key, expected_value],
        )
        .map_err(map_error)
}

/// Removes one exact key, returning the number of rows removed: `1` when the
/// key was present, `0` otherwise.
pub(super) fn delete_count<M, E>(
    transaction: &Transaction<'_>,
    key: &str,
    map_error: M,
) -> Result<usize, E>
where
    M: Fn(rusqlite::Error) -> E,
{
    transaction
        .execute(&format!("DELETE FROM {TABLE} WHERE key=?"), [key])
        .map_err(map_error)
}

/// Removes one exact key.
pub(super) fn delete<M, E>(transaction: &Transaction<'_>, key: &str, map_error: M) -> Result<(), E>
where
    M: Fn(rusqlite::Error) -> E,
{
    delete_count(transaction, key, map_error).map(|_| ())
}

/// Reads every key under one namespace, ordered by key.
pub(super) fn scan_keys<A: MetadataAccess, M, E>(
    connection: &A,
    prefix: &str,
    map_error: M,
) -> Result<Vec<String>, E>
where
    M: Fn(rusqlite::Error) -> E,
{
    let mut statement = connection
        .connection()
        .prepare(&format!(
            "SELECT key FROM {TABLE} WHERE key LIKE ? ORDER BY key"
        ))
        .map_err(&map_error)?;
    let keys = statement
        .query_map([format!("{prefix}%")], |row| row.get::<_, String>(0))
        .map_err(&map_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(&map_error)?;
    Ok(keys)
}

/// Reads every key and value under one namespace, ordered by key.
pub(super) fn scan_records<A: MetadataAccess, M, E>(
    connection: &A,
    prefix: &str,
    map_error: M,
) -> Result<Vec<(String, String)>, E>
where
    M: Fn(rusqlite::Error) -> E,
{
    let mut statement = connection
        .connection()
        .prepare(&format!(
            "SELECT key,value FROM {TABLE} WHERE key LIKE ? ORDER BY key"
        ))
        .map_err(&map_error)?;
    let rows = statement
        .query_map([format!("{prefix}%")], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(&map_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(&map_error)?;
    Ok(rows)
}
