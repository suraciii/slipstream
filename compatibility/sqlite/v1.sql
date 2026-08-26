CREATE TABLE library_metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE original_files(
  id TEXT PRIMARY KEY,
  relative_path TEXT NOT NULL UNIQUE,
  kind TEXT NOT NULL CHECK(kind IN ('raw','jpeg')),
  size INTEGER NOT NULL CHECK(size >= 0),
  mtime_ms REAL NOT NULL CHECK(mtime_ms >= 0),
  available INTEGER NOT NULL CHECK(available IN (0,1)),
  error_category TEXT CHECK(error_category IS NULL OR error_category IN ('unreadable','changed')),
  error_message TEXT CHECK(error_message IS NULL OR length(error_message) <= 120)
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
  sort_path TEXT NOT NULL
);
CREATE INDEX photos_raw ON photos(raw_original_id);
CREATE INDEX photos_jpeg ON photos(jpeg_original_id);
INSERT INTO original_files VALUES('v1-original', 'v1.jpg', 'jpeg', 6, 0, 1, NULL, NULL);
INSERT INTO photos VALUES('v1-photo', NULL, 'v1-original', 0, 1, 'inspection-pending', 'matching-jpeg', NULL, NULL, NULL, NULL, NULL, 'v1.jpg');
PRAGMA user_version = 1;
