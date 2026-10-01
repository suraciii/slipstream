//! Durable review, execution, and settlement for permanent Original deletion.

use super::mutation::{mutation_error_from_sqlite, mutation_transaction};
use super::owner::{MutationError, PERMANENT_DELETION_DELETED_ORIGINAL_PREFIX};
use super::scan::parse_kind;
use super::{DatabaseName, StateDirectory};
use crate::{
    OriginalFacts, PermanentDeletionItemResult, PermanentDeletionItemState,
    PermanentDeletionRejection, PermanentDeletionResult, PermanentDeletionReview,
    PermanentDeletionReviewItem, PermanentDeletionSelection, PermanentDeletionTarget,
    PermanentDeletionWorkItem, PhotoAlbumMembership, RelativeOriginalPath, TrashPhotoCandidate,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub(super) type TrashCandidateResult = Result<Vec<TrashPhotoCandidate>, MutationError>;
pub(super) type PermanentDeletionReviewResult = Result<PermanentDeletionReview, MutationError>;
pub(super) type PermanentDeletionWorkResult = Result<Vec<PermanentDeletionWorkItem>, MutationError>;
pub(super) type PermanentDeletionResultReply = Result<PermanentDeletionResult, MutationError>;

/// One retained Permanent Deletion operation: its fixed reviewed item set and
/// the candidates the review refused. Written once, when the review is
/// captured. Per-item progress lives in its own row so confirming one item
/// never rewrites the whole operation.
const PERMANENT_DELETION_RECEIPT_PREFIX: &str = "permanent_deletion_operation:";
/// One reviewed item's durable state: `permanent_deletion_item:<operation>:<photo>`.
const PERMANENT_DELETION_ITEM_PREFIX: &str = "permanent_deletion_item:";
/// The operation still owing this Photo an outcome: `permanent_deletion_unsettled:<photo>`.
/// Present only between the durable `deleting` mark and its settlement, so a
/// surface can refuse Restore and another destructive confirmation for a Photo
/// whose deletion is not resolved.
const PERMANENT_DELETION_UNSETTLED_PREFIX: &str = "permanent_deletion_unsettled:";
/// The confirmed permanent deletion of this Photo:
/// `permanent_deletion_deleted:<photo>`. An equality probe on this key is how
/// Trash, Restore, and removal learn that a Photo is permanently deleted.
pub(super) const PERMANENT_DELETION_DELETED_PREFIX: &str = "permanent_deletion_deleted:";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct PermanentDeletionReceipt {
    operation_id: String,
    items: Vec<PermanentDeletionStoredItem>,
    rejected: Vec<PermanentDeletionStoredRejection>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct PermanentDeletionStoredItem {
    pub(super) photo_id: String,
    pub(super) removed_at_ms: i64,
    pub(super) original_id: String,
    pub(super) relative_path: String,
    pub(super) kind: String,
    pub(super) size: u64,
    /// The reviewed mtime in milliseconds, held as its exact bits. A decimal
    /// round trip through this row's JSON returns a value one ULP away for
    /// some milliseconds, and the deletion compares the reviewed facts with
    /// the file's current facts for equality, so only the bits are kept. The
    /// derivative cache retains the same pair for the same reason.
    pub(super) mtime_bits: u64,
    pub(super) device: u64,
    pub(super) inode: u64,
    #[serde(default)]
    pub(super) albums: Vec<PermanentDeletionStoredAlbum>,
}

/// One reviewed item's durable progress. It is the only mutable part of an
/// operation, so it lives in its own metadata row: `state` advances without
/// rewriting the retained review.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct PermanentDeletionStoredItemState {
    original_id: String,
    state: PermanentDeletionStoredState,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct PermanentDeletionStoredAlbum {
    album_id: String,
    album_name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct PermanentDeletionStoredRejection {
    photo_id: String,
    rejection: PermanentDeletionStoredRejectionKind,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
enum PermanentDeletionStoredRejectionKind {
    Missing,
    ChangedElsewhere,
    PendingVerification,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
enum PermanentDeletionStoredState {
    Pending,
    Deleting,
    Deleted,
    Missing,
    Changed,
    Failed,
    Uncertain,
}

impl From<PermanentDeletionStoredState> for PermanentDeletionItemState {
    fn from(value: PermanentDeletionStoredState) -> Self {
        match value {
            PermanentDeletionStoredState::Pending => Self::Pending,
            PermanentDeletionStoredState::Deleting => Self::Deleting,
            PermanentDeletionStoredState::Deleted => Self::Deleted,
            PermanentDeletionStoredState::Missing => Self::Missing,
            PermanentDeletionStoredState::Changed => Self::Changed,
            PermanentDeletionStoredState::Failed => Self::Failed,
            PermanentDeletionStoredState::Uncertain => Self::Uncertain,
        }
    }
}

impl From<PermanentDeletionItemState> for PermanentDeletionStoredState {
    fn from(value: PermanentDeletionItemState) -> Self {
        match value {
            PermanentDeletionItemState::Pending => Self::Pending,
            PermanentDeletionItemState::Deleting => Self::Deleting,
            PermanentDeletionItemState::Deleted => Self::Deleted,
            PermanentDeletionItemState::Missing => Self::Missing,
            PermanentDeletionItemState::Changed => Self::Changed,
            PermanentDeletionItemState::Failed => Self::Failed,
            PermanentDeletionItemState::Uncertain => Self::Uncertain,
        }
    }
}
fn permanent_deletion_receipt_key(operation_id: &str) -> String {
    format!("{PERMANENT_DELETION_RECEIPT_PREFIX}{operation_id}")
}

fn read_permanent_deletion_receipt(
    connection: &Connection,
    operation_id: &str,
) -> Result<Option<PermanentDeletionReceipt>, MutationError> {
    let value = connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [permanent_deletion_receipt_key(operation_id)],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(mutation_error_from_sqlite)?;
    value
        .map(|value| serde_json::from_str(&value).map_err(|_| MutationError::Persistence))
        .transpose()
}

fn read_permanent_deletion_receipt_transaction(
    transaction: &Transaction<'_>,
    operation_id: &str,
) -> Result<Option<PermanentDeletionReceipt>, MutationError> {
    let value = transaction
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [permanent_deletion_receipt_key(operation_id)],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(mutation_error_from_sqlite)?;
    value
        .map(|value| serde_json::from_str(&value).map_err(|_| MutationError::Persistence))
        .transpose()
}

fn write_permanent_deletion_receipt(
    transaction: &Transaction<'_>,
    receipt: &PermanentDeletionReceipt,
) -> Result<(), MutationError> {
    let value = serde_json::to_string(receipt).map_err(|_| MutationError::Persistence)?;
    transaction
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES(?,?)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![permanent_deletion_receipt_key(&receipt.operation_id), value],
        )
        .map_err(mutation_error_from_sqlite)?;
    Ok(())
}

fn permanent_deletion_rejection(
    value: PermanentDeletionStoredRejectionKind,
) -> PermanentDeletionRejection {
    match value {
        PermanentDeletionStoredRejectionKind::Missing => PermanentDeletionRejection::Missing,
        PermanentDeletionStoredRejectionKind::ChangedElsewhere => {
            PermanentDeletionRejection::ChangedElsewhere
        }
        PermanentDeletionStoredRejectionKind::PendingVerification => {
            PermanentDeletionRejection::PendingVerification
        }
    }
}

fn permanent_deletion_review_from_receipt(
    receipt: PermanentDeletionReceipt,
) -> Result<PermanentDeletionReview, MutationError> {
    let items = receipt
        .items
        .into_iter()
        .map(|item| {
            Ok(PermanentDeletionReviewItem {
                photo_id: item.photo_id,
                removed_at_ms: item.removed_at_ms,
                original_id: item.original_id,
                relative_path: RelativeOriginalPath::parse(item.relative_path)
                    .map_err(|_| MutationError::Persistence)?,
                kind: parse_kind(&item.kind).map_err(|_| MutationError::Persistence)?,
                size: item.size,
                albums: item
                    .albums
                    .into_iter()
                    .map(|album| PhotoAlbumMembership {
                        album_id: album.album_id,
                        album_name: album.album_name,
                    })
                    .collect(),
            })
        })
        .collect::<Result<Vec<_>, MutationError>>()?;
    let rejected = receipt
        .rejected
        .into_iter()
        .map(|item| (item.photo_id, permanent_deletion_rejection(item.rejection)))
        .collect();
    Ok(PermanentDeletionReview {
        operation_id: receipt.operation_id,
        items,
        rejected,
    })
}

/// Durable result of one operation: the retained reviewed set in review order,
/// joined with each item's own durable state.
fn permanent_deletion_result_from_receipt(
    connection: &Connection,
    receipt: PermanentDeletionReceipt,
) -> Result<PermanentDeletionResult, MutationError> {
    let mut states = read_permanent_deletion_item_states(connection, &receipt.operation_id)?;
    let items = receipt
        .items
        .into_iter()
        .map(|item| {
            let progress = states.remove(&item.photo_id);
            let state: PermanentDeletionItemState = progress
                .as_ref()
                .map_or(PermanentDeletionStoredState::Pending, |progress| {
                    progress.state
                })
                .into();
            let message = progress.and_then(|progress| progress.message);
            Ok(PermanentDeletionItemResult {
                photo_id: item.photo_id,
                relative_path: RelativeOriginalPath::parse(item.relative_path)
                    .map_err(|_| MutationError::Persistence)?,
                kind: parse_kind(&item.kind).map_err(|_| MutationError::Persistence)?,
                state,
                size: matches!(state, PermanentDeletionItemState::Deleted).then_some(item.size),
                message,
            })
        })
        .collect::<Result<Vec<_>, MutationError>>()?;
    let logical_bytes_deleted = items
        .iter()
        .filter_map(|item| item.size)
        .fold(0_u64, u64::saturating_add);
    Ok(PermanentDeletionResult {
        operation_id: receipt.operation_id,
        reviewed: items.len(),
        logical_bytes_deleted,
        items,
    })
}

fn permanent_deletion_item_key(operation_id: &str, photo_id: &str) -> String {
    format!("{PERMANENT_DELETION_ITEM_PREFIX}{operation_id}:{photo_id}")
}

fn permanent_deletion_item_prefix(operation_id: &str) -> String {
    format!("{PERMANENT_DELETION_ITEM_PREFIX}{operation_id}:")
}

fn permanent_deletion_unsettled_key(photo_id: &str) -> String {
    format!("{PERMANENT_DELETION_UNSETTLED_PREFIX}{photo_id}")
}

fn permanent_deletion_deleted_key(photo_id: &str) -> String {
    format!("{PERMANENT_DELETION_DELETED_PREFIX}{photo_id}")
}

fn permanent_deletion_deleted_original_key(original_id: &str) -> String {
    format!("{PERMANENT_DELETION_DELETED_ORIGINAL_PREFIX}{original_id}")
}

/// The Photo identities whose Original was confirmed permanently deleted.
/// Every surface that must hide one reads this set.
fn permanently_deleted_photo_ids(
    connection: &Connection,
) -> Result<std::collections::HashSet<String>, MutationError> {
    let mut statement = connection
        .prepare("SELECT key FROM library_metadata WHERE key LIKE ?")
        .map_err(mutation_error_from_sqlite)?;
    let keys = statement
        .query_map([format!("{PERMANENT_DELETION_DELETED_PREFIX}%")], |row| {
            row.get::<_, String>(0)
        })
        .map_err(mutation_error_from_sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(mutation_error_from_sqlite)?;
    Ok(keys
        .into_iter()
        .filter_map(|key| {
            key.strip_prefix(PERMANENT_DELETION_DELETED_PREFIX)
                .map(str::to_owned)
        })
        .collect())
}

/// The Photo identities whose deletion started and has not settled. A review
/// refuses them until their outcome is known.
fn unsettled_permanent_deletion_photo_ids(
    connection: &Connection,
) -> Result<std::collections::HashSet<String>, MutationError> {
    let mut statement = connection
        .prepare("SELECT key FROM library_metadata WHERE key LIKE ?")
        .map_err(mutation_error_from_sqlite)?;
    let keys = statement
        .query_map([format!("{PERMANENT_DELETION_UNSETTLED_PREFIX}%")], |row| {
            row.get::<_, String>(0)
        })
        .map_err(mutation_error_from_sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(mutation_error_from_sqlite)?;
    Ok(keys
        .into_iter()
        .filter_map(|key| {
            key.strip_prefix(PERMANENT_DELETION_UNSETTLED_PREFIX)
                .map(str::to_owned)
        })
        .collect())
}

/// Whether one Photo's Original was confirmed permanently deleted. This is the
/// equality probe Trash, Restore, and removal use, so their cost does not grow
/// with the number of retained operations.
pub(super) fn photo_is_permanently_deleted(
    connection: &Connection,
    photo_id: &str,
) -> Result<bool, MutationError> {
    let found = connection
        .query_row(
            "SELECT 1 FROM library_metadata WHERE key=?",
            [permanent_deletion_deleted_key(photo_id)],
            |_| Ok(()),
        )
        .optional()
        .map_err(mutation_error_from_sqlite)?;
    Ok(found.is_some())
}
/// The retained operation that still owes this Photo an outcome, when its
/// deletion started and never settled. Restore and another destructive
/// confirmation stay unavailable while it is present.
pub(super) fn unsettled_permanent_deletion_operation(
    connection: &Connection,
    photo_id: &str,
) -> Result<Option<String>, MutationError> {
    connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [permanent_deletion_unsettled_key(photo_id)],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(mutation_error_from_sqlite)
}

/// One reviewed item's progress. An item with no row yet has not left
/// `pending`.
fn read_permanent_deletion_item_state(
    connection: &Connection,
    operation_id: &str,
    photo_id: &str,
) -> Result<Option<PermanentDeletionStoredItemState>, MutationError> {
    let value = connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [permanent_deletion_item_key(operation_id, photo_id)],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(mutation_error_from_sqlite)?;
    value
        .map(|value| serde_json::from_str(&value).map_err(|_| MutationError::Persistence))
        .transpose()
}

/// Every reviewed item's progress for one operation, keyed by Photo identity.
/// The rows are small and bounded by the reviewed set.
fn read_permanent_deletion_item_states(
    connection: &Connection,
    operation_id: &str,
) -> Result<HashMap<String, PermanentDeletionStoredItemState>, MutationError> {
    let prefix = permanent_deletion_item_prefix(operation_id);
    let mut statement = connection
        .prepare("SELECT key,value FROM library_metadata WHERE key LIKE ?")
        .map_err(mutation_error_from_sqlite)?;
    let rows = statement
        .query_map([format!("{prefix}%")], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(mutation_error_from_sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(mutation_error_from_sqlite)?;
    let mut states = HashMap::with_capacity(rows.len());
    for (key, value) in rows {
        let Some(photo_id) = key.strip_prefix(prefix.as_str()) else {
            continue;
        };
        let state: PermanentDeletionStoredItemState =
            serde_json::from_str(&value).map_err(|_| MutationError::Persistence)?;
        states.insert(photo_id.to_owned(), state);
    }
    Ok(states)
}

fn write_permanent_deletion_item_state(
    transaction: &Transaction<'_>,
    operation_id: &str,
    photo_id: &str,
    state: &PermanentDeletionStoredItemState,
) -> Result<(), MutationError> {
    let value = serde_json::to_string(state).map_err(|_| MutationError::Persistence)?;
    transaction
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES(?,?)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![permanent_deletion_item_key(operation_id, photo_id), value],
        )
        .map_err(mutation_error_from_sqlite)?;
    Ok(())
}

fn delete_metadata_key(transaction: &Transaction<'_>, key: &str) -> Result<(), MutationError> {
    transaction
        .execute("DELETE FROM library_metadata WHERE key=?", [key])
        .map_err(mutation_error_from_sqlite)?;
    Ok(())
}

pub(super) fn trash_candidates(
    connection: &Connection,
    selection: PermanentDeletionSelection,
) -> Result<Vec<TrashPhotoCandidate>, MutationError> {
    // A permanently deleted Photo is evidence and never returns to a review.
    // A Photo whose own deletion has not settled stays visible so the review
    // can refuse it with its own reason instead of dropping it silently.
    let deleted = permanently_deleted_photo_ids(connection)?;
    let unsettled = unsettled_permanent_deletion_photo_ids(connection)?;
    let rows = connection
        .prepare(
            "SELECT p.id,p.removed_at_ms,o.id,o.relative_path,o.kind,o.size,o.mtime_ms,o.available
             FROM photos p JOIN original_files o ON o.id=p.original_id
             WHERE p.removed_at_ms IS NOT NULL
             ORDER BY p.removed_at_ms DESC,p.id",
        )
        .map_err(mutation_error_from_sqlite)?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, f64>(6)?,
                row.get::<_, i64>(7)? != 0,
            ))
        })
        .map_err(mutation_error_from_sqlite)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(mutation_error_from_sqlite)?;
    let mut by_id = HashMap::with_capacity(rows.len());
    let mut ordered_ids = Vec::with_capacity(rows.len());
    for (photo_id, removed_at_ms, original_id, relative_path, kind, size, mtime_ms, available) in
        rows
    {
        if deleted.contains(&photo_id) {
            continue;
        }
        let size = size.try_into().map_err(|_| MutationError::Persistence)?;
        ordered_ids.push(photo_id.clone());
        let unsettled = unsettled.contains(&photo_id);
        by_id.insert(
            photo_id.clone(),
            TrashPhotoCandidate {
                photo_id,
                removed_at_ms,
                original_id,
                relative_path: RelativeOriginalPath::parse(relative_path)
                    .map_err(|_| MutationError::Persistence)?,
                kind: parse_kind(&kind).map_err(|_| MutationError::Persistence)?,
                size,
                mtime_ms,
                available,
                unsettled,
            },
        );
    }
    let candidates: Vec<TrashPhotoCandidate> = match selection {
        PermanentDeletionSelection::Photos(photo_ids) => photo_ids
            .into_iter()
            .filter_map(|photo_id| by_id.remove(&photo_id))
            .collect(),
        PermanentDeletionSelection::All { exclude_photo_ids } => ordered_ids
            .into_iter()
            .filter_map(|photo_id| by_id.remove(&photo_id))
            .filter(|candidate| !exclude_photo_ids.contains(&candidate.photo_id))
            .collect(),
    };
    // Every Trash row a review would consider counts against the bound one
    // review holds, so the retained review and its refusals stay bounded.
    if candidates.len() > crate::PERMANENT_DELETION_MAX {
        return Err(MutationError::Conflict);
    }
    Ok(candidates)
}

pub(super) fn prepare_permanent_deletion(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    operation_id: String,
    targets: Vec<PermanentDeletionTarget>,
) -> Result<PermanentDeletionReview, MutationError> {
    mutation_transaction(state, database_name, connection, |transaction| {
        if let Some(receipt) =
            read_permanent_deletion_receipt_transaction(transaction, &operation_id)?
        {
            let mut existing_ids = receipt
                .items
                .iter()
                .map(|item| item.photo_id.clone())
                .collect::<Vec<_>>();
            existing_ids.extend(receipt.rejected.iter().map(|item| item.photo_id.clone()));
            let target_ids = targets
                .iter()
                .map(|target| target.photo_id.clone())
                .collect::<Vec<_>>();
            if receipt.operation_id != operation_id || existing_ids != target_ids {
                return Err(MutationError::Conflict);
            }
            return permanent_deletion_review_from_receipt(receipt);
        }

        let mut items = Vec::new();
        let mut rejected = Vec::new();
        for target in targets {
            if unsettled_permanent_deletion_operation(transaction, &target.photo_id)?.is_some() {
                rejected.push(PermanentDeletionStoredRejection {
                    photo_id: target.photo_id,
                    rejection: PermanentDeletionStoredRejectionKind::PendingVerification,
                });
                continue;
            }
            let current = transaction
                .query_row(
                    "SELECT p.removed_at_ms,o.id,o.relative_path,o.kind,o.size,o.mtime_ms,o.available
                     FROM photos p JOIN original_files o ON o.id=p.original_id
                     WHERE p.id=?",
                    [&target.photo_id],
                    |row| {
                        Ok((
                            row.get::<_, Option<i64>>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, i64>(4)?,
                            row.get::<_, f64>(5)?,
                            row.get::<_, i64>(6)? != 0,
                        ))
                    },
                )
                .optional()
                .map_err(mutation_error_from_sqlite)?;
            let Some((removed_at_ms, original_id, relative_path, kind, size, mtime_ms, available)) =
                current
            else {
                rejected.push(PermanentDeletionStoredRejection {
                    photo_id: target.photo_id,
                    rejection: PermanentDeletionStoredRejectionKind::Missing,
                });
                continue;
            };
            let Some(facts) = target.facts else {
                rejected.push(PermanentDeletionStoredRejection {
                    photo_id: target.photo_id,
                    rejection: PermanentDeletionStoredRejectionKind::Missing,
                });
                continue;
            };
            let size: u64 = size.try_into().map_err(|_| MutationError::Persistence)?;
            if !available
                || removed_at_ms != Some(target.removed_at_ms)
                || size != facts.size
                || mtime_ms != facts.mtime_ms
            {
                rejected.push(PermanentDeletionStoredRejection {
                    photo_id: target.photo_id,
                    rejection: if !available {
                        PermanentDeletionStoredRejectionKind::Missing
                    } else {
                        PermanentDeletionStoredRejectionKind::ChangedElsewhere
                    },
                });
                continue;
            }
            let albums = transaction
                .prepare(
                    "SELECT a.id,a.name
                     FROM album_members m JOIN albums a ON a.id=m.album_id
                     WHERE m.photo_id=? ORDER BY a.created_at,a.id",
                )
                .map_err(mutation_error_from_sqlite)?
                .query_map([&target.photo_id], |row| {
                    Ok(PermanentDeletionStoredAlbum {
                        album_id: row.get(0)?,
                        album_name: row.get(1)?,
                    })
                })
                .map_err(mutation_error_from_sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(mutation_error_from_sqlite)?;
            items.push(PermanentDeletionStoredItem {
                photo_id: target.photo_id,
                removed_at_ms: target.removed_at_ms,
                original_id,
                relative_path,
                kind,
                size: facts.size,
                mtime_bits: facts.mtime_ms.to_bits(),
                device: facts.device,
                inode: facts.inode,
                albums,
            });
        }
        let receipt = PermanentDeletionReceipt {
            operation_id,
            items,
            rejected,
        };
        write_permanent_deletion_receipt(transaction, &receipt)?;
        // Every reviewed item gets its own progress row here, so confirming
        // one Original updates one small row instead of rewriting the whole
        // reviewed set.
        for item in &receipt.items {
            write_permanent_deletion_item_state(
                transaction,
                &receipt.operation_id,
                &item.photo_id,
                &PermanentDeletionStoredItemState {
                    original_id: item.original_id.clone(),
                    state: PermanentDeletionStoredState::Pending,
                    message: None,
                },
            )?;
        }
        permanent_deletion_review_from_receipt(receipt)
    })
}

/// Every item of one operation that still needs a deletion attempt, in the
/// reviewed order. One read serves the whole confirmation, so a large batch
/// does not re-read the retained review for every Original.
/// The exact filesystem facts one retained review recorded. The mtime is
/// rebuilt from its bits so the equality the deletion performs is the equality
/// the review observed.
pub(super) fn reviewed_facts(size: u64, mtime_bits: u64, device: u64, inode: u64) -> OriginalFacts {
    OriginalFacts {
        size,
        mtime_ms: f64::from_bits(mtime_bits),
        device,
        inode,
    }
}

pub(super) fn permanent_deletion_work(
    connection: &Connection,
    operation_id: &str,
    retry_unresolved: bool,
) -> Result<Vec<PermanentDeletionWorkItem>, MutationError> {
    let Some(receipt) = read_permanent_deletion_receipt(connection, operation_id)? else {
        return Err(MutationError::NotFound);
    };
    let states = read_permanent_deletion_item_states(connection, operation_id)?;
    let mut work = Vec::new();
    for item in &receipt.items {
        let state = states
            .get(&item.photo_id)
            .map_or(PermanentDeletionStoredState::Pending, |state| state.state);
        let unresolved = matches!(
            state,
            PermanentDeletionStoredState::Pending | PermanentDeletionStoredState::Deleting
        ) || (retry_unresolved
            && matches!(
                state,
                PermanentDeletionStoredState::Failed | PermanentDeletionStoredState::Uncertain
            ));
        if !unresolved {
            continue;
        }
        work.push(PermanentDeletionWorkItem {
            operation_id: receipt.operation_id.clone(),
            photo_id: item.photo_id.clone(),
            relative_path: RelativeOriginalPath::parse(item.relative_path.clone())
                .map_err(|_| MutationError::Persistence)?,
            facts: reviewed_facts(item.size, item.mtime_bits, item.device, item.inode),
            state: state.into(),
        });
    }
    Ok(work)
}

/// Durably marks one reviewed item as being deleted. A Photo that has not
/// settled carries the operation that owes it an outcome, so every surface can
/// refuse Restore and a second destructive confirmation until it does.
pub(super) fn mark_permanent_deletion_deleting(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    operation_id: &str,
    photo_id: &str,
) -> Result<(), MutationError> {
    mutation_transaction(state, database_name, connection, |transaction| {
        let Some(current) =
            read_permanent_deletion_item_state(transaction, operation_id, photo_id)?
        else {
            return Err(MutationError::NotFound);
        };
        // A settled outcome is authoritative: another confirmation already
        // owns this item's result. A failed or uncertain item is unresolved
        // and may be attempted again by an explicit retry.
        if matches!(
            current.state,
            PermanentDeletionStoredState::Deleted
                | PermanentDeletionStoredState::Missing
                | PermanentDeletionStoredState::Changed
        ) {
            return Err(MutationError::Conflict);
        }
        if current.state == PermanentDeletionStoredState::Deleting {
            return Ok(());
        }
        write_permanent_deletion_item_state(
            transaction,
            operation_id,
            photo_id,
            &PermanentDeletionStoredItemState {
                original_id: current.original_id,
                state: PermanentDeletionStoredState::Deleting,
                message: None,
            },
        )?;
        transaction
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES(?,?)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![permanent_deletion_unsettled_key(photo_id), operation_id],
            )
            .map_err(mutation_error_from_sqlite)?;
        Ok(())
    })
}

pub(super) fn settle_permanent_deletion(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    operation_id: &str,
    photo_id: &str,
    item_state: PermanentDeletionItemState,
    message: Option<String>,
) -> Result<(), MutationError> {
    if matches!(
        item_state,
        PermanentDeletionItemState::Pending | PermanentDeletionItemState::Deleting
    ) {
        return Err(MutationError::Invalid);
    }
    mutation_transaction(state, database_name, connection, |transaction| {
        let Some(current) =
            read_permanent_deletion_item_state(transaction, operation_id, photo_id)?
        else {
            return Err(MutationError::NotFound);
        };
        let current_state: PermanentDeletionItemState = current.state.into();
        if matches!(
            current_state,
            PermanentDeletionItemState::Deleted
                | PermanentDeletionItemState::Missing
                | PermanentDeletionItemState::Changed
                | PermanentDeletionItemState::Failed
                | PermanentDeletionItemState::Uncertain
        ) {
            return if current_state == item_state {
                Ok(())
            } else {
                Err(MutationError::Conflict)
            };
        }
        if item_state == PermanentDeletionItemState::Deleted {
            let memberships = transaction
                .prepare(
                    "SELECT album_id,position FROM album_members
                     WHERE photo_id=? ORDER BY album_id,position",
                )
                .map_err(mutation_error_from_sqlite)?
                .query_map([photo_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .map_err(mutation_error_from_sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(mutation_error_from_sqlite)?;
            for (album_id, position) in memberships {
                transaction
                    .execute(
                        "DELETE FROM album_members WHERE album_id=? AND photo_id=?",
                        params![album_id, photo_id],
                    )
                    .map_err(mutation_error_from_sqlite)?;
                transaction
                    .execute(
                        "UPDATE album_members SET position=position-1
                         WHERE album_id=? AND position>?",
                        params![album_id, position],
                    )
                    .map_err(mutation_error_from_sqlite)?;
            }
            transaction
                .execute(
                    "UPDATE photos SET available=0,preview_state='unavailable',
                            preview_source_revision=NULL,preview_width=NULL,
                            preview_height=NULL,cache_revision=NULL
                     WHERE id=?",
                    [photo_id],
                )
                .map_err(mutation_error_from_sqlite)?;
            transaction
                .execute(
                    "UPDATE original_files SET available=0,error_category='unreadable',
                            error_message='Original permanently deleted'
                     WHERE id=?",
                    [current.original_id.as_str()],
                )
                .map_err(mutation_error_from_sqlite)?;
            // The retained evidence that this Photo is permanently deleted.
            // Every surface reads this one key, so Trash and Restore never
            // rescan retained operations.
            transaction
                .execute(
                    "INSERT INTO library_metadata(key,value) VALUES(?,?)
                     ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    params![permanent_deletion_deleted_key(photo_id), operation_id],
                )
                .map_err(mutation_error_from_sqlite)?;
            // The same evidence keyed by the Original, so a scan reads the
            // tombstones it must honour in one bounded pass.
            transaction
                .execute(
                    "INSERT INTO library_metadata(key,value) VALUES(?,?)
                     ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    params![
                        permanent_deletion_deleted_original_key(&current.original_id),
                        operation_id
                    ],
                )
                .map_err(mutation_error_from_sqlite)?;
        }
        write_permanent_deletion_item_state(
            transaction,
            operation_id,
            photo_id,
            &PermanentDeletionStoredItemState {
                original_id: current.original_id,
                state: item_state.into(),
                message,
            },
        )?;
        if item_state == PermanentDeletionItemState::Uncertain {
            // The outcome is not known: the operation stays on record as the
            // one that must reconcile this Photo, and Restore plus another
            // destructive confirmation stay closed until it does.
            transaction
                .execute(
                    "INSERT INTO library_metadata(key,value) VALUES(?,?)
                     ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    params![permanent_deletion_unsettled_key(photo_id), operation_id],
                )
                .map_err(mutation_error_from_sqlite)?;
        } else {
            delete_metadata_key(transaction, &permanent_deletion_unsettled_key(photo_id))?;
        }
        Ok(())
    })
}

pub(super) fn read_permanent_deletion(
    connection: &Connection,
    operation_id: &str,
) -> Result<PermanentDeletionResult, MutationError> {
    let receipt = read_permanent_deletion_receipt(connection, operation_id)?
        .ok_or(MutationError::NotFound)?;
    permanent_deletion_result_from_receipt(connection, receipt)
}
