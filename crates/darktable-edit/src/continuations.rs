use crate::{Invocation, ToolError};
use photo_files::Directory;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use std::{os::unix::fs::MetadataExt, path::Path, time::Duration};

const MAX_BYTES: usize = 64 * 1024 * 1024;
fn error(code: &str, message: impl Into<String>) -> ToolError {
    let exit = if code == "cursor_expired" { 6 } else { 2 };
    let error = ToolError::new(code, message, exit);
    // Only the expired continuation discloses the route's cursor kind and the
    // reason its retained page is gone; malformed cursors are invalid input.
    if code == "cursor_expired" {
        error.detail(json!({"cursorKind": "darktable", "reason": "idle_or_evicted"}))
    } else {
        error
    }
}
fn io(e: impl std::fmt::Display) -> ToolError {
    ToolError::new("cursor_io", e.to_string(), 6)
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn cache() -> Result<(Directory, Connection), ToolError> {
    let root = if let Some(path) = std::env::var_os("XDG_CACHE_HOME") {
        std::path::PathBuf::from(path)
    } else {
        std::path::PathBuf::from(std::env::var_os("HOME").ok_or_else(|| io("HOME unavailable"))?)
            .join(".cache")
    };
    let dir = Directory::create(root.join("darktable-edit/continuations")).map_err(io)?;
    let metadata = dir.descriptor().metadata().map_err(io)?;
    if metadata.uid() != unsafe { libc::geteuid() } {
        return Err(io("Continuation cache is not owned by this user"));
    }
    if metadata.mode() & 0o077 != 0 {
        dir.descriptor()
            .set_permissions(std::fs::Permissions::from_mode(0o700))
            .map_err(io)?;
    }
    match dir.create_file("pages.sqlite3", 0o600) {
        Ok(file) => file.sync_all().map_err(io)?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let file = dir.open_file("pages.sqlite3").map_err(io)?;
            let m = file.metadata().map_err(io)?;
            if m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o077 != 0 {
                return Err(io("Continuation cache database must be private"));
            }
        }
        Err(e) => return Err(io(e)),
    }
    let db = Connection::open(dir.path().join("pages.sqlite3")).map_err(io)?;
    db.busy_timeout(Duration::from_secs(5)).map_err(io)?;
    db.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS pages(token TEXT PRIMARY KEY, operation TEXT NOT NULL, state TEXT, items TEXT NOT NULL, response TEXT NOT NULL, offset INTEGER NOT NULL, page_limit INTEGER NOT NULL, touched INTEGER NOT NULL);").map_err(io)?;
    Ok((dir, db))
}
use std::os::unix::fs::PermissionsExt;

