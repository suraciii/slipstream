use crate::{
    engine::Engine,
    protocol::{Invocation, ToolError},
};
use photo_files::{Directory, LeaseMode, PendingFile};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};
mod application;
mod cursors;
mod definitions;
mod resources;
#[cfg(test)]
mod tests;
const MAX_DEFINITION: usize = 8 * 1024 * 1024;
const MAX_RESOURCE: usize = 64 * 1024 * 1024;
pub(super) fn error(code: &str, message: impl Into<String>) -> ToolError {
    let exit = match code {
        "catalog_not_found" | "catalog_record_not_found" | "instance_not_found" => 3,
        "resource_in_use"
        | "request_conflict"
        | "state_conflict"
        | "incompatible_geometry"
        | "file_exists"
        | "state_busy"
        | "state_guard_required" => 4,
        "catalog_io" | "resource_corrupt" | "native_failure" | "receipt_expired"
        | "cursor_expired" => 6,
        "outcome_unknown" => 7,
        _ => 2,
    };
    ToolError::new(code, message, exit)
}
pub(super) fn io(e: impl std::fmt::Display) -> ToolError {
    let message = e.to_string();
    let code = if message.contains("File exists") {
        "file_exists"
    } else if message.contains("lock acquisition timed out") {
        "state_busy"
    } else {
        "catalog_io"
    };
    error(code, message)
}
pub(super) fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub(super) fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| error("invalid_request", format!("{key} must be a string")))
}
pub(super) fn keys(v: &Value, allowed: &[&str]) -> Result<(), ToolError> {
    let o = v
        .as_object()
        .ok_or_else(|| error("invalid_request", "expected object"))?;
    if let Some(k) = o.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(error("invalid_request", format!("unknown field {k}")));
    }
    Ok(())
}
pub(super) fn array<'a>(v: &'a Value, key: &str) -> Result<&'a Vec<Value>, ToolError> {
    v.get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| error("invalid_request", format!("{key} must be an array")))
}
pub(super) fn available() -> Value {
    json!({"read":true,"write":true,"execute":true,"reason":null,"recovery":null})
}
pub(super) struct Catalog {
    dir: Directory,
    db: Connection,
    descriptor: Value,
    _lifecycle: photo_files::FileLease,
    _ancestors: Vec<photo_files::FileLease>,
}
fn absolute(path: &Path) -> Result<PathBuf, ToolError> {
    if path.is_absolute() {
        Ok(path.into())
    } else {
        Ok(std::env::current_dir().map_err(io)?.join(path))
    }
}
fn location(state: Option<&Path>, scope: &str) -> Result<(PathBuf, &'static str), ToolError> {
    match scope {
        "local" => Ok((
            absolute(state.ok_or_else(|| {
                error(
                    "catalog_not_found",
                    "local catalog requires state directory",
                )
            })?)?,
            "edit-local",
        )),
        "global" => {
            if let Some(path) = std::env::var_os("DARKTABLE_EDIT_CATALOG") {
                Ok((absolute(Path::new(&path))?, "configured"))
            } else {
                let home = std::env::var_os("HOME").ok_or_else(|| {
                    error("catalog_not_found", "user catalog location unavailable")
                })?;
                Ok((
                    PathBuf::from(home).join(".local/share/darktable-edit/catalog"),
                    "user",
                ))
            }
        }
        _ => Err(error("invalid_request", "catalog must be local or global")),
    }
}
impl Catalog {
    fn open(state: Option<&Path>, scope: &str, create: bool) -> Result<Self, ToolError> {
        let (path, kind) = location(state, scope)?;
        let dir = if create {
            Directory::create(&path)
        } else {
            Directory::open(&path)
        }
        .map_err(|e| error("catalog_not_found", e.to_string()))?;
        let ancestors = photo_files::ancestor_leases(&path, Duration::from_secs(3)).map_err(io)?;
        let lifecycle = dir
            .lease(LeaseMode::Shared, Duration::from_secs(3))
            .map_err(io)?;
        let db_path = dir.path().join("catalog.sqlite3");
        if !create {
            dir.open_file("catalog.sqlite3").map_err(|_| {
                error(
                    "catalog_not_found",
                    "selected catalog has not been initialized",
                )
            })?;
        } else if dir.open_file("catalog.sqlite3").is_err() {
            match dir.create_file("catalog.sqlite3", 0o600) {
                Ok(_) => (),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(io(e)),
            }
        }
        let db = Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(io)?;
        db.busy_timeout(Duration::from_secs(3)).map_err(io)?;
        if create {
            db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS metadata(singleton INTEGER PRIMARY KEY CHECK(singleton=1),id TEXT NOT NULL); CREATE TABLE IF NOT EXISTS records(id TEXT NOT NULL,version TEXT NOT NULL,kind TEXT NOT NULL,record TEXT NOT NULL,bytes BLOB,PRIMARY KEY(id,version)); CREATE TABLE IF NOT EXISTS receipts(request TEXT PRIMARY KEY,intent TEXT NOT NULL,result TEXT NOT NULL,created INTEGER NOT NULL); CREATE TABLE IF NOT EXISTS pins(id TEXT NOT NULL,version TEXT NOT NULL,record TEXT NOT NULL,bytes BLOB NOT NULL,PRIMARY KEY(id,version));").map_err(io)?;
        }
        let id: Option<String> = db
            .query_row("SELECT id FROM metadata LIMIT 1", [], |r| r.get(0))
            .optional()
            .map_err(io)?;
        let id = match id {
            Some(id) => id,
            None if create => {
                let proposed = photo_files::identity("catalog").map_err(io)?;
                db.execute(
                    "INSERT OR IGNORE INTO metadata(singleton,id) VALUES(1,?)",
                    [&proposed],
                )
                .map_err(io)?;
                db.query_row("SELECT id FROM metadata WHERE singleton=1", [], |r| {
                    r.get::<_, String>(0)
                })
                .map_err(io)?
            }
            None => return Err(error("catalog_not_found", "catalog identity missing")),
        };
        let descriptor = json!({"catalogId":id,"scope":kind,"executionHost":std::env::var("HOSTNAME").unwrap_or_else(|_|"localhost".into()),"stateDirectory":if kind=="edit-local" {Some(path.to_string_lossy().into_owned())}else{None}});
        Ok(Self {
            dir,
            db,
            descriptor,
            _lifecycle: lifecycle,
            _ancestors: ancestors,
        })
    }
    pub(super) fn record(&self, id: &str, version: &str, kind: &str) -> Result<Value, ToolError> {
        let s: Option<String> = self
            .db
            .query_row(
                "SELECT record FROM records WHERE id=? AND version=? AND kind=?",
                params![id, version, kind],
                |r| r.get(0),
            )
            .optional()
            .map_err(io)?;
        s.map(|s|serde_json::from_str(&s).map_err(io)).transpose()?.ok_or_else(||error("catalog_record_not_found","record/version absent from selected catalog; select intended scope or import record").detail(json!({"catalog":self.descriptor,"id":id,"version":version})))
    }
    pub(super) fn resource(&self, id: &str, version: &str) -> Result<(Value, Vec<u8>), ToolError> {
        let row:Option<(String,Vec<u8>)>=self.db.query_row("SELECT record,bytes FROM records WHERE id=? AND version=? AND kind='resource' UNION ALL SELECT record,bytes FROM pins WHERE id=? AND version=? LIMIT 1",params![id,version,id,version],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(io)?;
        let (record, bytes) = row.ok_or_else(|| {
            error(
                "catalog_record_not_found",
                "resource version absent in selected catalog",
            )
            .detail(json!({"catalog":self.descriptor,"resourceId":id,"version":version}))
        })?;
        let record: Value = serde_json::from_str(&record).map_err(io)?;
        if record["sha256"] != digest(&bytes) {
            return Err(error(
                "resource_corrupt",
                "immutable resource digest mismatch",
            ));
        }
        Ok((record, bytes))
    }
}
pub fn initialize(state: &Path) -> Result<Value, ToolError> {
    Ok(Catalog::open(Some(state), "local", true)?.descriptor)
}
pub fn describe(state: Option<&Path>, scope: &str) -> Result<Value, ToolError> {
    Ok(Catalog::open(state, scope, false)?.descriptor)
}
pub fn dispatch(invocation: &Invocation, edit: Option<&Value>) -> Result<Value, ToolError> {
    let mut words = invocation.operation.split_whitespace();
    let family = words.next().unwrap_or("");
    let operation = words.next().unwrap_or("");
    let args = &invocation.arguments;
    if args.get("cursor").is_some() {
        return crate::continuations::resume(invocation);
    }
    let state = invocation.state_directory.as_deref();
    let scope = if operation == "save" {
        args.get("catalog")
            .and_then(Value::as_str)
            .unwrap_or("local")
    } else if state.is_some() {
        "local"
    } else {
        "global"
    };
    let create = matches!(operation, "import" | "save");
    let mut catalog = Catalog::open(state, scope, create)?;
    let _lease = if matches!(operation, "import" | "save" | "remove") {
        let lock = match catalog.dir.create_file("catalog.lock", 0o600) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                catalog.dir.open_file("catalog.lock").map_err(io)?
            }
            Err(e) => return Err(io(e)),
        };
        Some(
            photo_files::FileLease::acquire(lock, LeaseMode::Exclusive, Duration::from_secs(3))
                .map_err(io)?,
        )
    } else {
        None
    };
    let kind = match family {
        "resources" => "resource",
        "presets" => "preset",
        "styles" => "style",
        _ => return Err(error("invalid_request", "unknown catalog family")),
    };
    if operation == "kinds" {
        let discovery = Engine::open()?.discover()?;
        let mut items = resources::kinds().as_array().unwrap().clone();
        if let Some(native) = discovery.get("resourceKinds").and_then(Value::as_array) {
            for kind in native {
                if !items.iter().any(|item| item["kind"] == kind["kind"]) {
                    items.push(kind.clone());
                }
            }
        }
        return Ok(
            json!({"catalog":catalog.descriptor,"items":items,"maskGraphSchema":discovery["maskGraphSchema"],"maximumDefinitionBytes":MAX_DEFINITION}),
        );
    }
    if operation == "list" {
        return list(&catalog, args, kind);
    }
    let id_key = match kind {
        "resource" => "resourceId",
        "preset" => "presetId",
        _ => "styleId",
    };
    if operation == "show" {
        let record = catalog.record(text(args, id_key)?, text(args, "version")?, kind)?;
        let mut record = record;
        if kind == "resource" {
            record["references"] =
                reference_counts(&catalog, state, text(args, id_key)?, text(args, "version")?)?;
        }
        return Ok(json!({"catalog":catalog.descriptor,"record":record}));
    }
    if operation == "export" {
        let record = catalog.record(text(args, id_key)?, text(args, "version")?, kind)?;
        let bytes = serde_json::to_vec(&json!({"schemaVersion":1,"kind":kind,"record":record}))
            .map_err(io)?;
        let path = absolute(Path::new(text(args, "file")?))?;
        let parent = Directory::open(
            path.parent()
                .ok_or_else(|| error("invalid_request", "file parent missing"))?,
        )
        .map_err(io)?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| error("invalid_request", "invalid filename"))?;
        let mut pending = PendingFile::create(&parent).map_err(io)?;
        pending.file_mut().write_all(&bytes).map_err(io)?;
        pending.publish(name, MAX_DEFINITION as u64).map_err(io)?;
        return Ok(
            json!({"catalog":catalog.descriptor,"id":record["id"],"version":record["version"],"path":path,"sha256":digest(&bytes),"byteLength":bytes.len(),"fileCommitted":true}),
        );
    }
    let request = args
        .get("requestId")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or(photo_files::identity("request").map_err(io)?);
    let import_bytes = if operation == "import" {
        Some(read_file(
            Path::new(text(args, "file")?),
            if kind == "resource" {
                MAX_RESOURCE
            } else {
                MAX_DEFINITION
            },
        )?)
    } else {
        None
    };
    let file_digest = import_bytes.as_ref().map(|bytes| digest(bytes));
    let mut intent_args = args.clone();
    if operation == "import" {
        intent_args
            .as_object_mut()
            .ok_or_else(|| error("invalid_request", "arguments object required"))?
            .remove("file");
    }
    let intent=digest(&photo_files::canonical_json(&json!({"operation":invocation.operation,"arguments":intent_args,"catalog":catalog.descriptor,"fileDigest":file_digest})).map_err(io)?);
    let tx = catalog
        .db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(io)?;
    let prior: Option<(String, String, i64)> = tx
        .query_row(
            "SELECT intent,result,created FROM receipts WHERE request=?",
            [&request],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()
        .map_err(io)?;
    if let Some((old, result, created)) = prior {
        if old != intent {
            return Err(error(
                "request_conflict",
                "request identity reused with changed catalog intent",
            ));
        }
        if now() - created > 7 * 86400 {
            return Err(error(
                "receipt_expired",
                "catalog receipt expired; identity cannot be reused",
            ));
        }
        let mut result: Value = serde_json::from_str(&result).map_err(io)?;
        result["outcome"] = json!("replayed");
        return Ok(result);
    }
    if operation == "save" {
        let edit = edit.ok_or_else(|| error("invalid_request", "save requires edit context"))?;
        let guard = args.get("ifState").and_then(Value::as_str).ok_or_else(|| {
            error(
                "state_guard_required",
                "catalog save requires observed state guard",
            )
        })?;
        if Some(guard) != edit.get("stateToken").and_then(Value::as_str) {
            return Err(error("state_conflict", "observed capture state changed"));
        }
    }
    // Keep this transaction open while validation/capture occurs. All durable records,
    // resource closures and receipts commit in the same transaction.
    let result = if operation == "remove" {
        let id = text(args, id_key)?;
        let version = text(args, "version")?;
        let stored: Option<String> = tx
            .query_row(
                "SELECT record FROM records WHERE id=? AND version=? AND kind=?",
                params![id, version, kind],
                |r| r.get(0),
            )
            .optional()
            .map_err(io)?;
        if stored.is_none() {
            return Err(error(
                "catalog_record_not_found",
                "record version not found",
            ));
        }
        if kind == "resource" {
            let refs = reference_counts_db(&tx, state, id, version)?;
            if refs
                .as_object()
                .unwrap()
                .values()
                .any(|v| v.as_u64().unwrap_or(0) > 0)
            {
                return Err(error("resource_in_use", "resource is referenced").detail(refs));
            }
        }
        tx.execute(
            "DELETE FROM records WHERE id=? AND version=?",
            params![id, version],
        )
        .map_err(io)?;
        json!({"catalog":catalog.descriptor,"id":id,"version":version,"requestId":request,"outcome":"removed"})
    } else {
        let (mut record, bytes) = if operation == "import" && kind == "resource" {
            resources::import(args)?
        } else if operation == "import" {
            let bytes = import_bytes.as_ref().unwrap();
            let interchange: Value =
                photo_files::read_json(bytes.as_slice(), MAX_DEFINITION).map_err(io)?;
            keys(&interchange, &["schemaVersion", "kind", "record"])?;
            if interchange["schemaVersion"] != 1 || interchange["kind"] != kind {
                return Err(error(
                    "invalid_definition",
                    "invalid interchange schema or kind",
                ));
            }
            let record = interchange["record"].clone();
            let engine = Engine::open()?;
            definitions::validate(&record, kind, &engine.discover()?, engine.bundle_id())?;
            (record, None)
        } else if operation == "save" {
            let edit =
                edit.ok_or_else(|| error("invalid_request", "save requires edit context"))?;
            let engine = Engine::open()?;
            definitions::capture(kind, args, edit, &engine.discover()?, engine.bundle_id())?
        } else {
            return Err(error("invalid_request", "unsupported catalog command"));
        };
        if let Some(bytes) = bytes.as_deref()
            && Some(digest(bytes)) != file_digest
        {
            return Err(error(
                "state_conflict",
                "import file changed after request identity capture",
            ));
        }
        record["id"] = json!(if kind == "resource" {
            format!("resource-{}", text(&record, "sha256")?)
        } else {
            photo_files::identity(kind).map_err(io)?
        });
        record["version"] = json!(if kind == "resource" {
            text(&record, "sha256")?.to_owned()
        } else {
            photo_files::identity("version").map_err(io)?
        });
        record["createdAt"] = json!(utc(now())?);
        record["availability"] = available();
        if let Some(bytes) = bytes.as_deref() {
            publish_resource(&catalog.dir, text(&record, "sha256")?, bytes)?;
        }
        if kind != "resource" {
            retain_closure(
                &tx,
                &record,
                if operation == "save" { state } else { None },
                &catalog.dir,
            )?;
        }
        let encoded = serde_json::to_string(&record).map_err(io)?;
        if encoded.len() > MAX_DEFINITION {
            return Err(error(
                "definition_too_large",
                "definition exceeds advertised byte maximum",
            ));
        }
        tx.execute(
            "INSERT OR IGNORE INTO records(id,version,kind,record,bytes) VALUES(?,?,?,?,?)",
            params![
                text(&record, "id")?,
                text(&record, "version")?,
                kind,
                encoded,
                bytes
            ],
        )
        .map_err(io)?;
        let stored: String = tx
            .query_row(
                "SELECT record FROM records WHERE id=? AND version=?",
                params![text(&record, "id")?, text(&record, "version")?],
                |r| r.get(0),
            )
            .map_err(io)?;
        record = serde_json::from_str(&stored).map_err(io)?;
        json!({"catalog":catalog.descriptor,"record":record,"requestId":request,"outcome":"created","retainUntil":deadline()?})
    };
    tx.execute(
        "INSERT INTO receipts(request,intent,result,created) VALUES(?,?,?,?)",
        params![
            request,
            intent,
            serde_json::to_string(&result).map_err(io)?,
            now()
        ],
    )
    .map_err(io)?;
    tx.commit().map_err(io)?;
    Ok(result)
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn deadline() -> Result<String, ToolError> {
    let timestamp = time::OffsetDateTime::from_unix_timestamp(now() + 7 * 86400).map_err(io)?;
    timestamp
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(io)
}
fn utc(seconds: i64) -> Result<String, ToolError> {
    time::OffsetDateTime::from_unix_timestamp(seconds)
        .map_err(io)?
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(io)
}
pub(super) fn read_file(path: &Path, max: usize) -> Result<Vec<u8>, ToolError> {
    let path = absolute(path)?;
    let dir = Directory::open(
        path.parent()
            .ok_or_else(|| error("invalid_request", "file parent missing"))?,
    )
    .map_err(io)?;
    let mut file = dir
        .open_file(
            path.file_name()
                .ok_or_else(|| error("invalid_request", "filename missing"))?,
        )
        .map_err(io)?;
    let _lease = photo_files::FileLease::acquire(
        file.try_clone().map_err(io)?,
        LeaseMode::Shared,
        Duration::from_secs(3),
    )
    .map_err(io)?;
    let observation = photo_files::Observation::of(&file).map_err(io)?;
    if file.metadata().map_err(io)?.len() > max as u64 {
        return Err(error(
            "resource_too_large",
            "file exceeds advertised byte maximum",
        ));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(max as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    observation.verify(&file).map_err(io)?;
    if bytes.len() > max {
        return Err(error(
            "resource_too_large",
            "file exceeds advertised byte maximum",
        ));
    }
    Ok(bytes)
}
fn list(c: &Catalog, args: &Value, kind: &str) -> Result<Value, ToolError> {
    cursors::list(c, args, kind)
}
pub(super) fn refs(value: &Value, out: &mut Vec<(String, String)>) {
    match value {
        Value::Object(map) => {
            if let (Some(id), Some(version)) = (
                map.get("resourceId").and_then(Value::as_str),
                map.get("version").and_then(Value::as_str),
            ) {
                let pair = (id.to_owned(), version.to_owned());
                if !out.contains(&pair) {
                    out.push(pair);
                }
            }
            for v in map.values() {
                refs(v, out);
            }
        }
        Value::Array(a) => {
            for v in a {
                refs(v, out)
            }
        }
        _ => (),
    }
}
fn reference_counts(
    c: &Catalog,
    state: Option<&Path>,
    id: &str,
    version: &str,
) -> Result<Value, ToolError> {
    reference_counts_db(&c.db, state, id, version)
}
fn reference_counts_db(
    db: &Connection,
    state: Option<&Path>,
    id: &str,
    version: &str,
) -> Result<Value, ToolError> {
    let mut result = if let Some(state) = state {
        crate::state::catalog_references(state, id, version)?
    } else {
        json!({"currentEdits":0,"retainedRevisions":0,"tasks":0,"catalogs":0})
    };
    let mut q = db
        .prepare("SELECT record FROM records WHERE kind!='resource'")
        .map_err(io)?;
    let rows = q.query_map([], |r| r.get::<_, String>(0)).map_err(io)?;
    let mut count = 0;
    for row in rows {
        let v: Value = serde_json::from_str(&row.map_err(io)?).map_err(io)?;
        let mut found = Vec::new();
        refs(&v, &mut found);
        if found.contains(&(id.into(), version.into())) {
            count += 1;
        }
    }
    result["catalogs"] = json!(count);
    Ok(result)
}
fn retain_closure(
    db: &Connection,
    record: &Value,
    state: Option<&Path>,
    root: &Directory,
) -> Result<(), ToolError> {
    let mut dependencies = Vec::new();
    refs(record, &mut dependencies);
    for (id, version) in dependencies {
        let existing:Option<(String,Vec<u8>)>=db.query_row("SELECT record,bytes FROM records WHERE id=? AND version=? AND kind='resource' UNION ALL SELECT record,bytes FROM pins WHERE id=? AND version=? LIMIT 1",params![id,version,id,version],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(io)?;
        if let Some((r, b)) = existing {
            let r: Value = serde_json::from_str(&r).map_err(io)?;
            if digest(&b) != r["sha256"] {
                return Err(error(
                    "resource_corrupt",
                    "catalog dependency digest mismatch",
                ));
            }
            publish_resource(root, text(&r, "sha256")?, &b)?;
            continue;
        }
        let state=state.ok_or_else(||error("catalog_record_not_found","interchange dependency missing from selected catalog; import resource version first"))?;
        let local = Catalog::open(Some(state), "local", false)?;
        let row: Option<(String, Vec<u8>)> = local
            .db
            .query_row(
                "SELECT record,bytes FROM pins WHERE id=? AND version=?",
                params![id, version],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(io)?;
        let (r, b) = row.ok_or_else(|| {
            error(
                "catalog_record_not_found",
                "capture resource is not pinned in source revision",
            )
        })?;
        let value: Value = serde_json::from_str(&r).map_err(io)?;
        if digest(&b) != value["sha256"] {
            return Err(error(
                "resource_corrupt",
                "source pinned dependency digest mismatch",
            ));
        }
        publish_resource(root, text(&value, "sha256")?, &b)?;
        db.execute(
            "INSERT INTO pins(id,version,record,bytes) VALUES(?,?,?,?)",
            params![id, version, r, b],
        )
        .map_err(io)?;
    }
    Ok(())
}
pub fn pin_dependencies(state: &Path, snapshot: &Value, scope: &str) -> Result<Value, ToolError> {
    dependencies(state, snapshot, scope, true)
}
pub fn validate_dependencies(
    state: &Path,
    snapshot: &Value,
    scope: &str,
) -> Result<Value, ToolError> {
    dependencies(state, snapshot, scope, false)
}
pub fn validate_new_dependencies(
    state: &Path,
    before: &Value,
    after: &Value,
    scope: &str,
) -> Result<(), ToolError> {
    let mut old = Vec::new();
    let mut new = Vec::new();
    refs(before, &mut old);
    refs(after, &mut new);
    let added = new
        .into_iter()
        .filter(|reference| !old.contains(reference))
        .collect::<Vec<_>>();
    if added.is_empty() {
        return Ok(());
    }
    let selected = Catalog::open(Some(state), scope, false)?;
    for (id, version) in added {
        selected.resource(&id, &version)?;
    }
    Ok(())
}
fn dependencies(
    state: &Path,
    snapshot: &Value,
    scope: &str,
    pin: bool,
) -> Result<Value, ToolError> {
    let mut found = Vec::new();
    refs(snapshot, &mut found);
    if found.is_empty() {
        return Ok(json!([]));
    }
    let local = Catalog::open(Some(state), "local", false)?;
    let mut selected = None;
    let mut evidence = Vec::new();
    for (id, version) in found {
        let resource = local
            .db
            .query_row(
                "SELECT record,bytes FROM pins WHERE id=? AND version=?",
                params![id, version],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?)),
            )
            .optional()
            .map_err(io)?;
        let already_pinned = resource.is_some();
        let (record, bytes) = if let Some((r, b)) = resource {
            (serde_json::from_str::<Value>(&r).map_err(io)?, b)
        } else {
            if selected.is_none() {
                selected = Some(Catalog::open(Some(state), scope, false)?);
            }
            selected.as_ref().unwrap().resource(&id, &version)?
        };
        let sha = digest(&bytes);
        if record["sha256"] != sha {
            return Err(error("resource_corrupt", "pinned resource bytes changed"));
        }
        let path = if pin || already_pinned {
            local.dir.path().join("resources").join(&sha)
        } else {
            selected
                .as_ref()
                .unwrap()
                .dir
                .path()
                .join("resources")
                .join(&sha)
        };
        if pin {
            publish_resource(&local.dir, &sha, &bytes)?;
            local
                .db
                .execute(
                    "INSERT OR IGNORE INTO pins(id,version,record,bytes) VALUES(?,?,?,?)",
                    params![
                        id,
                        version,
                        serde_json::to_string(&record).map_err(io)?,
                        bytes
                    ],
                )
                .map_err(io)?;
        } else {
            let retained = read_file(&path, MAX_RESOURCE)?;
            if digest(&retained) != sha {
                return Err(error(
                    "resource_corrupt",
                    "resource file differs from immutable bytes",
                ));
            }
        }
        evidence.push(json!({"resourceId":id,"version":version,"kind":record["kind"],"mediaType":record["mediaType"],"facts":record["facts"],"sha256":sha,"byteLength":bytes.len(),"path":path}));
    }
    Ok(json!(evidence))
}
pub fn validate_kind_refs(
    snapshot: &Value,
    evidence: &Value,
    discovery: &Value,
) -> Result<(), ToolError> {
    let deps = evidence
        .as_array()
        .ok_or_else(|| error("invalid_request", "dependency evidence array missing"))?;
    for instance in array(snapshot, "instances")? {
        let module = definitions::module(discovery, text(instance, "moduleId")?)?;
        let properties = definitions::properties(module)?;
        for (id, value) in instance["parameters"]
            .as_object()
            .ok_or_else(|| error("invalid_request", "instance parameters missing"))?
        {
            if let Some(expected) = properties
                .get(id)
                .and_then(|p| p.get("resourceKind"))
                .and_then(Value::as_str)
            {
                let mut references = Vec::new();
                refs(value, &mut references);
                for (resource_id, version) in references {
                    let dependency = deps
                        .iter()
                        .find(|d| d["resourceId"] == resource_id && d["version"] == version)
                        .ok_or_else(|| {
                            error(
                                "catalog_record_not_found",
                                "typed resource dependency unavailable",
                            )
                        })?;
                    if dependency["kind"] != expected {
                        return Err(error(
                            "invalid_resource",
                            "resource kind incompatible with native parameter schema",
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}
pub fn apply(
    state: &Path,
    snapshot: &Value,
    change: &Value,
    scope: &str,
    engine: &Engine,
    input: &Path,
) -> Result<Value, ToolError> {
    application::apply(state, snapshot, change, scope, engine, input)
}
pub fn validate_apply(
    state: &Path,
    snapshot: &Value,
    change: &Value,
    scope: &str,
    engine: &Engine,
    input: &Path,
) -> Result<Value, ToolError> {
    application::validate_apply(state, snapshot, change, scope, engine, input)
}
pub fn pin_external_dependencies(state: &Path, dependencies: &Value) -> Result<Value, ToolError> {
    let dependencies = dependencies
        .as_array()
        .ok_or_else(|| error("invalid_request", "dependency evidence must be array"))?;
    let mut local = Catalog::open(Some(state), "local", false)?;
    let tx = local
        .db
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(io)?;
    let mut result = Vec::new();
    for evidence in dependencies {
        let id = text(evidence, "resourceId")?;
        let version = text(evidence, "version")?;
        let bytes = read_file(Path::new(text(evidence, "path")?), MAX_RESOURCE)?;
        let sha = digest(&bytes);
        if evidence["sha256"] != sha || evidence["byteLength"].as_u64() != Some(bytes.len() as u64)
        {
            return Err(error(
                "resource_corrupt",
                "captured external resource evidence differs",
            ));
        }
        let facts = resources::validate(text(evidence, "kind")?, &bytes)?;
        let record = json!({"id":id,"version":version,"kind":evidence["kind"],"name":id,"mediaType":facts["mediaType"],"sha256":sha,"byteLength":bytes.len(),"createdAt":utc(now())?,"availability":available(),"facts":facts});
        let old: Option<String> = tx
            .query_row(
                "SELECT record FROM pins WHERE id=? AND version=?",
                params![id, version],
                |r| r.get(0),
            )
            .optional()
            .map_err(io)?;
        if let Some(old) = old {
            let old: Value = serde_json::from_str(&old).map_err(io)?;
            if old["sha256"] != sha {
                return Err(error(
                    "resource_corrupt",
                    "same immutable dependency identity has different bytes",
                ));
            }
        }
        publish_resource(&local.dir, &sha, &bytes)?;
        tx.execute(
            "INSERT OR IGNORE INTO pins(id,version,record,bytes) VALUES(?,?,?,?)",
            params![id, version, record.to_string(), bytes],
        )
        .map_err(io)?;
        let mut pinned = evidence.clone();
        pinned["path"] = json!(local.dir.path().join("resources").join(&sha));
        pinned["mediaType"] = record["mediaType"].clone();
        pinned["facts"] = record["facts"].clone();
        result.push(pinned);
    }
    tx.commit().map_err(io)?;
    Ok(json!(result))
}
fn publish_resource(root: &Directory, sha: &str, bytes: &[u8]) -> Result<(), ToolError> {
    let dir = root.create_directory("resources").map_err(io)?;
    match dir.open_file(sha) {
        Ok(_) => {
            if digest(&read_file(&dir.path().join(sha), MAX_RESOURCE)?) != sha {
                return Err(error(
                    "resource_corrupt",
                    "retained resource digest mismatch",
                ));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let mut pending = PendingFile::create(&dir).map_err(io)?;
            pending.file_mut().write_all(bytes).map_err(io)?;
            match pending.publish(sha, MAX_RESOURCE as u64) {
                Ok(_) => (),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if digest(&read_file(&dir.path().join(sha), MAX_RESOURCE)?) != sha {
                        return Err(error(
                            "resource_corrupt",
                            "concurrent resource publication mismatch",
                        ));
                    }
                }
                Err(e) => return Err(io(e)),
            }
        }
        Err(e) => return Err(io(e)),
    }
    Ok(())
}
