use crate::{Invocation, ToolError, catalog, engine::Engine};
use photo_files::{Directory, FileLease, LeaseMode, canonical_json, fingerprint, identity};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
mod changes;
mod migration;
mod reads;
mod render;
mod restore;
pub use migration::{import_legacy, review_legacy_import};
#[cfg(test)]
mod tests;

pub(super) fn err(code: &str, message: impl Into<String>) -> ToolError {
    let exit = if code.ends_with("_not_found") {
        3
    } else if matches!(
        code,
        "invalid_input"
            | "invalid_command"
            | "schema_unsupported"
            | "limit_exceeded"
            | "invalid_reference"
            | "invalid_order"
    ) {
        2
    } else if code == "outcome_unknown" {
        7
    } else if matches!(
        code,
        "state_unavailable"
            | "file_error"
            | "migration_unavailable"
            | "initialization_unavailable"
            | "module_unavailable"
    ) {
        6
    } else {
        4
    };
    ToolError::new(code, message, exit)
}
pub(super) fn io(error: impl std::fmt::Display) -> ToolError {
    err("file_error", error.to_string())
}
pub(super) fn db(error: impl std::fmt::Display) -> ToolError {
    err("state_unavailable", error.to_string())
}
pub(super) fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, ToolError> {
    value[key]
        .as_str()
        .ok_or_else(|| err("invalid_input", format!("{key} must be a string")))
}
pub(super) fn absolute(path: &Path) -> Result<PathBuf, ToolError> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        Ok(std::env::current_dir().map_err(io)?.join(path))
    }
}
pub(super) fn digest(value: &Value) -> Result<String, ToolError> {
    Ok(format!(
        "{:x}",
        Sha256::digest(canonical_json(value).map_err(io)?)
    ))
}
pub(super) fn id(prefix: &str) -> Result<String, ToolError> {
    identity(prefix).map_err(io)
}
pub(super) fn identifier(value: &str) -> Result<(), ToolError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
    {
        Err(err("invalid_input", "Invalid identifier"))
    } else {
        Ok(())
    }
}
pub(super) fn host() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| "localhost".into())
}
pub(super) fn timestamp() -> String {
    utc(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs())
}
pub(super) fn deadline() -> String {
    utc(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        + 604800)
}
fn utc(seconds: u64) -> String {
    let seconds = seconds as libc::time_t;
    let mut result = unsafe { std::mem::zeroed::<libc::tm>() };
    unsafe { libc::gmtime_r(&seconds, &mut result) };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        result.tm_year + 1900,
        result.tm_mon + 1,
        result.tm_mday,
        result.tm_hour,
        result.tm_min,
        result.tm_sec
    )
}

