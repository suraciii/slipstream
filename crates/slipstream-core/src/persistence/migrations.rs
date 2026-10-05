use super::{
    DatabaseName, PersistenceError, SchemaVersion, StateDirectory, owner::allocate_library_id,
    validate_canonical_schema,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::collections::{HashMap, HashSet};

#[path = "migrations_v14.rs"]
mod migrations_v14;
const SCHEMA_V1_SQL: &str = include_str!("../../../../compatibility/sqlite/schema-v1.sql");
/// Issue #472: identifies the RAW Preview decoder generation that persists
/// unavailable facts. An unavailable RAW Preview recorded by a previous
/// decoder (production LibRaw 0.21.5b rejected the Sony ILCE-7CM2 ARW at
/// open) must not suppress extraction under the current decoder, so startup
/// transitions those facts back to inspection once per decoder revision.
/// Bump when the pinned decoder changes (see the Dockerfile LibRaw pin).
const RAW_PREVIEW_DECODER_REVISION: &str = "libraw-0.22.2";

pub(super) fn preflight_schema(
    connection: &Connection,
    canonical_root: &str,
) -> Result<(), PersistenceError> {
    preflight_schema_for_max_version(connection, canonical_root, 15)
}

pub(super) fn preflight_schema_for_max_version(
    connection: &Connection,
    canonical_root: &str,
    maximum_version: u32,
) -> Result<(), PersistenceError> {
    let version: u32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|_| PersistenceError::Storage)?;
    if version > maximum_version {
        return Err(PersistenceError::NewerSchema);
    }
    validate_root_binding(connection, canonical_root)?;
    match version {
        0 if table_exists(connection, "original_files")? => validate_legacy_v0(connection),
        0 => Ok(()),
        1 => validate_canonical_schema(connection, SchemaVersion::V1)
            .map_err(|_| PersistenceError::UnsupportedSchema),
        2 => validate_canonical_schema(connection, SchemaVersion::V2)
            .map_err(|_| PersistenceError::UnsupportedSchema),
        3 => validate_canonical_schema(connection, SchemaVersion::V3)
            .map_err(|_| PersistenceError::UnsupportedSchema),
        4 => validate_canonical_schema(connection, SchemaVersion::V4)
            .map_err(|_| PersistenceError::UnsupportedSchema),
        5 => validate_canonical_schema(connection, SchemaVersion::V5)
            .map_err(|_| PersistenceError::UnsupportedSchema),
        6 => validate_canonical_schema(connection, SchemaVersion::V6)
            .map_err(|_| PersistenceError::UnsupportedSchema),
        7 => validate_canonical_schema(connection, SchemaVersion::V7)
            .map_err(|_| PersistenceError::UnsupportedSchema),
        8 => validate_canonical_schema(connection, SchemaVersion::V8)
            .map_err(|_| PersistenceError::UnsupportedSchema),
        9 => validate_canonical_schema(connection, SchemaVersion::V9)
            .map_err(|_| PersistenceError::UnsupportedSchema),
        10 => validate_canonical_schema(connection, SchemaVersion::V10)
            .map_err(|_| PersistenceError::UnsupportedSchema),
        11 => validate_canonical_schema(connection, SchemaVersion::V11)
            .map_err(|_| PersistenceError::UnsupportedSchema),
        12 => validate_canonical_schema(connection, SchemaVersion::V12)
            .map_err(|_| PersistenceError::UnsupportedSchema),
        13 => validate_canonical_schema(connection, SchemaVersion::V13)
            .map_err(|_| PersistenceError::UnsupportedSchema),
        14 => validate_canonical_schema(connection, SchemaVersion::V14)
            .map_err(|_| PersistenceError::UnsupportedSchema),
        15 => validate_canonical_schema(connection, SchemaVersion::V15)
            .map_err(|_| PersistenceError::UnsupportedSchema),
        _ => unreachable!(),
    }
}

pub(super) fn validate_root_binding(
    connection: &Connection,
    canonical_root: &str,
) -> Result<(), PersistenceError> {
    if table_exists(connection, "library_metadata")? {
        let stored: Option<String> = connection
            .query_row(
                "SELECT value FROM library_metadata WHERE key='canonical_root'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| PersistenceError::Storage)?;
        if stored
            .as_deref()
            .is_some_and(|stored| stored != canonical_root)
        {
            return Err(PersistenceError::RootMismatch);
        }
    }
    Ok(())
}

pub(super) fn startup_schema(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    canonical_root: &str,
) -> Result<(), PersistenceError> {
    let version: u32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|_| PersistenceError::Storage)?;
    if version > 15 {
        return Err(PersistenceError::NewerSchema);
    }
    validate_root_binding(connection, canonical_root)?;
    state.admit_sidecars(database_name)?;
    let rebuild_selection_state = version < 15;
    if rebuild_selection_state {
        connection
            .pragma_update(None, "foreign_keys", false)
            .map_err(|_| PersistenceError::Storage)?;
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| PersistenceError::Storage)?;
    match version {
        0 => {
            migrate_v0(&transaction)?;
            migrate_v2(&transaction)?;
            migrate_v3(&transaction)?;
            migrate_v4(&transaction)?;
        }
        1 => {
            validate_canonical_schema(&transaction, SchemaVersion::V1)
                .map_err(|_| PersistenceError::UnsupportedSchema)?;
            migrate_v1(&transaction)?;
            migrate_v2(&transaction)?;
            migrate_v3(&transaction)?;
            migrate_v4(&transaction)?;
        }
        2 => {
            validate_canonical_schema(&transaction, SchemaVersion::V2)
                .map_err(|_| PersistenceError::UnsupportedSchema)?;
            migrate_v2(&transaction)?;
            migrate_v3(&transaction)?;
            migrate_v4(&transaction)?;
        }
        3 => {
            validate_canonical_schema(&transaction, SchemaVersion::V3)
                .map_err(|_| PersistenceError::UnsupportedSchema)?;
            migrate_v3(&transaction)?;
            migrate_v4(&transaction)?;
        }
        4 => {
            validate_canonical_schema(&transaction, SchemaVersion::V4)
                .map_err(|_| PersistenceError::UnsupportedSchema)?;
            migrate_v4(&transaction)?;
        }
        5 => validate_canonical_schema(&transaction, SchemaVersion::V5)
            .map_err(|_| PersistenceError::UnsupportedSchema)?,
        6 => validate_canonical_schema(&transaction, SchemaVersion::V6)
            .map_err(|_| PersistenceError::UnsupportedSchema)?,
        7 => validate_canonical_schema(&transaction, SchemaVersion::V7)
            .map_err(|_| PersistenceError::UnsupportedSchema)?,
        8 => validate_canonical_schema(&transaction, SchemaVersion::V8)
            .map_err(|_| PersistenceError::UnsupportedSchema)?,
        9 => validate_canonical_schema(&transaction, SchemaVersion::V9)
            .map_err(|_| PersistenceError::UnsupportedSchema)?,
        10 => validate_canonical_schema(&transaction, SchemaVersion::V10)
            .map_err(|_| PersistenceError::UnsupportedSchema)?,
        11 => validate_canonical_schema(&transaction, SchemaVersion::V11)
            .map_err(|_| PersistenceError::UnsupportedSchema)?,
        12 => validate_canonical_schema(&transaction, SchemaVersion::V12)
            .map_err(|_| PersistenceError::UnsupportedSchema)?,
        13 => validate_canonical_schema(&transaction, SchemaVersion::V13)
            .map_err(|_| PersistenceError::UnsupportedSchema)?,
        14 => validate_canonical_schema(&transaction, SchemaVersion::V14)
            .map_err(|_| PersistenceError::UnsupportedSchema)?,
        15 => validate_canonical_schema(&transaction, SchemaVersion::V15)
            .map_err(|_| PersistenceError::UnsupportedSchema)?,
        _ => unreachable!(),
    }
    if version < 6 {
        migrate_v5(&transaction)?;
    }
    if version < 7 {
        migrate_v6(&transaction)?;
    }
    if version < 8 {
        migrate_v7(&transaction)?;
    }
    if version < 9 {
        migrate_v8(&transaction)?;
    }
    if version < 10 {
        migrate_v9(&transaction)?;
    }
    if version < 11 {
        migrate_v10(&transaction)?;
    }
    if version < 12 {
        migrate_v11(&transaction)?;
    }
    if version < 13 {
        migrate_v12(&transaction)?;
    }
    if version < 14 {
        migrate_v13(&transaction)?;
    }
    if version < 15 {
        migrations_v14::migrate_v14(&transaction)?;
    }
    let stored: Option<String> = transaction
        .query_row(
            "SELECT value FROM library_metadata WHERE key='canonical_root'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    if stored.is_none() {
        transaction
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
                [canonical_root],
            )
            .map_err(|_| PersistenceError::Storage)?;
    }
    let recorded_decoder: Option<String> = transaction
        .query_row(
            "SELECT value FROM library_metadata WHERE key='raw_preview_decoder'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    if recorded_decoder.as_deref() != Some(RAW_PREVIEW_DECODER_REVISION) {
        // Only active Photos whose own Original is RAW return to inspection.
        // Missing (available=0, including removed) Photos keep their state —
        // a scan already re-inspects them when the Original returns — and
        // ready, failed, and JPEG unavailable facts are never decoder-bound.
        // Selection, rating, Album membership, and Original rows are
        // untouched.
        transaction
            .execute(
                "UPDATE photos SET preview_state='inspection-pending',
                   preview_source_revision=NULL,preview_width=NULL,
                   preview_height=NULL,cache_revision=NULL
                 WHERE preview_state='unavailable' AND available=1
                   AND original_id IN (SELECT id FROM original_files WHERE kind='raw')",
                [],
            )
            .map_err(|_| PersistenceError::Storage)?;
        transaction
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('raw_preview_decoder',?)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                [RAW_PREVIEW_DECODER_REVISION],
            )
            .map_err(|_| PersistenceError::Storage)?;
    }
    super::composable_recipe::migrate_legacy_recipes(&transaction)?;
    validate_database(&transaction)?;
    validate_canonical_schema(&transaction, SchemaVersion::V15)
        .map_err(|_| PersistenceError::UnsupportedSchema)?;
    transaction
        .commit()
        .map_err(|_| PersistenceError::Storage)?;
    if rebuild_selection_state {
        connection
            .pragma_update(None, "foreign_keys", true)
            .map_err(|_| PersistenceError::Storage)?;
    }
    Ok(())
}

