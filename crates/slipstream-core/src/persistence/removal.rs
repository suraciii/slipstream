//! Photo removal and restore: durable replay receipts and monotonic markers.
//! Every write runs on the serialized owner connection.

use super::mutation::{mutation_error_from_sqlite, mutation_transaction};
use super::owner::{MutationError, MutationVersions, unix_millis};
use super::permanent_deletion::{
    PERMANENT_DELETION_DELETED_PREFIX, photo_is_permanently_deleted,
    unsettled_permanent_deletion_operation,
};
use super::scan::{parse_selection_state, selection_state_value};
use super::{DatabaseName, StateDirectory};
use crate::{
    ExplicitPhotoRemovalMutation, ExplicitPhotoRestoreCounts, ExplicitPhotoRestoreMutation,
    ExplicitPhotoRestoreResult, PhotoOperationRemainder, PhotoRemovalCounts, PhotoRemovalMarker,
    PhotoRemovalMutation, PhotoRemovalResult, PhotoRestoration, PhotoRestorationCounts,
    PhotoRestorationResult, SelectionState,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

pub(super) use super::permanent_deletion::{
    PermanentDeletionResultReply, PermanentDeletionReviewResult, PermanentDeletionWorkResult,
    TrashCandidateResult, mark_permanent_deletion_deleting, permanent_deletion_work,
    prepare_permanent_deletion, read_permanent_deletion, settle_permanent_deletion,
    trash_candidates,
};
pub(super) use super::trash::removed_photos;
const PHOTO_REMOVAL_RECEIPT_PREFIX: &str = "photo_removal_receipt:";

#[derive(Clone, Debug, Deserialize, Serialize, Eq, PartialEq)]
struct PhotoRemovalIntent {
    kind: PhotoRemovalIntentKind,
    photo_ids: Vec<String>,
    expected_selection_states: Vec<String>,
    expected_decision_versions: Vec<String>,
    expected_removed_at_ms: Vec<Option<i64>>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
enum PhotoRemovalIntentKind {
    Browse,
    Explicit,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct PhotoRemovalReceipt {
    photo_ids: Vec<String>,
    outcomes: Vec<PhotoRemovalOutcome>,
    #[serde(default)]
    removed_at_ms: Vec<Option<i64>>,
    #[serde(default)]
    intent: Option<PhotoRemovalIntent>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
enum PhotoRemovalOutcome {
    Removed,
    ChangedElsewhere,
    Missing,
    AlreadyRemoved,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct PhotoRestoreReceipt {
    photo_ids: Vec<String>,
    removed_at_ms: Vec<i64>,
    outcomes: Vec<PhotoRestoreOutcome>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
enum PhotoRestoreOutcome {
    Restored,
    AlreadyActive,
    ChangedElsewhere,
    Missing,
}

const PHOTO_RESTORE_RECEIPT_PREFIX: &str = "photo_restore_receipt:";

pub(super) enum PhotoRemovalRequest {
    Browse(PhotoRemovalMutation),
    Explicit(ExplicitPhotoRemovalMutation),
}

impl PhotoRemovalRequest {
    fn operation_id(&self) -> &str {
        match self {
            Self::Browse(mutation) => &mutation.operation_id,
            Self::Explicit(mutation) => &mutation.operation_id,
        }
    }

    fn photo_ids(&self) -> Vec<String> {
        match self {
            Self::Browse(mutation) => mutation.photo_ids.clone(),
            Self::Explicit(mutation) => mutation
                .photos
                .iter()
                .map(|photo| photo.photo_id.clone())
                .collect(),
        }
    }
}
/// The key the Library keeps its removal-marker high water mark under. A
/// Library that predates the mark falls back to the greatest marker it still
/// holds, so the first marker written after an upgrade cannot repeat one.
const REMOVAL_MARKER_HIGH_WATER: &str = "removal_marker_high_water";

/// The next removal marker: the clock reading, and strictly greater than every
/// marker this Library assigned before.
///
/// A restore is a compare-and-set against the marker it read, so two removals
/// of one Photo must never carry the same marker — a clock reading alone can
/// repeat when a Photo is restored and removed again inside one millisecond,
/// which would let a stale listing clear the newer removal. The high water
/// mark is durable, so it also holds across restarts and rescan deletions that
/// leave no removed row behind to read a maximum from.
fn next_removal_marker(transaction: &Transaction<'_>) -> Result<i64, MutationError> {
    let stored: Option<String> = transaction
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [REMOVAL_MARKER_HIGH_WATER],
            |row| row.get(0),
        )
        .optional()
        .map_err(mutation_error_from_sqlite)?;
    let high_water = match stored.and_then(|value| value.parse::<i64>().ok()) {
        Some(value) => value,
        // A Library written before this high water mark existed has no row to
        // read: the markers it still holds are the floor, so the next marker
        // cannot repeat one of them either.
        None => transaction
            .query_row("SELECT max(removed_at_ms) FROM photos", [], |row| {
                row.get::<_, Option<i64>>(0)
            })
            .map_err(mutation_error_from_sqlite)?
            .unwrap_or(0),
    };
    let marker = unix_millis().max(high_water.saturating_add(1));
    transaction
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES(?,?)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![REMOVAL_MARKER_HIGH_WATER, marker.to_string()],
        )
        .map_err(mutation_error_from_sqlite)?;
    Ok(marker)
}
/// The largest number of Photo identities one removal or restore statement
/// addresses at once. Outcomes are still reported per requested Photo in
/// request order; the bound only keeps one SQLite statement small.
const PHOTO_REMOVAL_CHUNK: usize = 500;
fn photo_removal_receipt_key(operation_id: &str) -> String {
    format!("{PHOTO_REMOVAL_RECEIPT_PREFIX}{operation_id}")
}

pub(super) fn read_photo_removal_operation(
    connection: &Connection,
    operation_id: &str,
) -> Result<Option<PhotoRemovalReceipt>, MutationError> {
    let value = connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [photo_removal_receipt_key(operation_id)],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(mutation_error_from_sqlite)?;
    value
        .map(|value| serde_json::from_str(&value).map_err(|_| MutationError::Persistence))
        .transpose()
}

fn read_photo_removal_receipt(
    transaction: &Transaction<'_>,
    operation_id: &str,
) -> Result<Option<PhotoRemovalReceipt>, MutationError> {
    let value = transaction
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [photo_removal_receipt_key(operation_id)],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(mutation_error_from_sqlite)?;
    value
        .map(|value| serde_json::from_str(&value).map_err(|_| MutationError::Persistence))
        .transpose()
}

fn write_photo_removal_receipt(
    transaction: &Transaction<'_>,
    operation_id: &str,
    receipt: &PhotoRemovalReceipt,
) -> Result<(), MutationError> {
    let value = serde_json::to_string(receipt).map_err(|_| MutationError::Persistence)?;
    transaction
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES(?,?)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![photo_removal_receipt_key(operation_id), value],
        )
        .map_err(mutation_error_from_sqlite)?;
    Ok(())
}

pub(super) fn photo_removal_result_from_receipt(
    operation_id: &str,
    receipt: PhotoRemovalReceipt,
) -> Result<PhotoRemovalResult, MutationError> {
    if receipt.photo_ids.len() != receipt.outcomes.len()
        || (!receipt.removed_at_ms.is_empty()
            && receipt.photo_ids.len() != receipt.removed_at_ms.len())
    {
        return Err(MutationError::Persistence);
    }
    let markers = if receipt.removed_at_ms.is_empty() {
        vec![None; receipt.photo_ids.len()]
    } else {
        receipt.removed_at_ms.clone()
    };
    let ordered_photo_ids = receipt.photo_ids.clone();
    let mut result = PhotoRemovalResult {
        operation_id: operation_id.to_owned(),
        counts: PhotoRemovalCounts::default(),
        ordered_photo_ids,
        removed: Vec::new(),
        removed_markers: Vec::new(),
        newly_removed: Vec::new(),
        changed_elsewhere: Vec::new(),
        missing: Vec::new(),
        already_removed: Vec::new(),
    };
    for ((photo_id, outcome), removed_at_ms) in receipt
        .photo_ids
        .into_iter()
        .zip(receipt.outcomes)
        .zip(markers)
    {
        match outcome {
            PhotoRemovalOutcome::Removed => {
                result.removed.push(photo_id.clone());
                if let Some(removed_at_ms) = removed_at_ms {
                    result.removed_markers.push(PhotoRemovalMarker {
                        photo_id,
                        removed_at_ms,
                    });
                }
            }
            PhotoRemovalOutcome::ChangedElsewhere => result.changed_elsewhere.push(photo_id),
            PhotoRemovalOutcome::Missing => result.missing.push(photo_id),
            PhotoRemovalOutcome::AlreadyRemoved => result.already_removed.push(photo_id),
        }
    }
    result.counts = PhotoRemovalCounts {
        removed: result.removed.len(),
        changed_elsewhere: result.changed_elsewhere.len(),
        missing: result.missing.len(),
        already_removed: result.already_removed.len(),
    };
    Ok(result)
}

fn removal_intent(request: &PhotoRemovalRequest) -> PhotoRemovalIntent {
    match request {
        PhotoRemovalRequest::Browse(_) => PhotoRemovalIntent {
            kind: PhotoRemovalIntentKind::Browse,
            photo_ids: request.photo_ids(),
            expected_selection_states: Vec::new(),
            expected_decision_versions: Vec::new(),
            expected_removed_at_ms: Vec::new(),
        },
        PhotoRemovalRequest::Explicit(mutation) => PhotoRemovalIntent {
            kind: PhotoRemovalIntentKind::Explicit,
            photo_ids: mutation
                .photos
                .iter()
                .map(|photo| photo.photo_id.clone())
                .collect(),
            expected_selection_states: mutation
                .photos
                .iter()
                .map(|photo| selection_state_value(photo.expected_selection_state).to_owned())
                .collect(),
            expected_decision_versions: mutation
                .photos
                .iter()
                .map(|photo| photo.expected_decision_version.clone())
                .collect(),
            expected_removed_at_ms: mutation
                .photos
                .iter()
                .map(|photo| photo.expected_removed_at_ms)
                .collect(),
        },
    }
}

fn removal_intent_matches(
    request: &PhotoRemovalRequest,
    stored: Option<&PhotoRemovalIntent>,
) -> bool {
    match (request, stored) {
        (PhotoRemovalRequest::Browse(_), None) => true,
        (PhotoRemovalRequest::Browse(_), Some(intent)) => {
            intent.kind == PhotoRemovalIntentKind::Browse && intent.photo_ids == request.photo_ids()
        }
        (PhotoRemovalRequest::Explicit(_), Some(intent)) => intent == &removal_intent(request),
        (PhotoRemovalRequest::Explicit(_), None) => false,
    }
}

/// One confirmed removal. Every requested Photo is resolved inside one
/// transaction and reports exactly one outcome.
pub(super) fn remove_photos(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    versions: &MutationVersions,
    request: PhotoRemovalRequest,
) -> Result<PhotoRemovalResult, MutationError> {
    let operation_id = request.operation_id().to_owned();
    let photo_ids = request.photo_ids();
    let intent = removal_intent(&request);
    mutation_transaction(state, database_name, connection, |transaction| {
        if let Some(receipt) = read_photo_removal_receipt(transaction, &operation_id)? {
            if receipt.photo_ids != photo_ids
                || !removal_intent_matches(&request, receipt.intent.as_ref())
            {
                return Err(MutationError::Conflict);
            }
            return photo_removal_result_from_receipt(&operation_id, receipt);
        }

        let explicit_targets = match &request {
            PhotoRemovalRequest::Browse(_) => None,
            PhotoRemovalRequest::Explicit(mutation) => Some(
                mutation
                    .photos
                    .iter()
                    .map(|photo| (photo.photo_id.as_str(), photo))
                    .collect::<std::collections::HashMap<_, _>>(),
            ),
        };
        let mut marker: Option<i64> = None;
        let mut outcomes = Vec::with_capacity(photo_ids.len());
        let mut stored_markers = vec![None; photo_ids.len()];
        let mut result = PhotoRemovalResult {
            operation_id: operation_id.clone(),
            counts: PhotoRemovalCounts::default(),
            ordered_photo_ids: photo_ids.clone(),
            removed: Vec::new(),
            removed_markers: Vec::new(),
            newly_removed: Vec::new(),
            changed_elsewhere: Vec::new(),
            missing: Vec::new(),
            already_removed: Vec::new(),
        };
        for chunk in photo_ids.chunks(PHOTO_REMOVAL_CHUNK) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let mut statement = transaction
                .prepare(&format!(
                    "SELECT id,selection_state,removed_at_ms,removed_operation
                     FROM photos WHERE id IN ({placeholders})"
                ))
                .map_err(mutation_error_from_sqlite)?;
            let rows = statement
                .query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                })
                .map_err(mutation_error_from_sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(mutation_error_from_sqlite)?;
            let mut facts = std::collections::HashMap::with_capacity(rows.len());
            for (photo_id, selection_state, removed_at, removed_operation) in rows {
                facts.insert(photo_id, (selection_state, removed_at, removed_operation));
            }
            for photo_id in chunk {
                let Some((selection_state, removed_at, removed_operation)) = facts.get(photo_id)
                else {
                    outcomes.push(PhotoRemovalOutcome::Missing);
                    result.missing.push(photo_id.clone());
                    continue;
                };
                if photo_is_permanently_deleted(transaction, photo_id)? {
                    outcomes.push(PhotoRemovalOutcome::AlreadyRemoved);
                    result.already_removed.push(photo_id.clone());
                    continue;
                }
                if removed_at.is_some() {
                    if removed_operation.as_deref() == Some(operation_id.as_str()) {
                        outcomes.push(PhotoRemovalOutcome::Removed);
                        result.removed.push(photo_id.clone());
                        if let Some(marker_value) = *removed_at {
                            result.removed_markers.push(PhotoRemovalMarker {
                                photo_id: photo_id.clone(),
                                removed_at_ms: marker_value,
                            });
                            if let Some(index) = photo_ids.iter().position(|id| id == photo_id) {
                                stored_markers[index] = Some(marker_value);
                            }
                        }
                    } else {
                        outcomes.push(PhotoRemovalOutcome::AlreadyRemoved);
                        result.already_removed.push(photo_id.clone());
                    }
                    continue;
                }
                if let Some(target) = explicit_targets
                    .as_ref()
                    .and_then(|targets| targets.get(photo_id.as_str()))
                {
                    if target.expected_removed_at_ms.is_some()
                        || target.expected_decision_version != versions.photo(photo_id)
                        || target.expected_selection_state
                            != parse_selection_state(selection_state)
                                .map_err(|_| MutationError::Persistence)?
                    {
                        outcomes.push(PhotoRemovalOutcome::ChangedElsewhere);
                        result.changed_elsewhere.push(photo_id.clone());
                        continue;
                    }
                } else if parse_selection_state(selection_state)
                    .map_err(|_| MutationError::Persistence)?
                    != SelectionState::Rejected
                {
                    outcomes.push(PhotoRemovalOutcome::ChangedElsewhere);
                    result.changed_elsewhere.push(photo_id.clone());
                    continue;
                }
                let removed_at = match marker {
                    Some(marker) => marker,
                    None => {
                        let assigned = next_removal_marker(transaction)?;
                        marker = Some(assigned);
                        assigned
                    }
                };
                transaction
                    .execute(
                        "UPDATE photos SET removed_at_ms=?,removed_operation=?,association_generation=association_generation+1 WHERE id=? AND removed_at_ms IS NULL",
                        params![removed_at, operation_id, photo_id],
                    )
                    .map_err(mutation_error_from_sqlite)?;
                outcomes.push(PhotoRemovalOutcome::Removed);
                result.removed.push(photo_id.clone());
                result.removed_markers.push(PhotoRemovalMarker {
                    photo_id: photo_id.clone(),
                    removed_at_ms: removed_at,
                });
                result.newly_removed.push(photo_id.clone());
                if let Some(index) = photo_ids.iter().position(|id| id == photo_id) {
                    stored_markers[index] = Some(removed_at);
                }
            }
        }
        result.counts = PhotoRemovalCounts {
            removed: result.removed.len(),
            changed_elsewhere: result.changed_elsewhere.len(),
            missing: result.missing.len(),
            already_removed: result.already_removed.len(),
        };
        write_photo_removal_receipt(
            transaction,
            &operation_id,
            &PhotoRemovalReceipt {
                photo_ids,
                outcomes,
                removed_at_ms: stored_markers,
                intent: Some(intent),
            },
        )?;
        Ok(result)
    })
}

fn photo_restore_receipt_key(operation_id: &str) -> String {
    format!("{PHOTO_RESTORE_RECEIPT_PREFIX}{operation_id}")
}

pub(super) fn read_photo_restore_operation(
    connection: &Connection,
    operation_id: &str,
) -> Result<Option<PhotoRestoreReceipt>, MutationError> {
    let value = connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [photo_restore_receipt_key(operation_id)],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(mutation_error_from_sqlite)?;
    value
        .map(|value| serde_json::from_str(&value).map_err(|_| MutationError::Persistence))
        .transpose()
}

fn write_photo_restore_receipt(
    transaction: &Transaction<'_>,
    operation_id: &str,
    receipt: &PhotoRestoreReceipt,
) -> Result<(), MutationError> {
    let value = serde_json::to_string(receipt).map_err(|_| MutationError::Persistence)?;
    transaction
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES(?,?)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![photo_restore_receipt_key(operation_id), value],
        )
        .map_err(mutation_error_from_sqlite)?;
    Ok(())
}

