//! Scan application, recovery facts, fingerprint enrollment, snapshot decode,
//! and Preview seeding: the persisted state a scan publishes and recovers.

use super::owner::{
    PERMANENT_DELETION_RETIRED_LOCATION_PREFIX, PersistenceError, allocate_library_id,
    permanently_deleted_original_ids, write_transaction,
};
use super::{DatabaseName, StateDirectory};
use crate::identity::classify_name;
use crate::{
    AppliedRelocations, CaptureFact, CaptureMetadataState, CaptureTimeField, DiscoveredOriginal,
    OriginalErrorCategory, OriginalFacts, OriginalFingerprint, OriginalKind, OriginalRecord,
    OriginalScanError, PhotoRecord, PreviewSeed, PreviewSeedResult, PreviewState, RecoverySurvey,
    RelativeOriginalPath, RequestedRelocation, ScanSnapshot, SelectionState,
    UnavailablePhotoRecord, preview_should_preserve, reconcile, selected_source,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use std::collections::{HashMap, HashSet};

pub(super) fn recovery_facts(
    connection: &Connection,
    original_ids: &[String],
) -> Result<Vec<OriginalFingerprint>, PersistenceError> {
    let mut facts = Vec::with_capacity(original_ids.len());
    for original_id in original_ids {
        let row = connection
            .query_row(
                "SELECT digest,size,mtime_ms FROM original_fingerprints WHERE original_id=?",
                [original_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, f64>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|_| PersistenceError::Storage)?;
        if let Some((digest, size, mtime_ms)) = row {
            facts.push(OriginalFingerprint {
                original_id: original_id.clone(),
                digest,
                size: size.try_into().map_err(|_| PersistenceError::Storage)?,
                mtime_ms,
            });
        }
    }
    Ok(facts)
}

pub(super) fn next_fingerprint_target(
    connection: &Connection,
) -> Result<Option<FingerprintTarget>, PersistenceError> {
    let row = connection
        .query_row(
            "SELECT o.id,o.relative_path,o.kind,o.size,o.mtime_ms
             FROM original_files o
             LEFT JOIN original_fingerprints f ON f.original_id=o.id
             WHERE o.available=1 AND o.error_category IS NULL
               AND (f.original_id IS NULL OR f.size != o.size OR f.mtime_ms != o.mtime_ms)
             ORDER BY o.relative_path COLLATE BINARY
             LIMIT 1",
            [],
            |row| {
                Ok(FingerprintTarget {
                    original_id: row.get(0)?,
                    relative_path: row.get(1)?,
                    kind: match row.get::<_, String>(2)?.as_str() {
                        "raw" => crate::OriginalKind::Raw,
                        "jpeg" => crate::OriginalKind::Jpeg,
                        _ => return Err(rusqlite::Error::InvalidQuery),
                    },
                    size: row
                        .get::<_, i64>(3)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    mtime_ms: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    Ok(row)
}

pub(super) fn store_fingerprint(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    fingerprint: OriginalFingerprint,
) -> Result<(), PersistenceError> {
    write_transaction(state, database_name, connection, |transaction| {
        // A fingerprint is stored only for the revision the hasher observed;
        // an Original that moved on meanwhile keeps its stale row dropped by
        // the next scan and is re-targeted by enrollment.
        let stored = transaction
            .execute(
                "INSERT INTO original_fingerprints(original_id,digest,size,mtime_ms)
                 SELECT ?,?,?,? FROM original_files
                 WHERE id=? AND available=1 AND error_category IS NULL
                   AND size=? AND mtime_ms=?",
                params![
                    fingerprint.original_id,
                    fingerprint.digest,
                    i64::try_from(fingerprint.size).map_err(|_| PersistenceError::Storage)?,
                    fingerprint.mtime_ms,
                    fingerprint.original_id,
                    i64::try_from(fingerprint.size).map_err(|_| PersistenceError::Storage)?,
                    fingerprint.mtime_ms,
                ],
            )
            .map_err(|_| PersistenceError::Storage)?;
        if stored != 1 {
            return Err(PersistenceError::InvalidRecovery);
        }
        Ok(())
    })
}

/// One consistent read of every unavailable Photo plus the Album
/// memberships, for the bounded manual recovery review entry.
pub(super) fn recovery_survey(connection: &Connection) -> Result<RecoverySurvey, PersistenceError> {
    let mut unavailable = Vec::new();
    {
        let mut statement = connection
            .prepare(
                "SELECT o.id,o.relative_path,o.kind,p.id,p.rating,p.selection_state,f.digest,
                        (SELECT COUNT(*) FROM album_members m WHERE m.photo_id=p.id)
                 FROM photos p
                 JOIN original_files o ON o.id=p.original_id
                 LEFT JOIN original_fingerprints f ON f.original_id=o.id
                 WHERE p.available=0 AND p.removed_at_ms IS NULL
                 ORDER BY o.relative_path COLLATE BINARY",
            )
            .map_err(|_| PersistenceError::Storage)?;
        let rows = statement
            .query_map([], |row| {
                Ok(UnavailablePhotoRecord {
                    original_id: row.get(0)?,
                    relative_path: row.get(1)?,
                    kind: parse_kind(&row.get::<_, String>(2)?)?,
                    photo_id: row.get(3)?,
                    rating: row
                        .get::<_, i64>(4)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    selection_state: parse_selection_state(&row.get::<_, String>(5)?)?,
                    fingerprint: row.get(6)?,
                    album_count: row
                        .get::<_, i64>(7)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                })
            })
            .map_err(|_| PersistenceError::Storage)?;
        for row in rows {
            unavailable.push(row.map_err(|_| PersistenceError::Storage)?);
        }
    }
    let mut referenced_photo_ids = HashSet::new();
    {
        let mut statement = connection
            .prepare(
                "SELECT DISTINCT photo_id FROM (
                   SELECT photo_id FROM album_members
                   UNION ALL
                   SELECT photo_id FROM edit_recipes
                 )",
            )
            .map_err(|_| PersistenceError::Storage)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|_| PersistenceError::Storage)?;
        for row in rows {
            referenced_photo_ids.insert(row.map_err(|_| PersistenceError::Storage)?);
        }
    }
    Ok(RecoverySurvey {
        unavailable,
        referenced_photo_ids,
    })
}

/// Revalidates one confirmed manual relocation batch and commits it
/// atomically. Every mapping is rechecked against the persisted state
/// observed inside the transaction: stale confirmations, colliding
/// destinations, occupied Locations without an explicit retire, and
/// occupiers with independent user state reject the whole batch without
/// partial association.
pub(super) fn apply_manual_relocations(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    relocations: &[RequestedRelocation],
) -> Result<AppliedRelocations, PersistenceError> {
    write_transaction(state, database_name, connection, |transaction| {
        if relocations.is_empty() {
            return Err(PersistenceError::InvalidRecovery);
        }
        let mut destinations = HashSet::with_capacity(relocations.len());
        let mut relocating_ids = HashSet::with_capacity(relocations.len());
        for relocation in relocations {
            // Two mappings for one Original File are a colliding batch: only
            // one Location could win, so the batch is refused as a whole.
            if !relocating_ids.insert(relocation.original_id.clone()) {
                return Err(PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "colliding",
                });
            }
        }
        for relocation in relocations {
            let to = RelativeOriginalPath::parse(relocation.to_location.clone()).map_err(|_| {
                PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "invalid-location",
                }
            })?;
            if !destinations.insert(to.as_str().to_owned()) {
                return Err(PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "colliding",
                });
            }
            let persisted = transaction
                .query_row(
                    "SELECT available,kind FROM original_files WHERE id=?",
                    params![relocation.original_id],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(|_| PersistenceError::Storage)?;
            let Some((available, kind)) = persisted else {
                return Err(PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "stale",
                });
            };
            if available != 0 {
                return Err(PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "stale",
                });
            }
            let filename = to.as_str().rsplit('/').next().unwrap_or_default();
            let kind = parse_kind(&kind).map_err(|_| PersistenceError::Storage)?;
            if classify_name(filename) != Some(kind) {
                return Err(PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "kind-mismatch",
                });
            }
            // A destination owned by another Original requires either a
            // simultaneous relocation of that owner or an explicit retire of
            // an otherwise unreferenced default-state occupier.
            let owner = transaction
                .query_row(
                    "SELECT id FROM original_files WHERE relative_path=?",
                    params![to.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|_| PersistenceError::Storage)?;
            if let Some(owner_id) = owner
                && owner_id != relocation.original_id
                && !relocating_ids.contains(&owner_id)
            {
                if !relocation.retire_destination {
                    return Err(PersistenceError::InvalidRecoveryMapping {
                        original_id: relocation.original_id.clone(),
                        reason: "occupied",
                    });
                }
                let occupant = transaction
                    .query_row(
                        "SELECT id,rating,selection_state,
                            EXISTS(SELECT 1 FROM edit_recipes e WHERE e.photo_id=photos.id)
                     FROM photos WHERE original_id=?",
                        params![owner_id],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, i64>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, i64>(3)? != 0,
                            ))
                        },
                    )
                    .optional()
                    .map_err(|_| PersistenceError::Storage)?;
                if let Some((photo_id, rating, selection_state, has_saved_edits)) = occupant {
                    if rating != 0 || selection_state != "undecided" || has_saved_edits {
                        return Err(PersistenceError::InvalidRecoveryMapping {
                            original_id: relocation.original_id.clone(),
                            reason: "occupied",
                        });
                    }
                    let members = transaction
                        .query_row(
                            "SELECT COUNT(*) FROM album_members WHERE photo_id=?",
                            params![photo_id],
                            |row| row.get::<_, i64>(0),
                        )
                        .map_err(|_| PersistenceError::Storage)?;
                    if members != 0 {
                        return Err(PersistenceError::InvalidRecoveryMapping {
                            original_id: relocation.original_id.clone(),
                            reason: "occupied",
                        });
                    }
                    transaction
                        .execute(
                            "UPDATE photos SET association_generation=association_generation+1 WHERE id=?",
                            params![photo_id],
                        )
                        .map_err(|_| PersistenceError::Storage)?;
                    retire_sidecar_association(
                        transaction,
                        &photo_id,
                        &to.to_string(),
                        match kind {
                            crate::OriginalKind::Raw => "raw",
                            crate::OriginalKind::Jpeg => "jpeg",
                        },
                    )?;
                    transaction
                        .execute("DELETE FROM photos WHERE id=?", params![photo_id])
                        .map_err(|_| PersistenceError::Storage)?;
                }
                transaction
                    .execute("DELETE FROM original_files WHERE id=?", params![owner_id])
                    .map_err(|_| PersistenceError::Storage)?;
            }
        }
        // Two-phase Location updates keep direct swaps from violating the
        // UNIQUE(relative_path) constraint.
        for relocation in relocations {
            transaction
                .execute(
                    "UPDATE original_files SET relative_path=? WHERE id=?",
                    params![
                        format!("\u{0}manual/{}", relocation.original_id),
                        relocation.original_id
                    ],
                )
                .map_err(|_| PersistenceError::Storage)?;
        }
        for relocation in relocations {
            let to = RelativeOriginalPath::parse(relocation.to_location.clone()).map_err(|_| {
                PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "invalid-location",
                }
            })?;
            let destination_photo: Option<(String, String)> = transaction
                .query_row(
                    "SELECT p.id,o.kind FROM photos p JOIN original_files o ON o.id=p.original_id WHERE o.id=?",
                    [&relocation.original_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(|_| PersistenceError::Storage)?;
            if let Some((photo_id, kind)) = destination_photo {
                transaction
                    .execute(
                        "UPDATE photos SET association_generation=association_generation+1 WHERE id=?",
                        [&photo_id],
                    )
                    .map_err(|_| PersistenceError::Storage)?;
                retire_sidecar_association(transaction, &photo_id, to.as_str(), &kind)?;
            }
            let size =
                i64::try_from(relocation.facts.size).map_err(|_| PersistenceError::Storage)?;
            let changed = transaction
                .execute(
                    "UPDATE original_files SET relative_path=?,size=?,mtime_ms=?,available=1,
                       error_category=NULL,error_message=NULL,
                       capture_metadata_state='pending',capture_order_key=NULL,
                       capture_time_field=NULL,capture_offset_minutes=NULL,
                       capture_source_revision=NULL
                     WHERE id=?",
                    params![
                        to.as_str(),
                        size,
                        relocation.facts.mtime_ms,
                        relocation.original_id
                    ],
                )
                .map_err(|_| PersistenceError::Storage)?;
            if changed != 1 {
                return Err(PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "stale",
                });
            }
            let changed = transaction
                .execute(
                    "UPDATE photos SET available=1,preview_state='inspection-pending',
                       preview_source_revision=NULL,preview_width=NULL,preview_height=NULL,
                       cache_revision=NULL,sort_path=? WHERE original_id=?",
                    params![to.as_str(), relocation.original_id],
                )
                .map_err(|_| PersistenceError::Storage)?;
            if changed != 1 {
                return Err(PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "stale",
                });
            }
            // Fingerprints always describe the current persisted revision:
            // the Application layer verified the digest against the candidate
            // before submitting, so the observed facts replace the stale ones.
            transaction
                .execute(
                    "UPDATE original_fingerprints SET size=?,mtime_ms=? WHERE original_id=?",
                    params![size, relocation.facts.mtime_ms, relocation.original_id],
                )
                .map_err(|_| PersistenceError::Storage)?;
        }
        let unavailable = transaction
            .query_row("SELECT COUNT(*) FROM photos WHERE available=0", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(|_| PersistenceError::Storage)?;
        Ok(AppliedRelocations {
            relocated_photos: relocations.len() as u64,
            unavailable_photos: unavailable
                .try_into()
                .map_err(|_| PersistenceError::Storage)?,
        })
    })
}

