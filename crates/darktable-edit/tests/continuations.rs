#![cfg(feature = "runtime")]
use darktable_edit::catalog;
use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{path::Path, process::Command};

fn command(root: &Path, arguments: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_darktable-edit"))
        .args(arguments)
        .env("HOME", root)
        .env("XDG_CACHE_HOME", root.join("cache"))
        .env("DARKTABLE_EDIT_BUNDLE", root.join("missing-bundle"))
        .output()
        .unwrap();
    serde_json::from_slice(&output.stdout).unwrap_or_else(|_| {
        panic!(
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}
#[test]
fn catalog_cursor_continues_in_fresh_command_with_pinned_scope_and_records() {
    let root = std::env::temp_dir().join(photo_files::identity("cursor-process-test").unwrap());
    let state = root.join("state");
    let descriptor = catalog::initialize(&state).unwrap();
    let db = Connection::open(state.join("catalog.sqlite3")).unwrap();
    for id in ["a", "b", "c"] {
        db.execute("INSERT INTO records VALUES(?,?,?,?,NULL)", params![id,"v1","preset",json!({"id":id,"version":"v1","kind":"preset","name":id,"bundleId":"bundle","schemaVersion":"schema","createdAt":"2026-10-11T00:00:00Z","availability":{"read":true}}).to_string()]).unwrap();
    }
    let first = command(
        &root,
        &[
            "presets",
            "list",
            "--state",
            state.to_str().unwrap(),
            "--limit",
            "1",
        ],
    );
    assert_eq!(first["status"], "ok");
    assert_eq!(first["data"]["items"][0]["id"], "a");
    let token = first["data"]["nextCursor"].as_str().unwrap();
    db.execute("DELETE FROM records WHERE id='b'", []).unwrap();
    let misuse = command(&root, &["styles", "list", "--cursor", token]);
    assert_eq!(misuse["error"]["code"], "invalid_cursor");
    let second = command(&root, &["presets", "list", "--cursor", token]);
    assert_eq!(second["status"], "ok");
    assert_eq!(second["data"]["items"][0]["id"], "b");
    assert_eq!(second["data"]["catalog"], descriptor);
    let retained =
        Connection::open(root.join("cache/darktable-edit/continuations/pages.sqlite3")).unwrap();
    retained
        .execute("UPDATE pages SET touched=0 WHERE token=?", [token])
        .unwrap();
    let expired = command(&root, &["presets", "list", "--cursor", token]);
    assert_eq!(expired["error"]["code"], "cursor_expired");
    assert_eq!(
        expired["error"]["details"],
        json!({"cursorKind": "darktable", "reason": "idle_or_evicted"}),
        "an expired darktable continuation discloses its route and reason"
    );
    // A well-formed token whose retained page is gone is an expired cursor
    // carrying its route facts; a malformed one is invalid input.
    let unknown = command(&root, &["presets", "list", "--cursor", "absent-cursor"]);
    assert_eq!(unknown["error"]["code"], "cursor_expired");
    assert_eq!(
        unknown["error"]["details"],
        json!({"cursorKind": "darktable", "reason": "idle_or_evicted"}),
    );
    assert_eq!(
        command(&root, &["presets", "list", "--cursor", "!!"])["error"]["code"],
        "invalid_input"
    );
    drop(retained);
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn state_windows_continue_captured_revision_in_separate_commands() {
    let root =
        std::env::temp_dir().join(photo_files::identity("state-cursor-process-test").unwrap());
    let state = root.join("state");
    photo_files::Directory::create(&state)
        .unwrap()
        .create_file("state.lock", 0o600)
        .unwrap();
    let db = Connection::open(state.join("state.sqlite3")).unwrap();
    db.execute_batch("CREATE TABLE metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL); CREATE TABLE revisions(id TEXT PRIMARY KEY,parent TEXT,created TEXT NOT NULL,request TEXT,snapshot TEXT NOT NULL,input TEXT NOT NULL,bundle TEXT NOT NULL,schema TEXT NOT NULL,dependencies TEXT NOT NULL,targets TEXT NOT NULL);").unwrap();
    db.execute_batch("CREATE TABLE captures(id TEXT,revision TEXT,result TEXT);")
        .unwrap();
    catalog::initialize(&state).unwrap();
    for (key, value) in [
        ("identity", json!("test-state")),
        ("current", json!("r2")),
        ("baseline", json!("r1")),
        (
            "discovery",
            json!({"modules":[{"moduleId":"a"},{"moduleId":"b"}]}),
        ),
    ] {
        db.execute(
            "INSERT INTO metadata VALUES (?,?)",
            params![key, value.to_string()],
        )
        .unwrap();
    }
    for revision in ["r1", "r2"] {
        db.execute("INSERT INTO revisions VALUES (?,NULL,'2026-10-11T00:00:00Z',NULL,?,'{}','bundle','schema','[]','[]')",params![revision,json!({"instances":[{"instanceId":"a"},{"instanceId":"b"}],"masks":[{"maskId":"a"},{"maskId":"b"}],"actions":[{"actionId":"a"},{"actionId":"b"}]}).to_string()]).unwrap();
    }
    for (initial, continuation, key, next) in [
        (
            vec![
                "edit",
                "instances",
                "list",
                state.to_str().unwrap(),
                "--limit",
                "1",
            ],
            vec!["edit", "instances", "list"],
            "instanceId",
            "b",
        ),
        (
            vec![
                "edit",
                "masks",
                "list",
                state.to_str().unwrap(),
                "--limit",
                "1",
            ],
            vec!["edit", "masks", "list"],
            "maskId",
            "b",
        ),
        (
            vec![
                "edit",
                "actions",
                "list",
                state.to_str().unwrap(),
                "--limit",
                "1",
            ],
            vec!["edit", "actions", "list"],
            "actionId",
            "b",
        ),
        (
            vec!["history", "list", state.to_str().unwrap(), "--limit", "1"],
            vec!["history", "list"],
            "revision",
            "r1",
        ),
        (
            vec![
                "modules",
                "list",
                "--state",
                state.to_str().unwrap(),
                "--limit",
                "1",
            ],
            vec!["modules", "list"],
            "moduleId",
            "b",
        ),
    ] {
        let first = command(&root, &initial);
        assert_eq!(first["status"], "ok", "{first}");
        let token = first["data"]["nextCursor"].as_str().unwrap();
        let mut args = continuation;
        args.extend(["--cursor", token]);
        let second = command(&root, &args);
        assert_eq!(second["status"], "ok", "{second}");
        assert_eq!(second["data"]["items"][0][key], next);
        assert!(second["data"]["nextCursor"].is_null());
    }
    let instances: Vec<Value> = (0..61)
        .map(|i| json!({"instanceId":format!("instance-{i}")}))
        .collect();
    db.execute(
        "UPDATE revisions SET snapshot=?,input=? WHERE id='r2'",
        params![
            json!({"instances":instances,"masks":[],"actions":[]}).to_string(),
            json!({"path":root.join("missing-input.raw")}).to_string()
        ],
    )
    .unwrap();
    let view = command(&root, &["edit", "show", state.to_str().unwrap()]);
    assert_eq!(view["status"], "ok", "{view}");
    assert_eq!(
        view["data"]["instances"]["items"].as_array().unwrap().len(),
        60
    );
    let token = view["data"]["instances"]["nextCursor"].as_str().unwrap();
    let next = command(&root, &["edit", "instances", "list", "--cursor", token]);
    assert_eq!(next["status"], "ok", "{next}");
    assert_eq!(next["data"]["items"][0]["instanceId"], "instance-60");
    drop(db);
    std::fs::remove_dir_all(root).unwrap();
}
