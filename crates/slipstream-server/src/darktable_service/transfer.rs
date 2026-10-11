use super::{Result, ServiceError, ToolService};
use photo_files::{Directory, FileLease, LeaseMode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{fs::File, path::PathBuf, time::Duration};
pub(crate) const MAX_BYTES: u64 = 256 * 1024 * 1024;
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Transfer {
    pub id: String,
    pub scope: Option<String>,
    pub filename: String,
    pub path: PathBuf,
}
fn filename(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 240
        && !value.contains(['/', '\\', '\r', '\n', '\0', '"'])
        && value != "."
        && value != ".."
}
impl ToolService {
    pub(crate) fn transfer_directory(&self, scope: Option<&str>) -> Result<Directory> {
        if let Some(photo) = scope {
            self.workspaces.resolve_photo(photo)?;
            if !super::valid_id(photo) {
                return Err(ServiceError::new(
                    "invalid_input",
                    "Invalid Photo identity",
                    400,
                ));
            }
        }
        self.root
            .create_directory("transfers")
            .and_then(|d| d.create_directory(scope.unwrap_or("global")))
            .map_err(ServiceError::io)
    }
    pub(crate) fn upload_start(&self, scope: Option<&str>, name: &str) -> Result<(Transfer, File)> {
        if !filename(name) {
            return Err(ServiceError::new(
                "invalid_input",
                "Invalid transfer filename",
                400,
            ));
        }
        let directory = self.transfer_directory(scope)?;
        let id = photo_files::identity("transfer").map_err(ServiceError::io)?;
        let container = directory.create_directory(&id).map_err(ServiceError::io)?;
        let file = container
            .create_file(name, 0o600)
            .map_err(ServiceError::io)?;
        Ok((
            Transfer {
                id,
                scope: scope.map(str::to_owned),
                filename: name.into(),
                path: container.path().join(name),
            },
            file,
        ))
    }
    pub(crate) fn upload_finish(&self, transfer: &Transfer, sha: &str) -> Result<Value> {
        let directory = self
            .transfer_directory(transfer.scope.as_deref())?
            .child(&transfer.id)
            .map_err(ServiceError::io)?;
        let mut file = directory
            .open_file(&transfer.filename)
            .map_err(ServiceError::io)?;
        file.sync_all().map_err(ServiceError::io)?;
        let evidence = photo_files::fingerprint(&mut file, MAX_BYTES).map_err(ServiceError::io)?;
        if evidence.sha256 != sha {
            return Err(ServiceError::new(
                "invalid_input",
                "Transfer digest differs from supplied SHA-256",
                400,
            ));
        }
        photo_files::atomic_json(
            &directory,
            "receipt.json",
            &json!({"transfer":transfer,"evidence":evidence}),
        )
        .map_err(ServiceError::io)?;
        Ok(
            json!({"transferId":transfer.id,"filename":transfer.filename,"byteLength":evidence.observation.byte_length,"sha256":evidence.sha256}),
        )
    }
    pub(crate) fn prepare_transfer(
        &self,
        scope: Option<&str>,
        operation: &str,
        args: &mut Value,
    ) -> Result<Option<Transfer>> {
        let Some(file) = args.get("file").cloned() else {
            return Ok(None);
        };
        let object = file.as_object().ok_or_else(|| {
            ServiceError::new(
                "invalid_input",
                "file must be an authorized transfer handle",
                400,
            )
        })?;
        if operation.ends_with(" import") {
            if object.len() != 1 {
                return Err(ServiceError::new(
                    "invalid_input",
                    "Only transferId is accepted for import",
                    400,
                ));
            }
            let id = object
                .get("transferId")
                .and_then(Value::as_str)
                .filter(|s| super::valid_id(s))
                .ok_or_else(|| ServiceError::new("invalid_input", "transferId required", 400))?;
            let directory = self
                .transfer_directory(scope)?
                .child(id)
                .map_err(ServiceError::io)?;
            let receipt = photo_files::read_json(
                directory
                    .open_file("receipt.json")
                    .map_err(ServiceError::io)?,
                16384,
            )
            .map_err(ServiceError::io)?;
            let transfer: Transfer =
                serde_json::from_value(receipt["transfer"].clone()).map_err(ServiceError::io)?;
            if transfer.scope.as_deref() != scope || transfer.id != id {
                return Err(ServiceError::new(
                    "invalid_input",
                    "Transfer scope mismatch",
                    400,
                ));
            }
            let mut input = directory
                .open_file(&transfer.filename)
                .map_err(ServiceError::io)?;
            let evidence =
                photo_files::fingerprint(&mut input, MAX_BYTES).map_err(ServiceError::io)?;
            if serde_json::to_value(&evidence).map_err(ServiceError::io)? != receipt["evidence"] {
                return Err(ServiceError::new(
                    "state_conflict",
                    "Uploaded transfer changed",
                    409,
                ));
            }
            let content = self
                .transfer_directory(scope)?
                .create_directory("content")
                .map_err(ServiceError::io)?;
            let _lease = content
                .lease(LeaseMode::Exclusive, Duration::from_secs(2))
                .map_err(ServiceError::io)?;
            let name = format!("{}-{}", evidence.sha256, transfer.filename);
            match content.open_file(&name) {
                Ok(mut retained) => {
                    let facts = photo_files::fingerprint(&mut retained, MAX_BYTES)
                        .map_err(ServiceError::io)?;
                    if facts.sha256 != evidence.sha256 {
                        return Err(ServiceError::new(
                            "state_conflict",
                            "Retained upload changed",
                            409,
                        ));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    use std::io::Write;
                    let mut retained = content
                        .create_file(&name, 0o600)
                        .map_err(ServiceError::io)?;
                    std::io::copy(&mut input, &mut retained).map_err(ServiceError::io)?;
                    retained.flush().map_err(ServiceError::io)?;
                    retained.sync_all().map_err(ServiceError::io)?;
                    content.sync().map_err(ServiceError::io)?;
                }
                Err(error) => return Err(ServiceError::io(error)),
            }
            args["file"] = json!(content.path().join(&name));
            Ok(None)
        } else if operation.ends_with(" export") || operation == "export state-file" {
            if object.len() != 1 {
                return Err(ServiceError::new(
                    "invalid_input",
                    "Only filename is accepted for export",
                    400,
                ));
            }
            let name = object
                .get("filename")
                .and_then(Value::as_str)
                .filter(|n| filename(n))
                .ok_or_else(|| {
                    ServiceError::new("invalid_input", "Export filename required", 400)
                })?;
            let directory = self.transfer_directory(scope)?;
            let id = args["requestId"]
                .as_str()
                .filter(|id| super::valid_id(id))
                .map(|id| format!("export-{id}"))
                .unwrap_or(photo_files::identity("transfer").map_err(ServiceError::io)?);
            let container = directory.create_directory(&id).map_err(ServiceError::io)?;
            let transfer = Transfer {
                id,
                scope: scope.map(str::to_owned),
                filename: name.into(),
                path: container.path().join(name),
            };
            args["file"] = json!(transfer.path);
            Ok(Some(transfer))
        } else {
            Err(ServiceError::new(
                "invalid_input",
                "This operation cannot accept a file transfer",
                400,
            ))
        }
    }
    pub(crate) fn finish_transfer(&self, transfer: &Transfer, result: &mut Value) -> Result<()> {
        let directory = self
            .transfer_directory(transfer.scope.as_deref())?
            .child(&transfer.id)
            .map_err(ServiceError::io)?;
        finish(&directory, transfer, result)
    }
    pub(crate) fn transfer_download(
        &self,
        scope: Option<&str>,
        id: &str,
    ) -> Result<crate::workspace::WorkspaceDownload> {
        if !super::valid_id(id) {
            return Err(ServiceError::new(
                "invalid_input",
                "Invalid transfer identity",
                400,
            ));
        }
        let directory = self
            .transfer_directory(scope)?
            .child(id)
            .map_err(ServiceError::io)?;
        let lease = directory
            .lease(LeaseMode::Shared, Duration::from_secs(2))
            .map_err(ServiceError::io)?;
        let receipt = photo_files::read_json(
            directory
                .open_file("receipt.json")
                .map_err(ServiceError::io)?,
            16384,
        )
        .map_err(ServiceError::io)?;
        let transfer: Transfer =
            serde_json::from_value(receipt["transfer"].clone()).map_err(ServiceError::io)?;
        if transfer.scope.as_deref() != scope {
            return Err(ServiceError::new("not_found", "Transfer not found", 404));
        }
        let mut file = directory
            .open_file(&transfer.filename)
            .map_err(ServiceError::io)?;
        let filelease = FileLease::acquire(
            directory
                .open_file(&transfer.filename)
                .map_err(ServiceError::io)?,
            LeaseMode::Shared,
            Duration::from_secs(2),
        )
        .map_err(ServiceError::io)?;
        let evidence = photo_files::fingerprint(&mut file, MAX_BYTES).map_err(ServiceError::io)?;
        if serde_json::to_value(&evidence).map_err(ServiceError::io)? != receipt["evidence"] {
            return Err(ServiceError::new(
                "state_conflict",
                "Transfer bytes changed",
                409,
            ));
        }
        Ok(crate::workspace::WorkspaceDownload {
            file,
            filename: transfer.filename,
            byte_length: evidence.observation.byte_length,
            sha256: evidence.sha256,
            revision: id.into(),
            content_type: "application/octet-stream".into(),
            leases: vec![lease, filelease],
        })
    }
}
pub(crate) fn finish(directory: &Directory, transfer: &Transfer, result: &mut Value) -> Result<()> {
    let mut file = directory
        .open_file(&transfer.filename)
        .map_err(ServiceError::io)?;
    let evidence = photo_files::fingerprint(&mut file, MAX_BYTES).map_err(ServiceError::io)?;
    photo_files::atomic_json(
        directory,
        "receipt.json",
        &json!({"transfer":transfer,"evidence":evidence}),
    )
    .map_err(ServiceError::io)?;
    let object = result
        .as_object_mut()
        .ok_or_else(|| ServiceError::unknown("Tool transfer result is not an object"))?;
    object.insert("transferId".into(), json!(transfer.id));
    object.insert("filename".into(), json!(transfer.filename));
    object.insert("byteLength".into(), json!(evidence.observation.byte_length));
    object.insert("sha256".into(), json!(evidence.sha256));
    Ok(())
}
