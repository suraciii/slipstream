//! Compatibility negotiation before any operational request.

use super::{
    CLI_CONTRACT_VERSION, CONTRACT_HEADER, CommandFailure, ErrorResponse, MAXIMUM_LIST_PAGE,
    Operation, ServiceClient, access_boundary_failure, response_bytes, retry_after_seconds,
    validated_route_failure,
};
use reqwest::StatusCode;
use serde_json::{Value, json};

impl ServiceClient {
    /// Negotiates the contract and returns the advertised request bounds the
    /// operational commands enforce locally. Artifact download control reads
    /// use the command's bounded control timeout.
    pub(super) async fn capabilities(
        &self,
        operation: Operation,
    ) -> Result<AdvertisedLimits, CommandFailure> {
        if operation == Operation::ProcessingArtifactDownload {
            tokio::time::timeout(self.control_timeout, self.capabilities_unbounded(operation))
                .await
                .map_err(|_| CommandFailure::transport(operation))?
        } else {
            self.capabilities_unbounded(operation).await
        }
    }

    async fn capabilities_unbounded(
        &self,
        operation: Operation,
    ) -> Result<AdvertisedLimits, CommandFailure> {
        let url = self.endpoint(&["api", "capabilities"]);
        let response = self
            .client
            .get(url)
            .header(CONTRACT_HEADER, CLI_CONTRACT_VERSION)
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|_| CommandFailure::transport(operation))?;
        let status = response.status();
        if status.is_redirection() {
            return Err(CommandFailure::transport(operation));
        }
        let retry_after = retry_after_seconds(&response);
        if let Some(failure) = access_boundary_failure(status, retry_after, &[], operation) {
            return Err(failure);
        }
        let bytes = response_bytes(response, operation).await?;
        if let Some(failure) = access_boundary_failure(status, retry_after, &bytes, operation) {
            return Err(failure);
        }
        if status != StatusCode::OK {
            if let Ok(response) = serde_json::from_slice::<ErrorResponse>(&bytes)
                && response.error.code == "incompatible_server"
            {
                return Err(
                    validated_route_failure(response.error, operation, &self.token)
                        .unwrap_or_else(|| CommandFailure::transport(operation)),
                );
            }
            return Err(CommandFailure::incompatible(Vec::new()));
        }
        validate_capabilities(&bytes)
    }
}

/// The advertised request bounds one command run enforces locally.
#[derive(Clone, Copy, Debug)]
pub(super) struct AdvertisedLimits {
    pub(super) recovery_page_maximum: usize,
    pub(super) recovery_apply_maximum: usize,
}
fn validate_capabilities(bytes: &[u8]) -> Result<AdvertisedLimits, CommandFailure> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|_| capability_shape_failure(Vec::new(), "invalid-json", None))?;
    let versions = value
        .get("supportedCliContractVersions")
        .and_then(Value::as_array);
    let supported: Vec<u16> = versions
        .into_iter()
        .flatten()
        .filter_map(|version| u16::try_from(version.as_u64()?).ok())
        .collect();
    let failure = |field| {
        capability_shape_failure(supported.clone(), "missing-or-invalid-field", Some(field))
    };
    if versions.is_none_or(|versions| versions.len() != supported.len()) {
        return Err(failure("supportedCliContractVersions"));
    }
    if !supported.contains(&CLI_CONTRACT_VERSION) {
        return Err(CommandFailure::incompatible(supported));
    }
    if value
        .get("serverVersion")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        return Err(failure("serverVersion"));
    }
    let limits = value
        .get("limits")
        .and_then(Value::as_object)
        .ok_or_else(|| failure("limits"))?;
    for (name, field) in [
        ("listPageMaximum", "limits.listPageMaximum"),
        ("mutationPhotoIdsMaximum", "limits.mutationPhotoIdsMaximum"),
        ("removalPhotoIdsMaximum", "limits.removalPhotoIdsMaximum"),
        (
            "albumReorderMembersMaximum",
            "limits.albumReorderMembersMaximum",
        ),
        ("retainedQueryIdsMaximum", "limits.retainedQueryIdsMaximum"),
        (
            "retainedQueryIdleSeconds",
            "limits.retainedQueryIdleSeconds",
        ),
        ("recoveryPageMaximum", "limits.recoveryPageMaximum"),
        ("recoveryMappingsMaximum", "limits.recoveryMappingsMaximum"),
        ("recoveryApplyMaximum", "limits.recoveryApplyMaximum"),
        (
            "recoveryReviewIdleSeconds",
            "limits.recoveryReviewIdleSeconds",
        ),
    ] {
        let limit = limits.get(name).and_then(Value::as_u64);
        if limit.is_none_or(|limit| limit == 0)
            || (name == "listPageMaximum" && limit != Some(MAXIMUM_LIST_PAGE as u64))
        {
            return Err(failure(field));
        }
    }
    let advertised = |name: &str| -> Option<usize> {
        limits
            .get(name)
            .and_then(Value::as_u64)
            .and_then(|limit| usize::try_from(limit).ok())
    };
    Ok(AdvertisedLimits {
        recovery_page_maximum: advertised("recoveryPageMaximum")
            .ok_or_else(|| failure("limits.recoveryPageMaximum"))?,
        recovery_apply_maximum: advertised("recoveryApplyMaximum")
            .ok_or_else(|| failure("limits.recoveryApplyMaximum"))?,
    })
}

fn capability_shape_failure(
    supported: Vec<u16>,
    reason: &str,
    field: Option<&str>,
) -> CommandFailure {
    let mut failure = CommandFailure::incompatible(supported);
    failure.payload.message = "The service capability response is incomplete or invalid. Install a client and service from the same candidate revision.".to_owned();
    failure.payload.details["reason"] = json!(reason);
    failure.payload.details["field"] = json!(field);
    failure
}