fn migrate_v0(transaction: &Transaction<'_>) -> Result<(), PersistenceError> {
    if table_exists(transaction, "original_files")? {
        validate_legacy_v0(transaction)?;
        transaction
            .execute_batch(
                "ALTER TABLE original_files RENAME TO original_files_legacy;
                 ALTER TABLE photos RENAME TO photos_legacy;
                 CREATE TABLE original_files(
                   id TEXT PRIMARY KEY, relative_path TEXT NOT NULL UNIQUE,
                   kind TEXT NOT NULL CHECK(kind IN ('raw','jpeg')),
                   size INTEGER NOT NULL CHECK(size >= 0), mtime_ms REAL NOT NULL CHECK(mtime_ms >= 0),
                   available INTEGER NOT NULL CHECK(available IN (0,1)),
                   error_category TEXT CHECK(error_category IS NULL OR error_category IN ('unreadable','changed')),
                   error_message TEXT CHECK(error_message IS NULL OR length(error_message) <= 120));
                 CREATE TABLE photos(
                   id TEXT PRIMARY KEY, raw_original_id TEXT REFERENCES original_files(id), jpeg_original_id TEXT REFERENCES original_files(id),
                   ambiguous INTEGER NOT NULL CHECK(ambiguous IN (0,1)), available INTEGER NOT NULL CHECK(available IN (0,1)),
                   preview_state TEXT NOT NULL CHECK(preview_state IN ('inspection-pending','ready','failed','unavailable')),
                   preview_candidate TEXT CHECK(preview_candidate IS NULL OR preview_candidate IN ('matching-jpeg','embedded-raw-jpeg')),
                   preview_source TEXT CHECK(preview_source IS NULL OR preview_source IN ('matching-jpeg','embedded-raw-jpeg')),
                   preview_source_revision TEXT, preview_width INTEGER CHECK(preview_width IS NULL OR preview_width > 0),
                   preview_height INTEGER CHECK(preview_height IS NULL OR preview_height > 0), cache_revision TEXT, sort_path TEXT NOT NULL);
                 INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,error_category,error_message)
                   SELECT id,relative_path,kind,size,mtime_ms,available,NULL,NULL FROM original_files_legacy;
                 INSERT INTO photos(id,raw_original_id,jpeg_original_id,ambiguous,available,preview_state,preview_source,sort_path)
                   SELECT id,raw_original_id,jpeg_original_id,ambiguous,available,preview_state,preview_source,sort_path FROM photos_legacy;
                 DROP TABLE photos_legacy; DROP TABLE original_files_legacy;
                 CREATE INDEX photos_raw ON photos(raw_original_id);
                 CREATE INDEX photos_jpeg ON photos(jpeg_original_id);
                 PRAGMA user_version = 1;",
            )
            .map_err(|_| PersistenceError::Storage)?;
    } else {
        transaction
            .execute_batch(SCHEMA_V1_SQL)
            .map_err(|_| PersistenceError::Storage)?;
    }
    validate_canonical_schema(transaction, SchemaVersion::V1)
        .map_err(|_| PersistenceError::UnsupportedSchema)?;
    migrate_v1(transaction)
}

// album-language-legacy:start migrate-v1
fn migrate_v1(transaction: &Transaction<'_>) -> Result<(), PersistenceError> {
    // Creates schema v2 state: the legacy photo-set table names are part of
    // the immutable v2-v4 contracts and are renamed to albums by migrate_v4.
    transaction
        .execute_batch(
            "ALTER TABLE photos ADD COLUMN selection_state TEXT NOT NULL DEFAULT 'undecided'
               CHECK(selection_state IN ('undecided','selected','rejected'));
             ALTER TABLE photos ADD COLUMN rating INTEGER NOT NULL DEFAULT 0
               CHECK(rating BETWEEN 0 AND 5);
             CREATE TABLE photo_sets(
               id TEXT PRIMARY KEY,
               name TEXT NOT NULL UNIQUE COLLATE NOCASE CHECK(length(name) BETWEEN 1 AND 120),
               created_at INTEGER NOT NULL);
             CREATE TABLE photo_set_members(
               photo_set_id TEXT NOT NULL REFERENCES photo_sets(id) ON DELETE CASCADE,
               photo_id TEXT NOT NULL REFERENCES photos(id) ON DELETE RESTRICT,
               position INTEGER NOT NULL CHECK(position >= 0),
               PRIMARY KEY(photo_set_id, photo_id),
               UNIQUE(photo_set_id, position));
             CREATE TABLE review_progress(
               photo_set_id TEXT PRIMARY KEY REFERENCES photo_sets(id) ON DELETE CASCADE,
               photo_id TEXT NOT NULL,
               FOREIGN KEY(photo_set_id, photo_id)
                 REFERENCES photo_set_members(photo_set_id, photo_id) ON DELETE CASCADE);
             CREATE INDEX photo_set_members_photo ON photo_set_members(photo_id);
             PRAGMA user_version = 2;",
        )
        .map_err(|_| PersistenceError::Storage)
}
// album-language-legacy:end migrate-v1

fn migrate_v2(transaction: &Transaction<'_>) -> Result<(), PersistenceError> {
    transaction
        .execute_batch(
            "ALTER TABLE original_files ADD COLUMN capture_metadata_state TEXT NOT NULL DEFAULT 'pending'
               CHECK(capture_metadata_state IN ('pending','known','missing','invalid','failed'));
             ALTER TABLE original_files ADD COLUMN capture_order_key TEXT CHECK(capture_order_key IS NULL OR (
               length(capture_order_key)=29 AND substr(capture_order_key,5,1)='-' AND
               substr(capture_order_key,8,1)='-' AND substr(capture_order_key,11,1)='T' AND
               substr(capture_order_key,14,1)=':' AND substr(capture_order_key,17,1)=':' AND
               substr(capture_order_key,20,1)='.' AND
               replace(replace(replace(replace(capture_order_key,'-',''),':',''),'T',''),'.','')
                 NOT GLOB '*[^0-9]*'
             ));
             ALTER TABLE original_files ADD COLUMN capture_time_field TEXT CHECK(capture_time_field IS NULL OR capture_time_field IN ('date-time-original','date-time-digitized'));
             ALTER TABLE original_files ADD COLUMN capture_offset_minutes INTEGER CHECK(capture_offset_minutes IS NULL OR capture_offset_minutes BETWEEN -840 AND 840);
             ALTER TABLE original_files ADD COLUMN capture_source_revision TEXT;
             PRAGMA user_version = 3;",
        )
        .map_err(|_| PersistenceError::Storage)
}

fn migrate_v3(transaction: &Transaction<'_>) -> Result<(), PersistenceError> {
    transaction
        .execute_batch("PRAGMA user_version = 4;")
        .map_err(|_| PersistenceError::Storage)
}

// album-language-legacy:start migrate-v4
fn migrate_v4(transaction: &Transaction<'_>) -> Result<(), PersistenceError> {
    // Issue #95: rename the legacy v4 photo-set storage to canonical albums in
    // one transaction. The new tables use DDL text identical to
    // compatibility/sqlite/schema-v5.sql so the migrated database satisfies
    // the exact schema-v5 manifest. Every album id, name, creation order,
    // membership position, and saved position is copied unchanged.
    transaction
        .execute_batch(
            "CREATE TABLE albums(
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
               FOREIGN KEY(album_id, photo_id)
                 REFERENCES album_members(album_id, photo_id) ON DELETE CASCADE);
             CREATE INDEX album_members_photo ON album_members(photo_id);
             INSERT INTO albums(id,name,created_at)
               SELECT id,name,created_at FROM photo_sets;
             INSERT INTO album_members(album_id,photo_id,position)
               SELECT photo_set_id,photo_id,position FROM photo_set_members;
             INSERT INTO album_progress(album_id,photo_id)
               SELECT photo_set_id,photo_id FROM review_progress;
             DROP TABLE review_progress;
             DROP TABLE photo_set_members;
             DROP TABLE photo_sets;
             PRAGMA user_version = 5;",
        )
        .map_err(|_| PersistenceError::Storage)
}
// album-language-legacy:end migrate-v4

