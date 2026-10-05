use super::*;

pub(super) fn migrate_v14(transaction: &Transaction<'_>) -> Result<(), PersistenceError> {
    validate_canonical_schema(transaction, SchemaVersion::V14)
        .map_err(|_| PersistenceError::UnsupportedSchema)?;
    let malformed: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM photos
             WHERE selection_state NOT IN ('undecided','selected','rejected')",
            [],
            |row| row.get(0),
        )
        .map_err(|_| PersistenceError::Storage)?;
    if malformed != 0 {
        return Err(PersistenceError::InvalidLegacyData);
    }
    transaction
        .execute_batch(
            "CREATE TABLE photos_selection_v15(
               id TEXT PRIMARY KEY,
               original_id TEXT NOT NULL UNIQUE REFERENCES original_files(id) ON DELETE RESTRICT,
               available INTEGER NOT NULL CHECK(available IN (0,1)),
               preview_state TEXT NOT NULL CHECK(preview_state IN ('inspection-pending','ready','failed','unavailable')),
               preview_source_revision TEXT,
               preview_width INTEGER CHECK(preview_width IS NULL OR preview_width > 0),
               preview_height INTEGER CHECK(preview_height IS NULL OR preview_height > 0),
               cache_revision TEXT,
               sort_path TEXT NOT NULL,
               selection_state TEXT NOT NULL DEFAULT 'unflagged' CHECK(selection_state IN ('unflagged','picked','rejected')),
               rating INTEGER NOT NULL DEFAULT 0 CHECK(rating BETWEEN 0 AND 5),
               removed_at_ms INTEGER CHECK(removed_at_ms IS NULL OR removed_at_ms >= 0),
               removed_operation TEXT CHECK((removed_at_ms IS NULL) = (removed_operation IS NULL)),
               association_generation INTEGER NOT NULL DEFAULT 1 CHECK(association_generation > 0));
             INSERT INTO photos_selection_v15(
               id,original_id,available,preview_state,preview_source_revision,preview_width,
               preview_height,cache_revision,sort_path,selection_state,rating,removed_at_ms,
               removed_operation,association_generation)
             SELECT id,original_id,available,preview_state,preview_source_revision,preview_width,
               preview_height,cache_revision,sort_path,
               CASE selection_state WHEN 'undecided' THEN 'unflagged'
                    WHEN 'selected' THEN 'picked' ELSE 'rejected' END,
               rating,removed_at_ms,removed_operation,association_generation
             FROM photos;
             DROP TABLE photos;
             ALTER TABLE photos_selection_v15 RENAME TO photos;
             CREATE INDEX photos_original ON photos(original_id);
             PRAGMA user_version = 15;",
        )
        .map_err(|_| PersistenceError::InvalidLegacyData)?;
    validate_canonical_schema(transaction, SchemaVersion::V15)
        .map_err(|_| PersistenceError::UnsupportedSchema)
}
