//! Bounded Trash listing and pending-deletion status.

use super::owner::{PersistenceError, RemovedPhotoPageResult};
use super::permanent_deletion::{
    PERMANENT_DELETION_DELETED_PREFIX, unsettled_permanent_deletion_operation,
};
use crate::{PhotoOperationRemainder, RemovedPhotoRecord};
use rusqlite::{Connection, OptionalExtension, params};

/// Removed Photos whose Originals were not confirmed permanently deleted.
fn trash_rows() -> String {
    format!(
        "FROM photos p
         WHERE p.removed_at_ms IS NOT NULL
           AND p.id NOT IN (SELECT substr(m.key, {offset}) FROM library_metadata m
                            WHERE m.key LIKE '{prefix}%')",
        offset = PERMANENT_DELETION_DELETED_PREFIX.len() + 1,
        prefix = PERMANENT_DELETION_DELETED_PREFIX,
    )
}

pub(super) fn removed_photos(
    connection: &Connection,
    start: usize,
    limit: usize,
) -> RemovedPhotoPageResult {
    let remaining = trash_rows();
    let total: i64 = connection
        .query_row(&format!("SELECT COUNT(*) {remaining}"), [], |row| {
            row.get(0)
        })
        .map_err(|_| PersistenceError::Storage)?;
    let total = usize::try_from(total).map_err(|_| PersistenceError::Storage)?;
    let rows = connection
        .prepare(&format!(
            "SELECT p.id,p.removed_at_ms,p.removed_operation {remaining}
             ORDER BY p.removed_at_ms DESC,p.id
             LIMIT ? OFFSET ?"
        ))
        .map_err(|_| PersistenceError::Storage)?
        .query_map(
            params![
                i64::try_from(limit).map_err(|_| PersistenceError::Storage)?,
                i64::try_from(start).map_err(|_| PersistenceError::Storage)?,
            ],
            |row| {
                Ok((
                    RemovedPhotoRecord {
                        photo_id: row.get(0)?,
                        removed_at_ms: row.get(1)?,
                        pending_verification: None,
                    },
                    row.get::<_, Option<String>>(2)?,
                ))
            },
        )
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    let mut records = Vec::with_capacity(rows.len());
    for (mut record, _) in rows {
        record.pending_verification =
            unsettled_permanent_deletion_operation(connection, &record.photo_id)
                .map_err(|_| PersistenceError::Storage)?;
        records.push(record);
    }
    // The remainder surface names the newest removal operation that still owns
    // Trash items, and counts exactly those Photos.
    let operation = connection
        .query_row(
            &format!(
                "SELECT p.removed_operation,COUNT(*),MAX(p.removed_at_ms) {remaining}
                   AND p.removed_operation IS NOT NULL
                 GROUP BY p.removed_operation
                 ORDER BY MAX(p.removed_at_ms) DESC,p.removed_operation DESC
                 LIMIT 1"
            ),
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?
        .map(|(operation_id, removed, _)| {
            let removed = usize::try_from(removed).map_err(|_| PersistenceError::Storage)?;
            Ok::<PhotoOperationRemainder, PersistenceError>(PhotoOperationRemainder {
                operation_id,
                removed,
            })
        })
        .transpose()?;
    Ok((records, total, operation))
}
