//! Metadata sidecar records and the bounded work admission they describe:
//! what the owner hands to the metadata supervisor and what it reads back.

use rusqlite::{Connection, OptionalExtension, params};

/// Metadata work runs on the state owner, excluding lifecycle mutations.
#[derive(Clone, Debug, PartialEq)]
pub struct MetadataRecord {
    pub photo_id: String,
    pub original_id: String,
    pub relative_path: String,
    pub kind: &'static str,
    pub association_generation: u64,
    pub removed: bool,
    pub library_rating: u8,
    pub active: Option<ActiveAssociation>,
    pub orphan: Option<RetainedOrphan>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ActiveAssociation {
    pub sidecar_path: String,
    pub observed_size: Option<u64>,
    pub observed_mtime_ms: Option<f64>,
    pub observed_digest: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RetainedOrphan {
    pub sidecar_path: String,
    pub retired_photo_id: String,
    pub retired_original_path: String,
    pub original_kind: String,
    pub retired_generation: u64,
    pub observed_size: Option<u64>,
    pub observed_mtime_ms: Option<f64>,
    pub observed_digest: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetadataStoreError {
    PhotoMissing,
    PhotoRemoved,
    OriginalUnavailable,
    Storage,
}

#[derive(Clone, Debug)]
pub struct ObservedSidecar {
    pub state: ObservedSidecarState,
}

#[derive(Clone, Debug)]
pub enum ObservedSidecarState {
    Eligible {
        path: String,
        size: u64,
        mtime_ms: f64,
        digest: String,
    },
    Absent,
    Changed,
}

pub struct MetadataContext<'a> {
    connection: &'a Connection,
    record: MetadataRecord,
    candidate_path: String,
}

impl MetadataContext<'_> {
    pub fn record(&self) -> &MetadataRecord {
        &self.record
    }

    pub fn record_observation(
        &self,
        observation: &ObservedSidecar,
    ) -> Result<u64, MetadataStoreError> {
        let transaction = self
            .connection
            .unchecked_transaction()
            .map_err(|_| MetadataStoreError::Storage)?;
        match &observation.state {
            ObservedSidecarState::Eligible {
                path,
                size,
                mtime_ms,
                digest,
            } => {
                let owner: Option<String> = transaction
                    .query_row(
                        "SELECT photo_id FROM sidecar_associations WHERE sidecar_path=?",
                        [path],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(|_| MetadataStoreError::Storage)?;
                if let Some(owner) = owner.filter(|owner| owner != &self.record.photo_id) {
                    // Sidecar ownership follows the eligible Original: a RAW
                    // claim displaces a standing JPEG owner, and an owner whose
                    // Original is no longer available cannot write anyway.
                    // Every other standing owner is still the unambiguous
                    // writer, so the claim stays a conflict. The displaced
                    // owner's generation moves so its held evidence fails the
                    // next check.
                    let (owner_kind, owner_available): (String, bool) = transaction
                        .query_row(
                            "SELECT o.kind,o.available FROM photos p
                             JOIN original_files o ON o.id=p.original_id WHERE p.id=?",
                            [&owner],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        )
                        .optional()
                        .map_err(|_| MetadataStoreError::Storage)?
                        .ok_or(MetadataStoreError::Storage)?;
                    let displaces =
                        (self.record.kind == "raw" && owner_kind == "jpeg") || !owner_available;
                    if !displaces {
                        return Err(MetadataStoreError::Storage);
                    }
                    transaction
                        .execute(
                            "UPDATE photos SET association_generation=association_generation+1
                             WHERE id=?",
                            [&owner],
                        )
                        .map_err(|_| MetadataStoreError::Storage)?;
                    transaction
                        .execute(
                            "DELETE FROM sidecar_associations WHERE photo_id=?",
                            [&owner],
                        )
                        .map_err(|_| MetadataStoreError::Storage)?;
                }
                let size = i64::try_from(*size).map_err(|_| MetadataStoreError::Storage)?;
                transaction.execute(
                    "INSERT INTO sidecar_associations(photo_id,sidecar_path,observed_size,observed_mtime_ms,observed_digest)
                     VALUES(?,?,?,?,?) ON CONFLICT(photo_id) DO UPDATE SET sidecar_path=excluded.sidecar_path,
                     observed_size=excluded.observed_size,observed_mtime_ms=excluded.observed_mtime_ms,observed_digest=excluded.observed_digest",
                    params![self.record.photo_id, path, size, mtime_ms, digest],
                ).map_err(|_| MetadataStoreError::Storage)?;
                transaction.execute(
                    "DELETE FROM retained_sidecar_orphans WHERE sidecar_path=? AND
                     (observed_size IS NOT ? OR observed_mtime_ms IS NOT ? OR observed_digest IS NOT ?)",
                    params![path, size, mtime_ms, digest],
                ).map_err(|_| MetadataStoreError::Storage)?;
            }
            ObservedSidecarState::Absent => {
                transaction
                    .execute(
                        "DELETE FROM sidecar_associations WHERE photo_id=?",
                        [&self.record.photo_id],
                    )
                    .map_err(|_| MetadataStoreError::Storage)?;
            }
            ObservedSidecarState::Changed => {
                transaction
                    .execute(
                        "DELETE FROM retained_sidecar_orphans WHERE
                         substr(sidecar_path,1,length(sidecar_path)-4)=substr(?1,1,length(?1)-4)
                         AND lower(substr(sidecar_path,-4))='.xmp'",
                        [&self.candidate_path],
                    )
                    .map_err(|_| MetadataStoreError::Storage)?;
                // The Sidecar at this stem no longer reads as recorded, so no
                // active association at the stem keeps standing evidence: each
                // displaced owner's generation moves and its claim is dropped.
                transaction
                    .execute(
                        "UPDATE photos SET association_generation=association_generation+1
                         WHERE id IN (
                             SELECT photo_id FROM sidecar_associations WHERE
                             substr(sidecar_path,1,length(sidecar_path)-4)=substr(?1,1,length(?1)-4)
                             AND lower(substr(sidecar_path,-4))='.xmp')",
                        [&self.candidate_path],
                    )
                    .map_err(|_| MetadataStoreError::Storage)?;
                transaction
                    .execute(
                        "DELETE FROM sidecar_associations WHERE
                         substr(sidecar_path,1,length(sidecar_path)-4)=substr(?1,1,length(?1)-4)
                         AND lower(substr(sidecar_path,-4))='.xmp'",
                        [&self.candidate_path],
                    )
                    .map_err(|_| MetadataStoreError::Storage)?;
            }
        }
        let generation: i64 = transaction
            .query_row(
                "SELECT association_generation FROM photos WHERE id=?",
                [&self.record.photo_id],
                |row| row.get(0),
            )
            .map_err(|_| MetadataStoreError::Storage)?;
        transaction
            .commit()
            .map_err(|_| MetadataStoreError::Storage)?;
        Ok(generation as u64)
    }
}

pub(super) fn metadata_context<'a>(
    connection: &'a Connection,
    photo_id: &str,
) -> Result<MetadataContext<'a>, MetadataStoreError> {
    let row = connection.query_row(
        "SELECT p.original_id,o.relative_path,o.kind,p.association_generation,p.removed_at_ms,o.available,p.rating
         FROM photos p JOIN original_files o ON o.id=p.original_id WHERE p.id=?", [photo_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?,
                  row.get::<_, i64>(3)?, row.get::<_, Option<i64>>(4)?, row.get::<_, bool>(5)?, row.get::<_, u8>(6)?)),
    ).optional().map_err(|_| MetadataStoreError::Storage)?.ok_or(MetadataStoreError::PhotoMissing)?;
    if !row.5 {
        return Err(MetadataStoreError::OriginalUnavailable);
    }
    let kind = match row.2.as_str() {
        "raw" => "raw",
        "jpeg" => "jpeg",
        _ => return Err(MetadataStoreError::Storage),
    };
    let candidate_path = format!(
        "{}.xmp",
        row.1
            .rsplit_once('.')
            .map_or(row.1.as_str(), |(stem, _)| stem)
    );
    let active = connection.query_row(
        "SELECT sidecar_path,observed_size,observed_mtime_ms,observed_digest FROM sidecar_associations WHERE photo_id=?", [photo_id],
        |row| Ok(ActiveAssociation { sidecar_path: row.get(0)?, observed_size: row.get::<_, Option<i64>>(1)?.map(|size| size as u64), observed_mtime_ms: row.get(2)?, observed_digest: row.get(3)? }),
    ).optional().map_err(|_| MetadataStoreError::Storage)?;
    let orphan = connection.query_row(
        "SELECT sidecar_path,retired_photo_id,retired_original_path,original_kind,retired_generation,observed_size,observed_mtime_ms,observed_digest
         FROM retained_sidecar_orphans WHERE
         substr(sidecar_path,1,length(sidecar_path)-4)=substr(?1,1,length(?1)-4)
         AND lower(substr(sidecar_path,-4))='.xmp' ORDER BY sidecar_path LIMIT 1", [&candidate_path],
        |row| Ok(RetainedOrphan { sidecar_path: row.get(0)?, retired_photo_id: row.get(1)?, retired_original_path: row.get(2)?, original_kind: row.get(3)?,
            retired_generation: row.get::<_, i64>(4)? as u64, observed_size: row.get::<_, Option<i64>>(5)?.map(|size| size as u64), observed_mtime_ms: row.get(6)?, observed_digest: row.get(7)? }),
    ).optional().map_err(|_| MetadataStoreError::Storage)?;
    Ok(MetadataContext {
        connection,
        candidate_path,
        record: MetadataRecord {
            photo_id: photo_id.to_owned(),
            original_id: row.0,
            relative_path: row.1,
            kind,
            association_generation: row.3 as u64,
            removed: row.4.is_some(),
            library_rating: row.6,
            active,
            orphan,
        },
    })
}