pub(super) fn fingerprint_counts(
    connection: &Connection,
) -> Result<FingerprintCounts, PersistenceError> {
    connection
        .query_row(
            "SELECT
               SUM(CASE WHEN f.original_id IS NOT NULL AND f.size=o.size AND f.mtime_ms=o.mtime_ms THEN 1 ELSE 0 END),
               SUM(CASE WHEN o.available=1 AND o.error_category IS NULL AND (f.original_id IS NULL OR f.size != o.size OR f.mtime_ms != o.mtime_ms) THEN 1 ELSE 0 END)
             FROM original_files o
             LEFT JOIN original_fingerprints f ON f.original_id=o.id",
            [],
            |row| {
                Ok(FingerprintCounts {
                    enrolled: row.get::<_, Option<i64>>(0)?.unwrap_or(0).try_into().map_err(|_| rusqlite::Error::InvalidQuery)?,
                    pending: row.get::<_, Option<i64>>(1)?.unwrap_or(0).try_into().map_err(|_| rusqlite::Error::InvalidQuery)?,
                })
            },
        )
        .map_err(|_| PersistenceError::Storage)
}
pub(super) fn snapshot(connection: &Connection) -> Result<ScanSnapshot, PersistenceError> {
    let originals = connection
        .prepare(
            "SELECT id,relative_path,kind,size,mtime_ms,available,error_category,error_message,
                    capture_metadata_state,capture_order_key,capture_time_field,
                    capture_offset_minutes,capture_source_revision
             FROM original_files ORDER BY relative_path COLLATE BINARY",
        )
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], |row| {
            Ok(OriginalRecord {
                id: row.get(0)?,
                relative_path: crate::RelativeOriginalPath::parse(row.get::<_, String>(1)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                kind: parse_kind(&row.get::<_, String>(2)?)?,
                facts: OriginalFacts {
                    size: row
                        .get::<_, i64>(3)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    mtime_ms: row.get(4)?,
                    device: 0,
                    inode: 0,
                },
                available: row.get::<_, i64>(5)? != 0,
                error_category: parse_error_category(row.get(6)?)?,
                error_message: row.get(7)?,
                capture: parse_capture_fact(
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                    row.get(12)?,
                )?,
            })
        })
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    let photos = connection
        .prepare(
            "SELECT p.id,p.original_id,p.available,p.preview_state,
                    p.preview_source_revision,p.preview_width,p.preview_height,p.cache_revision,
                    p.sort_path,p.selection_state,p.rating,
                    EXISTS(SELECT 1 FROM edit_recipes e WHERE e.photo_id=p.id),
                    p.removed_at_ms
             FROM photos p
             LEFT JOIN original_files o ON o.id=p.original_id
             ORDER BY CASE WHEN o.capture_order_key IS NULL THEN 1 ELSE 0 END,
                      o.capture_order_key COLLATE BINARY,
                      p.sort_path COLLATE BINARY,p.id",
        )
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], |row| {
            Ok(PhotoRecord {
                id: row.get(0)?,
                original_id: row.get(1)?,
                available: row.get::<_, i64>(2)? != 0,
                preview_state: parse_preview_state(&row.get::<_, String>(3)?)?,
                preview_source_revision: row.get(4)?,
                preview_width: parse_dimension(row.get(5)?)?,
                preview_height: parse_dimension(row.get(6)?)?,
                cache_revision: row.get(7)?,
                sort_path: row.get(8)?,
                selection_state: parse_selection_state(&row.get::<_, String>(9)?)?,
                rating: row
                    .get::<_, i64>(10)?
                    .try_into()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                has_saved_edits: row.get::<_, i64>(11)? != 0,
                removed: row.get::<_, Option<i64>>(12)?.is_some(),
            })
        })
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    let published = connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key='published_once'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?
        .is_some_and(|value| value == "1");
    Ok(ScanSnapshot {
        published,
        originals,
        photos,
        errors: Vec::new(),
    })
}