pub(super) fn photo_restore_result_from_receipt(
    operation_id: &str,
    receipt: PhotoRestoreReceipt,
) -> Result<ExplicitPhotoRestoreResult, MutationError> {
    if receipt.photo_ids.len() != receipt.removed_at_ms.len()
        || receipt.photo_ids.len() != receipt.outcomes.len()
    {
        return Err(MutationError::Persistence);
    }
    let ordered_photo_ids = receipt.photo_ids.clone();
    let mut result = ExplicitPhotoRestoreResult {
        operation_id: operation_id.to_owned(),
        ordered_photo_ids,
        counts: ExplicitPhotoRestoreCounts::default(),
        restored: Vec::new(),
        already_active: Vec::new(),
        changed_elsewhere: Vec::new(),
        missing: Vec::new(),
    };
    for (photo_id, outcome) in receipt.photo_ids.into_iter().zip(receipt.outcomes) {
        match outcome {
            PhotoRestoreOutcome::Restored => result.restored.push(photo_id),
            PhotoRestoreOutcome::AlreadyActive => result.already_active.push(photo_id),
            PhotoRestoreOutcome::ChangedElsewhere => result.changed_elsewhere.push(photo_id),
            PhotoRestoreOutcome::Missing => result.missing.push(photo_id),
        }
    }
    result.counts = ExplicitPhotoRestoreCounts {
        restored: result.restored.len(),
        already_active: result.already_active.len(),
        changed_elsewhere: result.changed_elsewhere.len(),
        missing: result.missing.len(),
    };
    Ok(result)
}

