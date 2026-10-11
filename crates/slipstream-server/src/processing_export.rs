//! Explicit Processing Artifact HTTP surface and compatibility Export
//! execution (Issue #496).
//!
//! `POST /api/photos/{id}/processing-exports` submits the recipe's selected
//! current Processing Step for an explicit Export. Admission captures the
//! stored step's exact identity and validates the concrete input handoff;
//! a qualified adapter's acceptance is committed as a durable work record
//! before any engine runs, so a duplicate live request replays the
//! committed receipt and never starts a second execution. Qualified
//! execution begins one durable attempt, runs the confined development
//! workload, and settles the immutable artifact with the request's
//! acceptance receipt in one serialized owner operation; every execution
//! or validation failure is recorded as the work's first terminal
//! decision. The pinned darktable adapter consumes an explicitly bound
//! Original; the independently qualified SpektraFilm adapter consumes an
//! explicitly selected retained Development TIFF. Every other pairing
//! records a durable, replayable, structured refusal before execution.
//! Historical `/exports` reads retain acknowledged records and bytes.
//! The primary `/edit/export` route resolves the current Edit State and
//! delegates here; the named Processing Step surface remains compatibility
//! only.
//!
//! `GET /api/photos/{id}/processing-exports/{requestId}` reads the durable
//! work record — the committed lifecycle state a caller reconciles against
//! after a lost response — and `POST .../cancel` (or `DELETE` on the same
//! resource) records one explicit cancellation of live admitted work; the
//! first terminal decision always wins.
//!
//! `GET /api/processing-artifacts/{id}` reads one published immutable
//! artifact record's provenance, and
//! `GET /api/processing-artifacts/{id}/bytes` streams its published bytes
//! under one finite download lease. Publication itself happens only
//! through the serialized Library owner after a qualified, validated
//! execution; no route here can mint an artifact.

use axum::{
    Json,
    body::Body,
    extract::{Path, State},
    http::{Request, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
use slipstream_core::{
    ProcessingExportSubmitOutcome, ProcessingInput, ProcessingInputHandoffError,
    SubmitProcessingExport,
};
use slipstream_processing::modules::{DARKTABLE_MODULE, Parameters, SPEKTRAFILM_MODULE};
use std::sync::Arc;

use crate::processing_policy::{NO_QUALIFIED_ADAPTER, ProcessingModulePolicy};

use crate::http::{
    CLI_CONTRACT_HEADER, HttpState, require_cli_contract, require_published, valid_id,
};
#[path = "processing_export_execution.rs"]
mod execution;
use execution::{execute_admitted_export, fail_admitted_export, terminal_work_response};
#[path = "processing_artifact_download.rs"]
mod download;
pub(crate) use download::get_processing_artifact_bytes;
#[path = "processing_export_retained.rs"]
mod retained;
use retained::{artifact_timestamp, retained_artifact_json};
pub(crate) use retained::{list_processing_exports, retry_processing_export};
#[path = "processing_export_response.rs"]
mod response;
use response::outcome_response;

/// The closed terminal failure reason recorded when the deployment's
/// confined workload could not execute an admitted qualified step.
const EXECUTION_FAILED: &str = "execution_failed";

/// The closed terminal failure reason recorded when an executed output
/// could not form a valid immutable artifact record; no implicit
/// conversion ever repairs it.
const RECORD_FORMATION_FAILED: &str = "record_formation_failed";

/// The closed terminal failure reason restart reconciliation records for
/// live work a previous server process left behind.
const INTERRUPTED_BY_RESTART: &str = "interrupted_by_restart";

fn unix_seconds_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn error(status: StatusCode, code: &'static str, message: &'static str) -> Response {
    crate::http::cli_error(status, code, message, json!({})).into_response()
}

fn error_details(
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    details: Value,
) -> Response {
    crate::http::cli_error(status, code, message, details).into_response()
}

fn unknown_request() -> Response {
    error(
        StatusCode::NOT_FOUND,
        "unknown_request",
        "The Processing Export request is unknown or expired",
    )
}

/// Reads one Export request body. Malformed bodies and unknown fields are
/// shape violations refused with 422 `invalid_settings` before any state
/// change, exactly like the legacy Export surface; only the shared
/// body-size bound answers with its own limit refusal.
async fn read_export_json_body(request: Request<Body>) -> Result<SubmitBody, Response<Body>> {
    match crate::http::read_cli_json_body::<SubmitBody>(request).await {
        Ok(body) => Ok(body),
        Err(response) if response.status() == StatusCode::PAYLOAD_TOO_LARGE => Err(response),
        Err(_) => Err(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_settings",
            "The request body is outside the closed wire shape",
        )),
    }
}

/// The Export submission body. The submission carries no settings of its
/// own: admission captures the stored recipe's selected current step
/// exactly as saved.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SubmitBody {
    request_id: String,
    step_id: String,
    expected_recipe_revision: String,
    expected_source_revision: String,
}