pub(super) fn parse_kind(value: &str) -> rusqlite::Result<OriginalKind> {
    match value {
        "raw" => Ok(OriginalKind::Raw),
        "jpeg" => Ok(OriginalKind::Jpeg),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_error_category(value: Option<String>) -> rusqlite::Result<Option<OriginalErrorCategory>> {
    value
        .map(|value| match value.as_str() {
            "unreadable" => Ok(OriginalErrorCategory::Unreadable),
            "changed" => Ok(OriginalErrorCategory::Changed),
            _ => Err(rusqlite::Error::InvalidQuery),
        })
        .transpose()
}

fn capture_state_name(state: CaptureMetadataState) -> &'static str {
    match state {
        CaptureMetadataState::Pending => "pending",
        CaptureMetadataState::Known => "known",
        CaptureMetadataState::Missing => "missing",
        CaptureMetadataState::Invalid => "invalid",
        CaptureMetadataState::Failed => "failed",
    }
}

pub(super) fn parse_capture_fact(
    state: String,
    order_key: Option<String>,
    field: Option<String>,
    offset_minutes: Option<i64>,
    source_revision: Option<String>,
) -> rusqlite::Result<CaptureFact> {
    let state = match state.as_str() {
        "pending" => CaptureMetadataState::Pending,
        "known" => CaptureMetadataState::Known,
        "missing" => CaptureMetadataState::Missing,
        "invalid" => CaptureMetadataState::Invalid,
        "failed" => CaptureMetadataState::Failed,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let field = match field.as_deref() {
        None => None,
        Some(value) => Some(
            CaptureTimeField::parse_database_name(value).ok_or(rusqlite::Error::InvalidQuery)?,
        ),
    };
    let offset_minutes = offset_minutes
        .map(|value| value.try_into().map_err(|_| rusqlite::Error::InvalidQuery))
        .transpose()?;
    let fact = CaptureFact {
        state,
        order_key,
        field,
        offset_minutes,
        source_revision,
    };
    validate_capture_fact(&fact).map_err(|_| rusqlite::Error::InvalidQuery)?;
    Ok(fact)
}

fn valid_capture_order_key(value: &str) -> bool {
    value.len() == 29
        && value.as_bytes()[4] == b'-'
        && value.as_bytes()[7] == b'-'
        && value.as_bytes()[10] == b'T'
        && value.as_bytes()[13] == b':'
        && value.as_bytes()[16] == b':'
        && value.as_bytes()[19] == b'.'
        && value.bytes().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7 | 10 | 13 | 16 | 19) || byte.is_ascii_digit()
        })
}

