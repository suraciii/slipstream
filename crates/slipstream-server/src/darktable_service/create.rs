use super::{Result, ServiceError, ToolService, attach, process, receipts};
use serde_json::{Value, json};
use std::{sync::atomic::AtomicBool, time::Duration};
impl ToolService {
    pub(crate) fn create_edit(
        &self,
        photo: &str,
        edit: Option<&str>,
        mut args: Value,
    ) -> Result<(u16, Value)> {
        if edit.is_some() || !super::valid_id(photo) {
            return Err(ServiceError::new(
                "invalid_input",
                "New edit requires a valid photo and null editId",
                400,
            ));
        }
        let object = args.as_object().ok_or_else(|| {
            ServiceError::new("invalid_input", "arguments must be an object", 400)
        })?;
        if object
            .keys()
            .any(|k| !matches!(k.as_str(), "input" | "requestId" | "ifState"))
        {
            return Err(ServiceError::new(
                "invalid_input",
                "Unknown edit create field",
                400,
            ));
        }
        for k in ["requestId", "ifState"] {
            if object
                .get(k)
                .is_some_and(|v| !v.as_str().is_some_and(super::valid_id))
            {
                return Err(ServiceError::new(
                    "invalid_input",
                    "Invalid create request or guard",
                    400,
                ));
            }
        }
        let payload = args.clone();
        let request = receipts::request_identity(&args)?;
        let receipt = receipts::Receipt::open(&self.root, "edit-requests", photo, &request)?;
        let prior = receipt.retained(&payload, "Create request has different intent")?;
        if let Some(result) = prior
            .as_ref()
            .and_then(|record| record.get("result"))
            .cloned()
        {
            return Ok((201, result));
        }
        let workspace = self.workspaces.open_photo(photo)?;
        let workspace_id = workspace["workspaceId"]
            .as_str()
            .ok_or_else(|| ServiceError::unknown("Workspace identity missing"))?;
        let directory = self.workspaces.directory(workspace_id)?;
        let states = directory
            .create_directory(crate::workspace::layout::EDITS)
            .and_then(|d| d.create_directory("darktable"))
            .map_err(ServiceError::io)?;
        let id = if let Some(record) = &prior {
            record["editId"]
                .as_str()
                .ok_or_else(|| ServiceError::unknown("Create receipt identity missing"))?
                .to_owned()
        } else {
            photo_files::identity("edit").map_err(ServiceError::io)?
        };
        let state_path = states.path().join(&id);
        if prior.is_some() {
            match states.child(&id) {
                Ok(_) => {
                    let mut result = process::inspect_state(&state_path)?;
                    attach(&mut result, photo, workspace_id, &id);
                    receipt.publish(&json!({"payload":payload,"editId":id,"result":result}))?;
                    return Ok((201, result));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(ServiceError::io(e)),
            }
        }
        if prior.is_none()
            && let Some(token) = args["ifState"].as_str()
            && workspace["workspaceStateToken"] != token
        {
            return Err(ServiceError::new(
                "state_conflict",
                "Workspace state changed before edit creation",
                409,
            ));
        }
        let input=prior.as_ref().and_then(|record|record.get("input")).cloned().or_else(||args.get("input").cloned()).unwrap_or_else(||json!({"artifactId":workspace["inputArtifactId"],"revision":workspace["inputRevision"]}));
        let input = input.get("input").unwrap_or(&input);
        let (guard, binding) = lease_selected_input(&self.workspaces, photo, workspace_id, input)?;
        let previews = directory
            .create_directory(crate::workspace::layout::PREVIEWS)
            .map_err(ServiceError::io)?;
        let outputs = directory
            .create_directory(crate::workspace::layout::OUTPUTS)
            .map_err(ServiceError::io)?;
        args = json!({"inputPath":binding["path"],"previewDirectory":previews.path(),"outputDirectory":outputs.path()});
        receipt.publish(&json!({"payload":payload,"editId":id,"input":{"artifactId":binding["artifactId"],"revision":binding["revision"]}}))?;
        let _admission =
            super::processing_admission::acquire_blocking(self.root.path().parent().unwrap())
                .map_err(ServiceError::io)?;
        let mut result = process::invoke(
            Some(&state_path),
            "open",
            args,
            &AtomicBool::new(false),
            Duration::from_secs(120),
        )?;
        attach(&mut result, photo, workspace_id, &id);
        receipt.publish(&json!({"payload":payload,"editId":id,"result":result}))?;
        drop(guard);
        Ok((201, result))
    }
}

fn lease_selected_input(
    workspaces: &crate::workspace::WorkspaceManager,
    photo: &str,
    workspace_id: &str,
    input: &Value,
) -> Result<(crate::workspace::WorkspaceDownload, Value)> {
    let selected = if input["kind"] == "original" {
        let source = input["sourceRevision"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ServiceError::new("invalid_input", "input.sourceRevision required", 400)
            })?;
        workspaces.prepared_input(photo, source)?
    } else {
        input.clone()
    };
    workspaces
        .lease_input(
            workspace_id,
            selected["artifactId"].as_str().ok_or_else(|| {
                ServiceError::new("invalid_input", "input.artifactId required", 400)
            })?,
            selected["revision"].as_str().ok_or_else(|| {
                ServiceError::new("invalid_input", "input.revision required", 400)
            })?,
        )
        .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::{fs, sync::Arc};

