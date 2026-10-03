use super::*;
pub(crate) struct ServiceClient {
    pub(crate) origin: Url,
    pub(crate) client: Client,
    pub(crate) token: String,
    pub(crate) control_timeout: std::time::Duration,
}

impl fmt::Debug for ServiceClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServiceClient")
            .field("origin", &self.origin)
            .finish()
    }
}

impl ServiceClient {
    pub(crate) fn new(
        origin: Url,
        token: String,
        control_timeout: std::time::Duration,
    ) -> Result<Self, CommandFailure> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| CommandFailure::transport(Operation::Status))?;
        Ok(Self {
            origin,
            client,
            token,
            control_timeout,
        })
    }

    pub(crate) fn endpoint(&self, segments: &[&str]) -> Url {
        let mut url = self.origin.clone();
        {
            let mut path = url
                .path_segments_mut()
                .expect("HTTP origins can hold paths");
            path.clear();
            for segment in segments {
                path.push(segment);
            }
        }
        url
    }

    pub(crate) async fn json<T: DeserializeOwned>(
        &self,
        operation: Operation,
        method: Method,
        url: Url,
        body: Option<Value>,
    ) -> Result<T, CommandFailure> {
        let mut request = self
            .client
            .request(method, url)
            .header(CONTRACT_HEADER, CLI_CONTRACT_VERSION)
            .bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
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
            let error = serde_json::from_slice::<ErrorResponse>(&bytes)
                .map_err(|_| CommandFailure::transport(operation))?
                .error;
            return Err(validated_route_failure(error, operation, &self.token)
                .unwrap_or_else(|| CommandFailure::transport(operation)));
        }
        serde_json::from_slice(&bytes).map_err(|_| CommandFailure::transport(operation))
    }

    /// Performs one Album mutation. Everything after the request is handed to
    /// the transport can only be reported as an unknown outcome, because a
    /// dropped, malformed, or untrustworthy response is not evidence that the
    /// write was refused. A connect-phase failure never sent the request.
    pub(crate) async fn mutation<T: DeserializeOwned>(
        &self,
        identity: &MutationIdentity,
        admission: &AdmissionState,
        url: Url,
        body: Value,
    ) -> Result<T, CommandFailure> {
        self.mutation_admitting(identity, admission, url, body, &[StatusCode::OK])
            .await
    }

    /// One mutation request under an explicit method whose contract admits
    /// more than one success status. Returns the accepted status with the
    /// raw body, so the caller applies its own outcome-specific strict
    /// parsing. Everything after the request is handed to the transport can
    /// only be reported as an unknown outcome, because a dropped, malformed,
    /// or untrustworthy response is not evidence that the write was refused.
    /// A connect-phase failure never sent the request.
    pub(crate) async fn mutation_statuses(
        &self,
        method: Method,
        identity: &MutationIdentity,
        admission: &AdmissionState,
        url: Url,
        body: Option<Value>,
        accepted: &[StatusCode],
    ) -> Result<(StatusCode, Vec<u8>), CommandFailure> {
        let operation = identity.operation;
        let mut request = self
            .client
            .request(method, url)
            .header(CONTRACT_HEADER, CLI_CONTRACT_VERSION)
            .bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        admission.admit(identity.clone());
        let response = request.send().await.map_err(|error| {
            if error.is_connect() {
                CommandFailure::transport(operation)
            } else {
                CommandFailure::unknown(identity)
            }
        })?;
        let status = response.status();
        if status.is_redirection() {
            return Err(CommandFailure::unknown(identity));
        }
        let retry_after = retry_after_seconds(&response);
        if let Some(failure) = access_boundary_failure(status, retry_after, &[], operation) {
            return Err(failure);
        }
        let bytes = response_bytes(response, operation)
            .await
            .map_err(|_| CommandFailure::unknown(identity))?;
        if let Some(failure) = access_boundary_failure(status, retry_after, &bytes, operation) {
            return Err(failure);
        }
        if !accepted.contains(&status) {
            let error = serde_json::from_slice::<ErrorResponse>(&bytes)
                .map_err(|_| CommandFailure::unknown(identity))?
                .error;
            return Err(validated_route_failure(error, operation, &self.token)
                .unwrap_or_else(|| CommandFailure::unknown(identity)));
        }
        Ok((status, bytes))
    }

    /// One mutation whose contract admits more than one success status: the
    /// Export submission returns 201 for new work and 200 for a replay of
    /// the same identity and payload.
    pub(crate) async fn mutation_admitting<T: DeserializeOwned>(
        &self,
        identity: &MutationIdentity,
        admission: &AdmissionState,
        url: Url,
        body: Value,
        accepted: &[StatusCode],
    ) -> Result<T, CommandFailure> {
        let (_status, bytes) = self
            .mutation_statuses(Method::POST, identity, admission, url, Some(body), accepted)
            .await?;
        serde_json::from_slice(&bytes).map_err(|_| CommandFailure::unknown(identity))
    }

    /// Performs one Read Metadata request. Metadata endpoints answer with
    /// their own error envelope and exit-code table; a response that fits
    /// neither contract is a transport failure, never a silent fallback.
    pub(crate) async fn metadata_json<T: DeserializeOwned>(
        &self,
        operation: Operation,
        url: Url,
    ) -> Result<T, CommandFailure> {
        let request = self
            .client
            .request(Method::GET, url)
            .header(CONTRACT_HEADER, CLI_CONTRACT_VERSION)
            .bearer_auth(&self.token);
        let response = request
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
            let error = serde_json::from_slice::<MetadataErrorEnvelopeWire>(&bytes)
                .map_err(|_| CommandFailure::transport(operation))?
                .error;
            return Err(metadata_failure(error));
        }
        serde_json::from_slice(&bytes).map_err(|_| CommandFailure::transport(operation))
    }

    /// Performs one checked Save Metadata request. Everything after the
    /// request is handed to the transport can only be an unknown outcome:
    /// a dropped response is not evidence that the Sidecar write was
    /// refused, and the caller must re-read rather than retry blindly.
    pub(crate) async fn metadata_mutation<T: DeserializeOwned>(
        &self,
        identity: &MutationIdentity,
        admission: &AdmissionState,
        url: Url,
        body: Value,
    ) -> Result<T, CommandFailure> {
        let operation = identity.operation;
        let request = self
            .client
            .request(Method::POST, url)
            .header(CONTRACT_HEADER, CLI_CONTRACT_VERSION)
            .bearer_auth(&self.token)
            .json(&body);
        admission.admit(identity.clone());
        let response = request.send().await.map_err(|error| {
            if error.is_connect() {
                CommandFailure::transport(operation)
            } else {
                CommandFailure::unknown(identity)
            }
        })?;
        let status = response.status();
        if status.is_redirection() {
            return Err(CommandFailure::unknown(identity));
        }
        let retry_after = retry_after_seconds(&response);
        if let Some(failure) = access_boundary_failure(status, retry_after, &[], operation) {
            return Err(failure);
        }
        let bytes = response_bytes(response, operation)
            .await
            .map_err(|_| CommandFailure::unknown(identity))?;
        if let Some(failure) = access_boundary_failure(status, retry_after, &bytes, operation) {
            return Err(failure);
        }
        if status != StatusCode::OK {
            let error = serde_json::from_slice::<MetadataErrorEnvelopeWire>(&bytes)
                .map_err(|_| CommandFailure::unknown(identity))?
                .error;
            return Err(metadata_failure(error));
        }
        serde_json::from_slice(&bytes).map_err(|_| CommandFailure::unknown(identity))
    }
}

