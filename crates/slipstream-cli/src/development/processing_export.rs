use super::*;
/// The arguments of `slipstream photos processing-export`.
#[derive(Debug, clap::Args)]
pub struct ProcessingExportArgs {
    #[arg(value_name = "PHOTO_ID", value_parser = crate::nonempty)]
    pub photo_id: String,
    /// UTF-8 JSON file holding the complete guarded submission body; `-`
    /// reads standard input.
    #[arg(long, value_name = "FILE", value_parser = crate::nonempty)]
    pub input: String,
}

/// Reads and validates the composable Export submission body before any
/// network access, and returns the exact serialized body the mutation later
/// submits.
pub(crate) async fn prepare_processing_export(input: &str) -> Result<Value, CommandFailure> {
    parse_processing_export(read_input_bytes(input).await?)
}

/// The one accepted `photos processing-export --input` document shape: the
/// caller's request identity, the recipe's selected current step, and both
/// observed revision guards. The submission carries no settings of its own:
/// admission captures the stored step exactly as saved.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ProcessingExportInput {
    pub(super) request_id: String,
    pub(super) step_id: String,
    pub(super) expected_recipe_revision: String,
    pub(super) expected_source_revision: String,
}

/// Validates the submission body in the service's own admission order:
/// request identity, step identity, then both revision guards.
pub(super) fn parse_processing_export(bytes: Vec<u8>) -> Result<Value, CommandFailure> {
    let input: ProcessingExportInput = serde_json::from_slice(&bytes).map_err(|_| {
        CommandFailure::invalid(
            "input",
            "The input must be one JSON object with exactly requestId, stepId, \
             expectedRecipeRevision, and expectedSourceRevision.",
        )
    })?;
    validate_request_identity(&input.request_id)?;
    validate_bounded_argument("stepId", &input.step_id, MAXIMUM_STEP_ID_BYTES)?;
    validate_nonempty_revision(
        "expectedRecipeRevision",
        Some(&input.expected_recipe_revision),
    )?;
    validate_nonempty_revision(
        "expectedSourceRevision",
        Some(&input.expected_source_revision),
    )?;
    serde_json::to_value(&input).map_err(|_| unusable_input())
}

/// Submits one guarded composable Export of the recipe's selected current
/// Processing Step. The service captures the stored step's exact identity
/// and either settles the immutable Processing Artifact in the same
/// response, replays the committed receipt of one still-live duplicate, or
/// records a durable, replayable, structured refusal. A settled artifact
/// must repeat this Photo, this step, and a closed provenance record, and a
/// replayed receipt must be the live work record of exactly this request;
/// anything else stays an unknown outcome.
pub(crate) async fn execute_processing_export(
    client: &ServiceClient,
    admission: &AdmissionState,
    args: &ProcessingExportArgs,
    body: Value,
) -> Result<Value, CommandFailure> {
    let prepared_field = |key: &str| {
        body.get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let Some(request_id) = prepared_field("requestId") else {
        return Err(unusable_input());
    };
    let Some(step_id) = prepared_field("stepId") else {
        return Err(unusable_input());
    };
    let identity = MutationIdentity {
        operation: PROCESSING_EXPORT_OPERATION,
        photo_ids: vec![args.photo_id.clone()],
        album_id: None,
        album_name: None,
        mappings: Vec::new(),
    };
    let (status, bytes) = client
        .mutation_statuses(
            Method::POST,
            &identity,
            admission,
            client.endpoint(&["api", "photos", &args.photo_id, "processing-exports"]),
            Some(body),
            &[StatusCode::CREATED, StatusCode::ACCEPTED],
        )
        .await?;
    if status == StatusCode::ACCEPTED {
        let receipt: ProcessingExportPendingWire =
            serde_json::from_slice(&bytes).map_err(|_| CommandFailure::unknown(&identity))?;
        return confirmed_processing_export_pending(
            &identity,
            &args.photo_id,
            &request_id,
            &step_id,
            receipt,
            &client.origin,
        );
    }
    let result: ProcessingExportResultWire =
        serde_json::from_slice(&bytes).map_err(|_| CommandFailure::unknown(&identity))?;
    confirmed_processing_export(
        &identity,
        &args.photo_id,
        &request_id,
        &step_id,
        status,
        result,
        &client.origin,
    )
}

/// The replayed receipt of one still-live duplicate submission: the durable
/// work record of the committed admission.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProcessingExportPendingWire {
    pub(super) receipt: Value,
    pub(super) replayed: bool,
}

