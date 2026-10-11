use super::{Result, ServiceError, ToolService};
use crate::workspace::WorkspaceDownload;
use serde_json::Value;
use std::path::Path;
impl ToolService {
    pub(crate) fn rendered_download(
        &self,
        id: &str,
        comparison: bool,
    ) -> Result<(WorkspaceDownload, Value)> {
        let (workspace, result) = self.jobs.completed(id)?;
        if let Some(transfer) = result["transferId"].as_str() {
            return Ok((
                self.transfer_download(result["photoId"].as_str(), transfer)?,
                Value::Null,
            ));
        }
        let facts = if comparison {
            result
                .get("comparison")
                .filter(|v| !v.is_null())
                .ok_or_else(|| {
                    ServiceError::new("not_found", "Preview has no captured comparison", 404)
                })?
        } else {
            &result
        };
        let path = facts["path"]
            .as_str()
            .ok_or_else(|| ServiceError::new("not_found", "Completed file not found", 404))?;
        let root = self.workspaces.directory(&workspace)?;
        let relative = Path::new(path)
            .strip_prefix(root.path())
            .map_err(|_| ServiceError::unknown("Tool result is outside authorized workspace"))?;
        let mut cursor = None;
        loop {
            let page = self.workspaces.files(
                if cursor.is_none() {
                    Some(&workspace)
                } else {
                    None
                },
                None,
                60,
                cursor.as_deref(),
            )?;
            let items = page
                .get("files")
                .or_else(|| page.get("items"))
                .and_then(Value::as_array)
                .ok_or_else(|| ServiceError::unknown("Workspace listing omitted files"))?;
            for item in items {
                if item["relativePath"].as_str() == relative.to_str() {
                    if item["sha256"] != facts["sha256"]
                        || item["byteLength"] != facts["byteLength"]
                    {
                        return Err(ServiceError::new(
                            "artifact_changed",
                            "Rendered bytes no longer match the retained tool receipt",
                            409,
                        ));
                    }
                    return Ok((
                        self.workspaces.download(
                            &workspace,
                            item["artifactId"].as_str().ok_or_else(|| {
                                ServiceError::unknown("Artifact identity missing")
                            })?,
                            item["revision"].as_str(),
                        )?,
                        item["imageContract"].clone(),
                    ));
                }
            }
            cursor = page["nextCursor"].as_str().map(str::to_owned);
            if cursor.is_none() {
                return Err(ServiceError::new(
                    "not_found",
                    "Completed file is no longer in the workspace",
                    404,
                ));
            }
        }
    }
}
