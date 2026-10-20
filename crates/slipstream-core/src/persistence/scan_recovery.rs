//! Recovery facts, fingerprint enrollment, the manual relocation batch, and
//! the recovery plan a scan applies: the persisted state a scan recovers.

use super::codecs::{parse_kind, parse_selection_state};
use super::owner::{PersistenceError, write_transaction};
use super::{DatabaseName, StateDirectory};
use crate::identity::classify_name;
use crate::{
    AppliedRelocations, LibraryRoot, OriginalFingerprint, RecoveryRecord, RecoverySurvey,
    RelativeOriginalPath, RequestedRelocation,
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
                        (SELECT COUNT(*) FROM exports x WHERE x.photo_id=p.id),
                        EXISTS(SELECT 1 FROM development_proxies d WHERE d.photo_id=p.id)
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
                            row.get::<_, i64>(7)? != 0,
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
                has_proxy,
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
                || selection_state != "unflagged"
                || has_saved_edits
                || members != 0
                || exports != 0
                || has_proxy
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

pub(super) fn retire_sidecar_association(
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