/// Validates one replayed live receipt against the submitted request. The
/// receipt is the durable work record: it repeats this Photo and this
/// request and is still in a live state, so the caller reconciles the same
/// work through inspection instead of a second execution.
pub(super) fn confirmed_processing_export_pending(
    identity: &MutationIdentity,
    photo_id: &str,
    request_id: &str,
    step_id: &str,
    receipt: ProcessingExportPendingWire,
    origin: &Url,
) -> Result<Value, CommandFailure> {
    if !receipt.replayed
        || !processing_work_valid(&receipt.receipt, photo_id, Some(request_id))
        || receipt.receipt["stepId"] != json!(step_id)
        || !matches!(
            receipt.receipt["state"].as_str(),
            Some("accepted" | "executing")
        )
    {
        return Err(CommandFailure::unknown(identity));
    }
    let web_url = web_url(origin, &format!("/?photoId={photo_id}"))
        .map_err(|()| CommandFailure::unknown(identity))?;
    Ok(json!({
        "photoId": photo_id,
        "requestId": request_id,
        "stepId": step_id,
        "outcome": "pending",
        "state": receipt.receipt["state"],
        "replayed": true,
        "webUrl": web_url,
    }))
}

/// The settled outcome of one admitted composable Export: the published
/// immutable artifact and whether this request identity replayed it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProcessingExportResultWire {
    pub(super) artifact: Value,
    pub(super) replayed: bool,
}

/// Validates one confirmed composable Export against the submitted request.
/// The artifact is the record the service settled: it repeats the Photo and
/// exactly the submitted step with a closed provenance shape, and anything
/// else is an unknown outcome rather than a claimed artifact.
pub(super) fn confirmed_processing_export(
    identity: &MutationIdentity,
    photo_id: &str,
    request_id: &str,
    step_id: &str,
    status: StatusCode,
    result: ProcessingExportResultWire,
    origin: &Url,
) -> Result<Value, CommandFailure> {
    if status != StatusCode::CREATED
        || !processing_artifact_valid(&result.artifact, photo_id)
        || result.artifact["stepId"] != json!(step_id)
    {
        return Err(CommandFailure::unknown(identity));
    }
    let web_url = web_url(origin, &format!("/?photoId={photo_id}"))
        .map_err(|()| CommandFailure::unknown(identity))?;
    Ok(json!({
        "photoId": photo_id,
        "requestId": request_id,
        "stepId": step_id,
        "outcome": if result.replayed { "replayed" } else { "published" },
        "artifact": result.artifact,
        "webUrl": web_url,
    }))
}

/// Reads one published immutable Processing Artifact's provenance by the
/// caller-held identity. A record outside the closed provenance shape is a
/// transport failure, never a claimed artifact.
pub(crate) async fn processing_artifact(
    client: &ServiceClient,
    artifact_id: &str,
) -> Result<Value, CommandFailure> {
    let artifact: Value = client
        .json(
            ARTIFACT_OPERATION,
            Method::GET,
            client.endpoint(&["api", "processing-artifacts", artifact_id]),
            None,
        )
        .await?;
    if !processing_artifact_valid(&artifact, "") || artifact["artifactId"] != json!(artifact_id) {
        return Err(CommandFailure::transport(ARTIFACT_OPERATION));
    }
    let photo_id = artifact["photoId"].as_str().unwrap_or_default();
    let web_url = web_url(&client.origin, &format!("/?photoId={photo_id}"))
        .map_err(|()| CommandFailure::transport(ARTIFACT_OPERATION))?;
    let mut value = artifact;
    value["webUrl"] = Value::String(web_url);
    Ok(value)
}

/// Reads the durable work record of one submitted composable Export. The
/// record is what a lost response reconciles against: the committed
/// lifecycle state of exactly this Photo's request. A record outside the
/// closed work shape is a transport failure, never a claimed state.
pub(crate) async fn processing_export_status(
    client: &ServiceClient,
    photo_id: &str,
    request_id: &str,
) -> Result<Value, CommandFailure> {
    let work: Value = client
        .json(
            PROCESSING_EXPORT_STATUS_OPERATION,
            Method::GET,
            client.endpoint(&["api", "photos", photo_id, "processing-exports", request_id]),
            None,
        )
        .await?;
    if !processing_work_valid(&work, photo_id, Some(request_id)) {
        return Err(CommandFailure::transport(
            PROCESSING_EXPORT_STATUS_OPERATION,
        ));
    }
    let web_url = web_url(&client.origin, &format!("/?photoId={photo_id}"))
        .map_err(|()| CommandFailure::transport(PROCESSING_EXPORT_STATUS_OPERATION))?;
    let mut value = work;
    value["webUrl"] = Value::String(web_url);
    Ok(value)
}