fn validate_capture_fact(fact: &CaptureFact) -> Result<(), ()> {
    let source_revision = fact
        .source_revision
        .as_deref()
        .is_some_and(|value| !value.is_empty());
    let known = fact
        .order_key
        .as_deref()
        .is_some_and(valid_capture_order_key)
        && fact.field.is_some()
        && source_revision;
    let no_derived =
        fact.order_key.is_none() && fact.field.is_none() && fact.offset_minutes.is_none();
    match fact.state {
        CaptureMetadataState::Pending => no_derived && fact.source_revision.is_none(),
        CaptureMetadataState::Known => known,
        CaptureMetadataState::Missing | CaptureMetadataState::Invalid => {
            no_derived && source_revision
        }
        CaptureMetadataState::Failed => no_derived,
    }
    .then_some(())
    .ok_or(())
}

pub(super) fn parse_preview_state(value: &str) -> rusqlite::Result<PreviewState> {
    match value {
        "inspection-pending" => Ok(PreviewState::InspectionPending),
        "ready" => Ok(PreviewState::Ready),
        "failed" => Ok(PreviewState::Failed),
        "unavailable" => Ok(PreviewState::Unavailable),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

pub(super) fn parse_selection_state(value: &str) -> rusqlite::Result<SelectionState> {
    match value {
        "undecided" => Ok(SelectionState::Undecided),
        "selected" => Ok(SelectionState::Selected),
        "rejected" => Ok(SelectionState::Rejected),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

pub(super) fn selection_state_value(value: SelectionState) -> &'static str {
    match value {
        SelectionState::Undecided => "undecided",
        SelectionState::Selected => "selected",
        SelectionState::Rejected => "rejected",
    }
}
pub(super) fn parse_dimension(value: Option<i64>) -> rusqlite::Result<Option<u32>> {
    value
        .map(|value| value.try_into().map_err(|_| rusqlite::Error::InvalidQuery))
        .transpose()
}

fn preview_state_name(state: PreviewState) -> &'static str {
    match state {
        PreviewState::InspectionPending => "inspection-pending",
        PreviewState::Ready => "ready",
        PreviewState::Failed => "failed",
        PreviewState::Unavailable => "unavailable",
    }
}

/// The recovery decisions proven outside the state store and applied inside
/// one scan transaction. Relocations map a discovered Location to the
/// persisted Original File identity whose exact content was found there.
/// Fingerprints are keyed by the discovered Location they were computed at;
/// the transaction resolves each to the Original that Location was assigned.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScanRecoveryPlan {
    pub relocations: HashMap<String, String>,
    pub fingerprints: Vec<DiscoveredFingerprint>,
}

/// One complete-content digest computed for the file observed at one
/// discovered Location during this scan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredFingerprint {
    pub path: String,
    pub digest: String,
}

