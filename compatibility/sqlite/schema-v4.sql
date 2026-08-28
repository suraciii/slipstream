PRAGMA foreign_keys = ON;
CREATE TABLE library_metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE original_files(
  id TEXT PRIMARY KEY,
  relative_path TEXT NOT NULL UNIQUE,
  kind TEXT NOT NULL CHECK(kind IN ('raw','jpeg')),
  size INTEGER NOT NULL CHECK(size >= 0),
  mtime_ms REAL NOT NULL CHECK(mtime_ms >= 0),
  available INTEGER NOT NULL CHECK(available IN (0,1)),
  error_category TEXT CHECK(error_category IS NULL OR error_category IN ('unreadable','changed')),
  error_message TEXT CHECK(error_message IS NULL OR length(error_message) <= 120),
  capture_metadata_state TEXT NOT NULL DEFAULT 'pending'
    CHECK(capture_metadata_state IN ('pending','known','missing','invalid','failed')),
  capture_order_key TEXT CHECK(capture_order_key IS NULL OR (
    length(capture_order_key)=29 AND substr(capture_order_key,5,1)='-' AND
    substr(capture_order_key,8,1)='-' AND substr(capture_order_key,11,1)='T' AND
    substr(capture_order_key,14,1)=':' AND substr(capture_order_key,17,1)=':' AND
    substr(capture_order_key,20,1)='.' AND
    replace(replace(replace(replace(capture_order_key,'-',''),':',''),'T',''),'.','')
      NOT GLOB '*[^0-9]*'
  )),
  capture_time_field TEXT CHECK(capture_time_field IS NULL OR capture_time_field IN ('date-time-original','date-time-digitized')),
  capture_offset_minutes INTEGER CHECK(capture_offset_minutes IS NULL OR capture_offset_minutes BETWEEN -840 AND 840),
  capture_source_revision TEXT
);
CREATE TABLE photos(
  id TEXT PRIMARY KEY,
  raw_original_id TEXT REFERENCES original_files(id),
  jpeg_original_id TEXT REFERENCES original_files(id),
  ambiguous INTEGER NOT NULL CHECK(ambiguous IN (0,1)),
  available INTEGER NOT NULL CHECK(available IN (0,1)),
  preview_state TEXT NOT NULL CHECK(preview_state IN ('inspection-pending','ready','failed','unavailable')),
  preview_candidate TEXT CHECK(preview_candidate IS NULL OR preview_candidate IN ('matching-jpeg','embedded-raw-jpeg')),
  preview_source TEXT CHECK(preview_source IS NULL OR preview_source IN ('matching-jpeg','embedded-raw-jpeg')),
  preview_source_revision TEXT,
  preview_width INTEGER CHECK(preview_width IS NULL OR preview_width > 0),
  preview_height INTEGER CHECK(preview_height IS NULL OR preview_height > 0),
  cache_revision TEXT,
  sort_path TEXT NOT NULL,
  selection_state TEXT NOT NULL DEFAULT 'undecided' CHECK(selection_state IN ('undecided','selected','rejected')),
  rating INTEGER NOT NULL DEFAULT 0 CHECK(rating BETWEEN 0 AND 5)
);
CREATE INDEX photos_raw ON photos(raw_original_id);
CREATE INDEX photos_jpeg ON photos(jpeg_original_id);
CREATE TABLE photo_sets(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE COLLATE NOCASE CHECK(length(name) BETWEEN 1 AND 120),
  created_at INTEGER NOT NULL
);
CREATE TABLE photo_set_members(
  photo_set_id TEXT NOT NULL REFERENCES photo_sets(id) ON DELETE CASCADE,
  photo_id TEXT NOT NULL REFERENCES photos(id) ON DELETE RESTRICT,
  position INTEGER NOT NULL CHECK(position >= 0),
  PRIMARY KEY(photo_set_id, photo_id),
  UNIQUE(photo_set_id, position)
);
CREATE TABLE review_progress(
  photo_set_id TEXT PRIMARY KEY REFERENCES photo_sets(id) ON DELETE CASCADE,
  photo_id TEXT NOT NULL,
  FOREIGN KEY(photo_set_id, photo_id) REFERENCES photo_set_members(photo_set_id, photo_id) ON DELETE CASCADE
);
CREATE INDEX photo_set_members_photo ON photo_set_members(photo_id);
PRAGMA user_version = 4;
