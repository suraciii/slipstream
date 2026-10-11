use super::{Result, ServiceError, ToolService, attach, process};
use serde_json::{Value, json};
use std::{sync::atomic::AtomicBool, time::Duration};
impl ToolService {
    pub(crate) fn review_legacy(&self, photo: &str, input: &Value, legacy: Value) -> Result<Value> {
        let workspace = self.workspaces.resolve_photo(photo)?;
        let workspace_id = workspace["workspaceId"]
            .as_str()
            .ok_or_else(|| ServiceError::unknown("Workspace identity missing"))?;
        let (_guard, binding) = self.workspaces.lease_input(
            workspace_id,
            input["artifactId"].as_str().ok_or_else(|| {
                ServiceError::new("invalid_input", "Legacy input artifact missing", 400)
            })?,
            input["revision"].as_str().ok_or_else(|| {
                ServiceError::new("invalid_input", "Legacy input revision missing", 400)
            })?,
        )?;
        let mut review = process::invoke(
            None,
            "__migration-review",
            json!({"inputPath":binding["path"],"legacy":legacy}),
            &AtomicBool::new(false),
            Duration::from_secs(120),
        )?;
        review["ifState"] = legacy["recipeRevision"].clone();
        let token = review["migrationToken"]
            .as_str()
            .ok_or_else(|| ServiceError::unknown("Migration review omitted token"))?;
        if !super::valid_id(token) {
            return Err(ServiceError::unknown("Migration token invalid"));
        }
        let reviews = self
            .root
            .create_directory("migration-reviews")
            .map_err(ServiceError::io)?;
        photo_files::atomic_json(
            &reviews,
            format!("{token}.json"),
            &json!({"photoId":photo,"input":input,"legacy":legacy,"reviewed":review}),
        )
        .map_err(ServiceError::io)?;
        Ok(review)
    }
    pub(crate) fn import_legacy(
        &self,
        photo: &str,
        input: &Value,
        legacy: Value,
        reviewed: Value,
        request_id: &str,
        if_state: Option<&str>,
    ) -> Result<Value> {
        if !super::valid_id(request_id) {
            return Err(ServiceError::new(
                "invalid_input",
                "Migration request identity invalid",
                400,
            ));
        }
        let workspace = self.workspaces.resolve_photo(photo)?;
        let workspace_id = workspace["workspaceId"]
            .as_str()
            .ok_or_else(|| ServiceError::unknown("Workspace identity missing"))?;
        let (_guard, binding) = self.workspaces.lease_input(
            workspace_id,
            input["artifactId"].as_str().ok_or_else(|| {
                ServiceError::new("invalid_input", "Legacy input artifact missing", 400)
            })?,
            input["revision"].as_str().ok_or_else(|| {
                ServiceError::new("invalid_input", "Legacy input revision missing", 400)
            })?,
        )?;
        let directory = self.workspaces.directory(workspace_id)?;
        let id = format!("migration-{request_id}");
        let states = directory
            .create_directory(".edits")
            .and_then(|d| d.create_directory("darktable"))
            .map_err(ServiceError::io)?;
        let state_path = states.path().join(&id);
        let previews = directory
            .create_directory("previews")
            .map_err(ServiceError::io)?;
        let outputs = directory
            .create_directory("outputs")
            .map_err(ServiceError::io)?;
        let mut args = json!({"inputPath":binding["path"],"legacy":legacy,"reviewed":reviewed,"confirm":true,"requestId":request_id,"previewDirectory":previews.path(),"outputDirectory":outputs.path()});
        if let Some(token) = if_state {
            args["ifState"] = json!(token);
        }
        let mut result = process::invoke(
            Some(&state_path),
            "__migration-import",
            args,
            &AtomicBool::new(false),
            Duration::from_secs(120),
        )?;
        attach(&mut result, photo, workspace_id, &id);
        Ok(result)
    }
    pub(crate) fn legacy_review(&self, photo: &str, arguments: &Value) -> Result<Value> {
        let token = arguments["migrationToken"]
            .as_str()
            .filter(|s| super::valid_id(s))
            .ok_or_else(|| ServiceError::new("invalid_input", "Migration token required", 400))?;
        let reviews = self
            .root
            .child("migration-reviews")
            .map_err(ServiceError::io)?;
        let record = photo_files::read_json(
            reviews
                .open_file(format!("{token}.json"))
                .map_err(ServiceError::io)?,
            64 * 1024 * 1024,
        )
        .map_err(ServiceError::io)?;
        if record["photoId"] != photo || record["reviewed"]["ifState"] != arguments["ifState"] {
            return Err(ServiceError::new(
                "state_conflict",
                "Migration review identity changed",
                409,
            ));
        }
        Ok(record)
    }
    pub(crate) fn apply_legacy_review(
        &self,
        photo: &str,
        arguments: &Value,
        record: &Value,
    ) -> Result<Value> {
        if arguments["confirm"] != true {
            return Err(ServiceError::new(
                "confirmation_required",
                "Confirm reviewed migration explicitly",
                400,
            ));
        }
        let request = arguments["requestId"]
            .as_str()
            .ok_or_else(|| ServiceError::new("invalid_input", "Migration request required", 400))?;
        self.import_legacy(
            photo,
            &record["input"],
            record["legacy"].clone(),
            record["reviewed"].clone(),
            request,
            None,
        )
    }
}
