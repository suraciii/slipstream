//! Persistent Photo directories and their service-owned file index.
//! Tool settings remain in each tool's independently readable state directory.
mod completed;
mod dependencies;
mod files;
pub(crate) mod layout;
mod preparation;
#[cfg(test)]
mod preparation_tests;
mod query;
mod removal;
mod restore;
mod store;
pub(crate) use restore::restore as restore_verified;

use parking_lot::Mutex;
use photo_files::{Directory, FileLease, LeaseMode};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::{fs::File, io, path::PathBuf, sync::Arc, time::Duration};
use store::{record_receipt, replay, token};

pub(crate) const FILE_BYTES_MAX: u64 = 16 * 1024 * 1024 * 1024;
pub(crate) const DEFAULT_STORAGE_BYTES: u64 = 64 * 1024 * 1024 * 1024;
pub(crate) const ENTRY_MAX: usize = 100_000;
pub(crate) const LOCK_WAIT: Duration = Duration::from_secs(2);
pub(crate) type Result<T, E = WorkspaceError> = std::result::Result<T, E>;

#[derive(Debug)]
pub(crate) struct WorkspaceError {
    pub code: &'static str,
    pub message: String,
    pub details: Value,
    pub status: u16,
    pub effect: &'static str,
}
impl WorkspaceError {
    pub(crate) fn new(code: &'static str, status: u16, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: json!({}),
            status,
            effect: "none",
        }
    }
    pub(crate) fn detail(mut self, details: Value) -> Self {
        self.details = details;
        self
    }
}
impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl From<io::Error> for WorkspaceError {
    fn from(error: io::Error) -> Self {
        let (code, status) = match error.kind() {
            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => ("workspace_busy", 409),
            io::ErrorKind::NotFound => ("workspace_unavailable", 404),
            io::ErrorKind::StorageFull => ("resource_limit", 507),
            io::ErrorKind::PermissionDenied => ("workspace_unavailable", 403),
            _ => ("workspace_unavailable", 503),
        };
        Self::new(code, status, error.to_string())
    }
}
impl From<rusqlite::Error> for WorkspaceError {
    fn from(error: rusqlite::Error) -> Self {
        Self::new("workspace_unavailable", 503, error.to_string())
    }
}
impl From<serde_json::Error> for WorkspaceError {
    fn from(error: serde_json::Error) -> Self {
        Self::new("workspace_unavailable", 503, error.to_string())
    }
}

#[derive(Clone)]
pub(crate) struct Source {
    pub relative_path: slipstream_core::RelativeOriginalPath,
    pub revision: Option<String>,
}
pub(crate) type SourceResolver = Arc<dyn Fn(&str) -> Option<Source> + Send + Sync>;

pub(crate) struct WorkspaceManager {
    root: Directory,
    registry: Mutex<Connection>,
    library_root: PathBuf,
    resolve: SourceResolver,
    allowance: u64,
    queries: Mutex<query::Queries>,
    supervised_tools: Mutex<Vec<String>>,
}

#[derive(Clone, Debug)]
pub(crate) struct Workspace {
    pub id: String,
    pub photo_id: String,
}
pub(crate) struct WorkspaceSession {
    pub workspace: Workspace,
    pub directory: Directory,
    pub db: Connection,
    pub _leases: Vec<FileLease>,
}
pub(crate) struct WorkspaceDownload {
    pub file: File,
    pub filename: String,
    pub byte_length: u64,
    pub sha256: String,
    pub revision: String,
    pub content_type: String,
    pub leases: Vec<FileLease>,
}

pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
fn require_id(id: &str) -> Result<()> {
    if valid_id(id) {
        Ok(())
    } else {
        Err(WorkspaceError::new(
            "invalid_input",
            400,
            "Use the returned workspace or file identity.",
        ))
    }
}
pub(super) fn value(text: &str) -> Result<Value> {
    Ok(serde_json::from_str(text)?)
}
pub(super) fn encode(value: &Value) -> Result<String> {
    String::from_utf8(photo_files::canonical_json(value)?)
        .map_err(|e| WorkspaceError::new("invalid_input", 400, e.to_string()))
}

impl WorkspaceManager {
    pub(crate) fn set_supervised_tools(&self, tools: Vec<String>) {
        *self.supervised_tools.lock() = tools;
    }
    pub(crate) fn inspect(&self, workspace_id: &str, artifact_id: &str) -> Result<Value> {
        require_id(artifact_id)?;
        let mut session = self.locked(workspace_id, LeaseMode::Shared)?;
        files::reconcile(self, &mut session)?;
        files::inspect(self, &mut session, artifact_id)
    }
    pub(crate) fn download(
        &self,
        workspace_id: &str,
        artifact_id: &str,
        revision: Option<&str>,
    ) -> Result<WorkspaceDownload> {
        require_id(artifact_id)?;
        let mut session = self.locked(workspace_id, LeaseMode::Shared)?;
        files::reconcile(self, &mut session)?;
        files::download(self, session, artifact_id, revision)
    }
    pub(crate) fn admit_input(
        &self,
        workspace_id: &str,
        artifact_id: &str,
        revision: &str,
    ) -> Result<Value> {
        require_id(artifact_id)?;
        let mut session = self.locked(workspace_id, LeaseMode::Shared)?;
        files::reconcile(self, &mut session)?;
        files::admit(self, &mut session, artifact_id, revision)
    }
    pub(crate) fn lease_input(
        &self,
        workspace_id: &str,
        artifact_id: &str,
        revision: &str,
    ) -> Result<(WorkspaceDownload, Value)> {
        require_id(artifact_id)?;
        let mut session = self.locked(workspace_id, LeaseMode::Shared)?;
        let mut admitted = files::admit(self, &mut session, artifact_id, revision)?;
        admitted["workspaceId"] = json!(workspace_id);
        let id = admitted["artifactId"].as_str().ok_or_else(|| {
            WorkspaceError::new(
                "artifact_unavailable",
                409,
                "Input admission has no retained identity.",
            )
        })?;
        let revision = admitted["revision"].as_str().ok_or_else(|| {
            WorkspaceError::new(
                "artifact_unavailable",
                409,
                "Input admission has no checked revision.",
            )
        })?;
        let download = files::download(self, session, id, Some(revision))?;
        Ok((download, admitted))
    }
    pub(crate) fn prepared_input(&self, photo_id: &str, source_revision: &str) -> Result<Value> {
        let workspace = self.lookup_photo(photo_id)?.ok_or_else(|| {
            WorkspaceError::new("not_found", 404, "Open the Photo Workspace first.")
        })?;
        let mut session = self.locked(&workspace.id, LeaseMode::Shared)?;
        files::reconcile(self, &mut session)?;
        let id: Option<String> = session.db.query_row(
            "SELECT id FROM files WHERE removed=0 AND managed=1 AND kind='input' AND state='ready' AND source_revision=?1 ORDER BY id LIMIT 1",
            [source_revision], |row| row.get(0),
        ).optional()?;
        let id = id.ok_or_else(|| {
            WorkspaceError::new(
                "not_found",
                404,
                "Prepare the observed input revision explicitly before rebinding.",
            )
        })?;
        let file = files::file(&session, &id)?;
        let revision = file["revision"].as_str().ok_or_else(|| {
            WorkspaceError::new(
                "artifact_changed",
                409,
                "The prepared input is no longer ready.",
            )
        })?;
        files::admit(self, &mut session, &id, revision)
    }
}
use rusqlite::OptionalExtension;
