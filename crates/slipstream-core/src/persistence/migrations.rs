use super::{
    DatabaseName, PersistenceError, SchemaVersion, StateDirectory,
    owner::{allocate_library_id, names, table_columns, table_exists, validate_database},
    validate_canonical_schema,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::collections::{HashMap, HashSet};

const SCHEMA_V1_SQL: &str = include_str!("../../../../compatibility/sqlite/schema-v1.sql");
pub(super) fn preflight_schema(
    connection: &Connection,
    canonical_root: &str,
) -> Result<(), PersistenceError> {
    preflight_schema_for_max_version(connection, canonical_root, 11)
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
    if version > 11 {
        return Err(PersistenceError::NewerSchema);
    }
    validate_root_binding(connection, canonical_root)?;
    state.admit_sidecars(database_name)?;
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
    validate_database(&transaction)?;
    validate_canonical_schema(&transaction, SchemaVersion::V11)
        .map_err(|_| PersistenceError::UnsupportedSchema)?;
    transaction.commit().map_err(|_| PersistenceError::Storage)
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
