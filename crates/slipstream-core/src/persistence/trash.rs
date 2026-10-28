//! Bounded Trash listing.

use super::owner::{PersistenceError, RemovedPhotoPageResult};
use super::trash_cleanup::PERMANENT_DELETION_DELETED_PREFIX;
use crate::{PhotoOperationRemainder, RemovedPhotoRecord};
use rusqlite::{Connection, OptionalExtension, params};

const TRASH_ROWS: &str = "FROM photos p WHERE p.removed_at_ms IS NOT NULL";

pub(super) fn removed_photos(
    connection: &Connection,
    start: usize,
    limit: usize,
) -> RemovedPhotoPageResult {
    let remaining = TRASH_ROWS;
    let total: i64 = connection
        .query_row(&format!("SELECT COUNT(*) {remaining}"), [], |row| {
            row.get(0)
        })
        .map_err(|_| PersistenceError::Storage)?;
    let total = usize::try_from(total).map_err(|_| PersistenceError::Storage)?;
    let records = connection
        .prepare(&format!(
            "SELECT p.id,p.removed_at_ms {remaining}
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
                Ok(RemovedPhotoRecord {
                    photo_id: row.get(0)?,
                    removed_at_ms: row.get(1)?,
                    pending_verification: None,
                })
            },
        )
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    // Undo counts only records Restore can recover, not confirmed legacy deletions.
    let operation = connection
        .query_row(
            &format!(
                "SELECT p.removed_operation,COUNT(*),MAX(p.removed_at_ms) {remaining}
                   AND p.removed_operation IS NOT NULL
                   AND NOT EXISTS (SELECT 1 FROM library_metadata m WHERE m.key = ? || p.id)
                 GROUP BY p.removed_operation
                 ORDER BY MAX(p.removed_at_ms) DESC,p.removed_operation DESC
                 LIMIT 1"
            ),
            [PERMANENT_DELETION_DELETED_PREFIX],
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
