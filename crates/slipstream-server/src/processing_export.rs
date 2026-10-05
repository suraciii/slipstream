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
use slipstream_processing::modules::{
    DARKTABLE_MODULE, ModuleAvailability, ModuleRegistry, Parameters, SPEKTRAFILM_MODULE,
};
use std::sync::Arc;

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

/// The closed reason code recorded when the deployment's adapter boundary
/// refuses a selected module's complete parameter tree.
const NO_QUALIFIED_ADAPTER: &str = "module_parameters_unavailable";

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

/// The deployment's adapter qualification for one selected step. Each peer
/// is qualified independently: darktable consumes an explicit Original,
/// while SpektraFilm consumes an explicit retained Development TIFF artifact.
fn adapter_decision(
    module: &str,
    input: &slipstream_core::ProcessingInput,
    film_ready: bool,
) -> Option<slipstream_core::ProcessingExportAdapterDecision> {
    match (module, input) {
        (DARKTABLE_MODULE, slipstream_core::ProcessingInput::Original { .. }) => Some(
            slipstream_core::ProcessingExportAdapterDecision::Qualified {
                adapter_version: slipstream_processing::modules::DARKTABLE_ADAPTER_VERSION
                    .to_owned(),
                parameter_schema_version:
                    slipstream_processing::modules::DARKTABLE_PARAMETER_VERSION.to_owned(),
            },
        ),
        (SPEKTRAFILM_MODULE, slipstream_core::ProcessingInput::Artifact { .. }) if film_ready => {
            Some(
                slipstream_core::ProcessingExportAdapterDecision::Qualified {
                    adapter_version: slipstream_processing::modules::SPEKTRAFILM_ADAPTER_VERSION
                        .to_owned(),
                    parameter_schema_version:
                        slipstream_processing::modules::SPEKTRAFILM_PARAMETER_VERSION.to_owned(),
                },
            )
        }
        (DARKTABLE_MODULE | SPEKTRAFILM_MODULE, _) => Some(
            slipstream_core::ProcessingExportAdapterDecision::NoQualifiedAdapter {
                reason_code: NO_QUALIFIED_ADAPTER.to_owned(),
            },
        ),
        _ => None,
    }
}

