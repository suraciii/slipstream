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
  original_id TEXT NOT NULL UNIQUE REFERENCES original_files(id) ON DELETE RESTRICT,
  available INTEGER NOT NULL CHECK(available IN (0,1)),
  preview_state TEXT NOT NULL CHECK(preview_state IN ('inspection-pending','ready','failed','unavailable')),
  preview_source_revision TEXT,
  preview_width INTEGER CHECK(preview_width IS NULL OR preview_width > 0),
  preview_height INTEGER CHECK(preview_height IS NULL OR preview_height > 0),
  cache_revision TEXT,
  sort_path TEXT NOT NULL,
  selection_state TEXT NOT NULL DEFAULT 'undecided' CHECK(selection_state IN ('undecided','selected','rejected')),
  rating INTEGER NOT NULL DEFAULT 0 CHECK(rating BETWEEN 0 AND 5)
);
CREATE INDEX photos_original ON photos(original_id);
CREATE TABLE original_fingerprints(
  original_id TEXT PRIMARY KEY REFERENCES original_files(id) ON DELETE CASCADE,
  digest TEXT NOT NULL CHECK(length(digest) = 64),
  size INTEGER NOT NULL CHECK(size >= 0),
  mtime_ms REAL NOT NULL CHECK(mtime_ms >= 0)
);
CREATE INDEX original_fingerprints_digest ON original_fingerprints(digest);
CREATE TABLE albums(
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE COLLATE NOCASE CHECK(length(name) BETWEEN 1 AND 120),
  created_at INTEGER NOT NULL
);
CREATE TABLE album_members(
  album_id TEXT NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
  photo_id TEXT NOT NULL REFERENCES photos(id) ON DELETE RESTRICT,
  position INTEGER NOT NULL CHECK(position >= 0),
  PRIMARY KEY(album_id, photo_id),
  UNIQUE(album_id, position)
);
CREATE TABLE album_progress(
  album_id TEXT PRIMARY KEY REFERENCES albums(id) ON DELETE CASCADE,
  photo_id TEXT NOT NULL,
  FOREIGN KEY(album_id, photo_id) REFERENCES album_members(album_id, photo_id) ON DELETE CASCADE
);
CREATE INDEX album_members_photo ON album_members(photo_id);
CREATE TABLE edit_recipes(
  photo_id TEXT PRIMARY KEY REFERENCES photos(id) ON DELETE RESTRICT,
  revision TEXT NOT NULL CHECK(length(revision) > 0),
  source_revision TEXT NOT NULL CHECK(length(source_revision) > 0),
  exposure_ev REAL NOT NULL CHECK(exposure_ev BETWEEN -1.7976931348623157e308 AND 1.7976931348623157e308),
  white_balance_mode TEXT NOT NULL CHECK(white_balance_mode IN ('as-shot','temperature-tint')),
  temperature_kelvin INTEGER CHECK(temperature_kelvin IS NULL OR temperature_kelvin BETWEEN 1000 AND 40000),
  tint_milli INTEGER CHECK(tint_milli IS NULL OR tint_milli BETWEEN -150000 AND 150000)
);
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
PRAGMA user_version = 8;