// independent-photos-legacy:start migrate-v5
fn migrate_v5(transaction: &Transaction<'_>) -> Result<(), PersistenceError> {
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

fn migrate_v6(transaction: &Transaction<'_>) -> Result<(), PersistenceError> {
    validate_canonical_schema(transaction, SchemaVersion::V6)
        .map_err(|_| PersistenceError::UnsupportedSchema)?;
    transaction
        .execute_batch(
            "CREATE TABLE edit_recipes(
               photo_id TEXT PRIMARY KEY REFERENCES photos(id) ON DELETE RESTRICT,
               revision TEXT NOT NULL CHECK(length(revision) > 0),
               source_revision TEXT NOT NULL CHECK(length(source_revision) > 0),
               exposure_ev REAL NOT NULL CHECK(exposure_ev BETWEEN -1.7976931348623157e308 AND 1.7976931348623157e308),
               white_balance_mode TEXT NOT NULL CHECK(white_balance_mode = 'as-shot')
             );
             PRAGMA user_version = 7;",
        )
        .map_err(|_| PersistenceError::Storage)?;
    validate_canonical_schema(transaction, SchemaVersion::V7)
        .map_err(|_| PersistenceError::UnsupportedSchema)
}

fn migrate_v7(transaction: &Transaction<'_>) -> Result<(), PersistenceError> {
    validate_canonical_schema(transaction, SchemaVersion::V7)
        .map_err(|_| PersistenceError::UnsupportedSchema)?;
    // The closed white-balance payload bounds are published independent of
    // admission, so a recipe can retain a temperature-tint editing intent
    // that no capability admits for execution. The rebuild widens the mode
    // column and adds the two nullable intent values; existing as-shot rows
    // keep null values.
    transaction
        .execute_batch(
            "ALTER TABLE edit_recipes RENAME TO edit_recipes_v7;
             CREATE TABLE edit_recipes(
               photo_id TEXT PRIMARY KEY REFERENCES photos(id) ON DELETE RESTRICT,
               revision TEXT NOT NULL CHECK(length(revision) > 0),
               source_revision TEXT NOT NULL CHECK(length(source_revision) > 0),
               exposure_ev REAL NOT NULL CHECK(exposure_ev BETWEEN -1.7976931348623157e308 AND 1.7976931348623157e308),
               white_balance_mode TEXT NOT NULL CHECK(white_balance_mode IN ('as-shot','temperature-tint')),
               temperature_kelvin INTEGER CHECK(temperature_kelvin IS NULL OR temperature_kelvin BETWEEN 1000 AND 40000),
               tint_milli INTEGER CHECK(tint_milli IS NULL OR tint_milli BETWEEN -150000 AND 150000)
             );
             INSERT INTO edit_recipes(photo_id,revision,source_revision,exposure_ev,white_balance_mode)
               SELECT photo_id,revision,source_revision,exposure_ev,white_balance_mode FROM edit_recipes_v7;
             DROP TABLE edit_recipes_v7;
             CREATE TABLE exports(
               id TEXT PRIMARY KEY,
               photo_id TEXT NOT NULL REFERENCES photos(id) ON DELETE RESTRICT,
               target TEXT NOT NULL CHECK(target = 'development-tiff'),
               state TEXT NOT NULL CHECK(state IN ('queued','running','succeeded','failed','cancelled')),
               outcome TEXT CHECK(outcome IS NULL OR length(outcome) BETWEEN 1 AND 200),
               recipe_revision TEXT NOT NULL CHECK(length(recipe_revision) > 0),
               exposure_ev REAL NOT NULL,
               white_balance_mode TEXT NOT NULL CHECK(white_balance_mode = 'as-shot'),
               source_revision TEXT NOT NULL CHECK(length(source_revision) > 0),
               source_profile_id TEXT NOT NULL CHECK(length(source_profile_id) BETWEEN 1 AND 64),
               source_kind TEXT NOT NULL CHECK(source_kind = 'raw'),
               source_size INTEGER CHECK(source_size IS NULL OR source_size > 0),
               source_sha256 TEXT CHECK(source_sha256 IS NULL OR length(source_sha256) = 64),
               recipe_digest TEXT NOT NULL CHECK(length(recipe_digest) = 64),
               policy_id TEXT NOT NULL CHECK(length(policy_id) = 64),
               bundle_id TEXT NOT NULL CHECK(length(bundle_id) = 64),
               workload TEXT NOT NULL CHECK(workload = 'development-tiff'),
               attempt_incarnation TEXT CHECK(attempt_incarnation IS NULL OR length(attempt_incarnation) = 32),
               attempt_sequence INTEGER CHECK(attempt_sequence IS NULL OR attempt_sequence > 0),
               artifact_size INTEGER CHECK(artifact_size IS NULL OR artifact_size > 0),
               artifact_sha256 TEXT CHECK(artifact_sha256 IS NULL OR length(artifact_sha256) = 64),
               artifact_expires_at INTEGER CHECK(artifact_expires_at IS NULL OR artifact_expires_at >= 0),
               artifact_width INTEGER CHECK(artifact_width IS NULL OR artifact_width > 0),
               artifact_height INTEGER CHECK(artifact_height IS NULL OR artifact_height > 0),
               artifact_profile_identity TEXT CHECK(artifact_profile_identity IS NULL OR length(artifact_profile_identity) = 64),
               created_at INTEGER NOT NULL CHECK(created_at >= 0),
               settled_at INTEGER CHECK(settled_at IS NULL OR settled_at >= 0),
               retain_until INTEGER CHECK(retain_until IS NULL OR retain_until >= 0)
             );
             CREATE INDEX exports_photo ON exports(photo_id);
             CREATE TABLE export_download_leases(
               id TEXT PRIMARY KEY,
               export_id TEXT NOT NULL REFERENCES exports(id) ON DELETE CASCADE,
               created_at INTEGER NOT NULL CHECK(created_at >= 0)
             );
             PRAGMA user_version = 8;",
        )
        .map_err(|_| PersistenceError::Storage)?;
    validate_canonical_schema(transaction, SchemaVersion::V8)
        .map_err(|_| PersistenceError::UnsupportedSchema)
}

/// Issue #416: a removed Photo keeps its row and every retained fact. The
/// removal marker is application-owned Library state, so it is added to the
/// Photo row instead of a separate recovery record.
fn migrate_v8(transaction: &Transaction<'_>) -> Result<(), PersistenceError> {
    validate_canonical_schema(transaction, SchemaVersion::V8)
        .map_err(|_| PersistenceError::UnsupportedSchema)?;
    transaction
        .execute_batch(
            "ALTER TABLE photos ADD COLUMN removed_at_ms INTEGER
               CHECK(removed_at_ms IS NULL OR removed_at_ms >= 0);
             ALTER TABLE photos ADD COLUMN removed_operation TEXT
               CHECK((removed_at_ms IS NULL) = (removed_operation IS NULL));
             PRAGMA user_version = 9;",
        )
        .map_err(|_| PersistenceError::Storage)?;
    validate_canonical_schema(transaction, SchemaVersion::V9)
        .map_err(|_| PersistenceError::UnsupportedSchema)
}