/// Cancels one live submitted composable Export. The service's committed
/// record is authoritative: the cancellation records the first terminal
/// decision, an already-succeeded request replays its settled artifact, and
/// an already-terminal refusal answers with its own committed record.
pub(crate) async fn processing_export_cancel(
    client: &ServiceClient,
    admission: &AdmissionState,
    photo_id: &str,
    request_id: &str,
) -> Result<Value, CommandFailure> {
    let identity = MutationIdentity {
        operation: PROCESSING_EXPORT_CANCEL_OPERATION,
        photo_ids: vec![photo_id.to_owned()],
        album_id: None,
        album_name: None,
        mappings: Vec::new(),
    };
    let (status, bytes) = client
        .mutation_statuses(
            Method::POST,
            &identity,
            admission,
            client.endpoint(&[
                "api",
                "photos",
                photo_id,
                "processing-exports",
                request_id,
                "cancel",
            ]),
            None,
            &[StatusCode::OK, StatusCode::CREATED],
        )
        .await?;
    let web_url = web_url(&client.origin, &format!("/?photoId={photo_id}"))
        .map_err(|()| CommandFailure::unknown(&identity))?;
    if status == StatusCode::OK {
        let work: Value =
            serde_json::from_slice(&bytes).map_err(|_| CommandFailure::unknown(&identity))?;
        if !processing_work_valid(&work, photo_id, Some(request_id))
            || work["state"] != json!("cancelled")
        {
            return Err(CommandFailure::unknown(&identity));
        }
        let mut value = work;
        value["webUrl"] = Value::String(web_url);
        return Ok(value);
    }
    // The request had already succeeded; its committed artifact replays.
    let result: ProcessingExportResultWire =
        serde_json::from_slice(&bytes).map_err(|_| CommandFailure::unknown(&identity))?;
    if !processing_artifact_valid(&result.artifact, photo_id) {
        return Err(CommandFailure::unknown(&identity));
    }
    Ok(json!({
        "photoId": photo_id,
        "requestId": request_id,
        "stepId": result.artifact["stepId"],
        "outcome": "succeeded",
        "replayed": result.replayed,
        "artifact": result.artifact,
        "webUrl": web_url,
    }))
}

/// Validates one durable Processing Export work record against the closed
/// wire shape: the captured admission facts — bounded identities, the
/// complete parameter snapshot, the explicit input binding, and the bundle —
/// plus the committed lifecycle, where the terminal fields exist exactly
/// when the state is terminal. When `request_id` is `Some`, the record must
/// repeat exactly that request identity; `photo_id` must always match.
pub(crate) fn processing_work_valid(
    work: &Value,
    photo_id: &str,
    request_id: Option<&str>,
) -> bool {
    let Some(record) = work.as_object() else {
        return false;
    };
    let string = |name: &str| {
        record
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
    };
    let bounded =
        |name: &str, maximum: usize| string(name).is_some_and(|value| value.len() <= maximum);
    let state = record.get("state").and_then(Value::as_str);
    let terminal = matches!(state, Some("succeeded" | "failed" | "cancelled"));
    let optional_terminal_string = |name: &str, present: bool| match record.get(name) {
        Some(Value::Null) => !present,
        Some(value) => present && value.as_str().is_some_and(|value| !value.is_empty()),
        None => false,
    };
    let parameters = record.get("parameters").and_then(Value::as_object);
    (photo_id.is_empty() || string("photoId") == Some(photo_id))
        && request_id.is_none_or(|expected| string("requestId") == Some(expected))
        && string("requestId").is_some_and(valid_request_identity)
        && bounded("stepId", MAXIMUM_STEP_ID_BYTES)
        && bounded("module", MAXIMUM_MODULE_ID_BYTES)
        && bounded("recipeRevision", MAXIMUM_REVISION_BYTES)
        && bounded("sourceRevision", MAXIMUM_REVISION_BYTES)
        && bounded("adapterSchemaVersion", MAXIMUM_CONTRACT_NAME_BYTES)
        && parameters.is_some_and(|parameters| {
            parameters
                .get("schemaVersion")
                .and_then(Value::as_str)
                .is_some_and(|value| !value.is_empty())
                && parameters.get("tree").is_some_and(Value::is_object)
        })
        && record.get("input").is_some_and(Value::is_object)
        && bounded("bundleId", MAXIMUM_REVISION_BYTES)
        && matches!(
            state,
            Some("accepted" | "executing" | "succeeded" | "failed" | "cancelled")
        )
        && record.get("acceptedAt").and_then(Value::as_u64).is_some()
        && matches!(
            record.get("attempt"),
            Some(Value::Null) | Some(Value::Object(_))
        )
        && optional_terminal_string("artifactId", state == Some("succeeded"))
        && optional_terminal_string("failureReason", state == Some("failed"))
        && match record.get("terminalAt") {
            Some(Value::Null) => !terminal,
            Some(value) => terminal && value.as_u64().is_some(),
            None => false,
        }
        && matches!(
            record.get("retainUntil"),
            Some(Value::Null) | Some(Value::Number(_))
        )
}