pub(super) struct Store {
    pub dir: Directory,
    pub conn: Connection,
    _lifecycle: FileLease,
    _ancestors: Vec<FileLease>,
}
impl Store {
    pub fn read(path: &Path) -> Result<Self, ToolError> {
        let ancestors =
            photo_files::ancestor_leases(&absolute(path)?, Duration::from_secs(5)).map_err(io)?;
        let dir =
            Directory::open(absolute(path)?).map_err(|e| err("state_not_found", e.to_string()))?;
        let lifecycle = dir
            .lease(LeaseMode::Shared, Duration::from_secs(5))
            .map_err(io)?;
        dir.open_file("state.sqlite3")
            .map_err(|e| err("state_not_found", e.to_string()))?;
        let conn = Connection::open_with_flags(
            dir.path().join("state.sqlite3"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .map_err(db)?;
        conn.busy_timeout(Duration::from_secs(5)).map_err(db)?;
        Ok(Self {
            dir,
            conn,
            _lifecycle: lifecycle,
            _ancestors: ancestors,
        })
    }
    pub fn create(dir: Directory) -> Result<Self, ToolError> {
        let file = dir.create_file("state.sqlite3", 0o600).map_err(io)?;
        file.sync_all().map_err(io)?;
        dir.create_file("state.lock", 0o600)
            .map_err(io)?
            .sync_all()
            .map_err(io)?;
        let lifecycle = dir
            .lease(LeaseMode::Shared, Duration::from_secs(5))
            .map_err(io)?;
        let conn = Connection::open(dir.path().join("state.sqlite3")).map_err(db)?;
        conn.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; CREATE TABLE metadata(key TEXT PRIMARY KEY,value TEXT NOT NULL); CREATE TABLE revisions(id TEXT PRIMARY KEY,parent TEXT,created TEXT NOT NULL,request TEXT,snapshot TEXT NOT NULL,input TEXT NOT NULL,bundle TEXT NOT NULL,schema TEXT NOT NULL,dependencies TEXT NOT NULL,targets TEXT NOT NULL); CREATE TABLE receipts(id TEXT PRIMARY KEY,payload TEXT NOT NULL,result TEXT NOT NULL,retain_until TEXT NOT NULL); CREATE TABLE captures(id TEXT PRIMARY KEY,revision TEXT NOT NULL,snapshot TEXT NOT NULL,input TEXT NOT NULL,dependencies TEXT NOT NULL,result TEXT,payload TEXT NOT NULL,request TEXT NOT NULL UNIQUE); ").map_err(db)?;
        Ok(Self {
            dir,
            conn,
            _lifecycle: lifecycle,
            _ancestors: Vec::new(),
        })
    }
    pub fn lock(&self) -> Result<FileLease, ToolError> {
        FileLease::acquire(
            self.dir.open_file("state.lock").map_err(io)?,
            LeaseMode::Exclusive,
            Duration::from_secs(5),
        )
        .map_err(|e| err("state_busy", e.to_string()))
    }
    pub fn meta(&self, key: &str) -> Result<Value, ToolError> {
        let text: String = self
            .conn
            .query_row("SELECT value FROM metadata WHERE key=?", [key], |r| {
                r.get(0)
            })
            .map_err(db)?;
        serde_json::from_str(&text).map_err(db)
    }
    pub fn set_meta(&self, key: &str, value: &Value) -> Result<(), ToolError> {
        self.conn.execute("INSERT INTO metadata(key,value) VALUES (?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,value.to_string()]).map_err(db)?;
        Ok(())
    }
    pub fn revision(&self, revision: Option<&str>) -> Result<Value, ToolError> {
        let current = self.meta("current")?;
        let rev = revision.unwrap_or(current.as_str().unwrap_or(""));
        self.conn.query_row("SELECT id,parent,created,request,snapshot,input,bundle,schema,dependencies,targets FROM revisions WHERE id=?",[rev],|r| {
            let parse=|index|->rusqlite::Result<Value>{let s:String=r.get(index)?;serde_json::from_str(&s).map_err(|e|rusqlite::Error::FromSqlConversionFailure(index,rusqlite::types::Type::Text,Box::new(e)))};
            Ok(json!({"revision":r.get::<_,String>(0)?,"parent":r.get::<_,Option<String>>(1)?,"createdAt":r.get::<_,String>(2)?,"requestId":r.get::<_,Option<String>>(3)?,"snapshot":parse(4)?,"input":parse(5)?,"bundleId":r.get::<_,String>(6)?,"schemaVersion":r.get::<_,String>(7)?,"dependencies":parse(8)?,"changedTargets":parse(9)?}))
        }).optional().map_err(db)?.ok_or_else(||err("revision_not_found","Revision is not retained"))
    }
    pub fn token(&self, revision: &Value) -> Result<String, ToolError> {
        digest(
            &json!({"state":self.meta("identity")?,"revision":revision["revision"],"input":{"path":revision["input"]["path"],"boundRevision":revision["input"]["boundRevision"]},"bundle":revision["bundleId"],"schema":revision["schemaVersion"]}),
        )
    }
    pub fn guard(&self, args: &Value, revision: &Value, required: bool) -> Result<(), ToolError> {
        match args["ifState"].as_str() {
            Some(token) if token == self.token(revision)? => Ok(()),
            Some(_) => Err(err("state_conflict", "Observed state no longer matches")),
            None if required => Err(err(
                "state_guard_required",
                "This command requires --if-state",
            )),
            None => Ok(()),
        }
    }
    pub fn replay(&self, request: &str, payload: &str) -> Result<Option<Value>, ToolError> {
        let row: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT payload,result FROM receipts WHERE id=?",
                [request],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(db)?;
        if let Some((prior, result)) = row {
            if prior != payload {
                return Err(err(
                    "request_conflict",
                    "Request identity has different intent",
                ));
            }
            let mut value: Value = serde_json::from_str(&result).map_err(db)?;
            value["outcome"] = json!("replayed");
            let current = self.revision(None)?;
            value["current"] =
                json!({"revision":current["revision"],"stateToken":self.token(&current)?});
            return Ok(Some(value));
        }
        Ok(None)
    }
    pub fn reject_capture_request(&self, request: &str) -> Result<(), ToolError> {
        let reserved: bool = self
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM captures WHERE request=?)",
                [request],
                |row| row.get(0),
            )
            .map_err(db)?;
        if reserved {
            return Err(err(
                "request_conflict",
                "Request identity is reserved by an accepted render capture",
            ));
        }
        Ok(())
    }
    pub fn receipt(&self, request: &str, payload: &str, result: &Value) -> Result<(), ToolError> {
        self.conn
            .execute(
                "INSERT INTO receipts VALUES (?,?,?,?)",
                params![request, payload, result.to_string(), deadline()],
            )
            .map_err(db)?;
        Ok(())
    }
    pub fn insert_revision(&self, value: &Value) -> Result<(), ToolError> {
        self.conn
            .execute(
                "INSERT INTO revisions VALUES (?,?,?,?,?,?,?,?,?,?)",
                params![
                    value["revision"].as_str(),
                    value["parent"].as_str(),
                    value["createdAt"].as_str(),
                    value["requestId"].as_str(),
                    value["snapshot"].to_string(),
                    value["input"].to_string(),
                    value["bundleId"].as_str(),
                    value["schemaVersion"].as_str(),
                    value["dependencies"].to_string(),
                    value["changedTargets"].to_string()
                ],
            )
            .map_err(db)?;
        Ok(())
    }
}

pub(super) struct InputLease {
    _file: FileLease,
    _ancestors: Vec<FileLease>,
}

pub(super) fn evidence(path: &Path) -> Result<(Value, InputLease), ToolError> {
    let path = absolute(path)?;
    let ancestors = photo_files::ancestor_leases(&path, Duration::from_secs(5)).map_err(io)?;
    let parent = Directory::open(
        path.parent()
            .ok_or_else(|| err("invalid_input", "Input has no parent"))?,
    )
    .map_err(io)?;
    let mut file = parent
        .open_file(
            path.file_name()
                .ok_or_else(|| err("invalid_input", "Input has no filename"))?,
        )
        .map_err(io)?;
    parent.verify_path().map_err(io)?;
    let lease = FileLease::acquire(
        file.try_clone().map_err(io)?,
        LeaseMode::Shared,
        Duration::from_secs(5),
    )
    .map_err(io)?;
    let facts = serde_json::to_value(fingerprint(&mut file, u64::MAX).map_err(io)?).map_err(io)?;
    Ok((
        json!({"path":path,"fingerprint":facts,"byteLength":file.metadata().map_err(io)?.len(),"availability":{"read":true,"write":false,"execute":true,"reason":null,"recovery":null},"boundRevision":digest(&json!({"sha256":facts["sha256"],"byteLength":facts["byteLength"]}))?,"observedRevision":digest(&facts)?}),
        InputLease {
            _file: lease,
            _ancestors: ancestors,
        },
    ))
}
pub(super) fn verify_input(revision: &Value) -> Result<InputLease, ToolError> {
    let (observed, lease) = evidence(Path::new(string(&revision["input"], "path")?))?;
    if observed["fingerprint"] != revision["input"]["fingerprint"] {
        return Err(err(
            "input_changed",
            "Input bytes changed; explicit rebind is required",
        ));
    }
    Ok(lease)
}

pub fn execute(invocation: &Invocation) -> Result<Value, ToolError> {
    if invocation.arguments.get("cursor").is_some() {
        return crate::continuations::resume(invocation);
    }
    if invocation.operation == "__show-input" {
        let (input, _lease) = evidence(Path::new(string(&invocation.arguments, "inputPath")?))?;
        return Ok(
            json!({"stateDirectory":invocation.state_directory,"inputPath":input["path"],"input":input,"executionHost":host(),"revision":null,"baselineRevision":null,"stateToken":null,"engineId":"darktable","bundleId":null,"schemaVersion":null,"availability":{"read":true,"write":true,"execute":false,"reason":"uninitialized","recovery":"the first mutation captures the native baseline"},"initialization":{"state":"pending","reason":"No native baseline has been captured"},"instances":{"items":[],"total":0,"nextCursor":null},"masks":{"items":[],"total":0,"nextCursor":null},"actions":{"items":[],"total":0,"nextCursor":null},"output":{"settings":{},"imageContract":null},"frames":[],"previewCapabilities":{"regionSchema":null,"statisticsSchema":null,"maxFileBytes":67108864},"limits":{"maxChanges":100},"latestPreview":null}),
        );
    }
    if invocation.operation == "__migration-review" {
        return review_legacy_import(
            Path::new(string(&invocation.arguments, "inputPath")?),
            &invocation.arguments["legacy"],
        );
    }
    if invocation.operation == "__migration-import" {
        return import_legacy(
            invocation,
            &invocation.arguments["legacy"],
            &invocation.arguments["reviewed"],
        );
    }
    if invocation.operation == "open" {
        return open(invocation);
    }
    if invocation.operation == "__inspect" {
        return inspect_state(
            invocation
                .state_directory
                .as_ref()
                .ok_or_else(|| err("invalid_input", "State required"))?,
        );
    }
    if invocation.operation == "__capture-render" {
        return capture_render(invocation);
    }
    if invocation.state_directory.is_none() {
        if invocation.operation.starts_with("modules ") {
            return reads::discovery(invocation, None);
        }
        return catalog::dispatch(invocation, None);
    }
    let catalog_operation = invocation.operation.split_whitespace().nth(1);
    if matches!(
        invocation.operation.split_whitespace().next(),
        Some("resources" | "presets" | "styles")
    ) && !matches!(catalog_operation, Some("apply" | "save"))
    {
        return catalog::dispatch(invocation, None);
    }
    let store = Store::read(invocation.state_directory.as_ref().unwrap())?;
    if invocation.operation == "__restore-rebase" {
        return restore::rebase(&store, &invocation.arguments);
    }
    if invocation.operation == "__retry-capture" {
        return render::retry(&store, &invocation.arguments);
    }
    if invocation.operation == "__receipt" {
        return render::receipt(&store, string(&invocation.arguments, "requestId")?);
    }
    if invocation.operation == "__release-capture" {
        return render::release(&store, &invocation.arguments);
    }
    if reads::is_read(&invocation.operation) {
        return reads::dispatch(&store, invocation);
    }
    if invocation.operation == "preview render" || invocation.operation.starts_with("export ") {
        return render::dispatch(&store, invocation);
    }
    let _lock = store.lock()?;
    let revision = store.revision(None)?;
    if invocation.operation.starts_with("resources ")
        || invocation.operation.starts_with("presets ") && invocation.operation != "presets apply"
        || invocation.operation.starts_with("styles ") && invocation.operation != "styles apply"
    {
        let mut edit = revision.clone();
        edit["stateDirectory"] = json!(store.dir.path());
        edit["stateToken"] = json!(store.token(&revision)?);
        return catalog::dispatch(invocation, Some(&edit));
    }
    changes::dispatch(&store, invocation, &revision)
}
fn open(invocation: &Invocation) -> Result<Value, ToolError> {
    let path = absolute(
        invocation
            .state_directory
            .as_ref()
            .ok_or_else(|| err("invalid_input", "--state is required"))?,
    )?;
    let (parent, _ancestors) = create_state_parent(&path)?;
    let input_path = absolute(Path::new(string(&invocation.arguments, "inputPath")?))?;
    if let Ok(store) = Store::read(&path) {
        let _lock = store.lock()?;
        let revision = store.revision(None)?;
        if revision["input"]["path"] != json!(input_path) {
            return Err(err(
                "input_binding_conflict",
                "State is bound to another input",
            ));
        }
        let _input = verify_input(&revision)?;
        return reads::view(&store, &revision);
    }
    let (input, _input) = evidence(&input_path)?;
    let engine = Engine::open()?;
    let mut snapshot = engine.initialize(&input_path)?;
    remap_baseline(&mut snapshot)?;
    let staging_name = id("opening")?;
    let staging = parent.create_directory(&staging_name).map_err(io)?;
    let store = Store::create(staging)?;
    let revision_id = id("revision")?;
    store.dir.create_directory("render-locks").map_err(io)?;
    store.dir.create_directory("render-evidence").map_err(io)?;
    catalog::initialize(store.dir.path())?;
    store.set_meta("discovery", &engine.discover()?)?;
    let preview = absolute(Path::new(
        invocation.arguments["previewDirectory"]
            .as_str()
            .unwrap_or(path.join("previews").to_str().unwrap_or("")),
    ))?;
    let output = absolute(Path::new(
        invocation.arguments["outputDirectory"]
            .as_str()
            .unwrap_or(path.join("outputs").to_str().unwrap_or("")),
    ))?;
    store.conn.execute_batch("BEGIN IMMEDIATE").map_err(db)?;
    store.set_meta("identity", &json!(id("state")?))?;
    store.set_meta("current", &json!(revision_id))?;
    store.set_meta("baseline", &json!(revision_id))?;
    store.set_meta("previewDirectory", &json!(preview))?;
    store.set_meta("outputDirectory", &json!(output))?;
    store.insert_revision(&json!({"revision":revision_id,"parent":null,"createdAt":timestamp(),"requestId":null,"snapshot":snapshot,"input":input,"bundleId":engine.bundle_id(),"schemaVersion":engine.schema_version(),"dependencies":[],"changedTargets":[]}))?;
    store.conn.execute_batch("COMMIT").map_err(db)?;
    store.dir.sync().map_err(io)?;
    parent
        .rename(
            &staging_name,
            &parent,
            path.file_name()
                .ok_or_else(|| err("invalid_input", "State name missing"))?,
            false,
        )
        .map_err(io)?;
    let published = Store::read(&path)?;
    reads::view(&published, &published.revision(None)?)
}
pub fn retained_snapshots(path: &Path) -> Result<Vec<Value>, ToolError> {
    let store = Store::read(path)?;
    let mut query = store
        .conn
        .prepare("SELECT snapshot FROM revisions UNION ALL SELECT snapshot FROM captures")
        .map_err(db)?;
    let rows = query.query_map([], |r| r.get::<_, String>(0)).map_err(db)?;
    rows.map(|r| serde_json::from_str(&r.map_err(db)?).map_err(db))
        .collect()
}
pub fn inspect_state(path: &Path) -> Result<Value, ToolError> {
    let store = Store::read(path)?;
    let revision = store.revision(None)?;
    let retained = crate::inspect_retained_directory(&store.dir, &store._lifecycle)?;
    let mut result = reads::view(&store, &revision)?;
    result
        .as_object_mut()
        .unwrap()
        .extend(retained.as_object().unwrap().clone());
    Ok(result)
}
pub fn catalog_references(path: &Path, resource: &str, version: &str) -> Result<Value, ToolError> {
    fn contains(v: &Value, id: &str, version: &str) -> bool {
        if v["resourceId"] == id && v["version"] == version {
            return true;
        }
        match v {
            Value::Array(a) => a.iter().any(|v| contains(v, id, version)),
            Value::Object(o) => o.values().any(|v| contains(v, id, version)),
            _ => false,
        }
    }
    let store = Store::read(path)?;
    let current = contains(&store.revision(None)?["snapshot"], resource, version) as usize;
    let retained = retained_snapshots(path)?
        .iter()
        .filter(|s| contains(s, resource, version))
        .count();
    Ok(json!({"currentEdits":current,"retainedRevisions":retained,"tasks":0,"catalogs":0}))
}
pub fn capture_render(invocation: &Invocation) -> Result<Value, ToolError> {
    let store = Store::read(
        invocation
            .state_directory
            .as_ref()
            .ok_or_else(|| err("invalid_input", "State required"))?,
    )?;
    let mut actual = invocation.clone();
    if invocation.operation == "__capture-render" {
        actual.operation = string(&invocation.arguments, "operation")?.into();
        actual
            .arguments
            .as_object_mut()
            .ok_or_else(|| err("invalid_input", "Arguments must be object"))?
            .remove("operation");
    }
    if !matches!(
        actual.operation.as_str(),
        "preview render" | "export create" | "export state-file"
    ) {
        return Err(err("invalid_command", "Only render operations may capture"));
    }
    render::capture(&store, &actual)
}
pub(super) fn remap_baseline(snapshot: &mut Value) -> Result<(), ToolError> {
    let mut mappings = std::collections::HashMap::new();
    for (collection, key, prefix) in [
        ("instances", "instanceId", "instance"),
        ("masks", "maskId", "mask"),
    ] {
        for item in snapshot[collection].as_array().into_iter().flatten() {
            mappings.insert(string(item, key)?.to_owned(), id(prefix)?);
        }
    }
    fn replace(v: &mut Value, map: &std::collections::HashMap<String, String>) {
        match v {
            Value::String(s) => {
                if let Some(id) = map.get(s) {
                    *s = id.clone();
                }
            }
            Value::Array(a) => {
                for v in a {
                    replace(v, map)
                }
            }
            Value::Object(o) => {
                for v in o.values_mut() {
                    replace(v, map)
                }
            }
            _ => (),
        }
    }
    replace(snapshot, &mappings);
    Ok(())
}
pub(super) fn prune_history(store: &Store) -> Result<(), ToolError> {
    store.conn.execute("DELETE FROM revisions WHERE id NOT IN (SELECT id FROM revisions ORDER BY rowid DESC LIMIT 100) AND id != json_extract((SELECT value FROM metadata WHERE key='current'),'$') AND id != json_extract((SELECT value FROM metadata WHERE key='baseline'),'$') AND id NOT IN (SELECT revision FROM captures)",[]).map_err(db)?;
    Ok(())
}
fn create_state_parent(path: &Path) -> Result<(Directory, Vec<FileLease>), ToolError> {
    let parent = path
        .parent()
        .ok_or_else(|| err("invalid_input", "State directory has no parent"))?;
    leased_directory(parent)
}
fn leased_directory(path: &Path) -> Result<(Directory, Vec<FileLease>), ToolError> {
    let mut directory = Directory::open("/").map_err(io)?;
    let mut leases = vec![
        directory
            .lease(LeaseMode::Shared, Duration::from_secs(5))
            .map_err(io)?,
    ];
    for component in path.components() {
        match component {
            std::path::Component::RootDir => continue,
            std::path::Component::Normal(name) => {
                directory = match directory.child(name) {
                    Ok(child) => child,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        directory.create_directory(name).map_err(io)?
                    }
                    Err(error) => return Err(io(error)),
                };
                leases.push(
                    directory
                        .lease(LeaseMode::Shared, Duration::from_secs(5))
                        .map_err(io)?,
                );
            }
            _ => {
                return Err(err(
                    "invalid_input",
                    "Directory path cannot contain parent traversal",
                ));
            }
        }
    }
    Ok((directory, leases))
}