/// Issue #327: allow the first production Film workload without changing the
/// durable Export row shape. Rebuild only the two tables whose closed checks
/// widen from the V9 development workload to V10's two workloads.
fn migrate_v9(transaction: &Transaction<'_>) -> Result<(), PersistenceError> {
    validate_canonical_schema(transaction, SchemaVersion::V9)
        .map_err(|_| PersistenceError::UnsupportedSchema)?;
    transaction
        .execute_batch(
            "ALTER TABLE export_download_leases RENAME TO export_download_leases_v9;
             DROP INDEX exports_photo;
             ALTER TABLE exports RENAME TO exports_v9;
             CREATE TABLE exports(
               id TEXT PRIMARY KEY,
               photo_id TEXT NOT NULL REFERENCES photos(id) ON DELETE RESTRICT,
               target TEXT NOT NULL CHECK(target IN ('development-tiff','film-jpeg')),
               state TEXT NOT NULL CHECK(state IN ('queued','running','succeeded','failed','cancelled')),
               outcome TEXT CHECK(outcome IS NULL OR length(outcome) BETWEEN 1 AND 200),
               recipe_revision TEXT NOT NULL CHECK(length(recipe_revision) > 0),
               exposure_ev REAL NOT NULL,
               white_balance_mode TEXT NOT NULL CHECK(white_balance_mode = 'as-shot'),
               source_revision TEXT NOT NULL CHECK(length(source_revision) > 0),
               source_profile_id TEXT NOT NULL CHECK(length(source_profile_id) BETWEEN 1 AND 64),
               source_kind TEXT NOT NULL CHECK(source_kind = 'raw'),
               source_size INTEGER CHECK(source_size IS NULL OR source_size > 0),
               source_sha256 TEXT CHECK(source_sha256 IS NULL OR length(source_sha256) = 64),
               recipe_digest TEXT NOT NULL CHECK(length(recipe_digest) = 64),
               policy_id TEXT NOT NULL CHECK(length(policy_id) = 64),
               bundle_id TEXT NOT NULL CHECK(length(bundle_id) = 64),
               workload TEXT NOT NULL CHECK(workload IN ('development-tiff','film-jpeg')),
               attempt_incarnation TEXT CHECK(attempt_incarnation IS NULL OR length(attempt_incarnation) = 32),
               attempt_sequence INTEGER CHECK(attempt_sequence IS NULL OR attempt_sequence > 0),
               artifact_size INTEGER CHECK(artifact_size IS NULL OR artifact_size > 0),
               artifact_sha256 TEXT CHECK(artifact_sha256 IS NULL OR length(artifact_sha256) = 64),
               artifact_expires_at INTEGER CHECK(artifact_expires_at IS NULL OR artifact_expires_at >= 0),
               artifact_width INTEGER CHECK(artifact_width IS NULL OR artifact_width > 0),
               artifact_height INTEGER CHECK(artifact_height IS NULL OR artifact_height > 0),
               artifact_profile_identity TEXT CHECK(artifact_profile_identity IS NULL OR length(artifact_profile_identity) = 64),
               created_at INTEGER NOT NULL CHECK(created_at >= 0),
               settled_at INTEGER CHECK(settled_at IS NULL OR settled_at >= 0),
               retain_until INTEGER CHECK(retain_until IS NULL OR retain_until >= 0)
             );
             INSERT INTO exports(
               id,photo_id,target,state,outcome,recipe_revision,exposure_ev,
               white_balance_mode,source_revision,source_profile_id,source_kind,
               source_size,source_sha256,recipe_digest,policy_id,bundle_id,
               workload,attempt_incarnation,attempt_sequence,artifact_size,
               artifact_sha256,artifact_expires_at,artifact_width,artifact_height,
               artifact_profile_identity,created_at,settled_at,retain_until
             )
             SELECT
               id,photo_id,target,state,outcome,recipe_revision,exposure_ev,
               white_balance_mode,source_revision,source_profile_id,source_kind,
               source_size,source_sha256,recipe_digest,policy_id,bundle_id,
               workload,attempt_incarnation,attempt_sequence,artifact_size,
               artifact_sha256,artifact_expires_at,artifact_width,artifact_height,
               artifact_profile_identity,created_at,settled_at,retain_until
             FROM exports_v9;
             CREATE INDEX exports_photo ON exports(photo_id);
             CREATE TABLE export_download_leases_new(
               id TEXT PRIMARY KEY,
               export_id TEXT NOT NULL REFERENCES exports(id) ON DELETE CASCADE,
               created_at INTEGER NOT NULL CHECK(created_at >= 0)
             );
             INSERT INTO export_download_leases_new(id,export_id,created_at)
               SELECT id,export_id,created_at FROM export_download_leases_v9;
             DROP TABLE export_download_leases_v9;
             DROP TABLE exports_v9;
             ALTER TABLE export_download_leases_new RENAME TO export_download_leases;
             PRAGMA user_version = 10;",
        )
        .map_err(|_| PersistenceError::Storage)?;
    validate_canonical_schema(transaction, SchemaVersion::V10)
        .map_err(|_| PersistenceError::UnsupportedSchema)
}

/// Issue #276: metadata sidecar observations follow the Photo's association
/// generation, and a retired association keeps its observed facts until the
/// sidecar returns. Add the generation counter and the retained sidecar
/// records without changing any earlier table shape.
fn migrate_v10(transaction: &Transaction<'_>) -> Result<(), PersistenceError> {
    validate_canonical_schema(transaction, SchemaVersion::V10)
        .map_err(|_| PersistenceError::UnsupportedSchema)?;
    transaction
        .execute_batch(
            "ALTER TABLE photos ADD COLUMN association_generation INTEGER NOT NULL DEFAULT 1
               CHECK(association_generation > 0);
             CREATE TABLE sidecar_associations(
               photo_id TEXT PRIMARY KEY REFERENCES photos(id) ON DELETE RESTRICT,
               sidecar_path TEXT NOT NULL UNIQUE,
               observed_size INTEGER CHECK(observed_size IS NULL OR observed_size >= 0),
               observed_mtime_ms REAL CHECK(observed_mtime_ms IS NULL OR observed_mtime_ms >= 0),
               observed_digest TEXT CHECK(observed_digest IS NULL OR length(observed_digest) = 64),
               CHECK((observed_size IS NULL) = (observed_mtime_ms IS NULL))
             );
             CREATE TABLE retained_sidecar_orphans(
               sidecar_path TEXT PRIMARY KEY,
               retired_photo_id TEXT NOT NULL,
               retired_original_path TEXT NOT NULL,
               original_kind TEXT NOT NULL CHECK(original_kind IN ('raw','jpeg')),
               retired_generation INTEGER NOT NULL CHECK(retired_generation > 0),
               observed_size INTEGER CHECK(observed_size IS NULL OR observed_size >= 0),
               observed_mtime_ms REAL CHECK(observed_mtime_ms IS NULL OR observed_mtime_ms >= 0),
               observed_digest TEXT CHECK(observed_digest IS NULL OR length(observed_digest) = 64),
               CHECK((observed_size IS NULL) = (observed_mtime_ms IS NULL))
             );
             PRAGMA user_version = 11;",
        )
        .map_err(|_| PersistenceError::Storage)?;
    validate_canonical_schema(transaction, SchemaVersion::V11)
        .map_err(|_| PersistenceError::UnsupportedSchema)
}

/// Issue #6a14efa4: the camera identity that names a RAW source class must
/// be published atomically with the capture facts it was read with, so
/// source support derives from the same published read evidence instead of
/// an on-demand metadata read. Existing rows keep their capture facts and
/// carry `pending` identities; the next scan re-inspects them once (a
/// pending identity is not reusable) and publishes the observed identity.
fn migrate_v11(transaction: &Transaction<'_>) -> Result<(), PersistenceError> {
    validate_canonical_schema(transaction, SchemaVersion::V11)
        .map_err(|_| PersistenceError::UnsupportedSchema)?;
    transaction
        .execute_batch(
            "ALTER TABLE original_files ADD COLUMN camera_identity_state TEXT NOT NULL DEFAULT 'pending'
               CHECK(camera_identity_state IN ('pending','observed'));
             ALTER TABLE original_files ADD COLUMN camera_make TEXT CHECK(camera_make IS NULL OR (
               length(camera_make) > 0 AND length(camera_make) <= 128 AND
               camera_make NOT GLOB '*[^ -~]*'));
             ALTER TABLE original_files ADD COLUMN camera_model TEXT CHECK(camera_model IS NULL OR (
               length(camera_model) > 0 AND length(camera_model) <= 128 AND
               camera_model NOT GLOB '*[^ -~]*'));
             PRAGMA user_version = 12;",
        )
        .map_err(|_| PersistenceError::Storage)?;
    validate_canonical_schema(transaction, SchemaVersion::V12)
        .map_err(|_| PersistenceError::UnsupportedSchema)
}
/// Persist the service-owned development proxy publication record.
fn migrate_v12(transaction: &Transaction<'_>) -> Result<(), PersistenceError> {
    validate_canonical_schema(transaction, SchemaVersion::V12)
        .map_err(|_| PersistenceError::UnsupportedSchema)?;
    transaction
        .execute_batch(
            "CREATE TABLE development_proxies(
               photo_id TEXT PRIMARY KEY REFERENCES photos(id) ON DELETE RESTRICT,
               source_revision TEXT NOT NULL CHECK(length(source_revision) > 0),
               source_relative_path TEXT NOT NULL CHECK(length(source_relative_path) > 0),
               source_sha256 TEXT NOT NULL CHECK(length(source_sha256) = 64),
               source_size INTEGER NOT NULL CHECK(source_size > 0),
               profile_id TEXT NOT NULL CHECK(length(profile_id) BETWEEN 1 AND 64),
               pipeline_version TEXT NOT NULL CHECK(length(pipeline_version) BETWEEN 1 AND 32),
               bundle_sha256 TEXT NOT NULL CHECK(length(bundle_sha256) = 64),
               long_edge INTEGER NOT NULL CHECK(long_edge > 0),
               width INTEGER NOT NULL CHECK(width > 0),
               height INTEGER NOT NULL CHECK(height > 0),
               artifact_sha256 TEXT NOT NULL CHECK(length(artifact_sha256) = 64),
               artifact_bytes INTEGER NOT NULL CHECK(artifact_bytes > 0),
               created_at INTEGER NOT NULL CHECK(created_at >= 0)
             );
             PRAGMA user_version = 13;",
        )
        .map_err(|_| PersistenceError::Storage)?;
    validate_canonical_schema(transaction, SchemaVersion::V13)
        .map_err(|_| PersistenceError::UnsupportedSchema)
}

fn migrate_v13(transaction: &Transaction<'_>) -> Result<(), PersistenceError> {
    validate_canonical_schema(transaction, SchemaVersion::V13)
        .map_err(|_| PersistenceError::UnsupportedSchema)?;
    transaction.execute_batch(
        "CREATE TABLE xmp_exports(
           id TEXT PRIMARY KEY,
           photo_id TEXT NOT NULL REFERENCES photos(id) ON DELETE RESTRICT,
           request_id TEXT NOT NULL,
           payload_digest TEXT NOT NULL CHECK(length(payload_digest) = 64),
           recipe_revision TEXT NOT NULL CHECK(length(recipe_revision) > 0),
           source_revision TEXT NOT NULL CHECK(length(source_revision) > 0),
           exposure_ev REAL NOT NULL,
           white_balance_mode TEXT NOT NULL CHECK(white_balance_mode IN ('as-shot','temperature-tint')),
           temperature_kelvin INTEGER,
           tint_milli INTEGER,
           created_at INTEGER NOT NULL CHECK(created_at >= 0),
           expires_at INTEGER NOT NULL CHECK(expires_at >= created_at),
           filename TEXT NOT NULL CHECK(length(filename) > 0),
           document BLOB CHECK(document IS NULL OR length(document) > 0),
           byte_length INTEGER NOT NULL CHECK(byte_length > 0),
           sha256 TEXT NOT NULL CHECK(length(sha256) = 64),
           UNIQUE(photo_id, request_id)
         );
         PRAGMA user_version = 14;",
    ).map_err(|_| PersistenceError::Storage)?;
    validate_canonical_schema(transaction, SchemaVersion::V14)
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

