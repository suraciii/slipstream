CREATE TABLE library_metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE original_files(
  id TEXT PRIMARY KEY,
  relative_path TEXT UNIQUE,
  kind TEXT,
  size INTEGER,
  mtime_ms REAL,
  available INTEGER,
  inspection_error TEXT
);
CREATE TABLE photos(
  id TEXT PRIMARY KEY,
  raw_original_id TEXT,
  jpeg_original_id TEXT,
  ambiguous INTEGER,
  available INTEGER,
  preview_state TEXT,
  preview_source TEXT,
  sort_path TEXT
);
INSERT INTO original_files VALUES('legacy-original', 'legacy.jpg', 'jpeg', 6, 0, 1, NULL);
INSERT INTO photos VALUES('legacy-photo', NULL, 'legacy-original', 0, 1, 'inspection-pending', NULL, 'legacy.jpg');
