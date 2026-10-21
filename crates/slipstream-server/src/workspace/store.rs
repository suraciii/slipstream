use super::*;
use photo_files::{Evidence, Observation, ancestor_leases, identity};
use rusqlite::{OpenFlags, OptionalExtension, params};
use std::{io::ErrorKind, path::Path};

pub(super) const WORKSPACE_ADMISSION_LOCK: &str = ".workspace-admission-lock";
const REGISTRY_SCHEMA: &str = "
PRAGMA journal_mode=DELETE;
PRAGMA synchronous=FULL;
CREATE TABLE IF NOT EXISTS workspaces(id TEXT PRIMARY KEY,photo_id TEXT NOT NULL,state TEXT NOT NULL);
CREATE UNIQUE INDEX IF NOT EXISTS workspace_photo ON workspaces(photo_id) WHERE state='active';
CREATE TABLE IF NOT EXISTS deletions(workspace_id TEXT NOT NULL,request_id TEXT NOT NULL,intent TEXT NOT NULL,result TEXT NOT NULL,state TEXT NOT NULL,PRIMARY KEY(workspace_id,request_id));
CREATE TABLE IF NOT EXISTS artifact_aliases(id TEXT PRIMARY KEY,workspace_id TEXT NOT NULL,artifact_id TEXT NOT NULL,revision TEXT NOT NULL);
";
const WORKSPACE_SCHEMA: &str = "
PRAGMA journal_mode=DELETE;
PRAGMA synchronous=FULL;
CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY,value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS files(id TEXT PRIMARY KEY,path TEXT NOT NULL,kind TEXT NOT NULL,state TEXT NOT NULL,evidence TEXT,revision TEXT,contract TEXT,image_facts TEXT,source_revision TEXT,managed INTEGER NOT NULL DEFAULT 0,provenance TEXT,removed INTEGER NOT NULL DEFAULT 0);
CREATE UNIQUE INDEX IF NOT EXISTS live_file_path ON files(path) WHERE removed=0;
CREATE TABLE IF NOT EXISTS receipts(request_id TEXT PRIMARY KEY,intent TEXT NOT NULL,result TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS preparations(request_id TEXT PRIMARY KEY,intent TEXT NOT NULL,path TEXT NOT NULL,artifact_id TEXT NOT NULL,source_revision TEXT NOT NULL,evidence TEXT,result TEXT);
CREATE TABLE IF NOT EXISTS dependencies(owner TEXT NOT NULL,kind TEXT NOT NULL,artifact_id TEXT NOT NULL,revision TEXT NOT NULL,PRIMARY KEY(owner,kind,artifact_id));
CREATE TABLE IF NOT EXISTS captures(artifact_id TEXT NOT NULL,revision TEXT NOT NULL,captured_id TEXT NOT NULL,captured_revision TEXT NOT NULL,PRIMARY KEY(artifact_id,revision));
";

pub(super) fn regular_or_create(directory: &Directory, name: &str) -> Result<File> {
    match directory.create_file(name, 0o600) {
        Ok(file) => {
            file.sync_all()?;
            directory.sync()?;
            Ok(file)
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(directory.open_file(name)?),
        Err(error) => Err(error.into()),
    }
}
fn existing_file(directory: &Directory, name: &str) -> Result<File> {
    directory.open_file(name).map_err(|error| {
        WorkspaceError::new(
            "workspace_unavailable",
            503,
            format!("Required workspace file could not be opened: {error}"),
        )
    })
}

pub(super) fn connection(directory: &Directory, name: &str) -> Result<Connection> {
    let retained = regular_or_create(directory, name)?;
    let observation = Observation::of(&retained)?;
    let db = Connection::open_with_flags(
        directory.path().join(name),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    db.busy_timeout(LOCK_WAIT)?;
    let current = directory.open_file(name)?;
    let current = Observation::of(&current)?;
    if current.device != observation.device || current.inode != observation.inode {
        return Err(WorkspaceError::new(
            "workspace_unavailable",
            409,
            "Workspace database path changed while opening.",
        ));
    }
    Ok(db)
}

fn existing_connection(directory: &Directory, name: &str) -> Result<Connection> {
    let retained = existing_file(directory, name)?;
    let observation = Observation::of(&retained)?;
    let db = Connection::open_with_flags(
        directory.path().join(name),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    db.busy_timeout(LOCK_WAIT)?;
    let current = existing_file(directory, name)?;
    let current = Observation::of(&current)?;
    if current.device != observation.device || current.inode != observation.inode {
        return Err(WorkspaceError::new(
            "workspace_unavailable",
            409,
            "Workspace database path changed while opening.",
        ));
    }
    Ok(db)
}

impl WorkspaceManager {
    pub(crate) fn open(
        state_directory: &Path,
        library_root: PathBuf,
        resolve: SourceResolver,
        allowance: u64,
    ) -> Result<Self> {
        if allowance == 0 {
            return Err(WorkspaceError::new(
                "resource_limit",
                503,
                "Workspace storage allowance must be positive.",
            ));
        }
        let state = Directory::open(state_directory)?;
        let mut leases = ancestor_leases(state_directory, LOCK_WAIT)?;
        leases.push(state.lease(LeaseMode::Shared, LOCK_WAIT)?);
        let registry_lock = FileLease::acquire(
            regular_or_create(&state, ".workspaces-lock")?,
            LeaseMode::Exclusive,
            LOCK_WAIT,
        )?;
        let root = state.create_directory("workspaces")?;
        let registry = connection(&state, "workspaces.sqlite")?;
        registry.execute_batch(REGISTRY_SCHEMA)?;
        drop(registry_lock);
        Ok(Self {
            root,
            registry: Mutex::new(registry),
            library_root,
            resolve,
            allowance,
            queries: Mutex::new(query::Queries::new()),
            supervised_tools: Mutex::new(Vec::new()),
        })
    }
    pub(crate) fn lookup_photo(&self, photo_id: &str) -> Result<Option<Workspace>> {
        require_id(photo_id)?;
        Ok(self
            .registry
            .lock()
            .query_row(
                "SELECT id,photo_id FROM workspaces WHERE photo_id=?1 AND state='active'",
                [photo_id],
                |row| {
                    Ok(Workspace {
                        id: row.get(0)?,
                        photo_id: row.get(1)?,
                    })
                },
            )
            .optional()?)
    }
    pub(super) fn lookup(&self, id: &str) -> Result<Workspace> {
        require_id(id)?;
        self.registry
            .lock()
            .query_row(
                "SELECT id,photo_id FROM workspaces WHERE id=?1 AND state='active'",
                [id],
                |row| {
                    Ok(Workspace {
                        id: row.get(0)?,
                        photo_id: row.get(1)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| {
                WorkspaceError::new(
                    "not_found",
                    404,
                    "The workspace is absent or has been deleted.",
                )
            })
    }
    pub(super) fn create_workspace(&self, photo_id: &str) -> Result<Workspace> {
        require_id(photo_id)?;
        let _ancestors = ancestor_leases(self.root.path(), LOCK_WAIT)?;
        let _root = self.root.lease(LeaseMode::Shared, LOCK_WAIT)?;
        // Resolve only after the shared root lease is held. Recovery's
        // exclusive lease therefore wins before a new binding can be admitted.
        (self.resolve)(photo_id)?.ok_or_else(|| {
            WorkspaceError::new("not_found", 404, "The Photo is not in the Library.")
        })?;
        let mut registry = self.registry.lock();
        let transaction =
            registry.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if let Some(workspace) = transaction
            .query_row(
                "SELECT id,photo_id FROM workspaces WHERE photo_id=?1 AND state='active'",
                [photo_id],
                |row| {
                    Ok(Workspace {
                        id: row.get(0)?,
                        photo_id: row.get(1)?,
                    })
                },
            )
            .optional()?
        {
            return Ok(workspace);
        }
        let workspace = Workspace {
            id: identity("workspace")?,
            photo_id: photo_id.to_owned(),
        };
        let directory = self.root.create_directory(&workspace.id)?;
        let _admission = FileLease::acquire(
            regular_or_create(&directory, WORKSPACE_ADMISSION_LOCK)?,
            LeaseMode::Exclusive,
            LOCK_WAIT,
        )?;
        let _lock = FileLease::acquire(
            regular_or_create(&directory, ".workspace-lock")?,
            LeaseMode::Exclusive,
            LOCK_WAIT,
        )?;
        let db = connection(&directory, ".workspace.sqlite")?;
        db.execute_batch(WORKSPACE_SCHEMA)?;
        db.execute(
            "INSERT INTO settings(key,value) VALUES('initialized','false'),('defaultInput','null')",
            [],
        )?;
        transaction.execute(
            "INSERT INTO workspaces(id,photo_id,state) VALUES(?1,?2,'active')",
            params![workspace.id, photo_id],
        )?;
        transaction.commit()?;
        Ok(workspace)
    }
    pub(crate) fn directory(&self, workspace_id: &str) -> Result<WorkspaceDirectory> {
        // Keep recovery and workspace withdrawal excluded for the lifetime of
        // the returned directory; direct tool writers can mutate before a
        // workspace session is opened.
        let root_lease = self.root.lease(LeaseMode::Shared, LOCK_WAIT)?;
        let workspace = self.lookup(workspace_id)?;
        let directory = self.root.child(workspace_id)?;
        let admission_lock = regular_or_create(&directory, WORKSPACE_ADMISSION_LOCK)?;
        let admission_lease = FileLease::acquire(admission_lock, LeaseMode::Shared, LOCK_WAIT)?;
        let current = self.lookup(workspace_id)?;
        if current.photo_id != workspace.photo_id {
            return Err(WorkspaceError::new(
                "workspace_unavailable",
                409,
                "Workspace association changed while it was being opened.",
            ));
        }
        Ok(WorkspaceDirectory {
            directory,
            _root_lease: root_lease,
            _admission_lease: admission_lease,
        })
    }

    pub(super) fn locked(&self, id: &str, mode: LeaseMode) -> Result<WorkspaceSession> {
        self.locked_inner(id, mode, true, true)
    }

    fn locked_inner(
        &self,
        id: &str,
        mode: LeaseMode,
        include_root: bool,
        initialize: bool,
    ) -> Result<WorkspaceSession> {
        let workspace = self.lookup(id)?;
        let directory = self.root.child(id)?;
        let mut leases = if include_root {
            ancestor_leases(directory.path(), LOCK_WAIT)?
        } else {
            Vec::new()
        };
        leases.push(directory.lease(mode, LOCK_WAIT)?);
        // Recheck association after acquiring per-workspace coordination.
        // A waiter must not mutate a workspace withdrawn while it waited.
        let current = self.lookup(id)?;
        if current.photo_id != workspace.photo_id {
            return Err(WorkspaceError::new(
                "workspace_unavailable",
                409,
                "Workspace association changed while it was being opened.",
            ));
        }
        let lock = if initialize {
            regular_or_create(&directory, ".workspace-lock")?
        } else {
            existing_file(&directory, ".workspace-lock")?
        };
        leases.push(FileLease::acquire(lock, LeaseMode::Exclusive, LOCK_WAIT)?);
        let db = if initialize {
            connection(&directory, ".workspace.sqlite")?
        } else {
            existing_connection(&directory, ".workspace.sqlite")?
        };
        if initialize {
            db.execute_batch(WORKSPACE_SCHEMA)?;
        }
        Ok(WorkspaceSession {
            workspace,
            directory,
            db,
            _leases: leases,
        })
    }

    /// Takes the exclusive workspace-root lease used to coordinate a
    /// Retire-and-Bind commit with every cooperating workspace writer.
    pub(crate) fn recovery_guard(&self) -> Result<FileLease> {
        self.root
            .lease(LeaseMode::Exclusive, LOCK_WAIT)
            .map_err(Into::into)
    }

    /// Reads authoritative workspace references while `recovery_guard` is
    /// held. It must not reacquire the root's shared lease.
    pub(crate) fn retained_photo_references_under_recovery_guard(&self) -> Result<Vec<String>> {
        self.retained_photo_references_inner(false)
    }

    pub(crate) fn workspace_ids(&self) -> Result<Vec<String>> {
        let registry = self.registry.lock();
        let mut statement =
            registry.prepare("SELECT id FROM workspaces WHERE state='active' ORDER BY id")?;
        let ids = statement
            .query_map([], |row| row.get(0))?
            .collect::<std::result::Result<Vec<String>, _>>()?;
        if ids.len() > ENTRY_MAX {
            return Err(WorkspaceError::new(
                "resource_limit",
                503,
                "Workspace inventory exceeds the admitted limit.",
            ));
        }
        Ok(ids)
    }
    pub(super) fn reclaim_directory(&self, id: &str) -> Result<Directory> {
        require_id(id)?;
        let deleted: bool = self
            .registry
            .lock()
            .query_row(
                "SELECT state='deleted' FROM workspaces WHERE id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(false);
        if !deleted {
            return Err(WorkspaceError::new(
                "conflict",
                409,
                "Only a withdrawn workspace can be reclaimed.",
            ));
        }
        self.root.child(id).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                WorkspaceError::new(
                    "not_found",
                    404,
                    "The withdrawn workspace directory has already been reclaimed.",
                )
            } else {
                error.into()
            }
        })
    }
    pub(super) fn withdraw(
        &self,
        id: &str,
        request: &str,
        intent: &Value,
        result: &Value,
    ) -> Result<()> {
        let mut registry = self.registry.lock();
        let transaction =
            registry.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        transaction.execute("INSERT INTO deletions(workspace_id,request_id,intent,result,state) VALUES(?1,?2,?3,?4,'reclaiming')",params![id,request,encode(intent)?,encode(result)?])?;
        transaction.execute("UPDATE workspaces SET state='deleted' WHERE id=?1", [id])?;
        transaction.commit()?;
        Ok(())
    }
    pub(super) fn deletion_receipt(
        &self,
        id: &str,
        request: &str,
        intent: &Value,
    ) -> Result<Option<Value>> {
        let row: Option<(String, String)> = self
            .registry
            .lock()
            .query_row(
                "SELECT intent,result FROM deletions WHERE workspace_id=?1 AND request_id=?2",
                params![id, request],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        match row {
            Some((prior, result)) if prior == encode(intent)? => Ok(Some(value(&result)?)),
            Some(_) => Err(WorkspaceError::new(
                "request_conflict",
                409,
                "The request identity belongs to different deletion intent.",
            )),
            None => Ok(None),
        }
    }
    pub(super) fn finish_deletion(&self, id: &str, request: &str, result: &Value) -> Result<()> {
        self.registry.lock().execute("UPDATE deletions SET result=?3,state='settled' WHERE workspace_id=?1 AND request_id=?2",params![id,request,encode(result)?])?;
        Ok(())
    }
    pub(crate) fn register_dependency(
        &self,
        id: &str,
        artifact: &str,
        revision: &str,
        owner: &str,
        kind: &str,
    ) -> Result<()> {
        let mut session = self.locked(id, LeaseMode::Shared)?;
        files::reconcile(self, &mut session)?;
        let item = files::file(&session, artifact)?;
        if item["revision"].as_str() != Some(revision) || item["state"].as_str() != Some("ready") {
            return Err(WorkspaceError::new(
                "artifact_changed",
                409,
                "The selected file changed before the dependency was retained.",
            ));
        }
        session.db.execute("INSERT OR REPLACE INTO dependencies(owner,kind,artifact_id,revision) VALUES(?1,?2,?3,?4)",params![owner,kind,artifact,revision])?;
        Ok(())
    }
    pub(crate) fn release_dependency(&self, owner: &str) -> Result<()> {
        for id in self.workspace_ids()? {
            let session = self.locked(&id, LeaseMode::Shared)?;
            session
                .db
                .execute("DELETE FROM dependencies WHERE owner=?1", [owner])?;
        }
        Ok(())
    }
    pub(crate) fn processing_export_dependency_owners(&self) -> Result<Vec<String>> {
        let mut owners = std::collections::BTreeSet::new();
        for id in self.workspace_ids()? {
            let session = self.locked(&id, LeaseMode::Shared)?;
            let mut statement=session.db.prepare("SELECT DISTINCT owner FROM dependencies WHERE substr(owner,1,18)='processing-export:'")?;
            for owner in statement.query_map([], |row| row.get::<_, String>(0))? {
                owners.insert(owner?);
            }
        }
        Ok(owners.into_iter().collect())
    }

    /// The Photo identities every active workspace still retains, read from
    /// the authoritative workspace registry and each workspace's own
    /// database. An active workspace binds its Photo through the registry;
    /// retained workspace inputs and completed outputs bind the Photos named
    /// by their provenance, so a retire-and-bind can never replace a Photo
    /// that a workspace still holds. Any unreadable registry or workspace
    /// database is an error: a caller must fail closed rather than treat an
    /// unknown reference as absent.
    /// Reads references without creating a missing workspace lock or database.
    /// A recovery survey must preserve missing state and fail closed instead.
    pub(crate) fn retained_photo_references(&self) -> Result<Vec<String>> {
        self.retained_photo_references_inner(true)
    }

    fn retained_photo_references_inner(&self, include_root: bool) -> Result<Vec<String>> {
        let mut referenced = std::collections::BTreeSet::new();
        for id in self.workspace_ids()? {
            let session = self.locked_inner(&id, LeaseMode::Shared, include_root, false)?;
            referenced.insert(session.workspace.photo_id.clone());
            let mut statement = session.db.prepare(
                "SELECT provenance FROM files WHERE removed=0 AND provenance IS NOT NULL",
            )?;
            let rows = statement
                .query_map([], |row| row.get::<_, Option<String>>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            for row in rows {
                let Some(row) = row else { continue };
                let Ok(provenance) = value(&row) else {
                    return Err(WorkspaceError::new(
                        "workspace_unavailable",
                        503,
                        "Retained workspace provenance could not be read.",
                    ));
                };
                referenced.extend(provenance_photo_ids(&provenance)?);
            }
        }
        Ok(referenced.into_iter().collect())
    }

    pub(crate) fn record_artifact_alias(
        &self,
        id: &str,
        workspace_id: &str,
        artifact_id: &str,
        revision: &str,
    ) -> Result<()> {
        let registry = self.registry.lock();
        registry.execute("INSERT OR IGNORE INTO artifact_aliases(id,workspace_id,artifact_id,revision) VALUES(?1,?2,?3,?4)",params![id,workspace_id,artifact_id,revision])?;
        let actual: (String, String, String) = registry.query_row(
            "SELECT workspace_id,artifact_id,revision FROM artifact_aliases WHERE id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        if actual
            != (
                workspace_id.to_owned(),
                artifact_id.to_owned(),
                revision.to_owned(),
            )
        {
            return Err(WorkspaceError::new(
                "conflict",
                409,
                "An artifact identity is already bound to different bytes.",
            ));
        }
        Ok(())
    }
    pub(crate) fn resolve_artifact_alias(&self, id: &str) -> Result<Option<Value>> {
        let row: Option<(String, String, String)> = self
            .registry
            .lock()
            .query_row(
                "SELECT workspace_id,artifact_id,revision FROM artifact_aliases WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        Ok(row.map(|(workspace,artifact,revision)|json!({"workspaceId":workspace,"artifactId":artifact,"revision":revision})))
    }
}

pub(super) fn replay(
    session: &WorkspaceSession,
    request: &str,
    intent: &Value,
) -> Result<Option<Value>> {
    let prior: Option<(String, String)> = session
        .db
        .query_row(
            "SELECT intent,result FROM receipts WHERE request_id=?1",
            [request],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    match prior {
        Some((prior, result)) if prior == encode(intent)? => Ok(Some(value(&result)?)),
        Some(_) => Err(WorkspaceError::new(
            "request_conflict",
            409,
            "The request identity belongs to different workspace intent.",
        )),
        None => Ok(None),
    }
}
pub(super) fn record_receipt(
    session: &WorkspaceSession,
    request: &str,
    intent: &Value,
    result: &Value,
) -> Result<()> {
    session.db.execute(
        "INSERT INTO receipts(request_id,intent,result) VALUES(?1,?2,?3)",
        params![request, encode(intent)?, encode(result)?],
    )?;
    Ok(())
}
pub(super) fn setting(session: &WorkspaceSession, key: &str) -> Result<Value> {
    let text: String =
        session
            .db
            .query_row("SELECT value FROM settings WHERE key=?1", [key], |row| {
                row.get(0)
            })?;
    value(&text)
}
pub(super) fn set_setting(session: &WorkspaceSession, key: &str, value: &Value) -> Result<()> {
    session.db.execute("INSERT INTO settings(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,encode(value)?])?;
    Ok(())
}
pub(super) fn token(session: &WorkspaceSession, inventory: &Value) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut statement=session.db.prepare("SELECT id,path,kind,state,evidence,contract,managed,provenance,removed,image_facts,source_revision,revision FROM files ORDER BY id")?;
    let records=statement.query_map([],|row|Ok(json!({"id":row.get::<_,String>(0)?,"path":row.get::<_,String>(1)?,"kind":row.get::<_,String>(2)?,"state":row.get::<_,String>(3)?,"evidence":row.get::<_,Option<String>>(4)?,"contract":row.get::<_,Option<String>>(5)?,"managed":row.get::<_,bool>(6)?,"provenance":row.get::<_,Option<String>>(7)?,"removed":row.get::<_,bool>(8)?,"imageFacts":row.get::<_,Option<String>>(9)?,"sourceRevision":row.get::<_,Option<String>>(10)?,"revision":row.get::<_,Option<String>>(11)?})))?.collect::<std::result::Result<Vec<_>,_>>()?;
    let mut capture_query=session.db.prepare("SELECT artifact_id,revision,captured_id,captured_revision FROM captures ORDER BY artifact_id,revision")?;
    let captures = capture_query
        .query_map([], |row| {
            Ok(json!([
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?
            ]))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let evidence = json!({"workspaceId":session.workspace.id,"defaultInput":setting(session,"defaultInput")?,"files":records,"captures":captures,"inventory":inventory});
    Ok(format!(
        "{:x}",
        Sha256::digest(photo_files::canonical_json(&evidence)?)
    ))
}
pub(super) fn evidence(session: &WorkspaceSession, id: &str) -> Result<Evidence> {
    let encoded: String = session.db.query_row(
        "SELECT evidence FROM files WHERE id=?1 AND removed=0",
        [id],
        |row| row.get(0),
    )?;
    Ok(serde_json::from_str(&encoded)?)
}

/// The Photo identities named by one retained provenance document. Both
/// retained processing provenance shapes carry their owning Photo explicitly,
/// so a Photo an active workspace retains is never a retireable occupant.
fn provenance_photo_ids(provenance: &Value) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    let invalid = || {
        WorkspaceError::new(
            "workspace_unavailable",
            503,
            "Retained workspace provenance has an invalid Photo identity.",
        )
    };
    if provenance.get("photoId").is_some() {
        ids.push(
            provenance["photoId"]
                .as_str()
                .filter(|id| valid_id(id))
                .ok_or_else(invalid)?
                .to_owned(),
        );
    }
    if provenance.get("handle").is_some() {
        let handle = provenance["handle"].as_object().ok_or_else(invalid)?;
        if handle.get("photoId").is_some() {
            ids.push(
                handle["photoId"]
                    .as_str()
                    .filter(|id| valid_id(id))
                    .ok_or_else(invalid)?
                    .to_owned(),
            );
        }
    }
    if provenance.get("input").is_some() {
        let input = provenance["input"].as_object().ok_or_else(invalid)?;
        if input.get("binding").is_some() {
            let binding = input["binding"].as_object().ok_or_else(invalid)?;
            if binding.get("photoId").is_some() {
                ids.push(
                    binding["photoId"]
                        .as_str()
                        .filter(|id| valid_id(id))
                        .ok_or_else(invalid)?
                        .to_owned(),
                );
            }
        }
    }
    Ok(ids)
}