pub(crate) async fn response_bytes(
    mut response: reqwest::Response,
    operation: Operation,
) -> Result<Vec<u8>, CommandFailure> {
    if response
        .content_length()
        .is_some_and(|length| length > MAXIMUM_JSON_RESPONSE_BYTES as u64)
    {
        return Err(CommandFailure::transport(operation));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| CommandFailure::transport(operation))?
    {
        if bytes.len().saturating_add(chunk.len()) > MAXIMUM_JSON_RESPONSE_BYTES {
            return Err(CommandFailure::transport(operation));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

/// Checks one structured service error against the CLI reference shapes.
/// Returns `None` when the payload cannot be trusted as a confirmed refusal.
pub(crate) fn validated_route_failure(
    mut error: ErrorPayload,
    operation: Operation,
    secret: &str,
) -> Option<CommandFailure> {
    if error.effect != "none" || error.message.is_empty() {
        return None;
    }
    let details = error.details.as_object()?;
    let string = |name: &str| {
        details
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
    };
    let required_keys = |expected: &[&str]| expected.iter().all(|key| details.contains_key(*key));
    let code = error.code.as_str();
    let valid = match code {
        "invalid_input" => {
            required_keys(&["argument", "reason"])
                && string("argument").is_some()
                && string("reason").is_some()
        }
        "not_found" => {
            required_keys(&["resource", "reference"])
                && string("resource")
                    .is_some_and(|value| matches!(value, "photo" | "album" | "folder" | "original"))
                && string("reference").is_some()
        }
        "conflict" => {
            required_keys(&["resource", "reference", "currentVersion"])
                && string("resource").is_some_and(|value| matches!(value, "photo" | "album"))
                && string("reference").is_some()
                && string("currentVersion").is_some()
        }
        "name_conflict" => {
            required_keys(&["name", "albumId"])
                && string("name").is_some()
                && string("albumId").is_some()
        }
        "limit_exceeded" => {
            required_keys(&["limitName", "limit", "actual"])
                && string("limitName").is_some()
                && details.get("limit").and_then(Value::as_u64).is_some()
                && details.get("actual").and_then(Value::as_u64).is_some()
        }
        "cursor_expired" => {
            required_keys(&["cursorKind", "reason"])
                && string("cursorKind").is_some_and(|value| {
                    matches!(
                        value,
                        "folder" | "album" | "photo" | "unavailable" | "mappings"
                    )
                })
                && string("reason").is_some_and(|value| {
                    matches!(
                        value,
                        "publication_replaced" | "process_restarted" | "idle_or_evicted"
                    )
                })
        }
        "incompatible_server" => {
            required_keys(&["requestedContractVersion", "supportedContractVersions"])
                && details
                    .get("requestedContractVersion")
                    .and_then(Value::as_u64)
                    .is_some()
                && details
                    .get("supportedContractVersions")
                    .and_then(Value::as_array)
                    .is_some_and(|versions| {
                        versions.iter().all(|version| version.as_u64().is_some())
                    })
        }
        "library_unavailable" => {
            required_keys(&["scan"])
                && serde_json::from_value::<ScanStatus>(details["scan"].clone()).is_ok()
        }
        "preview_unavailable" => {
            required_keys(&["photoId", "state"])
                && string("photoId").is_some()
                && string("state").is_some_and(|value| {
                    matches!(value, "inspection-pending" | "failed" | "unavailable")
                })
        }
        "server_busy" => {
            required_keys(&["operation", "retryAfterSeconds"])
                && string("operation") == Some(operation.wire())
                && (details["retryAfterSeconds"].is_null()
                    || details["retryAfterSeconds"].as_u64().is_some())
        }
        "storage_failed" => {
            required_keys(&["operation"]) && string("operation") == Some(operation.wire())
        }
        // One refused recovery batch: a confirmed refusal that changed
        // nothing, with one reason per submitted mapping.
        "recovery_conflict" => {
            required_keys(&["appliedMappings", "refusedMappings", "rejections"])
                && details.get("appliedMappings").and_then(Value::as_u64) == Some(0)
                && details
                    .get("refusedMappings")
                    .and_then(Value::as_u64)
                    .is_some_and(|refused| refused > 0)
                && details
                    .get("rejections")
                    .and_then(Value::as_array)
                    .is_some_and(|rejections| {
                        !rejections.is_empty()
                            && rejections.iter().all(|rejection| {
                                rejection.as_object().is_some_and(|rejection| {
                                    let original_id =
                                        rejection.get("originalId").and_then(Value::as_str);
                                    let reason = rejection.get("reason").and_then(Value::as_str);
                                    original_id.is_some_and(|value| !value.is_empty())
                                        && reason.is_some_and(|value| !value.is_empty())
                                })
                            })
                    })
        }
        // A Folder-prefix scope that exceeds the advertised mapping bound is
        // refused before any continuation is issued.
        "recovery_scope_exceeded" => {
            required_keys(&["evaluated", "limit"])
                && details.get("evaluated").and_then(Value::as_u64).is_some()
                && details.get("limit").and_then(Value::as_u64).is_some()
                && details.get("evaluated").and_then(Value::as_u64)
                    > details.get("limit").and_then(Value::as_u64)
        }
        // Development routes carry structured recovery facts. A missing or
        // malformed conflict guard is not evidence of a confirmed refusal.
        // The composable surfaces carry the retained recipe itself.
        "recipe_conflict" | "source_changed" | "requires_rebind" => {
            details.is_empty()
                || (matches!(
                    operation,
                    Operation::PhotosProcessingRecipeSave
                        | Operation::PhotosProcessingRecipeRebind
                        | Operation::PhotosProcessingExport
                        | Operation::PhotosProcessingExportRetry
                ) && composable_recipe_details(details))
                || (string("currentSourceRevision").is_some_and(|value| {
                    !value.is_empty() && value.len() <= MAXIMUM_SOURCE_REVISION_BYTES
                }) && details.get("currentRecipeVersion").is_some_and(|value| {
                    value.is_null()
                        || value
                            .as_str()
                            .is_some_and(|value| !value.is_empty() && value.len() <= 128)
                }))
        }
        // The composable Processing Step surface: each refusal is a
        // confirmed refusal that changed nothing. The composable Export's
        // adapter refusal carries the durable, replayable refusal record,
        // and its incompatible artifact input names its closed reason.
        "invalid_recipe"
        | "unknown_module"
        | "unknown_step"
        | "step_not_current"
        | "source_unavailable"
        | "module_parameters_unavailable" => {
            details.is_empty()
                || (code == "module_parameters_unavailable"
                    && string("reasonCode").is_some_and(|value| !value.is_empty())
                    && string("reason").is_some_and(|value| !value.is_empty())
                    && details.len() == 2)
                || (matches!(
                    operation,
                    Operation::PhotosProcessingExport | Operation::PhotosProcessingExportRetry
                ) && processing_export_refusal_details(code, details))
        }
        "incompatible_input" => {
            details.is_empty()
                || (matches!(
                    operation,
                    Operation::PhotosProcessingExport | Operation::PhotosProcessingExportRetry
                ) && processing_export_refusal_details(code, details))
        }
        "invalid_settings" => {
            details.is_empty()
                || (string("argument").is_some() && string("reason").is_some())
                || (string("module").is_some_and(|value| !value.is_empty())
                    && string("reasonCode").is_some_and(|value| !value.is_empty())
                    && string("reason").is_some_and(|value| !value.is_empty())
                    && details.len() == 3)
        }
        "unknown_photo" => {
            details.is_empty()
                || (string("resource") == Some("photo") && string("reference").is_some())
        }
        // The composable Export's own closed per-state refusals: an unknown
        // or expired work record is a confirmed empty refusal, and a request
        // that already reached a terminal decision carries that committed
        // record as its receipt.
        "unknown_request" | "unknown_artifact" => details.is_empty(),
        "export_terminal" => {
            matches!(details.get("replayed"), None | Some(Value::Bool(_)))
                && details.len() <= 2
                && details
                    .get("receipt")
                    .is_some_and(|receipt| development::processing_work_valid(receipt, "", None))
        }
        "unsupported_photo" | "missing_recipe" | "request_conflict" => {
            details.is_empty() || string("photoId").is_some()
        }
        "resource_unavailable" | "processing_unavailable" => {
            let support_reason = |reason: &str| {
                matches!(
                    reason,
                    "original-missing"
                        | "original-unreadable"
                        | "read-pending"
                        | "resource-unavailable"
                )
            };
            (details.is_empty()
                || string("operation").is_some()
                || string("photoId").is_some()
                || string("reason").is_some())
                && details
                    .get("supportReason")
                    .is_none_or(|reason| reason.as_str().is_some_and(support_reason))
        }
        // A full-resolution Export the service refused because the Original
        // itself is unavailable: a confirmed refusal that changed nothing.
        "original_required" => details.is_empty() || string("photoId").is_some(),
        // An outcome_unknown response never proves refusal of a write.
        "unknown_export"
        | "export_conflict"
        | "output_unavailable"
        | "export_expired"
        | "receipt_expired"
        | "artifact_expired"
        | "retained_output_full" => details.is_empty(),
        _ => return None,
    };
    if !valid {
        return None;
    }
    redact_error(&mut error, secret);
    let exit_code = match error.code.as_str() {
        "invalid_input"
        | "limit_exceeded"
        | "invalid_settings"
        | "invalid_recipe"
        | "incompatible_input"
        | "unsupported_photo"
        | "recovery_scope_exceeded" => 2,
        "not_found" | "unknown_photo" | "unknown_export" | "unknown_artifact"
        | "missing_recipe" | "unknown_step" | "unknown_module" | "unknown_request" => 3,
        "conflict" | "name_conflict" | "recipe_conflict" | "source_changed" | "requires_rebind"
        | "request_conflict" | "export_conflict" | "output_unavailable" | "recovery_conflict"
        | "original_required" | "step_not_current" | "export_terminal" => 4,
        _ => 6,
    };
    Some(CommandFailure::from_payload(exit_code, error))
}

/// The retained composable recipe a `recipe_conflict` or `source_changed`
/// refusal carries: the top-level recipe facts with a nonempty revision and
/// source revision, a step array whose identities the selected current step
/// is one of, and a current step that is null only for the zero-step
/// recipe. The module-owned trees stay opaque facts the caller re-reads
/// anyway, so only the recovery-relevant shape is checked.
fn composable_recipe_details(details: &serde_json::Map<String, Value>) -> bool {
    let string = |name: &str| details.get(name).and_then(Value::as_str);
    let Some(steps) = details.get("steps").and_then(Value::as_array) else {
        return false;
    };
    let step_ids: Vec<&str> = steps
        .iter()
        .filter_map(|step| step.get("stepId").and_then(Value::as_str))
        .collect();
    string("photoId").is_some_and(|value| !value.is_empty())
        && string("revision").is_some_and(|value| !value.is_empty() && value.len() <= 128)
        && string("sourceRevision")
            .is_some_and(|value| !value.is_empty() && value.len() <= MAXIMUM_SOURCE_REVISION_BYTES)
        && step_ids.len() == steps.len()
        && step_ids.iter().all(|id| !id.is_empty())
        && match details.get("currentStepId") {
            Some(Value::Null) => steps.is_empty(),
            Some(value) => value
                .as_str()
                .is_some_and(|current| step_ids.contains(&current)),
            None => false,
        }
}

/// The structured details a composable Export refusal carries: the durable,
/// replayable adapter-refusal record, or the closed reason an incompatible
/// artifact input was refused. Both are confirmed refusals that changed
/// nothing and published no artifact.
fn processing_export_refusal_details(code: &str, details: &serde_json::Map<String, Value>) -> bool {
    if code == "incompatible_input" {
        return matches!(
            details.get("reason").and_then(Value::as_str),
            Some("artifact_missing") | Some("artifact_contract_mismatch")
        ) && details.len() == 1;
    }
    let Some(refusal) = details.get("refusal").and_then(Value::as_object) else {
        return false;
    };
    let string = |name: &str| refusal.get(name).and_then(Value::as_str);
    string("photoId").is_some_and(|value| !value.is_empty())
        && string("requestId").is_some_and(|value| !value.is_empty())
        && string("stepId").is_some_and(|value| !value.is_empty())
        && string("recipeRevision").is_some_and(|value| !value.is_empty() && value.len() <= 128)
        && string("sourceRevision")
            .is_some_and(|value| !value.is_empty() && value.len() <= MAXIMUM_SOURCE_REVISION_BYTES)
        && string("module").is_some_and(|value| !value.is_empty())
        && string("parameterSchemaVersion").is_some_and(|value| !value.is_empty())
        && string("parameterDigest").is_some_and(|value| !value.is_empty())
        && string("reasonCode").is_some_and(|value| !value.is_empty())
        && string("bundleId").is_some_and(|value| !value.is_empty())
        && refusal.get("input").is_some_and(Value::is_object)
        && details.get("replayed").and_then(Value::as_bool).is_some()
        && details.len() == 2
}
