//! Transactional, record-only Trash cleanup and immutable attempt receipts.

use super::codecs::parse_kind;
use super::mutation::{
    mutation_error_from_persistence, mutation_error_from_sqlite, mutation_transaction,
};
use super::owner::{MutationError, MutationVersions};
use super::{DatabaseName, StateDirectory};
use crate::{
    OriginalKind, RelativeOriginalPath, TrashAbsence, TrashCleanupBlocker, TrashCleanupCounts,
    TrashCleanupItemResult, TrashCleanupItemState, TrashCleanupResult, TrashCleanupTarget,
    TrashInspectionRecord,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};

const RECEIPT_PREFIX: &str = "trash_cleanup_operation:";
pub(super) const PERMANENT_DELETION_DELETED_PREFIX: &str = "permanent_deletion_deleted:";
const BLOCKER_ACTION: &str = "Remove this retained work before cleaning up the record.";

pub(super) fn photo_is_permanently_deleted(
    connection: &Connection,
    photo_id: &str,
) -> Result<bool, MutationError> {
    super::metadata_store::exists(
        connection,
        &format!("{PERMANENT_DELETION_DELETED_PREFIX}{photo_id}"),
        mutation_error_from_sqlite,
    )
}

enum LegacyLocationError {
    Invalid,
    Storage,
}

