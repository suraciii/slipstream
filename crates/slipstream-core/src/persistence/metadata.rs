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
