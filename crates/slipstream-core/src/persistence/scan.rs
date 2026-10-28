//! Scan application, snapshot decoding, and Preview seeding for the persisted
//! state a scan publishes.

use super::codecs::{
    camera_identity_name, capture_state_name, parse_camera_identity, parse_capture_fact,
    parse_dimension, parse_error_category, parse_kind, parse_preview_state, parse_selection_state,
    preview_state_name, validate_capture_fact,
};
use super::owner::{
    PERMANENT_DELETION_RETIRED_LOCATION_PREFIX, PersistenceError, allocate_library_id,
    permanently_deleted_original_ids, write_transaction,
};
use super::scan_recovery::{ScanRecoveryPlan, retire_sidecar_association};
use super::{DatabaseName, StateDirectory};
use crate::{
    CaptureTimeField, DiscoveredOriginal, OriginalErrorCategory, OriginalFacts, OriginalKind,
    OriginalRecord, OriginalScanError, PhotoRecord, PreviewSeed, PreviewSeedResult, PreviewState,
    ScanSnapshot, preview_should_preserve, reconcile, selected_source,
};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::{HashMap, HashSet};

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
    let published = super::metadata_store::read_value(connection, "published_once", |_| {
        PersistenceError::Storage
    })?
    .is_some_and(|value| value == "1");
    Ok(ScanSnapshot {
        published,
        originals,
        photos,
        errors: Vec::new(),
    })
}

/// The committed result of one scan application.
#[derive(Clone, Debug, PartialEq)]
pub struct ScanApplication {
    pub snapshot: ScanSnapshot,
    pub relocated_originals: usize,
    pub fingerprinted_originals: usize,
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
        super::metadata_store::upsert(transaction, "published_once", "1", |_| {
            PersistenceError::Storage
        })?;
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
    use super::super::scan_recovery::{
        DiscoveredFingerprint, ScanRecoveryPlan, ScanRelocationSource,
    };
    use super::*;
    use crate::persistence::test_support::*;
    use crate::persistence::{Persistence, PersistenceError};
    use crate::{
        OriginalFacts, OriginalKind, PreviewSeed, PreviewSeedResult, PreviewState, source_revision,
    };
    use std::fs;

    #[tokio::test]
    async fn scan_relocation_retires_sidecar_association() {
        let (_base, root, state, name, path) = fixture();
        fs::write(root.canonical_path().join("one.JPG"), b"one").unwrap();
        let library = crate::Library::open(sidecar_config(&root, &state, &name)).unwrap();
        let snapshot = library.scan().await.unwrap();
        let photo = &snapshot.photos[0].id;
        seed_sidecar(&path, photo, "dir/photo.xmp");
        let before = association_generation(&path, photo);
        library.shutdown().unwrap();
        Connection::open(&path)
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO original_fingerprints(original_id,digest,size,mtime_ms) VALUES(?,?,?,?)",
                params![
                    snapshot.originals[0].id,
                    crate::digest_bytes(b"one"),
                    i64::try_from(snapshot.originals[0].facts.size).unwrap(),
                    snapshot.originals[0].facts.mtime_ms,
                ],
            )
            .unwrap();
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