pub(crate) fn window(
    items: Vec<Value>,
    args: &Value,
    operation: &str,
    state: Option<&Path>,
    mut response: Value,
) -> Result<Value, ToolError> {
    let limit = args["limit"].as_u64().unwrap_or(50) as usize;
    if !(1..=60).contains(&limit) {
        return Err(error("invalid_input", "limit must be 1–60"));
    }
    let total = items.len();
    let end = limit.min(total);
    response["items"] = json!(&items[..end]);
    response["total"] = json!(total);
    response["nextCursor"] = Value::Null;
    if end < total {
        let encoded = serde_json::to_string(&items).map_err(io)?;
        let metadata = serde_json::to_string(&response).map_err(io)?;
        if encoded.len() + metadata.len() > MAX_BYTES {
            return Err(error(
                "limit_exceeded",
                "Captured continuation exceeds retained byte bound",
            ));
        }
        let (_dir, mut db) = cache()?;
        let tx = db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(io)?;
        tx.execute("DELETE FROM pages WHERE touched <= ?", [now() - 900])
            .map_err(io)?;
        loop {
            let (count, bytes): (i64, i64) = tx
                .query_row(
                    "SELECT count(*),coalesce(sum(length(items)+length(response)),0) FROM pages",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(io)?;
            let retained = usize::try_from(bytes).map_err(io)?;
            if count < 256 && retained + encoded.len() + metadata.len() <= MAX_BYTES {
                break;
            }
            tx.execute("DELETE FROM pages WHERE token=(SELECT token FROM pages ORDER BY touched,token LIMIT 1)", []).map_err(io)?;
        }
        let token = photo_files::identity("cursor").map_err(io)?;
        let state = state
            .map(|path| {
                let dir = Directory::open(path).map_err(io)?;
                let m = dir.descriptor().metadata().map_err(io)?;
                serde_json::to_string(&json!({"path":path,"device":m.dev(),"inode":m.ino()}))
                    .map_err(io)
            })
            .transpose()?;
        tx.execute(
            "INSERT INTO pages VALUES (?,?,?,?,?,?,?,?)",
            params![
                token,
                operation,
                state,
                encoded,
                metadata,
                end as i64,
                limit as i64,
                now()
            ],
        )
        .map_err(io)?;
        tx.commit().map_err(io)?;
        response["nextCursor"] = json!(token);
    }
    Ok(response)
}

pub(crate) fn resume(invocation: &Invocation) -> Result<Value, ToolError> {
    let args = invocation
        .arguments
        .as_object()
        .ok_or_else(|| error("invalid_cursor", "Cursor arguments must be an object"))?;
    if args.len() != 1 {
        return Err(error(
            "invalid_cursor",
            "Cursor continuation accepts no filters or target arguments",
        ));
    }
    let token = args["cursor"]
        .as_str()
        .ok_or_else(|| error("invalid_cursor", "Cursor must be a string"))?;
    if token.len() > 128
        || !token
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    {
        return Err(error("invalid_cursor", "Invalid cursor"));
    }
    let (_dir, mut db) = cache()?;
    let tx = db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(io)?;
    let row = tx.query_row("SELECT operation,state,items,response,offset,page_limit,touched FROM pages WHERE token=?", [token], |r| Ok((r.get::<_, String>(0)?,r.get::<_, Option<String>>(1)?,r.get::<_, String>(2)?,r.get::<_, String>(3)?,r.get::<_, i64>(4)?,r.get::<_, i64>(5)?,r.get::<_, i64>(6)?))).optional().map_err(io)?;
    let (operation, state, items, response, offset, limit, touched) =
        row.ok_or_else(|| error("cursor_expired", "Cursor expired or was evicted"))?;
    let offset = usize::try_from(offset).map_err(io)?;
    let limit = usize::try_from(limit).map_err(io)?;
    if touched <= now() - 900 {
        return Err(error(
            "cursor_expired",
            "Cursor expired after 15 minutes idle",
        ));
    }
    if operation != invocation.operation {
        return Err(error(
            "invalid_cursor",
            "Cursor belongs to another collection",
        ));
    }
    let binding: Option<Value> = state
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(io)?;
    let retained_dir = if let Some(binding) = &binding {
        let path = binding["path"]
            .as_str()
            .ok_or_else(|| io("Invalid retained scope"))?;
        if invocation
            .state_directory
            .as_ref()
            .is_some_and(|p| p != Path::new(path))
        {
            return Err(error("invalid_cursor", "Cursor belongs to another state"));
        }
        let dir = Directory::open(path).map_err(io)?;
        let m = dir.descriptor().metadata().map_err(io)?;
        if binding["device"] != json!(m.dev()) || binding["inode"] != json!(m.ino()) {
            return Err(error(
                "invalid_cursor",
                "Retained state directory identity changed",
            ));
        }
        Some(dir)
    } else {
        if invocation.state_directory.is_some() {
            return Err(error("invalid_cursor", "Cursor has global scope"));
        }
        None
    };
    let _ancestors = retained_dir
        .as_ref()
        .map(|d| photo_files::ancestor_leases(d.path(), Duration::from_secs(5)).map_err(io))
        .transpose()?;
    let _lifecycle = retained_dir
        .as_ref()
        .map(|d| {
            d.lease(photo_files::LeaseMode::Shared, Duration::from_secs(5))
                .map_err(io)
        })
        .transpose()?;
    let _state_lock = retained_dir
        .as_ref()
        .map(|d| {
            let filename = if operation.starts_with("presets ")
                || operation.starts_with("styles ")
                || operation.starts_with("resources ")
            {
                "catalog.lock"
            } else {
                "state.lock"
            };
            let file = match d.create_file(filename, 0o600) {
                Ok(file) => file,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    d.open_file(filename).map_err(io)?
                }
                Err(e) => return Err(io(e)),
            };
            photo_files::FileLease::acquire(file, photo_files::LeaseMode::Shared, Duration::ZERO)
                .map_err(io)
        })
        .transpose()?;
    let items: Vec<Value> = serde_json::from_str(&items).map_err(io)?;
    let mut response: Value = serde_json::from_str(&response).map_err(io)?;
    if offset > items.len() || !(1..=60).contains(&limit) {
        return Err(io("Invalid retained continuation"));
    }
    let end = (offset + limit).min(items.len());
    response["items"] = json!(&items[offset..end]);
    response["nextCursor"] = if end < items.len() {
        json!(token)
    } else {
        Value::Null
    };
    if end < items.len() {
        tx.execute(
            "UPDATE pages SET offset=?,touched=? WHERE token=?",
            params![end as i64, now(), token],
        )
        .map_err(io)?;
    } else {
        tx.execute("DELETE FROM pages WHERE token=?", [token])
            .map_err(io)?;
    }
    tx.commit().map_err(io)?;
    Ok(response)
}