/// The committed result of one scan application.
#[derive(Clone, Debug, PartialEq)]
pub struct ScanApplication {
    pub snapshot: ScanSnapshot,
    pub relocated_originals: usize,
    pub fingerprinted_originals: usize,
}

/// One enrollment work item: the persisted Original whose current observed
/// revision still needs a content fingerprint.
#[derive(Clone, Debug, PartialEq)]
pub struct FingerprintTarget {
    pub original_id: String,
    pub relative_path: String,
    pub kind: crate::OriginalKind,
    pub size: u64,
    pub mtime_ms: f64,
}

/// Truthful enrollment counters for status reporting.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FingerprintCounts {
    pub enrolled: usize,
    pub pending: usize,
}

fn retire_sidecar_association(
    transaction: &Transaction<'_>,
    photo_id: &str,
    retired_original_path: &str,
    kind: &str,
) -> Result<(), PersistenceError> {
    let association = transaction
        .query_row(
            "SELECT sidecar_path,observed_size,observed_mtime_ms,observed_digest,
                    (SELECT association_generation FROM photos WHERE id=?)
             FROM sidecar_associations WHERE photo_id=?",
            params![photo_id, photo_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, Option<f64>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, i64>(4)?,
                ))
            },
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    let Some((sidecar_path, observed_size, observed_mtime_ms, observed_digest, generation)) =
        association
    else {
        return Ok(());
    };
    transaction
        .execute(
            "INSERT OR REPLACE INTO retained_sidecar_orphans(
                sidecar_path,retired_photo_id,retired_original_path,original_kind,
                retired_generation,observed_size,observed_mtime_ms,observed_digest)
             VALUES(?,?,?,?,?,?,?,?)",
            params![
                sidecar_path,
                photo_id,
                retired_original_path,
                kind,
                generation,
                observed_size,
                observed_mtime_ms,
                observed_digest
            ],
        )
        .map_err(|_| PersistenceError::Storage)?;
    transaction
        .execute(
            "DELETE FROM sidecar_associations WHERE photo_id=?",
            [photo_id],
        )
        .map_err(|_| PersistenceError::Storage)?;
    Ok(())
}