pub(super) fn restore_photos_explicit(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    mutation: ExplicitPhotoRestoreMutation,
) -> Result<ExplicitPhotoRestoreResult, MutationError> {
    let operation_id = mutation.operation_id.clone();
    let photo_ids = mutation
        .photos
        .iter()
        .map(|photo| photo.photo_id.clone())
        .collect::<Vec<_>>();
    let removed_at_ms = mutation
        .photos
        .iter()
        .map(|photo| photo.removed_at_ms)
        .collect::<Vec<_>>();
    mutation_transaction(state, database_name, connection, |transaction| {
        if let Some(receipt) = read_photo_restore_operation(transaction, &operation_id)? {
            if receipt.photo_ids != photo_ids || receipt.removed_at_ms != removed_at_ms {
                return Err(MutationError::Conflict);
            }
            return photo_restore_result_from_receipt(&operation_id, receipt);
        }

        let mut outcomes = Vec::with_capacity(photo_ids.len());
        let mut result = ExplicitPhotoRestoreResult {
            operation_id: operation_id.clone(),
            ordered_photo_ids: photo_ids.clone(),
            counts: ExplicitPhotoRestoreCounts::default(),
            restored: Vec::new(),
            already_active: Vec::new(),
            changed_elsewhere: Vec::new(),
            missing: Vec::new(),
        };
        for photo in mutation.photos {
            let current = transaction
                .query_row(
                    "SELECT removed_at_ms FROM photos WHERE id=?",
                    [&photo.photo_id],
                    |row| row.get::<_, Option<i64>>(0),
                )
                .optional()
                .map_err(mutation_error_from_sqlite)?;
            let Some(current) = current else {
                outcomes.push(PhotoRestoreOutcome::Missing);
                result.missing.push(photo.photo_id);
                continue;
            };
            if photo_is_permanently_deleted(transaction, &photo.photo_id)?
                || unsettled_permanent_deletion_operation(transaction, &photo.photo_id)?.is_some()
            {
                outcomes.push(PhotoRestoreOutcome::ChangedElsewhere);
                result.changed_elsewhere.push(photo.photo_id);
                continue;
            }
            let Some(current_marker) = current else {
                outcomes.push(PhotoRestoreOutcome::AlreadyActive);
                result.already_active.push(photo.photo_id);
                continue;
            };
            if current_marker != photo.removed_at_ms {
                outcomes.push(PhotoRestoreOutcome::ChangedElsewhere);
                result.changed_elsewhere.push(photo.photo_id);
                continue;
            }
            let updated = transaction
                .execute(
                    "UPDATE photos SET removed_at_ms=NULL,removed_operation=NULL,association_generation=association_generation+1
                     WHERE id=? AND removed_at_ms=?",
                    params![&photo.photo_id, current_marker],
                )
                .map_err(mutation_error_from_sqlite)?;
            if updated == 0 {
                outcomes.push(PhotoRestoreOutcome::ChangedElsewhere);
                result.changed_elsewhere.push(photo.photo_id);
                continue;
            }
            outcomes.push(PhotoRestoreOutcome::Restored);
            result.restored.push(photo.photo_id);
        }
        result.counts = ExplicitPhotoRestoreCounts {
            restored: result.restored.len(),
            already_active: result.already_active.len(),
            changed_elsewhere: result.changed_elsewhere.len(),
            missing: result.missing.len(),
        };
        write_photo_restore_receipt(
            transaction,
            &operation_id,
            &PhotoRestoreReceipt {
                photo_ids,
                removed_at_ms,
                outcomes,
            },
        )?;
        Ok(result)
    })
}

