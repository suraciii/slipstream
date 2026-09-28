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
    LibraryRoot, OriginalErrorCategory, OriginalFacts, OriginalFingerprint, OriginalKind,
    OriginalRecord, OriginalScanError, PhotoRecord, PreviewSeed, PreviewSeedResult, PreviewState,
    RecoveryRecord, RecoverySurvey, RelativeOriginalPath, RequestedRelocation, ScanSnapshot,
    SelectionState, preview_should_preserve, reconcile, selected_source,
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

/// One consistent read of every unavailable Photo plus the Photos that keep
/// independent user state, for the bounded manual recovery review entry.
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
                Ok(RecoveryRecord {
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
                    available: false,
                    removed: false,
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
                   UNION ALL
                   SELECT photo_id FROM exports
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

/// Resolves current facts for one retained review membership. Every
/// requested Original ID gets its slot: `None` means the record no longer
/// exists. The order of `original_ids` is preserved so a reviewed result set
/// never shifts when the Library changes.
pub(super) fn recovery_records(
    connection: &Connection,
    original_ids: &[String],
) -> Result<Vec<Option<RecoveryRecord>>, PersistenceError> {
    let mut records = Vec::with_capacity(original_ids.len());
    let mut statement = connection
        .prepare(
            "SELECT o.relative_path,o.kind,p.id,p.rating,p.selection_state,f.digest,
                    (SELECT COUNT(*) FROM album_members m WHERE m.photo_id=p.id),
                    p.available,p.removed_at_ms
             FROM original_files o JOIN photos p ON p.original_id=o.id
             LEFT JOIN original_fingerprints f ON f.original_id=o.id
             WHERE o.id=?",
        )
        .map_err(|_| PersistenceError::Storage)?;
    for original_id in original_ids {
        let record = statement
            .query_row(params![original_id], |row| {
                Ok(RecoveryRecord {
                    original_id: original_id.clone(),
                    relative_path: row.get(0)?,
                    kind: parse_kind(&row.get::<_, String>(1)?)?,
                    photo_id: row.get(2)?,
                    rating: row
                        .get::<_, i64>(3)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    selection_state: parse_selection_state(&row.get::<_, String>(4)?)?,
                    fingerprint: row.get(5)?,
                    album_count: row
                        .get::<_, i64>(6)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    available: row.get::<_, i64>(7)? != 0,
                    removed: row.get::<_, Option<i64>>(8)?.is_some(),
                })
            })
            .optional()
            .map_err(|_| PersistenceError::Storage)?;
        records.push(record);
    }
    Ok(records)
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
    root: &LibraryRoot,
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
            // Filesystem evidence is checked again while the database
            // transaction is open. This closes the gap between proposal
            // evaluation and association: a replacement with identical
            // size/mtime cannot inherit a reviewed fingerprint.
            let capability = root.original(to.clone()).map_err(|_| {
                PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "unreadable",
                }
            })?;
            match relocation.fingerprint.as_deref() {
                Some(expected) => match capability.digest_file_if_present() {
                    Ok(Some(observed)) if observed.facts != relocation.facts => {
                        return Err(PersistenceError::InvalidRecoveryMapping {
                            original_id: relocation.original_id.clone(),
                            reason: "reviewed-stale",
                        });
                    }
                    Ok(Some(observed)) if observed.digest == expected => {}
                    Ok(Some(_)) => {
                        return Err(PersistenceError::InvalidRecoveryMapping {
                            original_id: relocation.original_id.clone(),
                            reason: "content-mismatch",
                        });
                    }
                    Ok(None) => {
                        return Err(PersistenceError::InvalidRecoveryMapping {
                            original_id: relocation.original_id.clone(),
                            reason: "missing",
                        });
                    }
                    Err(_) => {
                        return Err(PersistenceError::InvalidRecoveryMapping {
                            original_id: relocation.original_id.clone(),
                            reason: "unreadable",
                        });
                    }
                },
                None => match capability.facts_if_present() {
                    Ok(Some(observed)) if observed == relocation.facts => {}
                    Ok(Some(_)) => {
                        return Err(PersistenceError::InvalidRecoveryMapping {
                            original_id: relocation.original_id.clone(),
                            reason: "reviewed-stale",
                        });
                    }
                    Ok(None) => {
                        return Err(PersistenceError::InvalidRecoveryMapping {
                            original_id: relocation.original_id.clone(),
                            reason: "missing",
                        });
                    }
                    Err(_) => {
                        return Err(PersistenceError::InvalidRecoveryMapping {
                            original_id: relocation.original_id.clone(),
                            reason: "unreadable",
                        });
                    }
                },
            }
            // The transaction revalidates the reviewed correspondence: the
            // Original must still be an active unavailable Photo at the
            // remembered Location, and its destination must still be free or
            // occupied by exactly the Photo the confirmation named.
            let persisted = transaction
                .query_row(
                    "SELECT o.kind,o.relative_path,p.available,p.removed_at_ms
                     FROM original_files o JOIN photos p ON p.original_id=o.id
                     WHERE o.id=?",
                    params![relocation.original_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                            row.get::<_, Option<i64>>(3)?,
                        ))
                    },
                )
                .optional()
                .map_err(|_| PersistenceError::Storage)?;
            let Some((kind, remembered_path, available, removed_at_ms)) = persisted else {
                return Err(PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "stale",
                });
            };
            if relocation.mapping_id.is_empty() {
                return Err(PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "reviewed-stale",
                });
            }
            // A removed Photo keeps its own Trash Restore contract; Location
            // Recovery never revives it.
            if available != 0 || removed_at_ms.is_some() {
                return Err(PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "stale",
                });
            }
            if remembered_path != relocation.from_location {
                return Err(PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "reviewed-stale",
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
            // exactly the Photo the confirmation reviewed.
            let owner = transaction
                .query_row(
                    "SELECT id FROM original_files WHERE relative_path=?",
                    params![to.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|_| PersistenceError::Storage)?;
            let other_owner = owner.filter(|owner_id| owner_id != &relocation.original_id);
            let vacated = other_owner
                .as_ref()
                .is_some_and(|owner_id| relocating_ids.contains(owner_id));
            let Some(owner_id) = other_owner.filter(|_| !vacated) else {
                // A free or vacated destination has no retireable occupant,
                // so a confirmation that names one reviewed a different
                // correspondence.
                if relocation.retire_photo_id.is_some() {
                    return Err(PersistenceError::InvalidRecoveryMapping {
                        original_id: relocation.original_id.clone(),
                        reason: "reviewed-stale",
                    });
                }
                continue;
            };
            let Some(reviewed_photo_id) = relocation.retire_photo_id.as_deref() else {
                return Err(PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "occupied",
                });
            };
            let occupant = transaction
                .query_row(
                    "SELECT p.id,p.rating,p.selection_state,p.removed_at_ms,
                        EXISTS(SELECT 1 FROM edit_recipes e WHERE e.photo_id=p.id),
                        (SELECT COUNT(*) FROM album_members m WHERE m.photo_id=p.id),
                        (SELECT COUNT(*) FROM exports x WHERE x.photo_id=p.id)
                     FROM photos p WHERE p.original_id=?",
                    params![owner_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, i64>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<i64>>(3)?,
                            row.get::<_, i64>(4)? != 0,
                            row.get::<_, i64>(5)?,
                            row.get::<_, i64>(6)?,
                        ))
                    },
                )
                .optional()
                .map_err(|_| PersistenceError::Storage)?;
            let Some((
                photo_id,
                rating,
                selection_state,
                removed_at_ms,
                has_saved_edits,
                members,
                exports,
            )) = occupant
            else {
                return Err(PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "stale",
                });
            };
            // A confirmation names one destination Photo. A different Photo
            // that later occupies the same Location does not inherit it.
            if photo_id != reviewed_photo_id {
                return Err(PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "reviewed-stale",
                });
            }
            if removed_at_ms.is_some() {
                return Err(PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "destination-removed",
                });
            }
            if rating != 0
                || selection_state != "undecided"
                || has_saved_edits
                || members != 0
                || exports != 0
            {
                return Err(PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "destination-in-use",
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
            transaction
                .execute("DELETE FROM original_files WHERE id=?", params![owner_id])
                .map_err(|_| PersistenceError::Storage)?;
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
                       capture_source_revision=NULL,
                       camera_identity_state='pending',camera_make=NULL,camera_model=NULL
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
                    capture_offset_minutes,capture_source_revision,
                    camera_identity_state,camera_make,camera_model
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
                    parse_camera_identity(row.get(13)?, row.get(14)?, row.get(15)?)?,
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

fn camera_identity_name(identity: &crate::CameraIdentity) -> &'static str {
    match identity {
        crate::CameraIdentity::Pending => "pending",
        crate::CameraIdentity::Observed { .. } => "observed",
    }
}

pub(super) fn parse_camera_identity(
    state: String,
    make: Option<String>,
    model: Option<String>,
) -> rusqlite::Result<crate::CameraIdentity> {
    match state.as_str() {
        "pending" if make.is_none() && model.is_none() => Ok(crate::CameraIdentity::Pending),
        "observed" => Ok(crate::CameraIdentity::Observed { make, model }),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

pub(super) fn parse_capture_fact(
    state: String,
    order_key: Option<String>,
    field: Option<String>,
    offset_minutes: Option<i64>,
    source_revision: Option<String>,
    identity: crate::CameraIdentity,
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
        identity,
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
    // Only a completed inspection can carry a camera identity. Rows written
    // before the identity column existed keep `pending` identities with
    // their completed capture states until the next scan re-inspects and
    // publishes them.
    let identity_consistent = match fact.state {
        CaptureMetadataState::Pending | CaptureMetadataState::Failed => {
            !fact.identity.is_observed()
        }
        CaptureMetadataState::Known
        | CaptureMetadataState::Missing
        | CaptureMetadataState::Invalid => true,
    };
    match fact.state {
        CaptureMetadataState::Pending => no_derived && fact.source_revision.is_none(),
        CaptureMetadataState::Known => known,
        CaptureMetadataState::Missing | CaptureMetadataState::Invalid => {
            no_derived && source_revision
        }
        CaptureMetadataState::Failed => no_derived,
    }
    .then_some(())
    .ok_or(())?;
    identity_consistent.then_some(()).ok_or(())
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
    /// The source state each Original had when the scanner built the plan.
    /// Applying a stale plan must not overwrite a manual recovery committed
    /// while content inspection was in progress.
    pub relocation_sources: HashMap<String, ScanRelocationSource>,
    pub fingerprints: Vec<DiscoveredFingerprint>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScanRelocationSource {
    pub relative_path: String,
    pub facts: crate::OriginalFacts,
    pub available: bool,
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
    write_transaction(state, database_name, connection, |transaction| {
        let before = snapshot(transaction)?;
        let previous_originals = before
            .originals
            .iter()
            .map(|original| (original.relative_path.as_str().to_owned(), original.clone()))
            .collect::<std::collections::HashMap<_, _>>();
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
        // neither relocate them nor let a file that later appears at their
        // reviewed Location adopt them: the removed Photo must stay removed
        // evidence, and the new file is a new Original.
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
            let Some(source) = recovery.relocation_sources.get(original_id) else {
                return Err(PersistenceError::InvalidRecovery);
            };
            if persisted.relative_path.as_str() != source.relative_path
                || persisted.facts != source.facts
                || persisted.available != source.available
            {
                return Err(PersistenceError::InvalidRecovery);
            }
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
                       capture_source_revision=NULL,
                       camera_identity_state='pending',camera_make=NULL,camera_model=NULL
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
                        original.id.as_str(),
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
                    capture_offset_minutes,capture_source_revision,
                    camera_identity_state,camera_make,camera_model)
                 VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
                 ON CONFLICT(relative_path) DO UPDATE SET
                   kind=excluded.kind,size=excluded.size,mtime_ms=excluded.mtime_ms,
                   available=excluded.available,error_category=excluded.error_category,error_message=excluded.error_message,
                   capture_metadata_state=excluded.capture_metadata_state,
                   capture_order_key=excluded.capture_order_key,
                   capture_time_field=excluded.capture_time_field,
                   capture_offset_minutes=excluded.capture_offset_minutes,
                   capture_source_revision=excluded.capture_source_revision,
                   camera_identity_state=excluded.camera_identity_state,
                   camera_make=excluded.camera_make,
                   camera_model=excluded.camera_model",
            ).map_err(|_| PersistenceError::Storage)?;
        for (index, original) in discovered.iter().enumerate() {
            let published_prior = original
                .error_category
                .is_some()
                .then(|| previous_originals.get(original.path.as_str()))
                .flatten()
                .filter(|prior| prior.kind == original.kind);
            let facts = published_prior.map_or(original.facts, |prior| prior.facts);
            let (identity_make, identity_model) = match &original.capture.identity {
                crate::CameraIdentity::Observed { make, model } => {
                    (make.as_deref(), model.as_deref())
                }
                crate::CameraIdentity::Pending => (None, None),
            };
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
                    i64::try_from(facts.size).map_err(|_| PersistenceError::Storage)?,
                    facts.mtime_ms,
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
                    camera_identity_name(&original.capture.identity),
                    identity_make,
                    identity_model,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::removal::{PermanentDeletionStoredItem, reviewed_facts};
    use crate::persistence::test_support::*;
    use crate::persistence::{Persistence, PersistenceError};
    use crate::{
        OriginalFacts, OriginalKind, PreviewSeed, PreviewSeedResult, PreviewState, source_revision,
    };
    use std::fs;

    /// The retained review is read back from its own JSON row, so the facts it
    /// compares against the filesystem must survive that round trip exactly.
    /// A milliseconds mtime needs 17 significant digits for some files, and a
    /// decimal parse of those returns a neighboring f64, so the row keeps the
    /// bits.
    #[test]
    fn reviewed_facts_survive_the_retained_review_round_trip() {
        let reviewed = OriginalFacts {
            size: 629,
            mtime_ms: 1_790_379_911_790.761_5,
            device: 64_513,
            inode: 21_758_498,
        };
        let stored = PermanentDeletionStoredItem {
            photo_id: "photo".to_owned(),
            removed_at_ms: 1_790_379_911_955,
            original_id: "original".to_owned(),
            relative_path: "b.jpg".to_owned(),
            kind: "jpeg".to_owned(),
            size: reviewed.size,
            mtime_bits: reviewed.mtime_ms.to_bits(),
            device: reviewed.device,
            inode: reviewed.inode,
            albums: Vec::new(),
        };
        let row = serde_json::to_string(&stored).unwrap();
        let read: PermanentDeletionStoredItem = serde_json::from_str(&row).unwrap();
        assert_eq!(
            reviewed_facts(read.size, read.mtime_bits, read.device, read.inode),
            reviewed
        );
    }

    #[tokio::test]
    async fn scan_relocation_retires_sidecar_association() {
        let (_base, root, state, name, path) = fixture();
        fs::write(root.canonical_path().join("one.JPG"), b"one").unwrap();
        let library = crate::Library::open(sidecar_config(&root, &state, &name)).unwrap();
        let snapshot = library.scan().await.unwrap();
        let photo = &snapshot.photos[0].id;
        seed_sidecar(&path, photo, "dir/photo.xmp");
        let before = association_generation(&path, photo);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while library.fingerprint_counts().enrolled != 1 {
            assert!(
                std::time::Instant::now() < deadline,
                "fingerprint enrollment timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        library.shutdown().unwrap();
        fs::rename(
            root.canonical_path().join("one.JPG"),
            root.canonical_path().join("moved.JPG"),
        )
        .unwrap();
        let library = crate::Library::open(sidecar_config(&root, &state, &name)).unwrap();
        let relocated = library.scan().await.unwrap();
        assert_eq!(relocated.photos[0].id, *photo);
        assert_eq!(relocated.originals[0].relative_path.as_str(), "moved.JPG");
        let after = association_generation(&path, photo);
        assert!(after > before);
        assert_retired(&path, photo, "one.JPG", "dir/photo.xmp", after);
        library.shutdown().unwrap();
    }

    #[tokio::test]
    async fn applies_scans_transactionally_and_preserves_unavailable_pair_identity() {
        let (_base, library, state, name, _path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let raw = discovered("one.ARW", OriginalKind::Raw, 3, 1000.0);
        let jpeg = discovered("one.JPG", OriginalKind::Jpeg, 4, 1000.0);
        let first = persistence
            .apply_scan(vec![raw.clone(), jpeg.clone()], Vec::new())
            .await
            .unwrap();
        assert_eq!(first.originals.len(), 2);
        assert_eq!(first.photos.len(), 2);
        let raw_photo = first
            .photos
            .iter()
            .find(|photo| {
                first.originals.iter().any(|original| {
                    original.id == photo.original_id && original.kind == OriginalKind::Raw
                })
            })
            .unwrap();
        let jpeg_photo = first
            .photos
            .iter()
            .find(|photo| photo.id != raw_photo.id)
            .unwrap();
        assert!(raw_photo.available);
        assert!(jpeg_photo.available);
        assert_eq!(raw_photo.preview_state, PreviewState::InspectionPending);

        let unavailable = persistence
            .apply_scan(Vec::new(), Vec::new())
            .await
            .unwrap();
        let missing = unavailable
            .photos
            .iter()
            .find(|photo| photo.id == raw_photo.id)
            .unwrap();
        assert_eq!(missing.id, raw_photo.id);
        assert_eq!(missing.original_id, raw_photo.original_id);
        assert!(!missing.available);
        assert_eq!(missing.preview_state, PreviewState::Unavailable);

        let restored = persistence.apply_scan(vec![raw], Vec::new()).await.unwrap();
        let restored_photo = restored
            .photos
            .iter()
            .find(|photo| photo.id == raw_photo.id)
            .unwrap();
        assert_eq!(restored_photo.id, raw_photo.id);
        assert!(restored_photo.available);
        assert_eq!(restored_photo.original_id, raw_photo.original_id);
        assert_eq!(
            restored_photo.preview_state,
            PreviewState::InspectionPending
        );
        persistence.shutdown().unwrap();
    }

    /// An Original that discovery cannot inspect keeps the source facts of the
    /// current publication. Replacing them with the unreadable discovery facts
    /// would break the revision binding the published capture fact and the
    /// recipe guard depend on.
    #[tokio::test]
    async fn an_unreadable_discovery_keeps_the_published_source_facts() {
        let (_base, library, state, name, _path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let published = discovered("one.ARW", OriginalKind::Raw, 3, 1000.0);
        let first = persistence
            .apply_scan(vec![published.clone()], Vec::new())
            .await
            .unwrap();
        let stored = first.originals[0].facts;
        assert_eq!(
            (stored.size, stored.mtime_ms),
            (published.facts.size, published.facts.mtime_ms)
        );
        assert!(first.originals[0].available);

        let mut unreadable = discovered("one.ARW", OriginalKind::Raw, 0, 0.0);
        unreadable.facts = OriginalFacts::UNREADABLE;
        unreadable.error_category = Some(crate::OriginalErrorCategory::Unreadable);
        unreadable.error_message = Some("Original File could not be inspected".to_owned());
        let second = persistence
            .apply_scan(vec![unreadable], Vec::new())
            .await
            .unwrap();
        let kept = &second.originals[0];
        assert_eq!(
            (kept.facts.size, kept.facts.mtime_ms),
            (stored.size, stored.mtime_ms)
        );
        assert_eq!(kept.capture, published.capture);
        assert!(!kept.available);
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn relocation_resets_preview_and_moves_identity_in_one_transaction() {
        let (_base, library, state, name, _path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let first = persistence
            .apply_scan(
                vec![discovered("one.JPG", OriginalKind::Jpeg, 4, 1000.0)],
                Vec::new(),
            )
            .await
            .unwrap();
        let photo = &first.photos[0];
        let original_id = photo.original_id.clone();
        let photo_id = photo.id.clone();
        let revision = source_revision("one.JPG", 4, 1000.0).unwrap();
        assert_eq!(
            persistence
                .seed_preview(PreviewSeed {
                    photo_id: photo_id.clone(),
                    state: PreviewState::Ready,
                    source: crate::PreviewSource::JpegOriginal,
                    expected_source_revision: revision,
                    width: Some(100),
                    height: Some(50),
                    cache_revision: Some("cache-v1".to_owned()),
                })
                .await
                .unwrap(),
            PreviewSeedResult::Applied
        );

        // The same content re-discovered at a new Location with a proven
        // relocation keeps the Photo identity, resets Preview inspection, and
        // records the fresh fingerprint bound to the new Location.
        let digest = crate::recovery::digest_bytes(b"payload");
        let _ = digest;
        let recovery = ScanRecoveryPlan {
            relocations: [("moved/two.JPG".to_owned(), original_id.clone())].into(),
            relocation_sources: [(
                original_id.clone(),
                ScanRelocationSource {
                    relative_path: "one.JPG".to_owned(),
                    facts: OriginalFacts {
                        size: 4,
                        mtime_ms: 1000.0,
                        device: 0,
                        inode: 0,
                    },
                    available: true,
                },
            )]
            .into(),
            fingerprints: vec![DiscoveredFingerprint {
                path: "moved/two.JPG".to_owned(),
                digest: crate::recovery::digest_bytes(&[]),
            }],
        };
        let relocated = persistence
            .apply_scan_recovered(
                vec![discovered("moved/two.JPG", OriginalKind::Jpeg, 4, 1000.0)],
                Vec::new(),
                recovery,
            )
            .await
            .unwrap();
        assert_eq!(relocated.snapshot.photos.len(), 1);
        assert_eq!(relocated.snapshot.photos[0].id, photo_id);
        assert_eq!(relocated.relocated_originals, 1);
        assert_eq!(
            relocated.snapshot.originals[0].relative_path.as_str(),
            "moved/two.JPG"
        );
        assert_eq!(
            relocated.snapshot.photos[0].preview_state,
            PreviewState::InspectionPending
        );
        assert!(
            persistence
                .apply_scan_recovered(
                    vec![discovered("moved/two.JPG", OriginalKind::Jpeg, 4, 1000.0)],
                    Vec::new(),
                    ScanRecoveryPlan {
                        relocations: [("moved/two.JPG".to_owned(), original_id.clone())].into(),
                        relocation_sources: [(
                            original_id.clone(),
                            ScanRelocationSource {
                                relative_path: "one.JPG".to_owned(),
                                facts: OriginalFacts {
                                    size: 4,
                                    mtime_ms: 1000.0,
                                    device: 0,
                                    inode: 0,
                                },
                                available: true,
                            },
                        )]
                        .into(),
                        fingerprints: vec![DiscoveredFingerprint {
                            path: "moved/two.JPG".to_owned(),
                            digest: crate::recovery::digest_bytes(&[]),
                        }],
                    },
                )
                .await
                .is_err()
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn rolls_back_scan_and_keeps_the_prior_snapshot_without_partial_rows() {
        let (_base, library, state, name, _path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let initial = persistence
            .apply_scan(
                vec![discovered("one.JPG", OriginalKind::Jpeg, 4, 1000.0)],
                Vec::new(),
            )
            .await
            .unwrap();
        let failed = persistence
            .apply_scan_failure(
                vec![
                    discovered("new.JPG", OriginalKind::Jpeg, 5, 1001.0),
                    discovered("second.JPG", OriginalKind::Jpeg, 6, 1002.0),
                ],
                Vec::new(),
            )
            .await;
        assert!(matches!(failed, Err(PersistenceError::Storage)));
        let after = persistence.snapshot().await.unwrap();
        assert_eq!(after, initial);
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn stale_fallback_preview_completion_is_ignored_after_candidate_change() {
        let (_base, library, state, name, _path) = fixture();
        let raw = discovered("one.ARW", OriginalKind::Raw, 3, 1000.0);
        let first = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let initial = first
            .apply_scan(vec![raw.clone()], Vec::new())
            .await
            .unwrap();
        let photo_id = initial.photos[0].id.clone();
        let raw_revision = source_revision("one.ARW", 3, 1000.0).unwrap();
        assert_eq!(
            first
                .seed_preview(PreviewSeed {
                    photo_id: photo_id.clone(),
                    state: PreviewState::Ready,
                    source: crate::PreviewSource::RawEmbeddedJpeg,
                    expected_source_revision: raw_revision.clone(),
                    width: Some(512),
                    height: Some(341),
                    cache_revision: Some("raw-cache".to_owned()),
                })
                .await
                .unwrap(),
            PreviewSeedResult::Applied
        );
        // An unchanged rescan keeps the seeded preview facts bound to the
        // unchanged original.
        let unchanged = first
            .apply_scan(
                vec![discovered("one.ARW", OriginalKind::Raw, 3, 1000.0)],
                Vec::new(),
            )
            .await
            .unwrap();
        assert_eq!(unchanged.photos[0].preview_state, PreviewState::Ready);
        assert_eq!(
            unchanged.photos[0].cache_revision.as_deref(),
            Some("raw-cache")
        );
        // A second scan with changed RAW facts makes the old revision stale.
        let changed = discovered("one.ARW", OriginalKind::Raw, 7, 1002.0);
        let updated = first.apply_scan(vec![changed], Vec::new()).await.unwrap();
        let updated_photo = updated
            .photos
            .iter()
            .find(|photo| photo.id == photo_id)
            .unwrap();
        assert_eq!(updated_photo.preview_state, PreviewState::InspectionPending);
        // Seeding with the superseded revision must lose the compare-and-swap
        // on the RAW revision itself, not merely a source-kind guard.
        assert_eq!(
            first
                .seed_preview(PreviewSeed {
                    photo_id,
                    state: PreviewState::Ready,
                    source: crate::PreviewSource::RawEmbeddedJpeg,
                    expected_source_revision: raw_revision,
                    width: Some(512),
                    height: Some(341),
                    cache_revision: Some("stale".to_owned()),
                })
                .await
                .unwrap(),
            PreviewSeedResult::StaleIgnored
        );
        first.shutdown().unwrap();
    }
}