pub(super) fn apply_scan(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    discovered: &[DiscoveredOriginal],
    errors: &[OriginalScanError],
    recovery: &ScanRecoveryPlan,
    failure_after_first: bool,
) -> Result<ScanApplication, PersistenceError> {
    let before = snapshot(connection)?;
    let previous_originals = before
        .originals
        .iter()
        .map(|original| (original.relative_path.as_str().to_owned(), original.clone()))
        .collect::<std::collections::HashMap<_, _>>();
    write_transaction(state, database_name, connection, |transaction| {
        // Validate every proposed relocation against the persisted state and
        // the complete discovered set before any write.
        let mut persisted_by_id = HashMap::with_capacity(before.originals.len());
        let mut persisted_by_path = HashMap::with_capacity(before.originals.len());
        for original in &before.originals {
            persisted_by_id.insert(original.id.clone(), original.clone());
            persisted_by_path.insert(
                original.relative_path.as_str().to_owned(),
                original.id.clone(),
            );
        }
        let mut discovered_by_path = HashMap::with_capacity(discovered.len());
        for original in discovered {
            discovered_by_path.insert(original.path.as_str().to_owned(), original);
        }
        // Identities whose Photo was confirmed permanently deleted. A scan may
        // neither relocate one (its bytes are gone by definition) nor let a
        // file that later appears at its reviewed Location adopt it: the
        // removed Photo must stay removed evidence, and the new file is a new
        // Original.
        let deleted_originals = permanently_deleted_original_ids(transaction)?
            .into_iter()
            .collect::<HashSet<_>>();
        let mut relocation_by_id = HashMap::with_capacity(recovery.relocations.len());
        for (new_path, original_id) in &recovery.relocations {
            if deleted_originals.contains(original_id) {
                return Err(PersistenceError::InvalidRecovery);
            }
            let Some(persisted) = persisted_by_id.get(original_id) else {
                return Err(PersistenceError::InvalidRecovery);
            };
            let Some(discovered_original) = discovered_by_path.get(new_path.as_str()) else {
                return Err(PersistenceError::InvalidRecovery);
            };
            if discovered_original.kind != persisted.kind {
                return Err(PersistenceError::InvalidRecovery);
            }
            if relocation_by_id
                .insert(original_id.clone(), new_path.clone())
                .is_some()
            {
                return Err(PersistenceError::InvalidRecovery);
            }
        }
        // The final Location assignment must stay injective: a relocated
        // Original may land on a Location vacated by another relocation, but
        // never on one still owned by a non-relocating Original.
        let mut final_locations = std::collections::BTreeSet::new();
        for original in &before.originals {
            let final_path = relocation_by_id
                .get(&original.id)
                .map_or_else(|| original.relative_path.as_str().to_owned(), Clone::clone);
            if !final_locations.insert(final_path) {
                return Err(PersistenceError::InvalidRecovery);
            }
        }

        for original_id in relocation_by_id.keys() {
            let persisted = persisted_by_id
                .get(original_id)
                .ok_or(PersistenceError::InvalidRecovery)?;
            transaction
                .execute(
                    "UPDATE photos SET association_generation=association_generation+1
                     WHERE original_id=?",
                    [original_id],
                )
                .map_err(|_| PersistenceError::Storage)?;
            let photo_id: String = transaction
                .query_row(
                    "SELECT id FROM photos WHERE original_id=?",
                    [original_id],
                    |row| row.get(0),
                )
                .map_err(|_| PersistenceError::Storage)?;
            let kind = match persisted.kind {
                crate::OriginalKind::Raw => "raw",
                crate::OriginalKind::Jpeg => "jpeg",
            };
            retire_sidecar_association(
                transaction,
                &photo_id,
                persisted.relative_path.as_str(),
                kind,
            )?;
        }
        // Move every relocated Original to a temporary unique Location first
        // so direct swaps cannot violate the UNIQUE(relative_path) constraint.
        for original_id in relocation_by_id.keys() {
            transaction
                .execute(
                    "UPDATE original_files SET relative_path=? WHERE id=?",
                    params![format!("\u{0}reloc/{original_id}"), original_id],
                )
                .map_err(|_| PersistenceError::Storage)?;
        }
        for (original_id, new_path) in &relocation_by_id {
            let changed = transaction
                .execute(
                    "UPDATE original_files SET relative_path=?,
                       capture_metadata_state='pending',capture_order_key=NULL,
                       capture_time_field=NULL,capture_offset_minutes=NULL,
                       capture_source_revision=NULL
                     WHERE id=?",
                    params![new_path, original_id],
                )
                .map_err(|_| PersistenceError::Storage)?;
            if changed != 1 {
                return Err(PersistenceError::InvalidRecovery);
            }
        }

        // A permanently deleted Original whose reviewed Location now holds
        // some file retires to a reserved Location that no scan recognizes, so
        // the Library path is free for the file that appears there while the
        // retained row keeps its facts as evidence.
        for original in &before.originals {
            if !deleted_originals.contains(&original.id)
                || !discovered_by_path.contains_key(original.relative_path.as_str())
            {
                continue;
            }
            transaction
                .execute(
                    "UPDATE photos SET association_generation=association_generation+1
                     WHERE original_id=?",
                    [original.id.as_str()],
                )
                .map_err(|_| PersistenceError::Storage)?;
            let photo_id: String = transaction
                .query_row(
                    "SELECT id FROM photos WHERE original_id=?",
                    [&original.id],
                    |row| row.get(0),
                )
                .map_err(|_| PersistenceError::Storage)?;
            let kind = match original.kind {
                crate::OriginalKind::Raw => "raw",
                crate::OriginalKind::Jpeg => "jpeg",
            };
            retire_sidecar_association(
                transaction,
                &photo_id,
                original.relative_path.as_str(),
                kind,
            )?;
            transaction
                .execute(
                    "UPDATE original_files SET relative_path=?,available=0 WHERE id=?",
                    params![
                        format!(
                            "{PERMANENT_DELETION_RETIRED_LOCATION_PREFIX}{}",
                            original.id
                        ),
                        original.id
                    ],
                )
                .map_err(|_| PersistenceError::Storage)?;
        }

        let existing_ids = persisted_by_path;
        let mut reserved_ids = HashSet::new();
        let mut original_ids = HashMap::with_capacity(discovered.len());
        for original in discovered {
            let id = if let Some(id) = recovery.relocations.get(original.path.as_str()) {
                id.clone()
            } else if let Some(id) = existing_ids
                .get(original.path.as_str())
                .filter(|id| !deleted_originals.contains(*id))
            {
                id.clone()
            } else {
                allocate_library_id(transaction, &mut reserved_ids)?
            };
            original_ids.insert(original.path.as_str().to_owned(), id);
        }
        let reconciled = reconcile(discovered, &before.photos, &original_ids, || {
            allocate_library_id(transaction, &mut reserved_ids)
        })?;
        transaction
            .execute("UPDATE original_files SET available=0", [])
            .map_err(|_| PersistenceError::Storage)?;
        transaction
            .execute("UPDATE photos SET available=0", [])
            .map_err(|_| PersistenceError::Storage)?;
        let mut upsert_original = transaction
            .prepare(
                "INSERT INTO original_files(
                    id,relative_path,kind,size,mtime_ms,available,error_category,error_message,
                    capture_metadata_state,capture_order_key,capture_time_field,
                    capture_offset_minutes,capture_source_revision)
                 VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)
                 ON CONFLICT(relative_path) DO UPDATE SET
                   kind=excluded.kind,size=excluded.size,mtime_ms=excluded.mtime_ms,
                   available=excluded.available,error_category=excluded.error_category,error_message=excluded.error_message,
                   capture_metadata_state=excluded.capture_metadata_state,
                   capture_order_key=excluded.capture_order_key,
                   capture_time_field=excluded.capture_time_field,
                   capture_offset_minutes=excluded.capture_offset_minutes,
                   capture_source_revision=excluded.capture_source_revision",
            )
            .map_err(|_| PersistenceError::Storage)?;
        for (index, original) in discovered.iter().enumerate() {
            validate_capture_fact(&original.capture).map_err(|_| PersistenceError::Storage)?;
            let id = original_ids
                .get(original.path.as_str())
                .expect("assigned Original identity");
            upsert_original
                .execute(params![
                    id,
                    original.path.as_str(),
                    match original.kind {
                        OriginalKind::Raw => "raw",
                        OriginalKind::Jpeg => "jpeg",
                    },
                    i64::try_from(original.facts.size).map_err(|_| PersistenceError::Storage)?,
                    original.facts.mtime_ms,
                    i64::from(original.error_category.is_none()),
                    original
                        .error_category
                        .as_ref()
                        .map(|category| match category {
                            OriginalErrorCategory::Unreadable => "unreadable",
                            OriginalErrorCategory::Changed => "changed",
                        }),
                    original.error_message.as_deref(),
                    capture_state_name(original.capture.state),
                    original.capture.order_key.as_deref(),
                    original.capture.field.map(CaptureTimeField::database_name),
                    original.capture.offset_minutes.map(i64::from),
                    original.capture.source_revision.as_deref(),
                ])
                .map_err(|_| PersistenceError::Storage)?;
            if failure_after_first && index == 0 {
                return Err(PersistenceError::Storage);
            }
        }
        let mut upsert_photo = transaction
            .prepare(
                "INSERT INTO photos(id,original_id,available,
                    preview_state,preview_source_revision,
                    preview_width,preview_height,cache_revision,sort_path)
                 VALUES(?,?,?,?,?,?,?,?,?)
                 ON CONFLICT(id) DO UPDATE SET
                    original_id=excluded.original_id,available=excluded.available,
                    preview_state=excluded.preview_state,
                    preview_source_revision=excluded.preview_source_revision,
                    preview_width=excluded.preview_width,preview_height=excluded.preview_height,
                    cache_revision=excluded.cache_revision,sort_path=excluded.sort_path",
            )
            .map_err(|_| PersistenceError::Storage)?;
        for photo in &reconciled {
            let selected = selected_source(photo);
            let selected_path = selected.map(|(original, _)| original.path.as_str().to_owned());
            let preserve = photo
                .prior
                .as_ref()
                .is_some_and(|prior| preview_should_preserve(prior, selected, &previous_originals));
            let source_revision = if preserve {
                photo
                    .prior
                    .as_ref()
                    .and_then(|prior| prior.preview_source_revision.clone())
            } else {
                None
            };
            let preview_state = if preserve {
                photo.prior.as_ref().unwrap().preview_state
            } else if selected.is_some() {
                PreviewState::InspectionPending
            } else {
                PreviewState::Unavailable
            };
            upsert_photo
                .execute(params![
                    photo.id,
                    photo.original_id,
                    i64::from(
                        photo
                            .original
                            .as_ref()
                            .is_some_and(|original| original.error_category.is_none())
                    ),
                    preview_state_name(preview_state),
                    source_revision,
                    preserve
                        .then(|| photo.prior.as_ref().unwrap().preview_width)
                        .flatten()
                        .map(i64::from),
                    preserve
                        .then(|| photo.prior.as_ref().unwrap().preview_height)
                        .flatten()
                        .map(i64::from),
                    preserve
                        .then(|| photo.prior.as_ref().unwrap().cache_revision.clone())
                        .flatten(),
                    if photo.sort_path.is_empty() {
                        selected_path.unwrap_or_default()
                    } else {
                        photo.sort_path.clone()
                    },
                ])
                .map_err(|_| PersistenceError::Storage)?;
        }
        // Fingerprints always describe the current persisted revision: drop
        // rows whose observed facts no longer match, then record every digest
        // freshly computed for this scan.
        transaction
            .execute(
                "DELETE FROM original_fingerprints WHERE original_id IN (
                     SELECT f.original_id FROM original_fingerprints f
                     JOIN original_files o ON o.id=f.original_id
                     WHERE f.size != o.size OR f.mtime_ms != o.mtime_ms)",
                [],
            )
            .map_err(|_| PersistenceError::Storage)?;
        let mut upsert_fingerprint = transaction
            .prepare(
                "INSERT INTO original_fingerprints(original_id,digest,size,mtime_ms)
                 SELECT o.id,?,o.size,o.mtime_ms FROM original_files o WHERE o.relative_path=?
                 ON CONFLICT(original_id) DO UPDATE SET
                   digest=excluded.digest,size=excluded.size,mtime_ms=excluded.mtime_ms",
            )
            .map_err(|_| PersistenceError::Storage)?;
        for fingerprint in &recovery.fingerprints {
            let changed = upsert_fingerprint
                .execute(params![fingerprint.digest, fingerprint.path])
                .map_err(|_| PersistenceError::Storage)?;
            if changed != 1 {
                return Err(PersistenceError::InvalidRecovery);
            }
        }
        transaction
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('published_once','1')
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                [],
            )
            .map_err(|_| PersistenceError::Storage)?;
        Ok(())
    })?;
    let mut result = snapshot(connection)?;
    result.errors = errors.to_vec();
    Ok(ScanApplication {
        snapshot: result,
        relocated_originals: recovery.relocations.len(),
        fingerprinted_originals: recovery.fingerprints.len(),
    })
}