#[cfg(test)]
mod tests {
    use crate::PhotoRemovalMutation;
    use crate::persistence::Persistence;
    use crate::persistence::metadata;
    use crate::persistence::test_support::*;
    use rusqlite::Connection;
    use std::{fs, path::PathBuf};
    use tokio::sync::oneshot;

    fn metadata_fixture() -> (TempTree, Persistence, PathBuf) {
        let (base, root, state, name, path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            root.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        seed(&path, "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,capture_metadata_state)
             VALUES('original','dir/photo.JPG','jpeg',1,1,1,'pending'),('other-original','other.JPG','jpeg',1,1,1,'pending');
             INSERT INTO photos(id,original_id,available,preview_state,sort_path,selection_state,rating)
             VALUES('photo','original',1,'inspection-pending','dir/photo.JPG','undecided',0),
             ('other','other-original',1,'inspection-pending','other.JPG','undecided',0);");
        (base, persistence, path)
    }

    fn metadata_observation(size: u64) -> metadata::ObservedSidecar {
        metadata::ObservedSidecar {
            state: metadata::ObservedSidecarState::Eligible {
                path: "dir/photo.xmp".to_owned(),
                size,
                mtime_ms: 1234.5,
                digest: "a".repeat(64),
            },
        }
    }

    #[tokio::test]
    async fn with_metadata_reads_removed_but_rejects_missing_and_unavailable() {
        let (_base, persistence, path) = metadata_fixture();
        assert_eq!(
            persistence
                .with_metadata_receiver("missing".into(), |_| Ok(()))
                .unwrap()
                .await
                .unwrap(),
            Err(metadata::MetadataStoreError::PhotoMissing)
        );
        seed(
            &path,
            "UPDATE photos SET removed_at_ms=1,removed_operation='remove',rating=4 WHERE id='photo';",
        );
        let removed = persistence
            .with_metadata_receiver("photo".into(), |context| Ok(context.record().clone()))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(removed.removed);
        assert_eq!(removed.library_rating, 4);
        seed(
            &path,
            "UPDATE original_files SET available=0 WHERE id='original';",
        );
        assert_eq!(
            persistence
                .with_metadata_receiver("photo".into(), |_| Ok(()))
                .unwrap()
                .await
                .unwrap(),
            Err(metadata::MetadataStoreError::OriginalUnavailable)
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn with_metadata_observation_round_trips_and_protects_foreign_owner() {
        let (_base, persistence, path) = metadata_fixture();
        persistence
            .with_metadata_receiver("photo".into(), |context| {
                context.record_observation(&metadata_observation(7))
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let record = persistence
            .with_metadata_receiver("photo".into(), |context| Ok(context.record().clone()))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.kind, "jpeg");
        assert_eq!(record.original_id, "original");
        assert_eq!(record.relative_path, "dir/photo.JPG");
        assert_eq!(
            record.active,
            Some(metadata::ActiveAssociation {
                sidecar_path: "dir/photo.xmp".into(),
                observed_size: Some(7),
                observed_mtime_ms: Some(1234.5),
                observed_digest: Some("a".repeat(64))
            })
        );
        assert_eq!(
            persistence
                .with_metadata_receiver("other".into(), |context| context
                    .record_observation(&metadata_observation(8)))
                .unwrap()
                .await
                .unwrap(),
            Err(metadata::MetadataStoreError::Storage)
        );
        assert_eq!(
            Connection::open(&path)
                .unwrap()
                .query_row(
                    "SELECT observed_size FROM sidecar_associations WHERE photo_id='photo'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            7
        );
        persistence
            .with_metadata_receiver("photo".into(), |context| {
                context.record_observation(&metadata::ObservedSidecar {
                    state: metadata::ObservedSidecarState::Absent,
                })
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(
            persistence
                .with_metadata_receiver("photo".into(), |context| Ok(context
                    .record()
                    .active
                    .is_none()))
                .unwrap()
                .await
                .unwrap()
                .unwrap()
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn with_metadata_raw_claim_displaces_jpeg_owner_and_invalidates_evidence() {
        let (_base, persistence, path) = metadata_fixture();
        seed(
            &path,
            "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,capture_metadata_state)
             VALUES('raw-original','dir/photo.ARW','raw',1,1,1,'pending'),
                   ('raw-twin-original','dir/photo.CR2','raw',1,1,1,'pending');
             INSERT INTO photos(id,original_id,available,preview_state,sort_path,selection_state,rating)
             VALUES('raw-photo','raw-original',1,'inspection-pending','dir/photo.ARW','undecided',0),
                   ('raw-twin','raw-twin-original',1,'inspection-pending','dir/photo.CR2','undecided',0);",
        );
        persistence
            .with_metadata_receiver("photo".into(), |context| {
                context.record_observation(&metadata_observation(7))
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        // RAW priority hands the Sidecar to the RAW Photo; the displaced JPEG
        // owner loses its claim and its held evidence fails the generation.
        persistence
            .with_metadata_receiver("raw-photo".into(), |context| {
                context.record_observation(&metadata_observation(7))
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT photo_id FROM sidecar_associations WHERE sidecar_path='dir/photo.xmp'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "raw-photo"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sidecar_associations WHERE photo_id='photo'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        drop(connection);
        assert_eq!(association_generation(&path, "photo"), 2);
        assert_eq!(association_generation(&path, "raw-photo"), 1);
        // The displaced JPEG cannot reclaim while the RAW owner stands.
        assert_eq!(
            persistence
                .with_metadata_receiver("photo".into(), |context| context
                    .record_observation(&metadata_observation(8)))
                .unwrap()
                .await
                .unwrap(),
            Err(metadata::MetadataStoreError::Storage)
        );
        // A second available RAW of the same basename is a standing conflict.
        assert_eq!(
            persistence
                .with_metadata_receiver("raw-twin".into(), |context| context
                    .record_observation(&metadata_observation(8)))
                .unwrap()
                .await
                .unwrap(),
            Err(metadata::MetadataStoreError::Storage)
        );
        // The owner keeps updating its own claim.
        persistence
            .with_metadata_receiver("raw-photo".into(), |context| {
                context.record_observation(&metadata_observation(8))
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn with_metadata_claim_displaces_unavailable_owner_and_raw_claims_back() {
        let (_base, persistence, path) = metadata_fixture();
        seed(
            &path,
            "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,capture_metadata_state)
             VALUES('raw-original','dir/photo.ARW','raw',1,1,1,'pending');
             INSERT INTO photos(id,original_id,available,preview_state,sort_path,selection_state,rating)
             VALUES('raw-photo','raw-original',1,'inspection-pending','dir/photo.ARW','undecided',0);",
        );
        persistence
            .with_metadata_receiver("raw-photo".into(), |context| {
                context.record_observation(&metadata_observation(7))
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        seed(
            &path,
            "UPDATE original_files SET available=0 WHERE id='raw-original';",
        );
        // Without its Original the owner cannot write: the JPEG claim at the
        // stem displaces it and invalidates its evidence.
        persistence
            .with_metadata_receiver("photo".into(), |context| {
                context.record_observation(&metadata_observation(8))
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT photo_id FROM sidecar_associations WHERE sidecar_path='dir/photo.xmp'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "photo"
        );
        drop(connection);
        assert_eq!(association_generation(&path, "raw-photo"), 2);
        assert_eq!(association_generation(&path, "photo"), 1);
        // Once the RAW Original is available again, RAW priority reclaims the
        // Sidecar and the displaced JPEG's evidence fails.
        seed(
            &path,
            "UPDATE original_files SET available=1 WHERE id='raw-original';",
        );
        persistence
            .with_metadata_receiver("raw-photo".into(), |context| {
                context.record_observation(&metadata_observation(9))
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(association_generation(&path, "photo"), 2);
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn with_metadata_changed_clears_stale_claim_at_the_stem() {
        let (_base, persistence, path) = metadata_fixture();
        seed(
            &path,
            "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,capture_metadata_state)
             VALUES('raw-original','dir/photo.ARW','raw',1,1,1,'pending');
             INSERT INTO photos(id,original_id,available,preview_state,sort_path,selection_state,rating)
             VALUES('raw-photo','raw-original',1,'inspection-pending','dir/photo.ARW','undecided',0);",
        );
        persistence
            .with_metadata_receiver("photo".into(), |context| {
                context.record_observation(&metadata_observation(7))
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        // A Sidecar that no longer reads as recorded drops the stale JPEG
        // claim at the stem and invalidates its held evidence.
        persistence
            .with_metadata_receiver("raw-photo".into(), |context| {
                context.record_observation(&metadata::ObservedSidecar {
                    state: metadata::ObservedSidecarState::Changed,
                })
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let record = persistence
            .with_metadata_receiver("photo".into(), |context| Ok(context.record().clone()))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.active, None);
        assert_eq!(association_generation(&path, "photo"), 2);
        // The next readable inspection claims the Sidecar without a conflict.
        persistence
            .with_metadata_receiver("raw-photo".into(), |context| {
                context.record_observation(&metadata_observation(7))
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT photo_id FROM sidecar_associations WHERE sidecar_path='dir/photo.xmp'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "raw-photo"
        );
        drop(connection);
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn with_metadata_retains_unchanged_orphan_and_clears_corrections() {
        let (_base, persistence, path) = metadata_fixture();
        let sql = format!(
            "INSERT INTO retained_sidecar_orphans VALUES('dir/photo.xmp','retired','old.JPG','jpeg',3,7,1234.5,'{}');",
            "a".repeat(64)
        );
        seed(&path, &sql);
        persistence
            .with_metadata_receiver("photo".into(), |context| {
                context.record_observation(&metadata_observation(7))
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let orphan = persistence
            .with_metadata_receiver("photo".into(), |context| {
                Ok(context.record().orphan.clone())
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(orphan.retired_photo_id, "retired");
        assert_eq!(orphan.retired_generation, 3);
        persistence
            .with_metadata_receiver("photo".into(), |context| {
                context.record_observation(&metadata::ObservedSidecar {
                    state: metadata::ObservedSidecarState::Absent,
                })
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(
            persistence
                .with_metadata_receiver("photo".into(), |context| Ok(context
                    .record()
                    .orphan
                    .is_some()))
                .unwrap()
                .await
                .unwrap()
                .unwrap()
        );
        persistence
            .with_metadata_receiver("photo".into(), |context| {
                context.record_observation(&metadata_observation(8))
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(
            persistence
                .with_metadata_receiver("photo".into(), |context| Ok(context
                    .record()
                    .orphan
                    .is_none()))
                .unwrap()
                .await
                .unwrap()
                .unwrap()
        );
        seed(&path, &sql);
        persistence
            .with_metadata_receiver("photo".into(), |context| {
                context.record_observation(&metadata::ObservedSidecar {
                    state: metadata::ObservedSidecarState::Changed,
                })
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(
            persistence
                .with_metadata_receiver("photo".into(), |context| Ok(context
                    .record()
                    .orphan
                    .is_none()))
                .unwrap()
                .await
                .unwrap()
                .unwrap()
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn with_metadata_retains_uppercase_orphans_without_folding_basename() {
        let (_base, persistence, path) = metadata_fixture();
        seed(&path, &format!(
            "INSERT INTO retained_sidecar_orphans VALUES('dir/photo.XMP','retired','old.JPG','jpeg',3,7,1234.5,'{}');
             INSERT INTO retained_sidecar_orphans VALUES('dir/Photo.xmp','other','other.JPG','jpeg',3,7,1234.5,'{}');",
            "a".repeat(64), "a".repeat(64)
        ));
        let orphan = persistence
            .with_metadata_receiver("photo".into(), |context| {
                Ok(context.record().orphan.clone())
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(orphan.sidecar_path, "dir/photo.XMP");
        persistence
            .with_metadata_receiver("photo".into(), |context| {
                context.record_observation(&metadata::ObservedSidecar {
                    state: metadata::ObservedSidecarState::Changed,
                })
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(
            persistence
                .with_metadata_receiver("photo".into(), |context| {
                    Ok(context.record().orphan.is_none())
                })
                .unwrap()
                .await
                .unwrap()
                .unwrap()
        );
        assert_eq!(
            Connection::open(&path)
                .unwrap()
                .query_row(
                    "SELECT sidecar_path FROM retained_sidecar_orphans",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "dir/Photo.xmp"
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn with_metadata_save_serializes_before_concurrent_remove() {
        let (_base, persistence, path) = metadata_fixture();
        seed(
            &path,
            "UPDATE photos SET selection_state='rejected' WHERE id='photo';",
        );
        let (started_send, started_receive) = oneshot::channel();
        let (release_send, release_receive) = std::sync::mpsc::channel();
        let save = persistence
            .with_metadata_receiver("photo".into(), move |context| {
                started_send.send(()).unwrap();
                release_receive.recv().unwrap();
                context.record_observation(&metadata_observation(7))?;
                Ok(context.record().association_generation)
            })
            .unwrap();
        started_receive.await.unwrap();
        let remove = persistence
            .remove_photos_receiver(PhotoRemovalMutation {
                photo_ids: vec!["photo".into()],
                operation_id: "remove".into(),
            })
            .unwrap();
        assert_eq!(association_generation(&path, "photo"), 1);
        release_send.send(()).unwrap();
        assert_eq!(save.await.unwrap().unwrap(), 1);
        assert_eq!(remove.await.unwrap().unwrap().newly_removed, vec!["photo"]);
        assert_eq!(association_generation(&path, "photo"), 2);
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn with_metadata_library_round_trip() {
        let (_base, root, state, name, _path) = fixture();
        fs::write(root.canonical_path().join("one.JPG"), b"one").unwrap();
        let library = crate::Library::open(sidecar_config(&root, &state, &name)).unwrap();
        let snapshot = library.scan().await.unwrap();
        let photo = snapshot.photos[0].id.clone();
        library
            .with_metadata(photo.clone(), |context| {
                context.record_observation(&metadata::ObservedSidecar {
                    state: metadata::ObservedSidecarState::Eligible {
                        path: "one.xmp".into(),
                        size: 7,
                        mtime_ms: 1234.5,
                        digest: "a".repeat(64),
                    },
                })
            })
            .await
            .unwrap();
        let record = library
            .with_metadata(photo.clone(), |context| Ok(context.record().clone()))
            .await
            .unwrap();
        assert_eq!(record.photo_id, photo);
        assert_eq!(record.relative_path, "one.JPG");
        assert_eq!(record.active.unwrap().sidecar_path, "one.xmp");
        library.shutdown().unwrap();
        assert_eq!(
            library.with_metadata(photo, |_| Ok(())).await,
            Err(metadata::MetadataStoreError::Storage)
        );
    }
}
