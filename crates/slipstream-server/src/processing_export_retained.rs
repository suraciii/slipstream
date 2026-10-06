//! Retained Export history, captured-snapshot retries, and artifact metadata.

use super::*;

/// Reads retained work and artifacts independently of processing availability.
pub(crate) async fn list_processing_exports(
    State(state): State<HttpState>,
    Path(photo_id): Path<String>,
    request: Request<Body>,
) -> Response {
    if request.headers().contains_key(CLI_CONTRACT_HEADER)
        && let Err(response) = require_cli_contract(&request)
    {
        return *response;
    }
    if let Err(response) = require_published(&state.application) {
        return *response;
    }
    if !valid_id(&photo_id) {
        return error(
            StatusCode::NOT_FOUND,
            "unknown_photo",
            "The Photo is unknown",
        );
    }
    let retained = match state
        .application
        .library
        .list_processing_exports(&photo_id, unix_seconds_now())
        .await
    {
        Ok(retained) => retained,
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage",
                "Retained Exports could not be read",
            );
        }
    };
    let historical = match state.application.library.photo_exports(&photo_id).await {
        Ok(Some(records)) => records
            .into_iter()
            .take(64)
            .map(|record| crate::wire::export_inspect(&record))
            .collect::<Vec<_>>(),
        Ok(None) => {
            return error(
                StatusCode::NOT_FOUND,
                "unknown_photo",
                "The Photo is not part of the persisted Library",
            );
        }
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage",
                "Historical Exports could not be read",
            );
        }
    };
    let mut artifacts = Vec::with_capacity(retained.artifacts.len());
    for artifact in &retained.artifacts {
        match retained_artifact_json(&state, artifact).await {
            Ok(value) => artifacts.push(value),
            Err(response) => return response,
        }
    }
    Json(json!({"photoId": photo_id, "exports": retained.works.iter().map(work_json).collect::<Vec<_>>(), "artifacts": artifacts, "historicalExports": historical})).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RetryBody {
    request_id: String,
}