/// The `requestId` wire shape: 1 to 128 characters of ASCII letters,
/// digits, `.`, `_`, or `-`; chosen by the caller.
fn valid_export_request_id(request_id: &str) -> bool {
    !request_id.is_empty()
        && request_id.len() <= 128
        && request_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

pub(crate) fn input_json(input: &ProcessingInput) -> Value {
    match input {
        ProcessingInput::Original {
            photo_id,
            source_revision,
        } => json!({
            "kind": "original",
            "photoId": photo_id,
            "sourceRevision": source_revision,
        }),
        ProcessingInput::Artifact {
            artifact_id,
            contract,
        } => json!({
            "kind": "artifact",
            "artifactId": artifact_id.as_str(),
            "contract": {
                "format": contract.format,
                "precision": contract.precision,
                "colorSpace": contract.color_space,
                "transfer": contract.transfer,
                "geometry": {
                    "width": contract.geometry.width,
                    "height": contract.geometry.height,
                },
                "encoding": contract.encoding,
            },
        }),
        ProcessingInput::WorkspaceArtifact {
            workspace_id,
            artifact_id,
            revision,
            contract,
        } => json!({
            "kind": "workspace-artifact",
            "workspaceId": workspace_id,
            "artifactId": artifact_id,
            "revision": revision,
            "contract": {
                "format": contract.format,
                "precision": contract.precision,
                "colorSpace": contract.color_space,
                "transfer": contract.transfer,
                "geometry": {"width": contract.geometry.width, "height": contract.geometry.height},
                "encoding": contract.encoding,
            },
        }),
    }
}

fn refusal_json(refusal: &slipstream_core::ProcessingExportRefusal) -> Value {
    json!({
        "photoId": refusal.photo_id,
        "requestId": refusal.request_id,
        "stepId": refusal.step_id.as_str(),
        "recipeRevision": refusal.recipe_revision,
        "sourceRevision": refusal.source_revision,
        "module": refusal.module.as_str(),
        "parameterSchemaVersion": refusal.parameter_schema_version,
        "parameterDigest": refusal.parameter_digest,
        "input": input_json(&refusal.input),
        "bundleId": refusal.bundle_id,
        "reasonCode": refusal.reason_code,
    })
}

/// The exact captured identity of one admitted composable Export: the
/// fields every durable receipt — accepted, replayed, or terminal —
/// repeats unchanged from the committed admission.
fn admission_json(admission: &slipstream_core::ProcessingExportAdmission) -> Value {
    json!({
        "photoId": admission.photo_id,
        "requestId": admission.request_id,
        "stepId": admission.step_id.as_str(),
        "module": admission.module.as_str(),
        "recipeRevision": admission.recipe_revision,
        "sourceRevision": admission.source_revision,
        "adapterSchemaVersion": admission.adapter_schema_version,
        "parameters": {
            "schemaVersion": admission.parameters.schema_version,
            "tree": &admission.parameters.tree,
        },
        "input": input_json(&admission.input),
        "bundleId": admission.bundle_id,
    })
}

/// The durable work record of one admitted composable Export: the exact
/// captured admission plus its committed lifecycle state, its latest
/// begun attempt, and its terminal decision.
fn work_json(work: &slipstream_core::ProcessingExportWork) -> Value {
    let mut value = admission_json(&work.admission);
    let Some(object) = value.as_object_mut() else {
        return value;
    };
    object.insert("state".to_owned(), json!(work.state.as_str()));
    object.insert("acceptedAt".to_owned(), json!(work.accepted_at));
    object.insert(
        "attempt".to_owned(),
        match work.attempt {
            Some(attempt) => json!({"sequence": attempt.sequence, "beganAt": attempt.began_at}),
            None => Value::Null,
        },
    );
    object.insert(
        "artifactId".to_owned(),
        match work.artifact_id.as_ref() {
            Some(artifact_id) => json!(artifact_id.as_str()),
            None => Value::Null,
        },
    );
    object.insert(
        "failureReason".to_owned(),
        match work.failure_reason.as_deref() {
            Some(reason) => json!(reason),
            None => Value::Null,
        },
    );
    object.insert(
        "terminalAt".to_owned(),
        match work.terminal_at {
            Some(terminal_at) => json!(terminal_at),
            None => Value::Null,
        },
    );
    object.insert(
        "retainUntil".to_owned(),
        match work.retain_until {
            Some(retain_until) => json!(retain_until),
            None => Value::Null,
        },
    );
    value
}

/// `POST /api/photos/{id}/processing-exports`.
pub(crate) async fn submit_processing_export(
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
    let body: SubmitBody = match read_export_json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    if !valid_export_request_id(&body.request_id) || body.step_id.is_empty() {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_settings",
            "The submission carries a value outside the closed wire shape",
        );
    }
    let step_id = match slipstream_core::ProcessingStepId::new(&body.step_id) {
        Ok(step_id) => step_id,
        Err(_) => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_settings",
                "The submission carries a value outside the closed wire shape",
            );
        }
    };
    let replay = slipstream_core::ReplayProcessingExport {
        photo_id: photo_id.clone(),
        request_id: body.request_id.clone(),
        step_id: step_id.clone(),
        expected_recipe_revision: body.expected_recipe_revision.clone(),
        expected_source_revision: body.expected_source_revision.clone(),
    };
    match state
        .application
        .library
        .replay_processing_export(replay)
        .await
    {
        Ok(Some(outcome)) => return outcome_response(&state, outcome).await,
        Ok(None) => {}
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage",
                "The Export receipt could not be read",
            );
        }
    }
    // Retained caller intent has resolved; only a new request needs engines.
    if state.processing.is_none() || state.application.exports.is_none() {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "processing_unavailable",
            "Processing is not configured for this deployment",
        );
    }
    let processing = state.processing.as_ref().expect("processing configured");
    // The refusal's captured identity comes from the stored recipe inside
    // one serialized owner operation; this pre-read only decides which
    // adapter boundary the selected module faces. The recipe revision
    // guard inside admission pins the exact steps that decision names.
    let canonical = match crate::processing_recipe::read_recipe(&state, &photo_id).await {
        Ok(Some(read)) => read.recipe,
        Ok(None) => None,
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage",
                "The current Edit State could not be read",
            );
        }
    };
    let selected = match canonical.as_ref() {
        Some(recipe) => match recipe.current_step() {
            Some(step) => (
                step.module.as_str().to_owned(),
                step.input.clone(),
                Parameters {
                    module: step.module.as_str().to_owned(),
                    version: step.parameters.schema_version.clone(),
                    tree: step.parameters.tree.clone(),
                },
            ),
            None => {
                return error(
                    StatusCode::CONFLICT,
                    "step_not_current",
                    "The current Edit State has no selected Processing Engine",
                );
            }
        },
        None => {
            return error(
                StatusCode::NOT_FOUND,
                "missing_recipe",
                "Save an Edit State before submitting an Export",
            );
        }
    };
    if selected.0 == DARKTABLE_MODULE {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "use_darktable_namespace",
            "Use the darktable namespace to execute darktable state",
        );
    }
    let policy = ProcessingModulePolicy::new(processing);
    if let Err(reason) = policy.validate_parameters(&selected.2) {
        return error_details(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_settings",
            "The current Edit State carries Controls its Engine refuses",
            json!({
                "module": selected.0,
                "reasonCode": reason.code,
                "reason": reason.message,
            }),
        );
    }
    let Some(adapter) = policy.adapter_decision(&selected.0, &selected.1) else {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "unknown_module",
            "The current Edit State names an unknown Processing Engine",
        );
    };
    match policy.film_memory_requirement(&selected.0, &selected.1) {
        Err(_) => {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "resource_unavailable",
                "A finite Film processing memory allowance could not be verified",
            );
        }
        Ok(Some(requirement))
            if requirement.minimum_live_bytes > requirement.memory_limit_bytes =>
        {
            return error_details(
                StatusCode::SERVICE_UNAVAILABLE,
                "resource_unavailable",
                "The processing memory allowance cannot contain this full-resolution Film Export",
                json!({
                    "operation": "photos-processing-export",
                    "module": SPEKTRAFILM_MODULE,
                    "minimumLiveBytes": requirement.minimum_live_bytes,
                    "memoryLimitBytes": requirement.memory_limit_bytes,
                }),
            );
        }
        Ok(Some(_)) | Ok(None) => {}
    }
    let dependency_owner = format!("processing-export:{}", body.request_id);
    if let ProcessingInput::WorkspaceArtifact {
        workspace_id,
        artifact_id,
        revision,
        contract,
    } = &selected.1
    {
        let manager = Arc::clone(&state.application.workspaces);
        let workspace_id = workspace_id.clone();
        let artifact_id = artifact_id.clone();
        let revision = revision.clone();
        let contract = contract.clone();
        let owner = format!("processing-export:{}", body.request_id);
        let captured = tokio::task::spawn_blocking(move || {
            let (_lease, admitted) = manager
                .lease_input(&workspace_id, &artifact_id, &revision)
                .map_err(|error| error.to_string())?;
            let actual = &admitted["imageContract"];
            if actual["format"] != contract.format
                || actual["precision"] != contract.precision
                || actual["colorSpace"] != contract.color_space
                || actual["transfer"] != contract.transfer
                || actual["encoding"] != contract.encoding
                || actual["geometry"]["width"] != contract.geometry.width
                || actual["geometry"]["height"] != contract.geometry.height
            {
                return Err("The selected workspace image contract changed".to_owned());
            }
            let artifact_id = admitted["artifactId"]
                .as_str()
                .ok_or("Missing captured input identity")?
                .to_owned();
            let revision = admitted["revision"]
                .as_str()
                .ok_or("Missing captured input revision")?
                .to_owned();
            manager
                .register_dependency(&workspace_id, &artifact_id, &revision, &owner, "job")
                .map_err(|error| error.to_string())?;
            Ok::<(), String>(())
        })
        .await;
        if !matches!(captured, Ok(Ok(()))) {
            return error(
                StatusCode::CONFLICT,
                "input_unavailable",
                "The selected workspace input could not be captured",
            );
        }
    }
    let mutation = SubmitProcessingExport {
        photo_id,
        request_id: body.request_id,
        step_id,
        expected_recipe_revision: body.expected_recipe_revision,
        expected_source_revision: body.expected_source_revision,
        bundle_id: policy.bundle_id(&selected.0),
        retained_output_bytes_max: state
            .application
            .exports
            .as_ref()
            .map(|exports| exports.allowance())
            .unwrap_or(u64::MAX),
        adapter,
    };
    let outcome = match state
        .application
        .library
        .submit_processing_export_with_recipe(
            mutation,
            canonical.expect("selected recipe"),
            unix_seconds_now(),
        )
        .await
    {
        Ok(outcome) => outcome,
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "outcome_unknown",
                "The submission outcome is unconfirmed",
            );
        }
    };
    if !matches!(
        &outcome,
        ProcessingExportSubmitOutcome::Admitted(_)
            | ProcessingExportSubmitOutcome::Pending(_)
            | ProcessingExportSubmitOutcome::ArtifactReplayed(_)
    ) && release_export_dependency(&state, &dependency_owner)
        .await
        .is_err()
    {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "outcome_unknown",
            "The refused Export input pin could not be released",
        );
    }
    let artifact_replayed = matches!(&outcome, ProcessingExportSubmitOutcome::ArtifactReplayed(_));
    let response = dispatch_outcome(&state, processing, outcome).await;
    if artifact_replayed
        && response.status().is_success()
        && release_export_dependency(&state, &dependency_owner)
            .await
            .is_err()
    {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "outcome_unknown",
            "The reconciled Export input pin could not be released",
        );
    }
    response
}

