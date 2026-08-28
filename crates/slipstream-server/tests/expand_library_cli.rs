use rusqlite::{Connection, params};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

struct TempTree(PathBuf);

impl TempTree {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "slipstream-expand-cli-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn expand_library_command_updates_binding_and_location_then_scans() {
    let base = TempTree::new();
    let proposed = base.path().join("photos");
    let current = proposed.join("shoot");
    let state = base.path().join("state");
    let cache = base.path().join("cache");
    fs::create_dir(&proposed).unwrap();
    fs::create_dir(&current).unwrap();
    fs::create_dir(&state).unwrap();
    fs::create_dir(&cache).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&cache, fs::Permissions::from_mode(0o700)).unwrap();
    let original = current.join("a.ARW");
    fs::write(&original, b"camera-owned-bytes").unwrap();

    let database = state.join("library.sqlite");
    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch(include_str!("../../../compatibility/sqlite/schema-v4.sql"))
        .unwrap();
    connection
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
            [current.to_str().unwrap()],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,capture_metadata_state) VALUES('legacy-original','a.ARW','raw',18,1,1,'pending')",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO photos(id,raw_original_id,ambiguous,available,preview_state,sort_path,selection_state,rating) VALUES('legacy-photo','legacy-original',0,1,'inspection-pending','a.ARW','selected',4)",
            [],
        )
        .unwrap();
    drop(connection);

    let output = Command::new(env!("CARGO_BIN_EXE_slipstream-server"))
        .arg("expand-library")
        .env("SLIPSTREAM_LIBRARY_ROOT", &proposed)
        .env("SLIPSTREAM_STATE_DIRECTORY", &state)
        .env("SLIPSTREAM_CACHE_DIRECTORY", &cache)
        .env("SLIPSTREAM_DATABASE_BASENAME", "library.sqlite")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "Slipstream Library expansion completed\n"
    );

    let connection = Connection::open(&database).unwrap();
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
            .unwrap(),
        4
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT value FROM library_metadata WHERE key='canonical_root'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        proposed.to_str().unwrap()
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT relative_path FROM original_files WHERE id='legacy-original'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "shoot/a.ARW"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT selection_state,rating FROM photos WHERE id='legacy-photo'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, u8>(1)?)),
            )
            .unwrap(),
        ("selected".to_owned(), 4)
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM original_files WHERE id=? AND relative_path=?",
                params!["legacy-original", "shoot/a.ARW"],
                |row| row.get::<_, u32>(0),
            )
            .unwrap(),
        1
    );
    drop(connection);
    for suffix in ["-journal", "-wal", "-shm"] {
        assert!(!PathBuf::from(format!("{}{}", database.display(), suffix)).exists());
    }
    assert_eq!(fs::read(original).unwrap(), b"camera-owned-bytes");
}