    #[test]
    fn original_carrier_leases_exact_prepared_snapshot_and_refuses_lost_evidence() {
        use slipstream_core::{LibraryRoot, RelativeOriginalPath, capture_source_revision};
        use std::os::unix::fs::PermissionsExt;
        let root =
            std::env::temp_dir().join(photo_files::identity("original-binding-test").unwrap());
        let state = root.join("state");
        let library = root.join("library");
        fs::create_dir_all(&state).unwrap();
        fs::create_dir_all(&library).unwrap();
        let original_path = library.join("original.ARW");
        fs::write(&original_path, b"first protected original").unwrap();
        let source = Arc::new(Mutex::new(None));
        let observed = Arc::clone(&source);
        let workspaces = crate::workspace::WorkspaceManager::open(
            &state,
            library.clone(),
            Arc::new(move |_| Ok(observed.lock().clone())),
            1024 * 1024,
        )
        .unwrap();
        let observe = || {
            let relative = RelativeOriginalPath::parse("original.ARW").unwrap();
            let library_root = LibraryRoot::open(&library).unwrap();
            let original = library_root.original(relative.clone()).unwrap();
            let revision =
                capture_source_revision(relative.as_str(), original.facts().unwrap()).unwrap();
            *source.lock() = Some(crate::workspace::Source {
                relative_path: relative,
                revision: Some(revision.clone()),
            });
            revision
        };
        let first_source = observe();
        let opened = workspaces.open_photo("photo-test").unwrap();
        let workspace = opened["workspaceId"].as_str().unwrap();
        fs::write(&original_path, b"second protected original").unwrap();
        let second_source = observe();
        let refreshed = workspaces
            .refresh(workspace, &second_source, Some("refresh-test"))
            .unwrap();
        assert_ne!(opened["inputArtifactId"], refreshed["inputArtifactId"]);
        let first_carrier = json!({"kind":"original","sourceRevision":first_source});
        let (guard, binding) =
            lease_selected_input(&workspaces, "photo-test", workspace, &first_carrier).unwrap();
        assert_eq!(binding["artifactId"], opened["inputArtifactId"]);
        assert_eq!(binding["revision"], opened["inputRevision"]);
        assert_eq!(
            fs::read(binding["path"].as_str().unwrap()).unwrap(),
            b"first protected original"
        );
        drop(guard);
        let (guard, second_binding) = lease_selected_input(
            &workspaces,
            "photo-test",
            workspace,
            &json!({"kind":"original","sourceRevision":second_source}),
        )
        .unwrap();
        assert_eq!(second_binding["artifactId"], refreshed["inputArtifactId"]);
        drop(guard);
        fs::remove_file(&original_path).unwrap();
        *source.lock() = None;
        let (guard, retained) =
            lease_selected_input(&workspaces, "photo-test", workspace, &first_carrier).unwrap();
        assert_eq!(retained["revision"], opened["inputRevision"]);
        drop(guard);
        assert!(
            lease_selected_input(
                &workspaces,
                "photo-test",
                workspace,
                &json!({"kind":"original","sourceRevision":"never-prepared"})
            )
            .is_err()
        );
        let retained_path = std::path::Path::new(binding["path"].as_str().unwrap());
        fs::set_permissions(retained_path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(retained_path, b"changed retained snapshot").unwrap();
        assert!(
            lease_selected_input(&workspaces, "photo-test", workspace, &first_carrier).is_err()
        );
        fs::remove_file(retained_path).unwrap();
        assert!(
            lease_selected_input(&workspaces, "photo-test", workspace, &first_carrier).is_err()
        );
        drop(workspaces);
        fs::remove_dir_all(root).unwrap();
    }
}