async fn release_export_dependency(state: &HttpState, owner: &str) -> Result<(), String> {
    if let Some(request_id) = owner.strip_prefix("processing-export:") {
        match state
            .application
            .library
            .processing_export_work(request_id)
            .await
        {
            Ok(Some(work)) if !work.state.is_terminal() => return Ok(()),
            Ok(_) => {}
            Err(error) => return Err(error.to_string()),
        }
    }
    let manager = Arc::clone(&state.application.workspaces);
    let owner = owner.to_owned();
    tokio::task::spawn_blocking(move || manager.release_dependency(&owner))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

async fn dispatch_outcome(
    state: &HttpState,
    processing: &crate::config::ProcessingConfig,
    outcome: ProcessingExportSubmitOutcome,
) -> Response {
    let admission = match outcome {
        ProcessingExportSubmitOutcome::Admitted(admission) => admission,
        other => return outcome_response(state, other).await,
    };
    let request_id = admission.request_id.clone();
    let work = match state
        .application
        .library
        .processing_export_work(&request_id)
        .await
    {
        Ok(Some(work)) => work,
        _ => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "outcome_unknown",
                "The accepted receipt could not be read; reconcile through inspection",
            );
        }
    };
    let execution_state = state.clone();
    let processing = processing.clone();
    let Some(exports) = state.application.exports.as_ref() else {
        return fail_admitted_export(state, &request_id, EXECUTION_FAILED).await;
    };
    if !exports.spawn_task(async move {
        let _ = execute_admitted_export(&execution_state, &processing, admission).await;
    }) {
        return fail_admitted_export(state, &request_id, EXECUTION_FAILED).await;
    }
    (
        StatusCode::ACCEPTED,
        Json(json!({"outcome": "accepted", "receipt": work_json(&work)})),
    )
        .into_response()
}