/// Validates one Processing Artifact record against the closed provenance
/// shape: the immutable identity, the owning Photo and step, the module
/// with its pinned versions, the complete parameter snapshot, the input
/// binding with byte evidence, the validated output contract, the bundle,
/// and the published bytes' own identity. When `photo_id` is nonempty the
/// record must name exactly that Photo.
pub(super) fn processing_artifact_valid(artifact: &Value, photo_id: &str) -> bool {
    let bounded_name = |value: &Value, maximum: usize| {
        value
            .as_str()
            .is_some_and(|name| !name.is_empty() && name.len() <= maximum)
    };
    let contract_shape = |value: &Value| {
        let Some(contract) = value.as_object() else {
            return false;
        };
        let geometry = |edge: &'static str| {
            contract
                .get("geometry")
                .and_then(|geometry| geometry.get(edge))
                .and_then(Value::as_u64)
                .is_some_and(|edge| (1..=65_536).contains(&edge))
        };
        ["format", "precision", "colorSpace", "transfer", "encoding"]
            .iter()
            .all(|field| contract.get(*field).is_some_and(Value::is_string))
            && geometry("width")
            && geometry("height")
    };
    let input = artifact.get("input").and_then(Value::as_object);
    let input_valid = input.is_some_and(|input| {
        let binding = input.get("binding");
        match binding
            .and_then(|binding| binding.get("kind"))
            .and_then(Value::as_str)
        {
            Some("original") => {
                binding
                    .and_then(|binding| binding.get("photoId"))
                    .is_some_and(Value::is_string)
                    && binding
                        .and_then(|binding| binding.get("sourceRevision"))
                        .is_some_and(Value::is_string)
            }
            Some("artifact") => {
                binding
                    .and_then(|binding| binding.get("artifactId"))
                    .is_some_and(Value::is_string)
                    && binding
                        .and_then(|binding| binding.get("contract"))
                        .is_some_and(contract_shape)
            }
            _ => false,
        }
    });
    (photo_id.is_empty() || artifact["photoId"] == json!(photo_id))
        && bounded_name(&artifact["artifactId"], MAXIMUM_ARTIFACT_ID_BYTES)
        && bounded_name(&artifact["photoId"], MAXIMUM_PHOTO_ID_BYTES)
        && bounded_name(&artifact["stepId"], MAXIMUM_STEP_ID_BYTES)
        && bounded_name(&artifact["module"], MAXIMUM_MODULE_ID_BYTES)
        && bounded_name(
            &artifact["adapterSchemaVersion"],
            MAXIMUM_CONTRACT_NAME_BYTES,
        )
        && artifact
            .get("parameters")
            .and_then(|parameters| parameters.get("schemaVersion"))
            .is_some_and(|value| bounded_name(value, MAXIMUM_CONTRACT_NAME_BYTES))
        && artifact
            .get("parameters")
            .and_then(|parameters| parameters.get("tree"))
            .is_some_and(Value::is_object)
        && input_valid
        && input
            .and_then(|input| input.get("sha256"))
            .and_then(Value::as_str)
            .is_some_and(valid_sha256)
        && input
            .and_then(|input| input.get("byteLength"))
            .and_then(Value::as_u64)
            .is_some_and(|length| length > 0)
        && artifact.get("outputContract").is_some_and(contract_shape)
        && bounded_name(&artifact["bundleId"], MAXIMUM_REVISION_BYTES)
        && artifact
            .get("sha256")
            .and_then(Value::as_str)
            .is_some_and(valid_sha256)
        && artifact
            .get("byteLength")
            .and_then(Value::as_u64)
            .is_some_and(|length| length > 0)
}