fn input_json(input: &ProcessingInput) -> Value {
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
    let selected = match state
        .application
        .library
        .composable_edit_recipe(&photo_id)
        .await
    {
        Ok(Some(recipe)) => match recipe.current_step() {
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
        Ok(None) => {
            return error(
                StatusCode::NOT_FOUND,
                "missing_recipe",
                "Save an Edit State before submitting an Export",
            );
        }
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage",
                "The current Edit State could not be read",
            );
        }
    };
    if selected.0 == DARKTABLE_MODULE
        && let Some(reason) = processing.failure
    {
        return error_details(
            StatusCode::SERVICE_UNAVAILABLE,
            "module_parameters_unavailable",
            "The darktable module is unavailable",
            json!({"reasonCode": reason, "reason": "The configured darktable module failed availability verification"}),
        );
    }
    let registry = ModuleRegistry::new(ModuleAvailability::ready(), ModuleAvailability::ready());
    if let Err(reason) = registry.validate_parameters(&selected.2) {
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
    let film_ready = processing
        .film
        .as_ref()
        .is_some_and(crate::config::FilmConfig::ready);
    let Some(adapter) = adapter_decision(&selected.0, &selected.1, film_ready) else {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "unknown_module",
            "The current Edit State names an unknown Processing Engine",
        );
    };
    if selected.0 == SPEKTRAFILM_MODULE
        && film_ready
        && let ProcessingInput::Artifact { contract, .. } = &selected.1
    {
        let required = crate::film_resources::minimum_live_bytes(
            contract.geometry.width,
            contract.geometry.height,
        );
        let limit = match crate::film_resources::effective_memory_limit() {
            Ok(limit) => limit,
            Err(_) => {
                return error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "resource_unavailable",
                    "A finite Film processing memory allowance could not be verified",
                );
            }
        };
        if required > limit {
            return error_details(
                StatusCode::SERVICE_UNAVAILABLE,
                "resource_unavailable",
                "The processing memory allowance cannot contain this full-resolution Film Export",
                json!({"operation": "photos-processing-export", "module": SPEKTRAFILM_MODULE, "minimumLiveBytes": required, "memoryLimitBytes": limit}),
            );
        }
    }
    let mutation = SubmitProcessingExport {
        photo_id,
        request_id: body.request_id,
        step_id,
        expected_recipe_revision: body.expected_recipe_revision,
        expected_source_revision: body.expected_source_revision,
        bundle_id: if selected.0 == SPEKTRAFILM_MODULE {
            processing
                .film
                .as_ref()
                .filter(|film| film.ready())
                .map_or_else(
                    || processing.bundle_sha256.clone(),
                    |film| film.bundle_sha256.clone(),
                )
        } else {
            processing.bundle_sha256.clone()
        },
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
        .submit_processing_export(mutation, unix_seconds_now())
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
    dispatch_outcome(&state, processing, outcome).await
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

async fn artifact_created_response(
    state: &HttpState,
    artifact: &slipstream_core::ProcessingArtifact,
    replayed: bool,
) -> Response {
    let value = match retained_artifact_json(state, artifact).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let body = json!({"artifact": value, "replayed": replayed});
    (StatusCode::CREATED, axum::Json(body)).into_response()
}

async fn outcome_response(state: &HttpState, outcome: ProcessingExportSubmitOutcome) -> Response {
    match outcome {
        ProcessingExportSubmitOutcome::Refused(refusal) => error_details(
            StatusCode::SERVICE_UNAVAILABLE,
            "module_parameters_unavailable",
            "The current Edit State has no qualified Export adapter in this deployment",
            json!({"refusal": refusal_json(&refusal), "replayed": false}),
        ),
        ProcessingExportSubmitOutcome::Replayed(refusal) => error_details(
            StatusCode::SERVICE_UNAVAILABLE,
            "module_parameters_unavailable",
            "The current Edit State has no qualified Export adapter in this deployment",
            json!({"refusal": refusal_json(&refusal), "replayed": true}),
        ),
        ProcessingExportSubmitOutcome::Admitted(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "outcome_unknown",
            "The admitted execution was not dispatched",
        ),
        ProcessingExportSubmitOutcome::Pending(admission) => {
            // A duplicate of one still-live accepted execution: the
            // committed receipt replays and no second execution starts.
            // The live state is read back from the same owner so the
            // receipt names exactly what the work record says.
            let receipt = match state
                .application
                .library
                .processing_export_work(&admission.request_id)
                .await
            {
                Ok(Some(work)) => work_json(&work),
                _ => {
                    let mut receipt = admission_json(&admission);
                    if let Some(object) = receipt.as_object_mut() {
                        object.insert("state".to_owned(), json!("accepted"));
                    }
                    receipt
                }
            };
            (
                StatusCode::ACCEPTED,
                Json(json!({"outcome": "replayed", "receipt": receipt})),
            )
                .into_response()
        }
        ProcessingExportSubmitOutcome::FailureReplayed(work) => error_details(
            StatusCode::CONFLICT,
            "export_terminal",
            "The Export request already reached a terminal decision",
            json!({"receipt": work_json(&work), "replayed": true}),
        ),
        ProcessingExportSubmitOutcome::CancelledReplayed(work) => error_details(
            StatusCode::CONFLICT,
            "export_terminal",
            "The Export request already reached a terminal decision",
            json!({"receipt": work_json(&work), "replayed": true}),
        ),
        ProcessingExportSubmitOutcome::Expired => error(
            StatusCode::GONE,
            "artifact_expired",
            "The settled artifact's retention window has passed",
        ),
        ProcessingExportSubmitOutcome::ReservationFull => error_details(
            StatusCode::SERVICE_UNAVAILABLE,
            "server_busy",
            "The Library holds its maximum live composable Exports",
            json!({"operation": "photos-processing-export", "retryAfterSeconds": null}),
        ),
        ProcessingExportSubmitOutcome::RetainedOutputFull => error_details(
            StatusCode::SERVICE_UNAVAILABLE,
            "retained_output_full",
            "The retained-output allowance cannot admit another composable artifact",
            json!({"operation": "photos-processing-export", "retryAfterSeconds": null}),
        ),
        ProcessingExportSubmitOutcome::ArtifactReplayed(artifact) => {
            artifact_created_response(state, &artifact, true).await
        }
        ProcessingExportSubmitOutcome::RecipeConflict(recipe) => error_details(
            StatusCode::CONFLICT,
            "recipe_conflict",
            "The expected Edit revision is no longer current",
            recipe
                .as_ref()
                .map(crate::processing_recipe::recipe_json)
                .unwrap_or_else(|| json!({})),
        ),
        ProcessingExportSubmitOutcome::SourceChanged(recipe) => error_details(
            StatusCode::CONFLICT,
            "source_changed",
            "The guarded source revision is no longer current",
            recipe
                .as_ref()
                .map(crate::processing_recipe::recipe_json)
                .unwrap_or_else(|| json!({})),
        ),
        ProcessingExportSubmitOutcome::MissingPhoto => error(
            StatusCode::NOT_FOUND,
            "unknown_photo",
            "The Photo is not part of the persisted Library",
        ),
        ProcessingExportSubmitOutcome::Unavailable => error(
            StatusCode::CONFLICT,
            "source_unavailable",
            "The guarded source revision is not currently available",
        ),
        ProcessingExportSubmitOutcome::MissingRecipe => error(
            StatusCode::NOT_FOUND,
            "missing_recipe",
            "Save an Edit State before submitting an Export",
        ),
        ProcessingExportSubmitOutcome::StepNotCurrent(_) => error(
            StatusCode::CONFLICT,
            "step_not_current",
            "Export is bounded to the current Edit State",
        ),
        ProcessingExportSubmitOutcome::IncompatibleInput(reason) => match reason {
            ProcessingInputHandoffError::OriginalPhotoMismatch
            | ProcessingInputHandoffError::OriginalSourceStale => error(
                StatusCode::CONFLICT,
                "source_changed",
                "The selected Original input is stale",
            ),
            ProcessingInputHandoffError::ArtifactMissing => error_details(
                StatusCode::UNPROCESSABLE_ENTITY,
                "incompatible_input",
                "The selected artifact input has no published Processing Artifact",
                json!({"reason": "artifact_missing"}),
            ),
            ProcessingInputHandoffError::ArtifactContractMismatch => error_details(
                StatusCode::UNPROCESSABLE_ENTITY,
                "incompatible_input",
                "The selected artifact input no longer matches its published contract",
                json!({"reason": "artifact_contract_mismatch"}),
            ),
        },
        ProcessingExportSubmitOutcome::Invalid(_) => error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_settings",
            "The submission carries a value outside the closed wire shape",
        ),
        ProcessingExportSubmitOutcome::RequestConflict => error(
            StatusCode::CONFLICT,
            "request_conflict",
            "The request identity was already used with a different payload",
        ),
    }
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
        Ok(Some(artifact)) => match retained_artifact_json(&state, &artifact).await {
            Ok(value) => Json(value).into_response(),
            Err(response) => response,
        },
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
pub(crate) fn reconcile_processing_exports(application: &std::sync::Arc<crate::app::Application>) {
    let library = std::sync::Arc::clone(&application.library);
    tokio::spawn(async move {
        let Ok(unfinished) = library.unfinished_processing_exports().await else {
            return;
        };
        let now = unix_seconds_now();
        for work in unfinished {
            let _ = library
                .fail_processing_export(
                    work.admission.request_id.as_str(),
                    INTERRUPTED_BY_RESTART,
                    now,
                )
                .await;
        }
    });
}