async fn reconcile_workspace_artifact(
    state: &HttpState,
    artifact: &slipstream_core::ProcessingArtifact,
) -> Result<Option<crate::workspace::WorkspaceDownload>, String> {
    if artifact.module.as_str() != SPEKTRAFILM_MODULE {
        return Ok(None);
    }
    let workspace_id = match &artifact.input.input {
        ProcessingInput::WorkspaceArtifact { workspace_id, .. } => Some(workspace_id.clone()),
        _ => None,
    };
    let photo_id = artifact.photo_id.clone();
    let manager = Arc::clone(&state.application.workspaces);
    let legacy_id = artifact.artifact_id.as_str().to_owned();
    let path = state
        .application
        .export_artifacts_directory
        .join(format!("{legacy_id}.jpg"));
    let provenance = artifact_json(artifact);
    tokio::task::spawn_blocking(move || {
        let alias = match manager
            .resolve_artifact_alias(&legacy_id)
            .map_err(|error| error.to_string())?
        {
            Some(alias) => alias,
            None => {
                let workspace_id = match workspace_id {
                    Some(id) => Some(id),
                    None => manager
                        .lookup_photo(&photo_id)
                        .map_err(|error| error.to_string())?
                        .map(|workspace| workspace.id),
                };
                let Some(workspace_id) = workspace_id else {
                    return Ok(None);
                };
                manager
                    .adopt_output(&workspace_id, &legacy_id, &path, &provenance)
                    .map_err(|error| error.to_string())?
            }
        };
        let workspace = alias["workspaceId"]
            .as_str()
            .ok_or("Invalid artifact workspace alias")?;
        let artifact = alias["artifactId"]
            .as_str()
            .ok_or("Invalid artifact file alias")?;
        let revision = alias["revision"]
            .as_str()
            .ok_or("Invalid artifact revision alias")?;
        let download = manager
            .download(workspace, artifact, Some(revision))
            .map_err(|error| error.to_string())?;
        if Some(download.sha256.as_str()) != provenance["sha256"].as_str()
            || Some(download.byte_length) != provenance["byteLength"].as_u64()
        {
            return Err("Workspace output bytes differ from retained provenance".to_owned());
        }
        Ok(Some(download))
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn artifact_created_response(
    state: &HttpState,
    artifact: &slipstream_core::ProcessingArtifact,
    replayed: bool,
) -> Response {
    if reconcile_workspace_artifact(state, artifact).await.is_err() {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "outcome_unknown",
            "The retained workspace output publication could not be reconciled",
        );
    }
    let value = match retained_artifact_json(state, artifact).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let body = json!({"artifact": value, "replayed": replayed});
    (StatusCode::CREATED, axum::Json(body)).into_response()
}

fn artifact_json(artifact: &slipstream_core::ProcessingArtifact) -> Value {
    json!({
        "artifactId": artifact.artifact_id.as_str(),
        "photoId": artifact.photo_id,
        "stepId": artifact.step_id.as_str(),
        "module": artifact.module.as_str(),
        "adapterSchemaVersion": artifact.adapter_schema_version,
        "parameters": {
            "schemaVersion": artifact.parameters.schema_version,
            "tree": &artifact.parameters.tree,
        },
        "input": {
            "binding": input_json(&artifact.input.input),
            "sha256": artifact.input.sha256,
            "byteLength": artifact.input.byte_length,
        },
        "outputContract": {
            "format": artifact.output_contract.format,
            "precision": artifact.output_contract.precision,
            "colorSpace": artifact.output_contract.color_space,
            "transfer": artifact.output_contract.transfer,
            "geometry": {
                "width": artifact.output_contract.geometry.width,
                "height": artifact.output_contract.geometry.height,
            },
            "encoding": artifact.output_contract.encoding,
        },
        "bundleId": artifact.bundle_id,
        "sha256": artifact.sha256,
        "byteLength": artifact.byte_length,
    })
}

/// `GET /api/photos/{id}/processing-exports/{requestId}`: the durable work
/// record of one admitted composable Export — the committed lifecycle
/// state a caller reconciles against after a lost response. A request
/// identity with no committed work record — never admitted, or settled
/// and swept past its retention — answers the same closed refusal.
pub(crate) async fn get_processing_export_status(
    State(state): State<HttpState>,
    Path((photo_id, request_id)): Path<(String, String)>,
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
    if !valid_id(&photo_id) || !valid_export_request_id(&request_id) {
        return unknown_request();
    }
    match state
        .application
        .library
        .processing_export_work(&request_id)
        .await
    {
        // The record is scoped to the Photo whose recipe admitted it; a
        // different Photo's path never resolves another Photo's work.
        Ok(Some(work)) if work.admission.photo_id == photo_id => {
            let mut workspace_output = None;
            if work.state == slipstream_core::ProcessingExportWorkState::Succeeded {
                let artifact = match work.artifact_id.as_ref() {
                    Some(id) => {
                        state
                            .application
                            .library
                            .processing_artifact(id.as_str())
                            .await
                    }
                    None => {
                        return error(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "outcome_unknown",
                            "The settled Export artifact is unconfirmed",
                        );
                    }
                };
                let Ok(Some(artifact)) = artifact else {
                    return error(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "outcome_unknown",
                        "The settled Export artifact is unconfirmed",
                    );
                };
                workspace_output = match reconcile_workspace_artifact(&state, &artifact).await {
                    Ok(output) => output,
                    Err(_) => {
                        return error(
                            StatusCode::INTERNAL_SERVER_ERROR,
                            "outcome_unknown",
                            "The retained workspace output publication could not be reconciled",
                        );
                    }
                };
            }
            if work.state.is_terminal()
                && release_export_dependency(&state, &format!("processing-export:{request_id}"))
                    .await
                    .is_err()
            {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "outcome_unknown",
                    "The terminal Export input pin could not be released",
                );
            }
            drop(workspace_output);
            Json(work_json(&work)).into_response()
        }
        Ok(Some(_)) | Ok(None) => unknown_request(),
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage",
            "The Processing Export work record could not be read",
        ),
    }
}