/// Admits a new identity from one failed or cancelled captured snapshot.
pub(crate) async fn retry_processing_export(
    State(state): State<HttpState>,
    Path((photo_id, previous_request_id)): Path<(String, String)>,
    request: Request<Body>,
) -> Response {
    if request.headers().contains_key(CLI_CONTRACT_HEADER)
        && let Err(response) = require_cli_contract(&request)
    {
        return *response;
    }
    if let Err(response) = require_published(&state.application) {
        return *response;
    }
    if !valid_id(&photo_id) || !valid_export_request_id(&previous_request_id) {
        return unknown_request();
    }
    let body: RetryBody = match crate::http::read_cli_json_body(request).await {
        Ok(body) => body,
        Err(response) if response.status() == StatusCode::PAYLOAD_TOO_LARGE => return response,
        Err(_) => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_settings",
                "The retry body is invalid",
            );
        }
    };
    if !valid_export_request_id(&body.request_id) || body.request_id == previous_request_id {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_settings",
            "Retry requires a new request identity",
        );
    }
    match state
        .application
        .library
        .replay_processing_export_retry(&photo_id, &previous_request_id, &body.request_id)
        .await
    {
        Ok(Some(outcome)) => return outcome_response(&state, outcome).await,
        Ok(None) => {}
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage",
                "The retry receipt could not be read",
            );
        }
    }
    let captured = match state
        .application
        .library
        .processing_export_work(&previous_request_id)
        .await
    {
        Ok(Some(work)) if work.admission.photo_id == photo_id => work.admission,
        Ok(_) => return unknown_request(),
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage",
                "The captured Export could not be read",
            );
        }
    };
    let Some(processing) = state
        .processing
        .as_ref()
        .filter(|_| state.application.exports.is_some())
    else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "processing_unavailable",
            "Processing is not configured for this deployment",
        );
    };
    if captured.module.as_str() == DARKTABLE_MODULE
        && let Some(reason) = processing.failure
    {
        return error_details(
            StatusCode::SERVICE_UNAVAILABLE,
            "module_parameters_unavailable",
            "The captured darktable module is unavailable",
            json!({"reasonCode": reason, "reason": "The configured darktable module failed availability verification"}),
        );
    }
    let policy = ProcessingModulePolicy::new(processing);
    if let Err(reason) = policy.validate_parameters(&Parameters {
        module: captured.module.as_str().to_owned(),
        version: captured.parameters.schema_version.clone(),
        tree: captured.parameters.tree.clone(),
    }) {
        return error_details(
            StatusCode::SERVICE_UNAVAILABLE,
            "module_parameters_unavailable",
            "The captured parameter snapshot is unavailable",
            json!({"reasonCode": reason.code, "reason": reason.message}),
        );
    }
    let Some(adapter) = policy.adapter_decision(captured.module.as_str(), &captured.input) else {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "unknown_module",
            "The captured module is unknown",
        );
    };
    if !policy.adapter_matches(&adapter, &captured.adapter_schema_version) {
        return error_details(
            StatusCode::SERVICE_UNAVAILABLE,
            "module_parameters_unavailable",
            "The captured module adapter is unavailable",
            json!({"reasonCode": "captured_adapter_unavailable", "reason": "The captured adapter and parameter schema are unavailable"}),
        );
    }
    match policy.film_memory_requirement(captured.module.as_str(), &captured.input) {
        Err(_) => {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "resource_unavailable",
                "A finite processing memory allowance could not be verified",
            );
        }
        Ok(Some(requirement))
            if requirement.minimum_live_bytes > requirement.memory_limit_bytes =>
        {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "resource_unavailable",
                "The memory allowance cannot contain the captured Export",
            );
        }
        Ok(Some(_)) | Ok(None) => {}
    }
    let bundle_id = policy.bundle_id(captured.module.as_str());
    if bundle_id != captured.bundle_id {
        return error_details(
            StatusCode::SERVICE_UNAVAILABLE,
            "module_parameters_unavailable",
            "The captured processing bundle is unavailable",
            json!({"reasonCode": "captured_bundle_unavailable", "reason": "The captured processing bundle is unavailable"}),
        );
    }
    let outcome = match state
        .application
        .library
        .retry_processing_export(
            slipstream_core::RetryProcessingExport {
                photo_id,
                previous_request_id,
                request_id: body.request_id,
                bundle_id,
                retained_output_bytes_max: state
                    .application
                    .exports
                    .as_ref()
                    .expect("configured")
                    .allowance(),
                adapter,
            },
            unix_seconds_now(),
        )
        .await
    {
        Ok(outcome) => outcome,
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "outcome_unknown",
                "The retry outcome is unconfirmed",
            );
        }
    };
    dispatch_outcome(&state, processing, outcome).await
}

pub(super) fn artifact_timestamp(seconds: u64) -> String {
    time::OffsetDateTime::from_unix_timestamp(seconds as i64)
        .expect("persisted artifact timestamp")
        .format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 timestamp")
}

pub(super) async fn retained_artifact_json(
    state: &HttpState,
    artifact: &slipstream_core::ProcessingArtifact,
) -> Result<Value, Response> {
    let retention = match state
        .application
        .library
        .processing_artifact_retention(artifact.artifact_id.as_str())
        .await
    {
        Ok(Some(retention)) => retention,
        _ => {
            return Err(error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage",
                "The artifact retention evidence could not be read",
            ));
        }
    };
    let mut value = artifact_json(artifact);
    let object = value.as_object_mut().expect("artifact object");
    object.insert("filename".to_owned(), json!(artifact.filename()));
    object.insert(
        "publishedAt".to_owned(),
        json!(artifact_timestamp(retention.published_at_unix_seconds)),
    );
    object.insert(
        "expiresAt".to_owned(),
        json!(artifact_timestamp(retention.expires_at_unix_seconds)),
    );
    object.insert("orientation".to_owned(), json!("top-left"));
    object.insert("iccEmbedded".to_owned(), json!(true));
    object.insert(
        "sampleFormat".to_owned(),
        json!(artifact.output_contract.precision),
    );
    Ok(value)
}
