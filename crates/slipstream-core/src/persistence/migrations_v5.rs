use super::*;
// independent-photos-legacy:start migrate-v5
pub(super) fn migrate_v5(transaction: &Transaction<'_>) -> Result<(), PersistenceError> {
    // Issue #304: one independently managed Original File per Photo. A legacy
    // RAW/JPEG pair keeps its Photo identity, decisions, Album references,
    // and saved position on the RAW Original; its JPEG Original receives a new
    // independent Photo with default decisions. Content fingerprints start
    // empty and are enrolled in the background after migration.
    transaction
        .execute_batch(
            "CREATE TABLE original_fingerprints(
               original_id TEXT PRIMARY KEY REFERENCES original_files(id) ON DELETE CASCADE,
               digest TEXT NOT NULL CHECK(length(digest) = 64),
               size INTEGER NOT NULL CHECK(size >= 0),
               mtime_ms REAL NOT NULL CHECK(mtime_ms >= 0));
             CREATE INDEX original_fingerprints_digest ON original_fingerprints(digest);
             CREATE TABLE photos_v6(
               id TEXT PRIMARY KEY,
               original_id TEXT NOT NULL UNIQUE REFERENCES original_files(id) ON DELETE RESTRICT,
               available INTEGER NOT NULL CHECK(available IN (0,1)),
               preview_state TEXT NOT NULL CHECK(preview_state IN ('inspection-pending','ready','failed','unavailable')),
               preview_source_revision TEXT,
               preview_width INTEGER CHECK(preview_width IS NULL OR preview_width > 0),
               preview_height INTEGER CHECK(preview_height IS NULL OR preview_height > 0),
               cache_revision TEXT,
               sort_path TEXT NOT NULL,
               selection_state TEXT NOT NULL DEFAULT 'undecided' CHECK(selection_state IN ('undecided','selected','rejected')),
               rating INTEGER NOT NULL DEFAULT 0 CHECK(rating BETWEEN 0 AND 5));",
        )
        .map_err(|_| PersistenceError::Storage)?;

    let originals = original_facts_by_id(transaction)?;
    let photos = transaction
        .prepare(
            "SELECT id,raw_original_id,jpeg_original_id,preview_state,preview_source,
                    preview_source_revision,preview_width,preview_height,cache_revision,
                    sort_path,selection_state,rating
             FROM photos ORDER BY id",
        )
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], |row| {
            Ok(LegacyPhotoRow {
                id: row.get(0)?,
                raw_original_id: row.get(1)?,
                jpeg_original_id: row.get(2)?,
                preview_state: row.get(3)?,
                preview_source: row.get(4)?,
                preview_source_revision: row.get(5)?,
                preview_width: row.get(6)?,
                preview_height: row.get(7)?,
                cache_revision: row.get(8)?,
                sort_path: row.get(9)?,
                selection_state: row.get(10)?,
                rating: row.get(11)?,
            })
        })
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::InvalidLegacyData)?;
    let mut insert = transaction
        .prepare(
            "INSERT INTO photos_v6(id,original_id,available,preview_state,preview_source_revision,
               preview_width,preview_height,cache_revision,sort_path,selection_state,rating)
             VALUES(?,?,?,?,?,?,?,?,?,?,?)",
        )
        .map_err(|_| PersistenceError::Storage)?;
    let mut reserved_ids = HashSet::new();
    for photo in photos {
        let kept = photo
            .raw_original_id
            .clone()
            .or_else(|| photo.jpeg_original_id.clone());
        let Some(kept) = kept else {
            // A Photo with no Original is unusable; album references, if any,
            // fail closed through the RESTRICT foreign key.
            transaction
                .execute("DELETE FROM photos WHERE id=?", [&photo.id])
                .map_err(|_| PersistenceError::InvalidLegacyData)?;
            continue;
        };
        let Some((kept_available, kept_path, kept_size, kept_mtime, kept_kind)) =
            originals.get(&kept).cloned()
        else {
            return Err(PersistenceError::InvalidLegacyData);
        };
        let preserved = Some(kept_kind.preview_source().legacy_database_name())
            == photo.preview_source.as_deref()
            && revision_matches(
                photo.preview_source_revision.as_deref(),
                &kept_path,
                kept_size,
                kept_mtime,
            );
        let preview_state = if preserved {
            photo.preview_state
        } else {
            "inspection-pending".to_owned()
        };
        insert
            .execute(params![
                photo.id,
                kept,
                i64::from(kept_available),
                preview_state,
                preserved.then_some(photo.preview_source_revision).flatten(),
                preserved.then_some(photo.preview_width).flatten(),
                preserved.then_some(photo.preview_height).flatten(),
                preserved.then_some(photo.cache_revision).flatten(),
                photo.sort_path,
                photo.selection_state,
                photo.rating,
            ])
            .map_err(|_| PersistenceError::InvalidLegacyData)?;
        if let Some(jpeg_id) = photo.jpeg_original_id {
            if photo.raw_original_id.is_none() {
                continue;
            }
            let Some((jpeg_available, jpeg_path, _size, _mtime, _kind)) =
                originals.get(&jpeg_id).cloned()
            else {
                return Err(PersistenceError::InvalidLegacyData);
            };
            let new_id = allocate_library_id(transaction, &mut reserved_ids)?;
            insert
                .execute(params![
                    new_id,
                    jpeg_id,
                    i64::from(jpeg_available),
                    "inspection-pending",
                    Option::<String>::None,
                    Option::<i64>::None,
                    Option::<i64>::None,
                    Option::<String>::None,
                    jpeg_path,
                    "undecided",
                    0,
                ])
                .map_err(|_| PersistenceError::InvalidLegacyData)?;
        }
    }
    // Rebuild photos without violating the album foreign keys: unload the
    // album tables, replace photos, then recreate them with identical DDL and
    // every original row. One transaction keeps the migration atomic.
    let albums: Vec<(String, String, i64)> = transaction
        .prepare("SELECT id,name,created_at FROM albums ORDER BY created_at,id")
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::InvalidLegacyData)?;
    let members: Vec<(String, String, i64)> = transaction
        .prepare("SELECT album_id,photo_id,position FROM album_members ORDER BY album_id,position")
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::InvalidLegacyData)?;
    let progress: Vec<(String, String)> = transaction
        .prepare("SELECT album_id,photo_id FROM album_progress ORDER BY album_id")
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::InvalidLegacyData)?;
    transaction
        .execute_batch(
            "DROP TABLE album_progress;
             DROP TABLE album_members;
             DROP TABLE albums;
             DROP TABLE photos;
             ALTER TABLE photos_v6 RENAME TO photos;
             CREATE INDEX photos_original ON photos(original_id);
             CREATE TABLE albums(
               id TEXT PRIMARY KEY,
               name TEXT NOT NULL UNIQUE COLLATE NOCASE CHECK(length(name) BETWEEN 1 AND 120),
               created_at INTEGER NOT NULL);
             CREATE TABLE album_members(
               album_id TEXT NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
               photo_id TEXT NOT NULL REFERENCES photos(id) ON DELETE RESTRICT,
               position INTEGER NOT NULL CHECK(position >= 0),
               PRIMARY KEY(album_id, photo_id),
               UNIQUE(album_id, position));
             CREATE TABLE album_progress(
               album_id TEXT PRIMARY KEY REFERENCES albums(id) ON DELETE CASCADE,
               photo_id TEXT NOT NULL,
               FOREIGN KEY(album_id, photo_id) REFERENCES album_members(album_id, photo_id) ON DELETE CASCADE);
             CREATE INDEX album_members_photo ON album_members(photo_id);
             PRAGMA user_version = 6;",
        )
        .map_err(|_| PersistenceError::Storage)?;
    {
        let mut insert_album = transaction
            .prepare("INSERT INTO albums(id,name,created_at) VALUES(?,?,?)")
            .map_err(|_| PersistenceError::Storage)?;
        for (id, name, created_at) in &albums {
            insert_album
                .execute(params![id, name, created_at])
                .map_err(|_| PersistenceError::InvalidLegacyData)?;
        }
        let mut insert_member = transaction
            .prepare("INSERT INTO album_members(album_id,photo_id,position) VALUES(?,?,?)")
            .map_err(|_| PersistenceError::Storage)?;
        for (album_id, photo_id, position) in &members {
            insert_member
                .execute(params![album_id, photo_id, position])
                .map_err(|_| PersistenceError::InvalidLegacyData)?;
        }
        let mut insert_progress = transaction
            .prepare("INSERT INTO album_progress(album_id,photo_id) VALUES(?,?)")
            .map_err(|_| PersistenceError::Storage)?;
        for (album_id, photo_id) in &progress {
            insert_progress
                .execute(params![album_id, photo_id])
                .map_err(|_| PersistenceError::InvalidLegacyData)?;
        }
    }
    validate_canonical_schema(transaction, SchemaVersion::V6)
        .map_err(|_| PersistenceError::UnsupportedSchema)
}