/// One restore request: every Photo one operation still owns, or an explicit
/// set of Photos with the removal marker each was reviewed at. Each requested
/// Photo is compared and set inside one transaction, so a removal that changed
/// after the caller read it is reported instead of overwritten.
pub(super) fn restore_photos(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    restoration: PhotoRestoration,
) -> Result<PhotoRestorationResult, MutationError> {
    mutation_transaction(state, database_name, connection, |transaction| {
        let mut result = PhotoRestorationResult {
            restored: Vec::new(),
            counts: PhotoRestorationCounts::default(),
            changed_elsewhere: Vec::new(),
            missing: Vec::new(),
            operations: Vec::new(),
        };
        // Operations this request changed, so the response can report how many
        // Photos each still owns. The named operation counts even when it owns
        // nothing, because that is exactly what its Undo surface must learn.
        let mut touched = std::collections::BTreeSet::new();
        let requests = match restoration {
            PhotoRestoration::Operation(operation_id) => {
                touched.insert(operation_id.clone());
                let photo_ids = transaction
                    .prepare(
                        "SELECT id FROM photos
                         WHERE removed_operation=? AND removed_at_ms IS NOT NULL
                         ORDER BY removed_at_ms, id",
                    )
                    .map_err(mutation_error_from_sqlite)?
                    .query_map([operation_id.as_str()], |row| row.get::<_, String>(0))
                    .map_err(mutation_error_from_sqlite)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(mutation_error_from_sqlite)?;
                photo_ids
                    .into_iter()
                    .map(|photo_id| (photo_id, None))
                    .collect::<Vec<_>>()
            }
            PhotoRestoration::Photos(markers) => markers
                .into_iter()
                .map(|marker| (marker.photo_id, Some(marker.removed_at_ms)))
                .collect(),
        };
        for chunk in requests.chunks(PHOTO_REMOVAL_CHUNK) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let mut statement = transaction
                .prepare(&format!(
                    "SELECT id,removed_at_ms,removed_operation FROM photos WHERE id IN ({placeholders})"
                ))
                .map_err(mutation_error_from_sqlite)?;
            let rows = statement
                .query_map(
                    rusqlite::params_from_iter(chunk.iter().map(|(photo_id, _)| photo_id)),
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<i64>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                        ))
                    },
                )
                .map_err(mutation_error_from_sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(mutation_error_from_sqlite)?;
            let mut facts = std::collections::HashMap::with_capacity(rows.len());
            for (photo_id, removed_at, removed_operation) in rows {
                facts.insert(photo_id, (removed_at, removed_operation));
            }
            for (photo_id, expected_removed_at) in chunk {
                let Some((removed_at, removed_operation)) = facts.get(photo_id) else {
                    result.missing.push(photo_id.clone());
                    continue;
                };
                // A permanently deleted Photo is evidence, and one whose own
                // deletion has not settled has no known outcome to reverse;
                // neither may be restored.
                if photo_is_permanently_deleted(transaction, photo_id)?
                    || unsettled_permanent_deletion_operation(transaction, photo_id)?.is_some()
                {
                    result.changed_elsewhere.push(photo_id.clone());
                    continue;
                }
                // An explicit request restores the removal it reviewed. A
                // Photo whose marker moved on — restored and removed again, or
                // removed by another operation — is reported, never cleared.
                let reviewed = expected_removed_at.is_none_or(|expected| {
                    removed_at.is_some_and(|removed_at| removed_at == expected)
                });
                if !reviewed {
                    result.changed_elsewhere.push(photo_id.clone());
                    continue;
                }
                let Some(removed_at) = removed_at else {
                    result.changed_elsewhere.push(photo_id.clone());
                    continue;
                };
                let updated = transaction
                    .execute(
                        "UPDATE photos SET removed_at_ms=NULL,removed_operation=NULL,association_generation=association_generation+1
                         WHERE id=? AND removed_at_ms=?",
                        params![photo_id, removed_at],
                    )
                    .map_err(mutation_error_from_sqlite)?;
                if updated == 0 {
                    result.changed_elsewhere.push(photo_id.clone());
                    continue;
                }
                if let Some(operation_id) = removed_operation {
                    touched.insert(operation_id.clone());
                }
                result.restored.push(photo_id.clone());
            }
        }
        result.counts = PhotoRestorationCounts {
            restored: result.restored.len(),
            changed_elsewhere: result.changed_elsewhere.len(),
            missing: result.missing.len(),
        };
        result.operations = operation_remainders(transaction, &touched)?;
        Ok(result)
    })
}