/// `POST /api/photos/{id}/processing-exports/{requestId}/cancel` and
/// `DELETE /api/photos/{id}/processing-exports/{requestId}`: records one
/// explicit cancellation of live admitted work. The first terminal
/// decision wins — an already-terminal request answers with its committed
/// record, and an in-flight attempt's settlement is refused unchanged.
pub(crate) async fn cancel_processing_export(
    State(state): State<HttpState>,
    Path((photo_id, request_id)): Path<(String, String)>,
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
    if !valid_id(&photo_id) || !valid_export_request_id(&request_id) {
        return unknown_request();
    }
    // Cancellation is scoped to the Photo whose recipe admitted the work.
    match state
        .application
        .library
        .processing_export_work(&request_id)
        .await
    {
        Ok(Some(work)) if work.admission.photo_id == photo_id => {}
        Ok(Some(_)) | Ok(None) => return unknown_request(),
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage",
                "The Processing Export work record could not be read",
            );
        }
    }
    match state
        .application
        .library
        .cancel_processing_export(&request_id, unix_seconds_now())
        .await
    {
        Ok(slipstream_core::ProcessingExportCancelOutcome::Cancelled(work)) => {
            if let Some(exports) = state.application.exports.as_ref() {
                exports.cancel_running(&request_id);
            }
            Json(work_json(&work)).into_response()
        }
        Ok(slipstream_core::ProcessingExportCancelOutcome::Terminal(work)) => {
            if let Some(exports) = state.application.exports.as_ref() {
                exports.cancel_running(&request_id);
            }
            terminal_work_response(&state, work).await
        }
        Ok(slipstream_core::ProcessingExportCancelOutcome::Missing) => unknown_request(),
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "outcome_unknown",
            "The cancellation outcome is unconfirmed; reconcile through inspection",
        ),
    }
}