fn validate_legacy_v0(connection: &Connection) -> Result<(), PersistenceError> {
    let tables = names(connection, "table")?;
    if tables != ["library_metadata", "original_files", "photos"] {
        return Err(PersistenceError::UnsupportedSchema);
    }
    let expected = [
        ("library_metadata", &["key", "value"][..]),
        (
            "original_files",
            &[
                "id",
                "relative_path",
                "kind",
                "size",
                "mtime_ms",
                "available",
                "inspection_error",
            ][..],
        ),
        (
            "photos",
            &[
                "id",
                "raw_original_id",
                "jpeg_original_id",
                "ambiguous",
                "available",
                "preview_state",
                "preview_source",
                "sort_path",
            ][..],
        ),
    ];
    for (table, columns) in expected {
        if table_columns(connection, table)? != columns {
            return Err(PersistenceError::UnsupportedSchema);
        }
    }
    let invalid_original: Option<u8> = connection
        .query_row(
            "SELECT 1 FROM original_files WHERE
             typeof(id) != 'text' OR id = '' OR typeof(relative_path) != 'text' OR relative_path = '' OR
             kind NOT IN ('raw','jpeg') OR typeof(size) != 'integer' OR size < 0 OR
             typeof(mtime_ms) NOT IN ('integer','real') OR mtime_ms < 0 OR
             typeof(available) != 'integer' OR available NOT IN (0,1) LIMIT 1",
            [], |row| row.get(0),
        ).optional().map_err(|_| PersistenceError::Storage)?;
    let invalid_photo: Option<u8> = connection
        .query_row(
            "SELECT 1 FROM photos WHERE
             typeof(id) != 'text' OR id = '' OR typeof(ambiguous) != 'integer' OR ambiguous NOT IN (0,1) OR
             typeof(available) != 'integer' OR available NOT IN (0,1) OR
             preview_state NOT IN ('inspection-pending','ready','failed','unavailable') OR
             (preview_source IS NOT NULL AND preview_source NOT IN ('matching-jpeg','embedded-raw-jpeg')) OR
             typeof(sort_path) != 'text' OR
             (raw_original_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM original_files o WHERE o.id=photos.raw_original_id)) OR
             (jpeg_original_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM original_files o WHERE o.id=photos.jpeg_original_id)) LIMIT 1",
            [], |row| row.get(0),
        ).optional().map_err(|_| PersistenceError::Storage)?;
    if invalid_original.is_some() || invalid_photo.is_some() {
        return Err(PersistenceError::InvalidLegacyData);
    }
    Ok(())
}

pub(super) fn validate_database(connection: &Connection) -> Result<(), PersistenceError> {
    if connection
        .prepare("PRAGMA foreign_key_check")
        .and_then(|mut statement| statement.exists([]))
        .map_err(|_| PersistenceError::Storage)?
    {
        return Err(PersistenceError::Storage);
    }
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|_| PersistenceError::Storage)?;
    if integrity != "ok" {
        return Err(PersistenceError::Storage);
    }
    Ok(())
}

pub(super) fn table_exists(connection: &Connection, name: &str) -> Result<bool, PersistenceError> {
    connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?",
            [name],
            |_| Ok(()),
        )
        .optional()
        .map(|value| value.is_some())
        .map_err(|_| PersistenceError::Storage)
}