/// How many Photos each named operation still owns. An operation with nothing
/// left is reported with zero rather than omitted, so a surface holding an
/// Undo for it can stop offering a count the Library no longer holds.
fn operation_remainders(
    transaction: &Transaction<'_>,
    operation_ids: &std::collections::BTreeSet<String>,
) -> Result<Vec<PhotoOperationRemainder>, MutationError> {
    let mut remainders = operation_ids
        .iter()
        .map(|operation_id| PhotoOperationRemainder {
            operation_id: operation_id.clone(),
            removed: 0,
        })
        .collect::<Vec<_>>();
    let deleted_prefix = PERMANENT_DELETION_DELETED_PREFIX.to_owned();
    let ids = operation_ids.iter().cloned().collect::<Vec<_>>();
    for chunk in ids.chunks(PHOTO_REMOVAL_CHUNK) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let mut statement = transaction
            .prepare(&format!(
                "SELECT removed_operation FROM photos p
                 WHERE removed_at_ms IS NOT NULL AND removed_operation IN ({placeholders})
                   AND NOT EXISTS (SELECT 1 FROM library_metadata m
                                   WHERE m.key = ? || p.id)"
            ))
            .map_err(mutation_error_from_sqlite)?;
        let rows = statement
            .query_map(
                rusqlite::params_from_iter(chunk.iter().chain(std::iter::once(&deleted_prefix))),
                |row| row.get::<_, String>(0),
            )
            .map_err(mutation_error_from_sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(mutation_error_from_sqlite)?;
        for operation_id in rows {
            if let Some(remainder) = remainders
                .iter_mut()
                .find(|remainder| remainder.operation_id == operation_id)
            {
                remainder.removed = remainder.removed.saturating_add(1);
            }
        }
    }
    Ok(remainders)
}

#[cfg(test)]
mod tests;