/// `GET /api/processing-artifacts/{id}`.
pub(crate) async fn get_processing_artifact(
    State(state): State<HttpState>,
    Path(artifact_id): Path<String>,
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
    if slipstream_core::ProcessingArtifactId::new(&artifact_id).is_err() {
        return error(
            StatusCode::NOT_FOUND,
            "unknown_artifact",
            "The Processing Artifact is unknown",
        );
    }
    match state
        .application
        .library
        .processing_artifact(&artifact_id)
        .await
    {
        Ok(Some(artifact)) => {
            if reconcile_workspace_artifact(&state, &artifact)
                .await
                .is_err()
            {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "outcome_unknown",
                    "The retained workspace output publication could not be reconciled",
                );
            }
            match retained_artifact_json(&state, &artifact).await {
                Ok(value) => Json(value).into_response(),
                Err(response) => response,
            }
        }
        Ok(None) => error(
            StatusCode::NOT_FOUND,
            "unknown_artifact",
            "The Processing Artifact is unknown",
        ),
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage",
            "The Processing Artifact could not be read",
        ),
    }
}

/// Reconciles composable Export work left live by a previous server process.
///
/// The local execution seam does not persist an engine publication claim that
/// can identify an orphaned, pre-settlement TIFF after a crash. Recovery
/// therefore fails the accepted identity durably instead of guessing which
/// bytes belong to it or launching a replacement attempt. The bytes remain
/// untouched; the retention/orphan sweep removes only bytes that persistence
/// proves are unclaimed.
pub(crate) async fn reconcile_processing_exports(
    application: &std::sync::Arc<crate::app::Application>,
) -> Result<(), String> {
    let library = &application.library;
    let workspaces = &application.workspaces;
    let unfinished = library
        .unfinished_processing_exports()
        .await
        .map_err(|error| error.to_string())?;
    let now = unix_seconds_now();
    for work in unfinished {
        let outcome = library
            .fail_processing_export(
                work.admission.request_id.as_str(),
                INTERRUPTED_BY_RESTART,
                now,
            )
            .await
            .map_err(|error| error.to_string())?;
        if matches!(&outcome,slipstream_core::ProcessingExportFailureOutcome::Failed(terminal)|slipstream_core::ProcessingExportFailureOutcome::Terminal(terminal) if matches!(terminal.state,slipstream_core::ProcessingExportWorkState::Failed|slipstream_core::ProcessingExportWorkState::Cancelled))
        {
            let manager = Arc::clone(workspaces);
            let owner = format!("processing-export:{}", work.admission.request_id);
            tokio::task::spawn_blocking(move || manager.release_dependency(&owner))
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}
