mod continuations;
mod create;
mod files;
mod jobs;
mod migration;
mod process;
mod proxy;
mod receipts;
mod restore;
mod transfer;
pub(crate) use restore::rebase_restored_host;
pub(crate) fn restore_state(path: &Path, roots: &Value, cancel: &AtomicBool) -> Result<Value> {
    process::invoke(
        Some(path),
        "__restore-rebase",
        json!({"roots":roots}),
        cancel,
        Duration::from_secs(120),
    )
}
pub(crate) mod http;
use crate::workspace::WorkspaceManager;
use photo_files::Directory;
pub(crate) use process::inspect_state_retained;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
    time::Duration,
};
pub(crate) type Result<T, E = ServiceError> = std::result::Result<T, E>;
#[derive(Debug, Clone)]
pub(crate) struct ServiceError {
    pub value: Value,
    pub status: u16,
}
impl ServiceError {
    pub(crate) fn new(code: &str, message: impl Into<String>, status: u16) -> Self {
        Self {
            value: json!({"error":{"code":code,"message":message.into(),"effect":"none","details":{}}}),
            status,
        }
    }
    pub(crate) fn io(e: impl std::fmt::Display) -> Self {
        Self::new("local_io_failed", e.to_string(), 500)
    }
    pub(crate) fn unknown(message: &str) -> Self {
        Self::new("unknown_result", message, 409).effect("unknown")
    }
    pub(crate) fn effect(mut self, effect: &str) -> Self {
        self.value["error"]["effect"] = json!(effect);
        self
    }
    pub(crate) fn tool(error: Value) -> Self {
        let status = match error["code"].as_str().unwrap_or("") {
            "not_found" => 404,
            "state_conflict" | "request_conflict" | "edit_ambiguous" => 409,
            "engine_unavailable" => 503,
            _ => 400,
        };
        Self {
            value: json!({"error":error}),
            status,
        }
    }
}
impl From<crate::workspace::WorkspaceError> for ServiceError {
    fn from(e: crate::workspace::WorkspaceError) -> Self {
        Self {
            status: e.status,
            value: json!({"error":{"code":e.code,"message":e.message,"details":e.details,"effect":e.effect}}),
        }
    }
}
pub(crate) struct ToolService {
    pub(crate) workspaces: Arc<WorkspaceManager>,
    pub(crate) root: Directory,
    pub(crate) jobs: jobs::Jobs,
}
impl ToolService {
    pub(crate) fn open(
        state_directory: &Path,
        workspaces: Arc<WorkspaceManager>,
    ) -> std::result::Result<Self, String> {
        let root =
            Directory::create(state_directory.join("darktable-host")).map_err(|e| e.to_string())?;
        let jobs = jobs::Jobs::open(&root).map_err(|e| e.value.to_string())?;
        if process::invoke(
            None,
            "modules list",
            json!({}),
            &AtomicBool::new(false),
            Duration::from_secs(30),
        )
        .is_ok()
        {
            workspaces.set_supervised_tools(vec!["darktable".into()]);
        } else {
            workspaces.set_supervised_tools(Vec::new());
        }
        Ok(Self {
            workspaces,
            root,
            jobs,
        })
    }
    pub(crate) fn begin_shutdown(&self) {
        self.jobs.begin_shutdown();
    }
    pub(crate) async fn shutdown(&self) -> std::result::Result<(), String> {
        self.jobs.shutdown().await.map_err(|e| e.value.to_string())
    }
    pub(crate) fn command(
        &self,
        photo: Option<&str>,
        edit: Option<&str>,
        operation: &str,
        mut arguments: Value,
    ) -> Result<(u16, Value)> {
        if operation.starts_with('_') || operation == "open" {
            return Err(ServiceError::new(
                "invalid_input",
                "Private tool operations are not remote commands",
                400,
            ));
        }
        if forbidden_paths(&arguments) {
            return Err(ServiceError::new(
                "invalid_input",
                "Server paths are host-owned",
                400,
            ));
        }
        if operation == "edit create" {
            let (status, result) = self.create_edit(
                photo.ok_or_else(|| {
                    ServiceError::new("invalid_input", "Edit creation requires photo", 400)
                })?,
                edit,
                arguments,
            )?;
            if let (Some(photo), Some(workspace), Some(edit)) = (
                result["photoId"].as_str(),
                result["workspaceId"].as_str(),
                result["editId"].as_str(),
            ) {
                self.retain_continuations(
                    &result,
                    "edit show",
                    Some(&(photo.into(), workspace.into(), edit.into())),
                    None,
                )?;
            }
            return Ok((status, result));
        }
        let object = arguments.as_object_mut().ok_or_else(|| {
            ServiceError::new("invalid_input", "arguments must be an object", 400)
        })?;
        for key in [
            "stateDirectory",
            "inputPath",
            "directory",
            "previewDirectory",
            "outputDirectory",
            "captureId",
        ] {
            if object.contains_key(key) {
                return Err(ServiceError::new(
                    "invalid_input",
                    format!("{key} is host-owned"),
                    400,
                ));
            }
        }
        let consumed = object
            .get("cursor")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let retained_scope = if let Some(token) = &consumed {
            if object.len() != 1 || photo.is_some() || edit.is_some() {
                return Err(ServiceError::new(
                    "invalid_cursor",
                    "Cursor continuation accepts no filters or target arguments",
                    400,
                ));
            }
            Some(self.continuation_scope(token, operation)?)
        } else {
            None
        };
        let photo = retained_scope
            .as_ref()
            .and_then(|(p, _)| p.as_deref())
            .or(photo);
        let edit = retained_scope
            .as_ref()
            .and_then(|(_, e)| e.as_deref())
            .or(edit);
        if let Some(photo) = photo {
            if matches!(
                operation,
                "export status" | "export wait" | "export cancel" | "export retry"
            ) {
                if object
                    .keys()
                    .any(|k| !matches!(k.as_str(), "id" | "requestId"))
                {
                    return Err(ServiceError::new(
                        "invalid_input",
                        "Unknown job command field",
                        400,
                    ));
                }
                let id = object
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ServiceError::new("invalid_input", "id required", 400))?;
                if operation == "export retry" {
                    return Ok((
                        202,
                        self.jobs.retry(
                            photo,
                            id,
                            object.get("requestId").and_then(Value::as_str),
                        )?,
                    ));
                }
                return Ok((
                    200,
                    self.jobs.status(photo, id, operation == "export cancel")?,
                ));
            }
            if operation == "export list" {
                return Ok((200, self.jobs.list(photo, edit)));
            }
        }
        let mut context = None;
        let mut input_guard = None;
        let state = if let Some(photo) = photo {
            let workspace = if operation == "edit create" {
                self.workspaces.open_photo(photo)?
            } else {
                self.workspaces.resolve_photo(photo)?
            };
            let workspace_id = workspace["workspaceId"]
                .as_str()
                .ok_or_else(|| ServiceError::unknown("Workspace has no identity"))?;
            let directory = self.workspaces.directory(workspace_id)?;
            let edits = workspace["edits"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|e| e["tool"] == "darktable")
                .collect::<Vec<_>>();
            if edits.is_empty() && edit.is_none() && operation == "edit show" {
                let (guard, input) = self.workspaces.lease_input(
                    workspace_id,
                    workspace["inputArtifactId"].as_str().ok_or_else(|| {
                        ServiceError::new(
                            "input_unavailable",
                            "Workspace input is unavailable",
                            409,
                        )
                    })?,
                    workspace["inputRevision"].as_str().ok_or_else(|| {
                        ServiceError::new(
                            "input_unavailable",
                            "Workspace input has no revision",
                            409,
                        )
                    })?,
                )?;
                let mut result = process::invoke(
                    None,
                    "__show-input",
                    json!({"inputPath":input["path"]}),
                    &AtomicBool::new(false),
                    Duration::from_secs(30),
                )?;
                drop(guard);
                if let Some(o) = result.as_object_mut() {
                    o.insert("photoId".into(), json!(photo));
                    o.insert("workspaceId".into(), json!(workspace_id));
                    o.insert("editId".into(), Value::Null);
                }
                return Ok((200, result));
            }
            if edits.is_empty() && edit.is_none() && focused_mutation(operation) {
                let (_, created) = self.command(Some(photo), None, "edit create", json!({}))?;
                let id = created["editId"]
                    .as_str()
                    .ok_or_else(|| ServiceError::unknown("New edit identity missing"))?;
                return self.command(Some(photo), Some(id), operation, arguments);
            }
            let selected = if let Some(id) = edit {
                edits
                    .iter()
                    .find(|e| e["editId"] == id)
                    .cloned()
                    .ok_or_else(|| ServiceError::new("not_found", "Edit not found", 404))?
            } else {
                match edits.len() {
                    0 => {
                        return Err(ServiceError::new(
                            "not_found",
                            "No existing edit; create an edit with an explicit input",
                            404,
                        ));
                    }
                    1 => edits[0].clone(),
                    _ => {
                        return Err(ServiceError::new(
                            "edit_ambiguous",
                            "Select editId explicitly",
                            409,
                        ));
                    }
                }
            };
            let id = selected["editId"]
                .as_str()
                .ok_or_else(|| ServiceError::unknown("Edit has no identity"))?
                .to_owned();
            if !valid_id(&id) {
                return Err(ServiceError::unknown("Invalid workspace edit identity"));
            }
            let state = directory
                .child(
                    PathBuf::from(crate::workspace::layout::EDITS)
                        .join("darktable")
                        .join(&id),
                )
                .map_err(ServiceError::io)?;
            if operation == "edit rebind" {
                let input = object.remove("input").ok_or_else(|| {
                    ServiceError::new("invalid_input", "Explicit input binding required", 400)
                })?;
                let input = input.get("input").unwrap_or(&input);
                let (guard, admitted) = self.workspaces.lease_input(
                    workspace_id,
                    input["artifactId"].as_str().ok_or_else(|| {
                        ServiceError::new("invalid_input", "input.artifactId required", 400)
                    })?,
                    input["revision"].as_str().ok_or_else(|| {
                        ServiceError::new("invalid_input", "input.revision required", 400)
                    })?,
                )?;
                object.insert("input".into(), json!({"inputPath":admitted["path"]}));
                input_guard = Some(guard);
            }
            if operation == "preview render"
                && let Some(comparison) = object
                    .get_mut("input")
                    .and_then(|v| v.get_mut("comparison"))
                    .and_then(Value::as_object_mut)
                && let Some(id) = comparison.remove("editId")
            {
                let id = id.as_str().ok_or_else(|| {
                    ServiceError::new("invalid_input", "Comparison editId must be a string", 400)
                })?;
                if !edits.iter().any(|e| e["editId"] == id) {
                    return Err(ServiceError::new(
                        "not_found",
                        "Comparison edit not found",
                        404,
                    ));
                }
                let selected = directory
                    .child(
                        PathBuf::from(crate::workspace::layout::EDITS)
                            .join("darktable")
                            .join(id),
                    )
                    .map_err(ServiceError::io)?;
                comparison.insert("stateDirectory".into(), json!(selected.path()));
            }
            context = Some((photo.to_owned(), workspace_id.to_owned(), id));
            Some(state.path().to_owned())
        } else {
            if edit.is_some() || !is_global(operation) {
                return Err(ServiceError::new(
                    "invalid_input",
                    "Operation requires an authorized photo/edit",
                    400,
                ));
            }
            None
        };
        if operation == "export state-file" && arguments.get("file").is_none() {
            arguments["file"] = json!({"filename":format!("{}.xmp",arguments["requestId"].as_str().unwrap_or("state"))});
        }
        let transfer = self.prepare_transfer(photo, operation, &mut arguments)?;
        if matches!(
            operation,
            "preview render" | "export create" | "export state-file"
        ) {
            let (photo, workspace, edit) = context.ok_or_else(|| {
                ServiceError::new("invalid_input", "Render requires an edit", 400)
            })?;
            let result = self.jobs.admit(jobs::Admission {
                state: state.as_deref().unwrap(),
                photo: &photo,
                workspace: &workspace,
                edit: &edit,
                operation,
                arguments,
                transfer,
            })?;
            return Ok((202, result));
        }
        let _admission = if operation == "edit actions run" {
            Some(
                acquire_processing_admission(self.root.path().parent().unwrap())
                    .map_err(ServiceError::io)?,
            )
        } else {
            None
        };
        let mut result = process::invoke(
            state.as_deref(),
            operation,
            arguments,
            &AtomicBool::new(false),
            Duration::from_secs(120),
        )?;
        drop(input_guard);
        if let Some(transfer) = transfer {
            self.finish_transfer(&transfer, &mut result)?;
        }
        self.retain_continuations(&result, operation, context.as_ref(), consumed.as_deref())?;
        if let Some((photo, workspace, edit)) = context {
            attach(&mut result, &photo, &workspace, &edit);
        }
        Ok((200, result))
    }
}
fn attach(value: &mut Value, photo: &str, workspace: &str, edit: &str) {
    if let Some(o) = value.as_object_mut() {
        o.insert("photoId".into(), json!(photo));
        o.insert("workspaceId".into(), json!(workspace));
        o.insert("editId".into(), json!(edit));
        o.insert(
            "webUrl".into(),
            json!(format!("/?photoId={photo}&edit=1&editId={edit}")),
        );
    }
}
pub(crate) fn valid_id(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}
fn is_global(operation: &str) -> bool {
    matches!(
        operation,
        "modules list"
            | "modules describe"
            | "resources kinds"
            | "resources list"
            | "resources show"
            | "resources import"
            | "resources export"
            | "resources remove"
            | "presets list"
            | "presets show"
            | "presets import"
            | "presets export"
            | "presets remove"
            | "styles list"
            | "styles show"
            | "styles import"
            | "styles export"
            | "styles remove"
    )
}
pub(crate) fn acquire_processing_admission(
    state_directory: &Path,
) -> std::result::Result<photo_files::FileLease, String> {
    Directory::create(state_directory.join(".processing-admission"))
        .and_then(|d| d.lease(photo_files::LeaseMode::Exclusive, Duration::from_secs(900)))
        .map_err(|e| e.to_string())
}
fn forbidden_paths(value: &Value) -> bool {
    match value {
        Value::Object(o) => o.iter().any(|(k, v)| {
            matches!(
                k.as_str(),
                "stateDirectory"
                    | "inputPath"
                    | "directory"
                    | "previewDirectory"
                    | "outputDirectory"
                    | "captureId"
                    | "path"
            ) || forbidden_paths(v)
        }),
        Value::Array(a) => a.iter().any(forbidden_paths),
        _ => false,
    }
}
fn focused_mutation(operation: &str) -> bool {
    matches!(
        operation,
        "edit set"
            | "edit apply"
            | "edit reset"
            | "edit instances add"
            | "edit instances clone"
            | "edit instances enable"
            | "edit instances remove"
            | "edit instances reorder"
            | "edit masks create"
            | "edit masks set"
            | "edit masks remove"
            | "edit output set"
            | "edit actions run"
            | "presets apply"
            | "styles apply"
    )
}