pub(super) fn names(connection: &Connection, kind: &str) -> Result<Vec<String>, PersistenceError> {
    connection
        .prepare("SELECT name FROM sqlite_master WHERE type=? AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .and_then(|mut statement| {
            statement
                .query_map([kind], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|_| PersistenceError::Storage)
}

pub(super) fn table_columns(
    connection: &Connection,
    table: &str,
) -> Result<Vec<String>, PersistenceError> {
    connection
        .prepare(&format!("PRAGMA table_info(\"{table}\")"))
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get(1))?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|_| PersistenceError::Storage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::migrations;
    use crate::persistence::owner::Command;
    use crate::persistence::owner::reserve_library_id;
    use crate::persistence::schema::{SchemaVersion, validate_canonical_schema};
    use crate::persistence::test_support::*;
    use crate::persistence::{MutationError, Persistence, PersistenceError};
    use crate::{
        AlbumMutation, CaptureFact, CaptureMetadataState, CaptureTimeField, OriginalKind,
        PreviewState, SelectionState, original_id, source_revision,
    };
    use rusqlite::Connection;
    use rusqlite::params;
    use serde::Deserialize;
    use std::{fs, os::unix::fs::PermissionsExt};
    use tokio::sync::oneshot;

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RejectionFixture {
        name: String,
        version: u32,
        sql: String,
        expected_error: String,
    }

    include!("migrations_v15_tests.rs");

    #[tokio::test]
    async fn v6_to_v7_migration_preserves_existing_library_rows() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v6.sql"),
        );
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
                [library.canonical_path().to_str().unwrap()],
            )
            .unwrap();
        add_recipe_test_photo(
            &connection,
            RecipeTestPhoto {
                original_id: "raw-original",
                photo_id: "raw-photo",
                relative_path: "shoot/one.ARW",
                kind: "raw",
                available: true,
                size: 17,
                mtime_ms: 1_000.0,
            },
        );
        connection
            .execute(
                "INSERT INTO original_fingerprints(original_id,digest,size,mtime_ms) VALUES(?,?,?,?)",
                params!["raw-original", "a".repeat(64), 17_i64, 1_000.0_f64],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO albums(id,name,created_at) VALUES('album-one','Preserved',9)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO album_members(album_id,photo_id,position) VALUES('album-one','raw-photo',0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE photos SET selection_state='selected',rating=4 WHERE id='raw-photo'",
                [],
            )
            .unwrap();
        drop(connection);

        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence.snapshot().await.unwrap();
        assert_eq!(snapshot.originals.len(), 1);
        assert_eq!(snapshot.originals[0].id, "raw-original");
        assert_eq!(
            snapshot.originals[0].relative_path.as_str(),
            "shoot/one.ARW"
        );
        assert_eq!(snapshot.originals[0].facts.size, 17);
        assert_eq!(snapshot.photos.len(), 1);
        assert_eq!(snapshot.photos[0].id, "raw-photo");
        assert_eq!(snapshot.photos[0].selection_state, SelectionState::Picked);
        assert_eq!(snapshot.photos[0].rating, 4);
        assert!(!snapshot.photos[0].has_saved_edits);
        assert_eq!(
            persistence.list_albums().await.unwrap()[0].members[0].photo_id,
            "raw-photo"
        );
        persistence.shutdown().unwrap();
        let connection = Connection::open(&path).unwrap();
        validate_canonical_schema(&connection, SchemaVersion::V15).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                .unwrap(),
            15
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT digest,size,mtime_ms FROM original_fingerprints WHERE original_id='raw-original'",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, f64>(2)?)),
                )
                .unwrap(),
            ("a".repeat(64), 17, 1_000.0)
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM edit_recipes", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    // album-language-legacy:start v4-migration-test
    #[tokio::test]
    async fn v4_migration_preserves_album_state_through_current_schema() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v4.sql"),
        );
        let first = "00000000-0000-4000-8000-000000000031";
        let second = "00000000-0000-4000-8000-000000000032";
        let photo_one = "photo-one";
        let photo_two = "photo-two";
        let connection = Connection::open(&path).unwrap();
        for (id, path_text, sort_path) in [
            ("original-one", "one.JPG", "one.JPG"),
            ("original-two", "two.JPG", "two.JPG"),
        ] {
            connection
                .execute(
                    "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,capture_metadata_state) VALUES(?,?, 'jpeg',9,1.0,1,'pending')",
                    params![id, path_text],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO photos(id,jpeg_original_id,ambiguous,available,preview_state,sort_path,selection_state,rating) VALUES(?,?,0,1,'inspection-pending',?,'undecided',0)",
                    params![sort_path.replace(".JPG", ""), id, sort_path],
                )
                .unwrap();
        }
        connection
            .execute(
                "UPDATE photos SET id=? WHERE jpeg_original_id='original-one'",
                [photo_one],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE photos SET id=? WHERE jpeg_original_id='original-two'",
                [photo_two],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO photo_sets(id,name,created_at) VALUES(?,?,?)",
                params![first, "Shoot", 7_i64],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO photo_sets(id,name,created_at) VALUES(?,?,?)",
                params![second, "Client", 9_i64],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO photo_set_members(photo_set_id,photo_id,position) VALUES(?,?,1)",
                params![first, photo_two],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO photo_set_members(photo_set_id,photo_id,position) VALUES(?,?,0)",
                params![first, photo_one],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO review_progress(photo_set_id,photo_id) VALUES(?,?)",
                params![first, photo_two],
            )
            .unwrap();
        drop(connection);
        let persistence = Persistence::open(
            state,
            name.clone(),
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let albums = persistence.list_albums().await.unwrap();
        assert_eq!(albums.len(), 2);
        assert_eq!(albums[0].id, first);
        assert_eq!(albums[0].name, "Shoot");
        assert_eq!(albums[1].id, second);
        assert_eq!(albums[1].name, "Client");
        assert_eq!(albums[1].members.len(), 0);
        assert_eq!(albums[1].last_reviewed_photo_id, None);
        let members = &albums[0].members;
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].photo_id, photo_one);
        assert_eq!(members[0].position, 0);
        assert_eq!(members[1].photo_id, photo_two);
        assert_eq!(members[1].position, 1);
        assert_eq!(albums[0].last_reviewed_photo_id.as_deref(), Some(photo_two));
        persistence.shutdown().unwrap();
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                .unwrap(),
            15
        );
        validate_canonical_schema(&connection, SchemaVersion::V15).unwrap();
        // The legacy photo-set tables are gone rather than left as aliases.
        for legacy in ["photo_sets", "photo_set_members", "review_progress"] {
            assert!(!table_exists(&connection, legacy).unwrap(), "{legacy}");
        }
    }
    // album-language-legacy:end v4-migration-test

    #[test]
    fn newer_v16_database_is_rejected_without_changes() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v15.sql"),
        );
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", 16)
            .unwrap();
        let before = fs::read(&path).unwrap();
        assert!(matches!(
            Persistence::open(
                state,
                name,
                library.canonical_path().to_str().unwrap().to_owned(),
            ),
            Err(PersistenceError::NewerSchema)
        ));
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn v11_migration_preserves_populated_rows_with_pending_camera_identity() {
        // A populated v11 database keeps every published fact — capture
        // state, capture revision, and saved recipe — through the v12
        // migration, while the camera identity starts `pending` with null
        // make/model until the next scan publishes the observed identity.
        // The rollback binary cannot read v12, so a verified snapshot is the
        // only rollback path (see RUNBOOK).
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v11.sql"),
        );
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute(
                    "INSERT INTO original_files(
                       id,relative_path,kind,size,mtime_ms,available,
                       capture_metadata_state,capture_order_key,capture_time_field,
                       capture_offset_minutes,capture_source_revision
                     ) VALUES(
                       'raw-original','shoot/raw.ARW','raw',11,1.0,1,
                       'known','2026-09-28T10:00:00.000000000','date-time-original',60,
                       'published-source-revision'
                     )",
                    [],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO photos(id,original_id,available,preview_state,sort_path)
                     VALUES('photo-one','raw-original',1,'inspection-pending','shoot/raw.ARW')",
                    [],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO edit_recipes(
                       photo_id,revision,source_revision,exposure_ev,white_balance_mode
                     ) VALUES(
                       'photo-one','recipe-revision-1','published-source-revision',0.0,'as-shot'
                     )",
                    [],
                )
                .unwrap();
        }
        Persistence::open(
            state,
            name,
            library.canonical_path().to_str().unwrap().to_owned(),
        )
        .unwrap()
        .shutdown()
        .unwrap();
        let connection = Connection::open(&path).unwrap();
        let version: u32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 15);
        validate_canonical_schema(&connection, SchemaVersion::V15).unwrap();
        let (state, order_key, source_revision, identity_state, make, model): (
            String,
            String,
            String,
            String,
            Option<String>,
            Option<String>,
        ) = connection
            .query_row(
                "SELECT capture_metadata_state,capture_order_key,capture_source_revision,
                        camera_identity_state,camera_make,camera_model
                 FROM original_files WHERE id='raw-original'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(state, "known");
        assert_eq!(order_key, "2026-09-28T10:00:00.000000000");
        assert_eq!(source_revision, "published-source-revision");
        assert_eq!(identity_state, "pending");
        assert_eq!(make, None);
        assert_eq!(model, None);
        let (revision, recipe_source): (String, String) = connection
            .query_row(
                "SELECT revision,source_revision FROM edit_recipes WHERE photo_id='photo-one'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(revision, "recipe-revision-1");
        assert_eq!(recipe_source, "published-source-revision");
    }

    #[tokio::test]
    async fn migrates_v12_to_v13_adds_development_proxies() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v12.sql"),
        );
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
                [library.canonical_path().to_str().unwrap()],
            )
            .unwrap();
        drop(connection);

        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        persistence.shutdown().unwrap();

        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                .unwrap(),
            15
        );
        assert!(table_exists(&connection, "development_proxies").unwrap());
        validate_canonical_schema(&connection, SchemaVersion::V15).unwrap();
    }

    #[test]
    fn startup_transitions_stale_unavailable_raw_previews_once_per_decoder() {
        // Issue #472: an unavailable RAW Preview fact recorded under a
        // previous decoder returns to inspection-pending exactly once, so the
        // upgraded decoder re-inspects the Photo. Ready and failed RAW facts,
        // unavailable JPEG facts, missing-Original records, and the Photo's
        // own decisions stay untouched, and a decoder-fresh database never
        // resets an unavailable fact again.
        let (base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v13.sql"),
        );
        {
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(
                    "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available)
                     VALUES
                       ('raw-stale','shoot/stale.ARW','raw',11,1.0,1),
                       ('raw-ready','shoot/ready.ARW','raw',12,1.0,1),
                       ('jpeg-stale','shoot/stale.JPG','jpeg',13,1.0,1),
                       ('raw-missing','shoot/missing.ARW','raw',14,1.0,0);
                     INSERT INTO photos(
                       id,original_id,available,preview_state,preview_source_revision,
                       preview_width,preview_height,cache_revision,sort_path,
                       selection_state,rating
                     ) VALUES
                       ('photo-stale','raw-stale',1,'unavailable','stale-revision',NULL,NULL,NULL,
                        'shoot/stale.ARW','selected',4),
                       ('photo-ready','raw-ready',1,'ready','ready-revision',1600,1200,'cache-1',
                        'shoot/ready.ARW','rejected',2),
                       ('photo-jpeg','jpeg-stale',1,'unavailable','jpeg-revision',NULL,NULL,NULL,
                        'shoot/stale.JPG','undecided',0),
                       ('photo-missing','raw-missing',0,'unavailable',NULL,NULL,NULL,NULL,
                        'shoot/missing.ARW','undecided',0);",
                )
                .unwrap();
        }
        Persistence::open(
            state,
            name.clone(),
            library.canonical_path().to_str().unwrap().to_owned(),
        )
        .unwrap()
        .shutdown()
        .unwrap();

        let connection = Connection::open(&path).unwrap();
        let facts = |photo: &str| {
            connection
                .query_row(
                    "SELECT preview_state,preview_source_revision,preview_width,preview_height,
                            cache_revision,selection_state,rating
                     FROM photos WHERE id=?",
                    [photo],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<i64>>(2)?,
                            row.get::<_, Option<i64>>(3)?,
                            row.get::<_, Option<String>>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, i64>(6)?,
                        ))
                    },
                )
                .unwrap()
        };
        assert_eq!(
            facts("photo-stale"),
            (
                "inspection-pending".to_owned(),
                None,
                None,
                None,
                None,
                "picked".to_owned(),
                4
            )
        );
        assert_eq!(
            facts("photo-ready"),
            (
                "ready".to_owned(),
                Some("ready-revision".to_owned()),
                Some(1600),
                Some(1200),
                Some("cache-1".to_owned()),
                "rejected".to_owned(),
                2
            )
        );
        assert_eq!(facts("photo-jpeg").0, "unavailable");
        assert_eq!(facts("photo-missing").0, "unavailable");
        let recorded: String = connection
            .query_row(
                "SELECT value FROM library_metadata WHERE key='raw_preview_decoder'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(recorded, RAW_PREVIEW_DECODER_REVISION);

        // A Photo the current decoder itself found unavailable is durable
        // again: the recorded decoder gates the transition, so a reopen never
        // resets it.
        connection
            .execute(
                "UPDATE photos SET preview_state='unavailable',
                   preview_source_revision='current-revision'
                 WHERE id='photo-stale'",
                [],
            )
            .unwrap();
        drop(connection);
        let reopened = StateDirectory::open_or_create(&library, base.0.join("state")).unwrap();
        Persistence::open(
            reopened,
            name,
            library.canonical_path().to_str().unwrap().to_owned(),
        )
        .unwrap()
        .shutdown()
        .unwrap();
        let connection = Connection::open(&path).unwrap();
        let (state_after, revision_after): (String, Option<String>) = connection
            .query_row(
                "SELECT preview_state,preview_source_revision FROM photos WHERE id='photo-stale'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(state_after, "unavailable");
        assert_eq!(revision_after.as_deref(), Some("current-revision"));
    }

    #[test]
    fn legacy_binary_fence_rejects_canonical_schema_above_the_max_version_without_changes() {
        // Every legacy database whose canonical version exceeds the supported
        // maximum is rejected without touching a byte, for both fence gaps.
        for (sql, version, max_version) in [
            (
                include_str!("../../../../compatibility/sqlite/schema-v5.sql"),
                SchemaVersion::V5,
                4,
            ),
            (
                include_str!("../../../../compatibility/sqlite/schema-v4.sql"),
                SchemaVersion::V4,
                3,
            ),
        ] {
            let (_base, library, _state, _name, path) = fixture();
            seed(&path, sql);
            let connection = Connection::open(&path).unwrap();
            validate_canonical_schema(&connection, version).unwrap();

            let sidecars = ["-journal", "-wal", "-shm"]
                .map(|suffix| path.with_file_name(format!("library.sqlite{suffix}")));
            let persisted_paths = std::iter::once(path.clone())
                .chain(sidecars.iter().cloned())
                .collect::<Vec<_>>();
            let before = persisted_paths
                .iter()
                .map(|path| fs::read(path).ok())
                .collect::<Vec<_>>();

            assert!(matches!(
                migrations::preflight_schema_for_max_version(
                    &connection,
                    library.canonical_path().to_str().unwrap(),
                    max_version,
                ),
                Err(PersistenceError::NewerSchema)
            ));

            let after = persisted_paths
                .iter()
                .map(|path| fs::read(path).ok())
                .collect::<Vec<_>>();
            assert_eq!(after, before);
        }
    }

    #[tokio::test]
    async fn migrates_shared_v0_and_v1_to_current_schema_and_rejects_malformed_v2() {
        for sql in [
            include_str!("../../../../compatibility/sqlite/v0.sql"),
            include_str!("../../../../compatibility/sqlite/v1.sql"),
        ] {
            let (_base, library, state, name, path) = fixture();
            seed(&path, sql);
            let persistence = Persistence::open(
                state,
                name,
                library.canonical_path().to_string_lossy().into_owned(),
            )
            .unwrap();
            persistence.shutdown().unwrap();
            let connection = Connection::open(&path).unwrap();
            validate_canonical_schema(&connection, SchemaVersion::V15).unwrap();
        }
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/malformed-v2.sql"),
        );
        assert!(matches!(
            Persistence::open(
                state,
                name,
                library.canonical_path().to_string_lossy().into_owned()
            ),
            Err(PersistenceError::UnsupportedSchema)
        ));
        assert_eq!(
            Connection::open(path)
                .unwrap()
                .pragma_query_value::<u8, _>(None, "user_version", |row| row.get(0))
                .unwrap(),
            2
        );
    }

    #[test]
    fn shared_rejection_fixtures_are_rejected_without_database_changes() {
        let fixtures: Vec<RejectionFixture> = serde_json::from_str(include_str!(
            "../../../../compatibility/sqlite/rejections.json"
        ))
        .unwrap();
        for rejection in fixtures {
            let (_base, library, state, name, path) = fixture();
            seed(&path, &rejection.sql);
            let before = fs::read(&path).unwrap();
            let result = Persistence::open(
                state,
                name,
                library.canonical_path().to_str().unwrap().to_owned(),
            );
            assert!(
                matches!(
                    result,
                    Err(PersistenceError::UnsupportedSchema | PersistenceError::InvalidLegacyData)
                ),
                "{} expected {}",
                rejection.name,
                rejection.expected_error
            );
            assert_eq!(fs::read(&path).unwrap(), before, "{}", rejection.name);
            assert_eq!(
                Connection::open(&path)
                    .unwrap()
                    .pragma_query_value::<u32, _>(None, "user_version", |row| row.get(0))
                    .unwrap(),
                rejection.version,
                "{}",
                rejection.name
            );
        }
    }

    // album-language-legacy:start v3-migration-test
    #[tokio::test]
    async fn v3_migration_preserves_every_row_identity_and_user_owned_state() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v3.sql"),
        );
        let original_id = original_id("shoot/A.JPG");
        let photo_id = "photo-preserved";
        let legacy_album_id = "00000000-0000-4000-8000-000000000027";
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO original_files VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
                params![
                    original_id,
                    "shoot/A.JPG",
                    "jpeg",
                    12_i64,
                    1_000.0_f64,
                    1_i64,
                    Option::<String>::None,
                    Option::<String>::None,
                    "known",
                    "2026-01-01T10:00:00.000000000",
                    "date-time-original",
                    60_i64,
                    "capture-revision"
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO photos(id,jpeg_original_id,ambiguous,available,preview_state,preview_candidate,preview_source,preview_source_revision,preview_width,preview_height,cache_revision,sort_path,selection_state,rating) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                params![photo_id, original_id, 0_i64, 1_i64, "ready", "matching-jpeg", "matching-jpeg", source_revision("shoot/A.JPG", 12, 1000.0).unwrap(), 8_i64, 4_i64, "cache-revision", "shoot/A.JPG", "selected", 5_i64],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO photo_sets(id,name,created_at) VALUES(?,?,?)",
                params![legacy_album_id, "Preserved", 1_i64],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO photo_set_members(photo_set_id,photo_id,position) VALUES(?,?,?)",
                params![legacy_album_id, photo_id, 0_i64],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO review_progress(photo_set_id,photo_id) VALUES(?,?)",
                params![legacy_album_id, photo_id],
            )
            .unwrap();
        drop(connection);
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence.snapshot().await.unwrap();
        let photo = &snapshot.photos[0];
        assert_eq!(photo.id, photo_id);
        assert_eq!(photo.original_id, original_id);
        assert!(photo.available);
        assert_eq!(photo.preview_state, PreviewState::Ready);
        assert_eq!(
            photo.preview_source_revision.as_deref(),
            Some(source_revision("shoot/A.JPG", 12, 1000.0).unwrap().as_str())
        );
        assert_eq!(photo.preview_width, Some(8));
        assert_eq!(photo.preview_height, Some(4));
        assert_eq!(photo.cache_revision.as_deref(), Some("cache-revision"));
        assert_eq!(photo.sort_path, "shoot/A.JPG");
        assert_eq!(photo.selection_state, SelectionState::Picked);
        assert_eq!(photo.rating, 5);
        let original = &snapshot.originals[0];
        assert_eq!(original.id, original_id);
        assert_eq!(original.relative_path.as_str(), "shoot/A.JPG");
        assert_eq!(original.kind, OriginalKind::Jpeg);
        assert_eq!(original.facts.size, 12);
        assert_eq!(original.facts.mtime_ms, 1_000.0);
        assert!(original.available);
        assert_eq!(original.error_category, None);
        assert_eq!(original.error_message, None);
        assert_eq!(
            original.capture,
            CaptureFact {
                state: CaptureMetadataState::Known,
                order_key: Some("2026-01-01T10:00:00.000000000".to_owned()),
                field: Some(CaptureTimeField::DateTimeOriginal),
                offset_minutes: Some(60),
                source_revision: Some("capture-revision".to_owned()),
                identity: crate::CameraIdentity::Pending,
            }
        );
        let album = persistence.list_albums().await.unwrap().remove(0);
        assert_eq!(album.id, legacy_album_id);
        assert_eq!(album.name, "Preserved");
        assert_eq!(album.members.len(), 1);
        assert_eq!(album.members[0].photo_id, photo_id);
        assert_eq!(album.members[0].position, 0);
        assert!(album.members[0].available);
        assert_eq!(album.members[0].selection_state, SelectionState::Picked);
        assert_eq!(album.members[0].rating, 5);
        assert_eq!(album.last_reviewed_photo_id.as_deref(), Some(photo_id));
        persistence.shutdown().unwrap();
        let connection = Connection::open(&path).unwrap();
        validate_canonical_schema(&connection, SchemaVersion::V15).unwrap();
    }
    // album-language-legacy:end v3-migration-test

    #[tokio::test]
    async fn every_present_sidecar_rejects_startup_before_creating_database() {
        for suffix in ["-journal", "-wal", "-shm"] {
            let (_base, library, state, name, path) = fixture();
            let sidecar = path.with_file_name(format!("library.sqlite{suffix}"));
            fs::write(&sidecar, b"operator recovery data").unwrap();
            fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o600)).unwrap();
            let result = Persistence::open(
                state,
                name,
                library.canonical_path().to_str().unwrap().to_owned(),
            );
            assert!(matches!(result, Err(PersistenceError::RecoveryRequired)));
            assert!(
                !path.exists(),
                "{suffix} must be checked before database creation"
            );
            assert_eq!(fs::read(sidecar).unwrap(), b"operator recovery data");
        }
    }

    #[tokio::test]
    async fn every_present_sidecar_blocks_writes_without_changing_database() {
        for suffix in ["-journal", "-wal", "-shm"] {
            let (_base, library, state, name, path) = fixture();
            let persistence = Persistence::open(
                state,
                name,
                library.canonical_path().to_str().unwrap().to_owned(),
            )
            .unwrap();
            let sidecar = path.with_file_name(format!("library.sqlite{suffix}"));
            fs::write(&sidecar, b"operator recovery data").unwrap();
            fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o600)).unwrap();
            let before = fs::read(&path).unwrap();
            assert_eq!(
                persistence
                    .mutate_album(AlbumMutation::Create {
                        name: format!("Blocked {suffix}"),
                    })
                    .await,
                Err(MutationError::Persistence)
            );
            assert_eq!(
                fs::read(&path).unwrap(),
                before,
                "{suffix} changed database"
            );
            assert_eq!(fs::read(&sidecar).unwrap(), b"operator recovery data");
            persistence.shutdown().unwrap();
        }
    }

    #[tokio::test]
    async fn malformed_v2_wal_rejection_preserves_database_and_sidecars() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/malformed-v2.sql"),
        );
        let connection = Connection::open(&path).unwrap();
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .unwrap();
        connection
            .pragma_update(None, "wal_autocheckpoint", 0)
            .unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata VALUES('wal_probe','unchanged')",
                [],
            )
            .unwrap();
        let paths = [
            path.clone(),
            path.with_file_name("library.sqlite-wal"),
            path.with_file_name("library.sqlite-shm"),
        ];
        let before = paths
            .iter()
            .map(|path| fs::read(path).ok())
            .collect::<Vec<_>>();
        let result = Persistence::open(
            state,
            name,
            library.canonical_path().to_str().unwrap().to_owned(),
        );
        assert!(matches!(result, Err(PersistenceError::RecoveryRequired)));
        let after = paths
            .iter()
            .map(|path| fs::read(path).ok())
            .collect::<Vec<_>>();
        assert_eq!(after, before);
        drop(connection);
    }

    #[tokio::test]
    async fn root_mismatch_rejects_before_migration_without_changes() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v1.sql"),
        );
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata VALUES('canonical_root','/different')",
                [],
            )
            .unwrap();
        drop(connection);
        let before = fs::read(&path).unwrap();
        assert!(matches!(
            Persistence::open(
                state,
                name,
                library.canonical_path().to_string_lossy().into_owned()
            ),
            Err(PersistenceError::RootMismatch)
        ));
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[tokio::test]
    async fn wal_only_root_binding_is_rejected_without_changing_database_or_sidecars() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v2.sql"),
        );
        let connection = Connection::open(&path).unwrap();
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .unwrap();
        connection
            .pragma_update(None, "wal_autocheckpoint", 0)
            .unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata VALUES('canonical_root','/different')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata VALUES('wal_probe','unchanged')",
                [],
            )
            .unwrap();
        let paths = [
            path.clone(),
            path.with_file_name("library.sqlite-wal"),
            path.with_file_name("library.sqlite-shm"),
        ];
        let before = paths
            .iter()
            .map(|path| fs::read(path).ok())
            .collect::<Vec<_>>();
        let open_result = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        );
        assert!(matches!(
            open_result,
            Err(PersistenceError::RecoveryRequired)
        ));
        let after = paths
            .iter()
            .map(|path| fs::read(path).ok())
            .collect::<Vec<_>>();
        assert_eq!(after, before);
        assert_eq!(
            connection
                .query_row(
                    "SELECT value FROM library_metadata WHERE key='wal_probe'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "unchanged"
        );
        drop(connection);
    }

    // album-language-legacy:start v2-reconciliation-test
    #[tokio::test]
    async fn preserves_decisions_and_memberships_while_reconciling_rows() {
        let (_base, library, state, name, path) = fixture();
        let raw = discovered("one.ARW", OriginalKind::Raw, 3, 1000.0);
        let jpeg = discovered("one.JPG", OriginalKind::Jpeg, 4, 1000.0);
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v2.sql"),
        );
        let raw_id = original_id(raw.path.as_str());
        let jpeg_id = original_id(jpeg.path.as_str());
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO original_files VALUES(?,?,?,?,?,?,?,?)",
                params![
                    raw_id,
                    "one.ARW",
                    "raw",
                    3_i64,
                    1000.0_f64,
                    1_i64,
                    Option::<String>::None,
                    Option::<String>::None
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO original_files VALUES(?,?,?,?,?,?,?,?)",
                params![
                    jpeg_id,
                    "one.JPG",
                    "jpeg",
                    4_i64,
                    1000.0_f64,
                    1_i64,
                    Option::<String>::None,
                    Option::<String>::None
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO photos(id,raw_original_id,jpeg_original_id,ambiguous,available,preview_state,preview_candidate,selection_state,rating,sort_path) VALUES(?,?,?,?,?,?,?,?,?,?)",
                params!["stable-photo", raw_id, jpeg_id, 0_i64, 1_i64, "inspection-pending", "matching-jpeg", "selected", 5_i64, "one.ARW"],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO photo_sets(id,name,created_at) VALUES(?,?,?)",
                params!["00000000-0000-4000-8000-000000000021", "Keep", 1_i64],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO photo_set_members(photo_set_id,photo_id,position) VALUES(?,?,?)",
                params![
                    "00000000-0000-4000-8000-000000000021",
                    "stable-photo",
                    0_i64
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO review_progress(photo_set_id,photo_id) VALUES(?,?)",
                params!["00000000-0000-4000-8000-000000000021", "stable-photo"],
            )
            .unwrap();
        drop(connection);
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(vec![raw, jpeg], Vec::new())
            .await
            .unwrap();
        assert_eq!(snapshot.photos[0].id, "stable-photo");
        assert_eq!(snapshot.photos[0].selection_state, SelectionState::Picked);
        assert_eq!(snapshot.photos[0].rating, 5);
        persistence.shutdown().unwrap();
        let connection = Connection::open(path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT photo_id FROM album_progress WHERE album_id=?",
                    ["00000000-0000-4000-8000-000000000021"],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "stable-photo"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM album_members WHERE album_id=?",
                    ["00000000-0000-4000-8000-000000000021"],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
    }
    // album-language-legacy:end v2-reconciliation-test

    /// The v8 fixture carries the Photos, decisions, and Album membership the
    /// migration chain must preserve; the resulting current schema starts
    /// with an empty removal marker.
    #[tokio::test]
    async fn v8_to_current_migration_preserves_photos_and_starts_unremoved() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v8.sql"),
        );
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
                [library.canonical_path().to_str().unwrap()],
            )
            .unwrap();
        add_recipe_test_photo(
            &connection,
            RecipeTestPhoto {
                original_id: "raw-original",
                photo_id: "raw-photo",
                relative_path: "shoot/one.ARW",
                kind: "raw",
                available: true,
                size: 17,
                mtime_ms: 1_000.0,
            },
        );
        connection
            .execute(
                "UPDATE photos SET selection_state='rejected',rating=4 WHERE id='raw-photo'",
                [],
            )
            .unwrap();
        drop(connection);

        let persistence = Persistence::open(
            state,
            name.clone(),
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .snapshot_receiver()
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.photos.len(), 1);
        assert_eq!(snapshot.photos[0].selection_state, SelectionState::Rejected);
        assert_eq!(snapshot.photos[0].rating, 4);
        assert!(!snapshot.photos[0].removed);

        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                .unwrap(),
            15
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT removed_at_ms,removed_operation FROM photos WHERE id='raw-photo'",
                    [],
                    |row| Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                    )),
                )
                .unwrap(),
            (None, None)
        );
        drop(connection);
        persistence.shutdown().unwrap();
    }

    // Issue #276 metadata records join the Film v10 schema as v11. The
    // migration must add the sidecar records without disturbing the Film
    // export rows, their download leases, or any Photo's identity and
    // user-owned state, and every Photo starts at the first generation.
    #[tokio::test]
    async fn v10_to_v11_migration_preserves_film_export_state_and_starts_generation() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v10.sql"),
        );
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
                [library.canonical_path().to_str().unwrap()],
            )
            .unwrap();
        add_recipe_test_photo(
            &connection,
            RecipeTestPhoto {
                original_id: "raw-original",
                photo_id: "raw-photo",
                relative_path: "shoot/one.ARW",
                kind: "raw",
                available: true,
                size: 17,
                mtime_ms: 1_000.0,
            },
        );
        connection
            .execute(
                "UPDATE photos SET selection_state='selected',rating=4 WHERE id='raw-photo'",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO exports(id,photo_id,target,state,recipe_revision,exposure_ev,white_balance_mode,source_revision,source_profile_id,source_kind,recipe_digest,policy_id,bundle_id,workload,created_at)
                 VALUES('export-one','raw-photo','film-jpeg','succeeded','recipe-1',0.25,'as-shot','source-1','profile-1','raw',?,?,?,'film-jpeg',1)",
                params!["d".repeat(64), "e".repeat(64), "f".repeat(64)],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO export_download_leases(id,export_id,created_at) VALUES('lease-one','export-one',2)",
                [],
            )
            .unwrap();
        drop(connection);

        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        persistence.shutdown().unwrap();

        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                .unwrap(),
            15
        );
        validate_canonical_schema(&connection, SchemaVersion::V15).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT target,state,workload,recipe_digest,policy_id,bundle_id,exposure_ev,white_balance_mode
                     FROM exports WHERE id='export-one'",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, f64>(6)?,
                            row.get::<_, String>(7)?,
                        ))
                    },
                )
                .unwrap(),
            (
                "film-jpeg".to_owned(),
                "succeeded".to_owned(),
                "film-jpeg".to_owned(),
                "d".repeat(64),
                "e".repeat(64),
                "f".repeat(64),
                0.25,
                "as-shot".to_owned(),
            )
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT export_id,created_at FROM export_download_leases WHERE id='lease-one'",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .unwrap(),
            ("export-one".to_owned(), 2)
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT id,sort_path,selection_state,rating,association_generation
                     FROM photos WHERE id='raw-photo'",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, i64>(4)?,
                        ))
                    },
                )
                .unwrap(),
            (
                "raw-photo".to_owned(),
                "shoot/one.ARW".to_owned(),
                "picked".to_owned(),
                4,
                1,
            )
        );
    }
    #[test]
    fn new_library_id_collision_is_rejected_before_insertion() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(include_str!(
                "../../../../compatibility/sqlite/schema-v4.sql"
            ))
            .unwrap();
        connection.execute(
            "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,capture_metadata_state) VALUES('collision','a.JPG','jpeg',1,1,1,'pending')",
            [],
        ).unwrap();
        let transaction = connection.unchecked_transaction().unwrap();
        assert!(matches!(
            reserve_library_id(&transaction, &mut HashSet::new(), "collision".to_owned()),
            Err(PersistenceError::IdCollision)
        ));
        assert_eq!(
            transaction
                .query_row("SELECT count(*) FROM original_files", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }
}
