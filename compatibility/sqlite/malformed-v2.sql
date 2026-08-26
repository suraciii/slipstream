CREATE TABLE library_metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE original_files(id TEXT PRIMARY KEY);
CREATE TABLE photos(id TEXT PRIMARY KEY);
PRAGMA user_version = 2;