pub(super) fn seed_preview(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    preview: PreviewSeed,
) -> Result<PreviewSeedResult, PersistenceError> {
    write_transaction(state, database_name, connection, |transaction| {
        // The compare-and-set anchor is the persisted Original's current
        // revision: any scan that changed the Original between inspection and
        // this seed makes the computed revision differ and the seed stale.
        let row = transaction
            .query_row(
                "SELECT o.relative_path,o.size,o.mtime_ms,o.kind,p.original_id
                 FROM photos p JOIN original_files o ON o.id=p.original_id
                 WHERE p.id=?",
                [&preview.photo_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, f64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(|_| PersistenceError::Storage)?;
        let Some((path, size, mtime_ms, kind, original_id)) = row else {
            return Ok(PreviewSeedResult::StaleIgnored);
        };
        let Ok(size) = u64::try_from(size) else {
            return Ok(PreviewSeedResult::StaleIgnored);
        };
        let parsed_kind = match kind.as_str() {
            "raw" => crate::OriginalKind::Raw,
            "jpeg" => crate::OriginalKind::Jpeg,
            _ => return Ok(PreviewSeedResult::StaleIgnored),
        };
        if parsed_kind.preview_source() != preview.source {
            return Ok(PreviewSeedResult::StaleIgnored);
        }
        let Some(current_revision) = crate::source_revision(&path, size, mtime_ms).ok() else {
            return Ok(PreviewSeedResult::StaleIgnored);
        };
        if current_revision != preview.expected_source_revision {
            return Ok(PreviewSeedResult::StaleIgnored);
        }
        let changed = transaction
            .execute(
                "UPDATE photos SET preview_state=?,preview_source_revision=?,
                        preview_width=?,preview_height=?,cache_revision=?
                 WHERE id=? AND original_id=?",
                params![
                    preview_state_name(preview.state),
                    current_revision,
                    preview.width.map(i64::from),
                    preview.height.map(i64::from),
                    preview.cache_revision,
                    preview.photo_id,
                    original_id,
                ],
            )
            .map_err(|_| PersistenceError::Storage)?;
        Ok(if changed == 1 {
            PreviewSeedResult::Applied
        } else {
            PreviewSeedResult::StaleIgnored
        })
    })
}
