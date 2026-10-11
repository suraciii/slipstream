use super::{Result, ServiceError, ToolService};
use photo_files::{Directory, FileLease, LeaseMode};
use serde_json::{Value, json};
use std::time::Duration;
pub(crate) struct Admission {
    pub directory: Directory,
    pub lease: FileLease,
    pub name: String,
    pub payload: Value,
    pub replay: Option<(u16, Value)>,
}
impl ToolService {
    pub(crate) fn admit_proxy(
        &self,
        photo: &str,
        operation: &str,
        arguments: &Value,
    ) -> Result<Admission> {
        if !super::valid_id(photo) {
            return Err(ServiceError::new(
                "invalid_input",
                "Photo identity invalid",
                400,
            ));
        }
        for key in ["requestId", "ifState"] {
            if arguments
                .get(key)
                .is_some_and(|v| !v.as_str().is_some_and(super::valid_id))
            {
                return Err(ServiceError::new(
                    "invalid_input",
                    "Proxy guard/request identity invalid",
                    400,
                ));
            }
        }
        let directory = self
            .root
            .create_directory("proxy-requests")
            .and_then(|d| d.create_directory(photo))
            .map_err(ServiceError::io)?;
        let lease = directory
            .lease(LeaseMode::Exclusive, Duration::from_secs(120))
            .map_err(ServiceError::io)?;
        let request = arguments["requestId"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or(photo_files::identity("request").map_err(ServiceError::io)?);
        let name = format!("{request}.json");
        let payload = json!({"operation":operation,"arguments":arguments});
        match directory.open_file(&name) {
            Ok(file) => {
                let record = photo_files::read_json(file, 1024 * 1024).map_err(ServiceError::io)?;
                if record["payload"] != payload {
                    return Err(ServiceError::new(
                        "request_conflict",
                        "Proxy request identity has different intent",
                        409,
                    ));
                }
                let result = record.get("result").cloned().ok_or_else(|| {
                    ServiceError::unknown("Proxy request outcome requires lifecycle reconciliation")
                })?;
                return Ok(Admission {
                    directory,
                    lease,
                    name,
                    payload,
                    replay: Some((record["status"].as_u64().unwrap_or(200) as u16, result)),
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(ServiceError::io(e)),
        }
        if let Some(token) = arguments["ifState"].as_str() {
            let workspace = self.workspaces.resolve_photo(photo)?;
            if workspace["workspaceStateToken"] != token {
                return Err(ServiceError::new(
                    "state_conflict",
                    "Workspace state changed before proxy operation",
                    409,
                ));
            }
        }
        if operation != "preview proxy get" {
            photo_files::atomic_json(&directory, &name, &json!({"payload":payload}))
                .map_err(ServiceError::io)?;
        }
        Ok(Admission {
            directory,
            lease,
            name,
            payload,
            replay: None,
        })
    }
}