struct LegacyPhotoRow {
    id: String,
    raw_original_id: Option<String>,
    jpeg_original_id: Option<String>,
    preview_state: String,
    preview_source: Option<String>,
    preview_source_revision: Option<String>,
    preview_width: Option<i64>,
    preview_height: Option<i64>,
    cache_revision: Option<String>,
    sort_path: String,
    selection_state: String,
    rating: i64,
}

type LegacyOriginalFacts = (bool, String, u64, f64, crate::OriginalKind);

fn original_facts_by_id(
    transaction: &Transaction<'_>,
) -> Result<HashMap<String, LegacyOriginalFacts>, PersistenceError> {
    transaction
        .prepare("SELECT id,available,relative_path,size,mtime_ms,kind FROM original_files")
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (
                    row.get::<_, i64>(1)? != 0,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    row.get::<_, f64>(4)?,
                    row.get::<_, String>(5)?,
                ),
            ))
        })
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<HashMap<_, _>, _>>()
        .map_err(|_| PersistenceError::Storage)?
        .into_iter()
        .map(|(id, (available, path, size, mtime_ms, kind))| {
            let kind = match kind.as_str() {
                "raw" => crate::OriginalKind::Raw,
                "jpeg" => crate::OriginalKind::Jpeg,
                _ => return Err(PersistenceError::InvalidLegacyData),
            };
            Ok((id, (available, path, size, mtime_ms, kind)))
        })
        .collect()
}

fn revision_matches(stored: Option<&str>, path: &str, size: u64, mtime_ms: f64) -> bool {
    let Some(stored) = stored else { return false };
    crate::source_revision(path, size, mtime_ms).is_ok_and(|current| current == stored)
}
// independent-photos-legacy:end migrate-v5