/// Recover the reviewed Location of a confirmed legacy deletion. A retired
/// synthetic Location is not evidence about the original filesystem entry.
fn legacy_location(
    connection: &Connection,
    photo_id: &str,
    original_id: &str,
) -> Result<Option<RelativeOriginalPath>, LegacyLocationError> {
    let Some(operation_id) = super::metadata_store::read_value(
        connection,
        &format!("{PERMANENT_DELETION_DELETED_PREFIX}{photo_id}"),
        |_| LegacyLocationError::Storage,
    )?
    else {
        return Ok(None);
    };
    let value = super::metadata_store::read_value(
        connection,
        &format!("permanent_deletion_operation:{operation_id}"),
        |_| LegacyLocationError::Storage,
    )?
    .ok_or(LegacyLocationError::Invalid)?;
    let receipt: serde_json::Value =
        serde_json::from_str(&value).map_err(|_| LegacyLocationError::Invalid)?;
    if receipt.get("operation_id").and_then(|value| value.as_str()) != Some(operation_id.as_str()) {
        return Err(LegacyLocationError::Invalid);
    }
    let items = receipt
        .get("items")
        .and_then(|value| value.as_array())
        .ok_or(LegacyLocationError::Invalid)?;
    let mut matches = items
        .iter()
        .filter(|item| item.get("photo_id").and_then(|value| value.as_str()) == Some(photo_id));
    let item = matches.next().ok_or(LegacyLocationError::Invalid)?;
    if matches.next().is_some()
        || item.get("original_id").and_then(|value| value.as_str()) != Some(original_id)
    {
        return Err(LegacyLocationError::Invalid);
    }
    let path = item
        .get("relative_path")
        .and_then(|value| value.as_str())
        .ok_or(LegacyLocationError::Invalid)?;
    let path =
        RelativeOriginalPath::parse(path.to_owned()).map_err(|_| LegacyLocationError::Invalid)?;
    let current = connection.query_row(
        "SELECT o.relative_path FROM photos p JOIN original_files o ON o.id=p.original_id WHERE p.id=? AND o.id=?",
        params![photo_id, original_id], |row| row.get::<_, String>(0),
    ).optional().map_err(|_| LegacyLocationError::Storage)?.ok_or(LegacyLocationError::Invalid)?;
    let retired = format!(
        "{}{original_id}",
        super::owner::PERMANENT_DELETION_RETIRED_LOCATION_PREFIX
    );
    if current != path.as_str() && current != retired {
        return Err(LegacyLocationError::Invalid);
    }
    Ok(Some(path))
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Intent {
    photo_id: String,
    removed_at_ms: i64,
    original_id: String,
    relative_path: String,
}

impl From<&TrashCleanupTarget> for Intent {
    fn from(target: &TrashCleanupTarget) -> Self {
        Self {
            photo_id: target.photo_id.clone(),
            removed_at_ms: target.removed_at_ms,
            original_id: target.original_id.clone(),
            relative_path: target.relative_path.as_str().to_owned(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Receipt {
    intent: Vec<Intent>,
    items: Vec<StoredItem>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredItem {
    photo_id: String,
    relative_path: String,
    kind: String,
    state: String,
    message: Option<String>,
    blockers: Vec<StoredBlocker>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredBlocker {
    kind: String,
    name: String,
    action: String,
}

impl From<&TrashCleanupItemResult> for StoredItem {
    fn from(item: &TrashCleanupItemResult) -> Self {
        Self {
            photo_id: item.photo_id.clone(),
            relative_path: item.relative_path.as_str().to_owned(),
            kind: match item.kind {
                OriginalKind::Raw => "raw",
                OriginalKind::Jpeg => "jpeg",
            }
            .to_owned(),
            state: match item.state {
                TrashCleanupItemState::Cleaned => "cleaned",
                TrashCleanupItemState::Changed => "changed",
                TrashCleanupItemState::Present => "present",
                TrashCleanupItemState::Blocked => "blocked",
                TrashCleanupItemState::Unavailable => "unavailable",
                TrashCleanupItemState::InspectionFailed => "inspection-failed",
            }
            .to_owned(),
            message: item.message.clone(),
            blockers: item
                .blockers
                .iter()
                .map(|blocker| StoredBlocker {
                    kind: blocker.kind.clone(),
                    name: blocker.name.clone(),
                    action: blocker.action.clone(),
                })
                .collect(),
        }
    }
}

fn result_from_receipt(
    operation_id: &str,
    receipt: Receipt,
) -> Result<TrashCleanupResult, MutationError> {
    if receipt.intent.len() != receipt.items.len()
        || receipt.intent.is_empty()
        || receipt.intent.len() > crate::TRASH_CLEANUP_MAX
    {
        return Err(MutationError::Persistence);
    }
    let mut counts = TrashCleanupCounts::default();
    let mut items = Vec::with_capacity(receipt.items.len());
    for (intent, item) in receipt.intent.into_iter().zip(receipt.items) {
        if intent.photo_id != item.photo_id || intent.relative_path != item.relative_path {
            return Err(MutationError::Persistence);
        }
        let state = match item.state.as_str() {
            "cleaned" => {
                counts.cleaned += 1;
                TrashCleanupItemState::Cleaned
            }
            "changed" => {
                counts.changed += 1;
                TrashCleanupItemState::Changed
            }
            "present" => {
                counts.present += 1;
                TrashCleanupItemState::Present
            }
            "blocked" => {
                counts.blocked += 1;
                TrashCleanupItemState::Blocked
            }
            "unavailable" => {
                counts.unavailable += 1;
                TrashCleanupItemState::Unavailable
            }
            "inspection-failed" => {
                counts.inspection_failed += 1;
                TrashCleanupItemState::InspectionFailed
            }
            _ => return Err(MutationError::Persistence),
        };
        items.push(TrashCleanupItemResult {
            photo_id: item.photo_id,
            relative_path: RelativeOriginalPath::parse(item.relative_path)
                .map_err(|_| MutationError::Persistence)?,
            kind: parse_kind(&item.kind).map_err(|_| MutationError::Persistence)?,
            state,
            message: item.message,
            blockers: item
                .blockers
                .into_iter()
                .map(|blocker| TrashCleanupBlocker {
                    kind: blocker.kind,
                    name: blocker.name,
                    action: blocker.action,
                })
                .collect(),
        });
    }
    Ok(TrashCleanupResult {
        operation_id: operation_id.to_owned(),
        reviewed: items.len(),
        counts,
        items,
    })
}

fn read_receipt(
    connection: &Connection,
    operation_id: &str,
) -> Result<Option<Receipt>, MutationError> {
    super::metadata_store::read_value(
        connection,
        &format!("{RECEIPT_PREFIX}{operation_id}"),
        mutation_error_from_sqlite,
    )?
    .map(|value| serde_json::from_str(&value).map_err(|_| MutationError::Persistence))
    .transpose()
}

pub(super) fn read_trash_cleanup(
    connection: &Connection,
    operation_id: &str,
) -> Result<TrashCleanupResult, MutationError> {
    let receipt = read_receipt(connection, operation_id)?.ok_or(MutationError::NotFound)?;
    result_from_receipt(operation_id, receipt)
}

pub(super) fn trash_cleanup_replay(
    connection: &Connection,
    operation_id: &str,
    targets: &[TrashCleanupTarget],
) -> Result<Option<TrashCleanupResult>, MutationError> {
    let Some(receipt) = read_receipt(connection, operation_id)? else {
        return Ok(None);
    };
    if receipt.intent != targets.iter().map(Intent::from).collect::<Vec<_>>() {
        return Err(MutationError::Conflict);
    }
    result_from_receipt(operation_id, receipt).map(Some)
}

pub(super) fn trash_inspection(
    connection: &Connection,
) -> Result<Vec<TrashInspectionRecord>, MutationError> {
    let mut statement = connection
        .prepare(
            "SELECT p.id,p.removed_at_ms,o.id,o.relative_path,o.kind,o.size,o.available
         FROM photos p JOIN original_files o ON o.id=p.original_id
         WHERE p.removed_at_ms IS NOT NULL ORDER BY p.removed_at_ms DESC,p.id",
        )
        .map_err(mutation_error_from_sqlite)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
            ))
        })
        .map_err(mutation_error_from_sqlite)?;
    rows.map(|row| {
        let (photo_id, removed_at_ms, original_id, path, kind, size, available) =
            row.map_err(mutation_error_from_sqlite)?;
        let remembered = match legacy_location(connection, &photo_id, &original_id) {
            Ok(Some(path)) => path.as_str().to_owned(),
            Ok(None) | Err(LegacyLocationError::Invalid) => path,
            Err(LegacyLocationError::Storage) => return Err(MutationError::Persistence),
        };
        Ok(TrashInspectionRecord {
            photo_id,
            removed_at_ms,
            original_id,
            relative_path: RelativeOriginalPath::parse(remembered)
                .map_err(|_| MutationError::Persistence)?,
            kind: parse_kind(&kind).map_err(|_| MutationError::Persistence)?,
            size: u64::try_from(size).map_err(|_| MutationError::Persistence)?,
            available: available != 0,
        })
    })
    .collect()
}

fn blocker(kind: &str, name: String) -> TrashCleanupBlocker {
    TrashCleanupBlocker {
        kind: kind.to_owned(),
        name,
        action: BLOCKER_ACTION.to_owned(),
    }
}

// Resolve processing references by their owned Photo identity, never by substring.
// Malformed retained records fail closed instead of silently hiding dependencies.
fn processing_blockers(
    connection: &Connection,
) -> Result<HashMap<String, Vec<TrashCleanupBlocker>>, MutationError> {
    super::processing_export::scan_artifact_photo_ids(connection)
        .map_err(mutation_error_from_persistence)?;
    let mut by_photo: HashMap<String, Vec<TrashCleanupBlocker>> = HashMap::new();
    let mut artifacts = HashMap::new();
    for (key, value) in super::metadata_store::scan_records(
        connection,
        "processing_artifact:",
        mutation_error_from_sqlite,
    )? {
        let record: serde_json::Value =
            serde_json::from_str(&value).map_err(|_| MutationError::Persistence)?;
        let photo = record
            .get("photo_id")
            .and_then(|value| value.as_str())
            .ok_or(MutationError::Persistence)?;
        let name = key
            .strip_prefix("processing_artifact:")
            .ok_or(MutationError::Persistence)?;
        artifacts.insert(name.to_owned(), photo.to_owned());
        by_photo
            .entry(photo.to_owned())
            .or_default()
            .push(blocker("processing-artifact", name.to_owned()));
    }
    for (key, value) in super::metadata_store::scan_records(
        connection,
        "processing_workspace_artifact:",
        mutation_error_from_sqlite,
    )? {
        let id = key
            .strip_prefix("processing_workspace_artifact:")
            .ok_or(MutationError::Persistence)?;
        crate::processing::validate_bounded_name(id, crate::processing::MAXIMUM_ARTIFACT_ID_BYTES)
            .map_err(|_| MutationError::Persistence)?;
        value
            .parse::<u64>()
            .map_err(|_| MutationError::Persistence)?;
        // A workspace publication with no artifact record cannot establish its
        // Photo owner. Refuse rather than silently discarding retained work.
        if !artifacts.contains_key(id) {
            return Err(MutationError::Persistence);
        }
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Retention {
        artifact_id: String,
        retain_until: u64,
        requests: Vec<String>,
    }
    for (key, value) in super::metadata_store::scan_records(
        connection,
        "processing_artifact_retention:",
        mutation_error_from_sqlite,
    )? {
        let retention: Retention =
            serde_json::from_str(&value).map_err(|_| MutationError::Persistence)?;
        crate::processing::validate_bounded_name(
            &retention.artifact_id,
            crate::processing::MAXIMUM_ARTIFACT_ID_BYTES,
        )
        .map_err(|_| MutationError::Persistence)?;
        if key != format!("processing_artifact_retention:{}", retention.artifact_id)
            || retention.requests.iter().any(String::is_empty)
            || !artifacts.contains_key(&retention.artifact_id)
        {
            return Err(MutationError::Persistence);
        }
        let _ = retention.retain_until;
    }
    for work in super::processing_export::scan_processing_export_works(connection)
        .map_err(mutation_error_from_persistence)?
    {
        if !work.state.is_terminal() {
            by_photo
                .entry(work.admission.photo_id.clone())
                .or_default()
                .push(blocker(
                    "processing-artifact",
                    work.admission.request_id.clone(),
                ));
        }
    }
    Ok(by_photo)
}

fn retained_blockers(
    connection: &Connection,
    photo_id: &str,
    processing: &HashMap<String, Vec<TrashCleanupBlocker>>,
) -> Result<Vec<TrashCleanupBlocker>, MutationError> {
    let mut blockers = Vec::new();
    for (sql, kind) in [
        (
            "SELECT photo_id FROM edit_recipes WHERE photo_id=? ORDER BY photo_id",
            "edit-recipe",
        ),
        (
            "SELECT id FROM exports WHERE photo_id=? ORDER BY id",
            "export",
        ),
        (
            "SELECT photo_id FROM development_proxies WHERE photo_id=? ORDER BY photo_id",
            "development-proxy",
        ),
    ] {
        let mut statement = connection
            .prepare(sql)
            .map_err(mutation_error_from_sqlite)?;
        let rows = statement
            .query_map([photo_id], |row| row.get::<_, String>(0))
            .map_err(mutation_error_from_sqlite)?;
        for name in rows {
            blockers.push(blocker(kind, name.map_err(mutation_error_from_sqlite)?));
        }
    }
    if super::metadata_store::exists(
        connection,
        &format!("composable_edit_recipe:{photo_id}"),
        mutation_error_from_sqlite,
    )? {
        blockers.push(blocker("edit-recipe", photo_id.to_owned()));
    }
    let now = super::export::export_unix_seconds() as i64;
    let mut statement = connection
        .prepare("SELECT id FROM xmp_exports WHERE photo_id=? AND expires_at > ? ORDER BY id")
        .map_err(mutation_error_from_sqlite)?;
    let rows = statement
        .query_map(params![photo_id, now], |row| row.get::<_, String>(0))
        .map_err(mutation_error_from_sqlite)?;
    for name in rows {
        blockers.push(blocker(
            "metadata-export",
            name.map_err(mutation_error_from_sqlite)?,
        ));
    }
    if let Some(retained) = processing.get(photo_id) {
        blockers.extend(retained.iter().cloned());
    }
    Ok(blockers)
}

fn remove_photo(
    transaction: &Transaction<'_>,
    photo_id: &str,
    original_id: &str,
    path: &str,
    kind: &str,
    affected_albums: &mut BTreeSet<String>,
) -> Result<(), MutationError> {
    super::scan_recovery::retire_sidecar_association(transaction, photo_id, path, kind)
        .map_err(mutation_error_from_persistence)?;
    let memberships = {
        let mut statement = transaction
            .prepare(
                "SELECT album_id,position FROM album_members WHERE photo_id=? ORDER BY album_id",
            )
            .map_err(mutation_error_from_sqlite)?;
        let rows = statement
            .query_map([photo_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(mutation_error_from_sqlite)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(mutation_error_from_sqlite)?
    };
    for (album_id, position) in memberships {
        transaction
            .execute(
                "DELETE FROM album_members WHERE album_id=? AND photo_id=?",
                params![album_id, photo_id],
            )
            .map_err(mutation_error_from_sqlite)?;
        // Move later positions out of the unique index's way before compacting.
        let maximum: Option<i64> = transaction
            .query_row(
                "SELECT MAX(position) FROM album_members WHERE album_id=?",
                [&album_id],
                |row| row.get(0),
            )
            .map_err(mutation_error_from_sqlite)?;
        if let Some(maximum) = maximum.filter(|max| *max > position) {
            let offset = maximum
                .checked_add(1)
                .and_then(|offset| maximum.checked_add(offset).map(|_| offset))
                .ok_or(MutationError::Persistence)?;
            transaction
                .execute(
                    "UPDATE album_members SET position=position+? WHERE album_id=? AND position>?",
                    params![offset, album_id, position],
                )
                .map_err(mutation_error_from_sqlite)?;
            transaction.execute("UPDATE album_members SET position=position-?-1 WHERE album_id=? AND position>=?", params![offset, album_id, offset]).map_err(mutation_error_from_sqlite)?;
        }
        affected_albums.insert(album_id);
    }
    super::xmp::archive_for_photo(transaction, photo_id)
        .map_err(mutation_error_from_persistence)?;
    transaction
        .execute(
            "DELETE FROM xmp_exports WHERE photo_id=? AND expires_at <= ?",
            params![photo_id, super::export::export_unix_seconds() as i64],
        )
        .map_err(mutation_error_from_sqlite)?;
    transaction
        .execute("DELETE FROM photos WHERE id=?", [photo_id])
        .map_err(mutation_error_from_sqlite)?;
    transaction.execute("DELETE FROM original_files WHERE id=? AND NOT EXISTS (SELECT 1 FROM photos WHERE original_id=?)", params![original_id, original_id]).map_err(mutation_error_from_sqlite)?;
    Ok(())
}

fn evaluate_target(
    connection: &Connection,
    target: &TrashCleanupTarget,
    processing: &HashMap<String, Vec<TrashCleanupBlocker>>,
) -> Result<TrashCleanupItemResult, MutationError> {
    let current = connection.query_row(
        "SELECT p.removed_at_ms,o.id,o.relative_path,o.kind FROM photos p JOIN original_files o ON o.id=p.original_id WHERE p.id=?",
        [&target.photo_id], |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?)),
    ).optional().map_err(mutation_error_from_sqlite)?;
    let inferred_kind =
        crate::identity::classify_name(target.relative_path.as_str()).unwrap_or(OriginalKind::Raw);
    let mut item = TrashCleanupItemResult {
        photo_id: target.photo_id.clone(),
        relative_path: target.relative_path.clone(),
        kind: inferred_kind,
        state: TrashCleanupItemState::Unavailable,
        message: None,
        blockers: Vec::new(),
    };
    if let Some((marker, original_id, path, kind)) = current {
        item.kind = parse_kind(&kind).map_err(|_| MutationError::Persistence)?;
        let remembered = match legacy_location(connection, &target.photo_id, &original_id) {
            Err(LegacyLocationError::Storage) => return Err(MutationError::Persistence),
            other => other,
        };
        if marker != Some(target.removed_at_ms) || original_id != target.original_id {
            item.state = TrashCleanupItemState::Changed;
        } else if matches!(remembered, Err(LegacyLocationError::Invalid)) {
            item.state = TrashCleanupItemState::InspectionFailed;
        } else if remembered
            .ok()
            .flatten()
            .as_ref()
            .map_or(path.as_str(), |value| value.as_str())
            != target.relative_path.as_str()
        {
            item.state = TrashCleanupItemState::Changed;
        } else {
            item.blockers = retained_blockers(connection, &target.photo_id, processing)?;
            item.blockers.extend(target.blockers.iter().cloned());
            if !item.blockers.is_empty() {
                item.state = TrashCleanupItemState::Blocked;
            } else {
                item.state = match target.absence {
                    TrashAbsence::Present => TrashCleanupItemState::Present,
                    TrashAbsence::Uninspectable => TrashCleanupItemState::InspectionFailed,
                    TrashAbsence::Absent => TrashCleanupItemState::Cleaned,
                };
            }
        }
    }
    Ok(item)
}

/// Read-only revalidation used by the review. It performs the same record,
/// retained-work, and filesystem checks as cleanup but commits nothing.
pub(super) fn preview_trash_cleanup(
    connection: &mut Connection,
    targets: &[TrashCleanupTarget],
) -> Result<crate::TrashCleanupPreview, MutationError> {
    if targets.is_empty() || targets.len() > crate::TRASH_CLEANUP_MAX {
        return Err(MutationError::Invalid);
    }
    let mut seen = HashSet::with_capacity(targets.len());
    if targets.iter().any(|target| {
        target.photo_id.is_empty()
            || target.original_id.is_empty()
            || target.removed_at_ms < 0
            || !seen.insert(&target.photo_id)
    }) {
        return Err(MutationError::Invalid);
    }
    let transaction = connection
        .transaction()
        .map_err(mutation_error_from_sqlite)?;
    let processing = processing_blockers(&transaction)?;
    let items = targets
        .iter()
        .map(|target| {
            let item = evaluate_target(&transaction, target, &processing)?;
            Ok(crate::TrashCleanupPreviewItem {
                photo_id: item.photo_id,
                relative_path: item.relative_path,
                kind: item.kind,
                state: item.state,
                absence: target.absence,
                message: item.message,
                blockers: item.blockers,
            })
        })
        .collect::<Result<Vec<_>, MutationError>>()?;
    transaction.rollback().map_err(mutation_error_from_sqlite)?;
    Ok(crate::TrashCleanupPreview { items })
}

pub(super) fn cleanup_trash(
    state: &StateDirectory,
    db: &DatabaseName,
    connection: &mut Connection,
    operation_id: String,
    targets: Vec<TrashCleanupTarget>,
    versions: &mut MutationVersions,
) -> Result<TrashCleanupResult, MutationError> {
    if operation_id.is_empty() || targets.is_empty() || targets.len() > crate::TRASH_CLEANUP_MAX {
        return Err(MutationError::Invalid);
    }
    let mut seen = HashSet::with_capacity(targets.len());
    if targets.iter().any(|target| {
        target.photo_id.is_empty()
            || target.original_id.is_empty()
            || target.removed_at_ms < 0
            || !seen.insert(&target.photo_id)
    }) {
        return Err(MutationError::Invalid);
    }
    let mut affected_albums = BTreeSet::new();
    let mut affected_photos = BTreeSet::new();
    let result = mutation_transaction(state, db, connection, |transaction| {
        if let Some(result) = trash_cleanup_replay(transaction, &operation_id, &targets)? {
            return Ok(result);
        }
        let processing = processing_blockers(transaction)?;
        let items = targets
            .iter()
            .map(|target| evaluate_target(transaction, target, &processing))
            .collect::<Result<Vec<_>, _>>()?;
        for (target, result) in targets.iter().zip(&items) {
            if result.state != TrashCleanupItemState::Cleaned {
                continue;
            }
            let kind = match result.kind {
                OriginalKind::Raw => "raw",
                OriginalKind::Jpeg => "jpeg",
            };
            remove_photo(
                transaction,
                &target.photo_id,
                &target.original_id,
                target.relative_path.as_str(),
                kind,
                &mut affected_albums,
            )?;
            affected_photos.insert(target.photo_id.clone());
        }
        if affected_albums
            .iter()
            .any(|id| !versions.can_advance_album(id))
            || affected_photos
                .iter()
                .any(|id| !versions.can_advance_photo(id))
        {
            return Err(MutationError::Persistence);
        }
        let receipt = Receipt {
            intent: targets.iter().map(Intent::from).collect(),
            items: items.iter().map(StoredItem::from).collect(),
        };
        let value = serde_json::to_string(&receipt).map_err(|_| MutationError::Persistence)?;
        super::metadata_store::insert_only(
            transaction,
            &format!("{RECEIPT_PREFIX}{operation_id}"),
            &value,
            mutation_error_from_sqlite,
        )?;
        result_from_receipt(&operation_id, receipt)
    })?;
    for id in &affected_photos {
        versions.advance_photo(id)?;
    }
    for id in &affected_albums {
        versions.advance_album(id)?;
    }
    Ok(result)
}
