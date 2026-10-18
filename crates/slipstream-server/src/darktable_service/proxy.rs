use super::{Result, ServiceError, ToolService, receipts};
use serde_json::{Value, json};

/// An admitted proxy request: its retained identity under the admission lease,
/// together with the outcome this service observed for it.
pub(crate) struct Admission {
    pub(crate) receipt: receipts::Receipt,
    pub(crate) payload: Value,
    pub(crate) replay: Option<(u16, Value)>,
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
        let request = receipts::request_identity(arguments)?;
        let receipt = receipts::Receipt::open(&self.root, "proxy-requests", photo, &request)?;
        let payload = json!({"operation":operation,"arguments":arguments});
        let prior = receipt.retained(&payload, "Proxy request identity has different intent")?;
        if let Some(record) = prior {
            let result = record.get("result").cloned().ok_or_else(|| {
                ServiceError::unknown("Proxy request outcome requires lifecycle reconciliation")
            })?;
            return Ok(Admission {
                receipt,
                payload,
                replay: Some((record["status"].as_u64().unwrap_or(200) as u16, result)),
            });
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
            receipt.publish(&json!({"payload":payload}))?;
        }
        Ok(Admission {
            receipt,
            payload,
            replay: None,
        })
    }
}
