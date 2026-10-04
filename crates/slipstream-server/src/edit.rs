//! Agent-facing stateful Photo editing surface (GitHub Issue #509).
//!
//! This is a narrow projection over the durable composable recipe store: the
//! Agent addresses one qualified Engine Module control at a time, while the
//! complete recipe remains an internal persistence representation.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use serde_json::{Value, json};
use slipstream_core::{
    ComposableEditRecipe, ComposableEditRecipeRead, ComposableEditRecipeWriteOutcome, LibraryError,
    ProcessingArtifactId, ProcessingImageContract, ProcessingInput, ProcessingModuleId,
    ProcessingParameterSnapshot, ProcessingStep, ProcessingStepId, SaveComposableEditRecipe,
};
use slipstream_processing::modules::{
    DARKTABLE_MODULE, DARKTABLE_PARAMETER_VERSION, ModuleAvailability, ModuleErrorCode,
    ModuleRegistry, apply_darktable_control, reset_darktable_control,
};

use crate::http::HttpState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SetBody {
    request_id: String,
    expected_edit_revision: Option<String>,
    target: String,
    control: String,
    value: Value,
    from: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ResetBody {
    request_id: String,
    expected_edit_revision: Option<String>,
    target: String,
    control: String,
}

fn error_value(
    status: StatusCode,
    code: &'static str,
    message: impl Into<String>,
    details: Value,
) -> Response {
    crate::http::cli_error(status, code, message, details).into_response()
}

fn error(status: StatusCode, code: &'static str, message: impl Into<String>) -> Response {
    error_value(status, code, message, json!({}))
}

fn registry() -> ModuleRegistry {
    ModuleRegistry::new(ModuleAvailability::ready(), ModuleAvailability::ready())
}

fn catalog_json(registry: &ModuleRegistry) -> Vec<Value> {
    registry
        .descriptions()
        .iter()
        .map(|description| {
            json!({
                "engine": description.id.name,
                "adapterVersion": description.id.adapter_version,
                "schemaVersions": description.parameter_versions,
                "availability": description.availability,
                "modules": description.engine_modules,
            })
        })
        .collect()
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
            "contract": contract_json(contract),
        }),
    }
}

fn contract_json(contract: &ProcessingImageContract) -> Value {
    json!({
        "format": contract.format,
        "precision": contract.precision,
        "colorSpace": contract.color_space,
        "transfer": contract.transfer,
        "geometry": {
            "width": contract.geometry.width,
            "height": contract.geometry.height,
        },
        "encoding": contract.encoding,
    })
}

fn current_requires_rebind(
    read: &ComposableEditRecipeRead,
    recipe: Option<&ComposableEditRecipe>,
) -> bool {
    let Some(recipe) = recipe else {
        return false;
    };
    let Some(step_id) = recipe.current_step_id.as_ref() else {
        return false;
    };
    let Some(step) = recipe.steps.iter().find(|step| &step.step_id == step_id) else {
        return true;
    };
    match &step.input {
        ProcessingInput::Original {
            photo_id,
            source_revision,
        } => {
            photo_id != &recipe.photo_id
                || source_revision != &recipe.source_revision
                || read.current_source_revision.as_deref() != Some(recipe.source_revision.as_str())
        }
        ProcessingInput::Artifact { .. } => false,
    }
}

fn current_requires_source(recipe: Option<&ComposableEditRecipe>) -> bool {
    let Some(recipe) = recipe else {
        return false;
    };
    let Some(step_id) = recipe.current_step_id.as_ref() else {
        return false;
    };
    recipe
        .steps
        .iter()
        .find(|step| &step.step_id == step_id)
        .is_some_and(|step| matches!(step.input, ProcessingInput::Original { .. }))
}

fn current_projection(recipe: Option<&ComposableEditRecipe>) -> (Option<Value>, Option<String>) {
    let Some(recipe) = recipe else {
        return (None, None);
    };
    let Some(step_id) = recipe.current_step_id.as_ref() else {
        return (None, None);
    };
    let Some(step) = recipe.steps.iter().find(|step| &step.step_id == step_id) else {
        return (None, None);
    };
    let controls = if step.module.as_str() == DARKTABLE_MODULE {
        let ev = step
            .parameters
            .tree
            .get("stack")
            .and_then(Value::as_array)
            .and_then(|stack| {
                stack
                    .iter()
                    .find(|entry| entry.get("operation") == Some(&Value::String("exposure".into())))
            })
            .and_then(|entry| entry.get("params"))
            .and_then(|params| params.get("exposure"))
            .cloned();
        json!({"exposure": {"ev": ev, "reset": 0.0}})
    } else {
        json!({})
    };
    (
        Some(json!({
            "stepId": step.step_id.as_str(),
            "engine": step.module.as_str(),
            "input": input_json(&step.input),
            "controls": controls,
        })),
        Some(step.step_id.as_str().to_owned()),
    )
}

fn state_json(
    photo_id: &str,
    read: &ComposableEditRecipeRead,
    recipe: Option<&ComposableEditRecipe>,
) -> Value {
    let (current, current_step_id) = current_projection(recipe);
    let registry = registry();
    let requires_rebind = current_requires_rebind(read, recipe);
    let can_process = current.is_some()
        && !requires_rebind
        && (!current_requires_source(recipe) || read.source_available);
    json!({
        "photoId": photo_id,
        "sourceRevision": read.current_source_revision,
        "sourceAvailable": read.source_available,
        "editRevision": recipe.map(|recipe| recipe.revision.as_str()),
        "currentStepId": current_step_id,
        "current": current,
        "engineModules": catalog_json(&registry),
        "canSave": read.source_available && read.current_source_revision.is_some(),
        "canPreview": can_process,
        "canExport": can_process,
        "preview": crate::processing_preview::preview_status(photo_id, recipe),
        "recentOutputs": [],
    })
}

async fn read_photo(
    state: &HttpState,
    photo_id: &str,
) -> Result<ComposableEditRecipeRead, Response> {
    match state
        .application
        .library
        .composable_edit_recipe_read(photo_id)
        .await
    {
        Ok(Some(read)) => Ok(read),
        Ok(None) => Err(error(
            StatusCode::NOT_FOUND,
            "unknown_photo",
            "The Photo is not part of the persisted Library",
        )),
        Err(library_error) => Err(error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage",
            library_error.to_string(),
        )),
    }
}
async fn recent_outputs(state: &HttpState, photo_id: &str) -> Result<Value, Response> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let list = state
        .application
        .library
        .list_processing_exports(photo_id, now)
        .await
        .map_err(|library_error| {
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage",
                library_error.to_string(),
            )
        })?;
    let mut outputs = list
        .works
        .into_iter()
        .map(|work| {
            json!({
                "kind": "export",
                "requestId": work.admission.request_id,
                "stepId": work.admission.step_id.as_str(),
                "engine": work.admission.module.as_str(),
                "editRevision": work.admission.recipe_revision,
                "state": work.state.as_str(),
                "artifactId": work.artifact_id.map(|id| id.as_str().to_owned()),
                "acceptedAt": work.accepted_at,
                "terminalAt": work.terminal_at,
            })
        })
        .collect::<Vec<_>>();
    outputs.extend(list.artifacts.into_iter().map(|artifact| {
        json!({
            "kind": "artifact",
            "artifactId": artifact.artifact_id.as_str(),
            "stepId": artifact.step_id.as_str(),
            "engine": artifact.module.as_str(),
            "bundleId": artifact.bundle_id,
            "sha256": artifact.sha256,
            "byteLength": artifact.byte_length,
        })
    }));
    Ok(Value::Array(outputs))
}

#[allow(clippy::result_large_err)]
fn parse_target(target: &str, control: &str) -> Result<String, Response> {
    if target != "darktable.exposure" {
        return Err(error_value(
            StatusCode::UNPROCESSABLE_ENTITY,
            "unsupported_control",
            "The requested Engine Module is not qualified for stateful editing",
            json!({"target": target, "control": control}),
        ));
    }
    if control != "ev" && control != "all" {
        return Err(error_value(
            StatusCode::UNPROCESSABLE_ENTITY,
            "unsupported_control",
            "The requested control is not qualified for stateful editing",
            json!({"target": target, "control": control}),
        ));
    }
    Ok(format!(
        "{target}.{}",
        if control == "all" { "ev" } else { control }
    ))
}

#[allow(clippy::result_large_err)]
fn parse_input(
    photo_id: &str,
    source_revision: &str,
    from: Option<&str>,
) -> Result<ProcessingInput, Response> {
    match from.unwrap_or("original") {
        "original" => Ok(ProcessingInput::Original {
            photo_id: photo_id.to_owned(),
            source_revision: source_revision.to_owned(),
        }),
        value if value.starts_with("artifact:") => {
            let artifact_id = value.trim_start_matches("artifact:");
            let artifact_id = ProcessingArtifactId::new(artifact_id).map_err(|error| {
                error_value(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "invalid_input",
                    error.to_string(),
                    json!({"from": value}),
                )
            })?;
            Ok(ProcessingInput::Artifact {
                artifact_id,
                contract: ProcessingImageContract {
                    format: "unresolved".into(),
                    precision: "unresolved".into(),
                    color_space: "unresolved".into(),
                    transfer: "unresolved".into(),
                    geometry: slipstream_core::ProcessingGeometry::new(1, 1).expect("bounded"),
                    encoding: "unresolved".into(),
                },
            })
        }
        value => Err(error_value(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_input",
            "from must be `original` or `artifact:ARTIFACT_ID`",
            json!({"from": value}),
        )),
    }
}

async fn resolve_input(
    state: &HttpState,
    photo_id: &str,
    source_revision: &str,
    from: Option<&str>,
) -> Result<ProcessingInput, Response> {
    let input = parse_input(photo_id, source_revision, from)?;
    let ProcessingInput::Artifact { artifact_id, .. } = input else {
        return Ok(input);
    };
    let Some(artifact) = state
        .application
        .library
        .processing_artifact(artifact_id.as_str())
        .await
        .map_err(|library_error| {
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage",
                library_error.to_string(),
            )
        })?
    else {
        return Err(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "incompatible_input",
            "The processing artifact is missing or expired",
        ));
    };
    if artifact.photo_id != photo_id {
        return Err(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "incompatible_input",
            "The processing artifact belongs to a different Photo",
        ));
    }
    Ok(ProcessingInput::Artifact {
        artifact_id,
        contract: artifact.output_contract,
    })
}

#[allow(clippy::result_large_err)]
fn next_step_id(recipe: Option<&ComposableEditRecipe>) -> Result<ProcessingStepId, Response> {
    let used = recipe
        .map(|recipe| {
            recipe
                .steps
                .iter()
                .map(|step| step.step_id.as_str())
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_default();
    for index in 1..=64 {
        let candidate = format!("edit-{index}");
        if !used.contains(candidate.as_str()) {
            return ProcessingStepId::new(&candidate).map_err(|contract_error| {
                error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "invalid_edit",
                    contract_error.to_string(),
                )
            });
        }
    }
    Err(error(
        StatusCode::UNPROCESSABLE_ENTITY,
        "edit_limit",
        "The Photo has reached the stateful edit step limit",
    ))
}

fn map_module_error(
    error: slipstream_processing::modules::ModuleError,
    target: &str,
    control: &str,
) -> Response {
    let (code, status) = match error.code {
        ModuleErrorCode::UnsupportedControl => {
            ("unsupported_control", StatusCode::UNPROCESSABLE_ENTITY)
        }
        _ => ("invalid_value", StatusCode::UNPROCESSABLE_ENTITY),
    };
    error_value(
        status,
        code,
        error.message,
        json!({"target": target, "control": control}),
    )
}

#[allow(clippy::result_large_err, clippy::too_many_arguments)]
fn build_recipe(
    photo_id: &str,
    source_revision: &str,
    existing: Option<&ComposableEditRecipe>,
    target: &str,
    control: &str,
    value: Option<&Value>,
    input: ProcessingInput,
    explicit_input: bool,
) -> Result<ComposableEditRecipe, Response> {
    if control == "all" && value.is_some() {
        return Err(error_value(
            StatusCode::UNPROCESSABLE_ENTITY,
            "unsupported_control",
            "`all` is only valid for reset",
            json!({"target": target, "control": control}),
        ));
    }
    let canonical = parse_target(target, control)?;
    let module = ProcessingModuleId::new(DARKTABLE_MODULE).expect("constant module id");
    let current = existing.and_then(|recipe| {
        recipe
            .current_step_id
            .as_ref()
            .and_then(|id| recipe.steps.iter().find(|step| &step.step_id == id))
    });
    let same_step = current
        .filter(|step| step.module.as_str() == DARKTABLE_MODULE)
        .filter(|_| value.is_none() || !explicit_input)
        .filter(|step| {
            matches!(
                existing,
                Some(recipe) if recipe.current_step_id.as_ref() == Some(&step.step_id)
            )
        });
    let (step_id, previous_tree, input) = if let Some(step) = same_step {
        (
            step.step_id.clone(),
            Some(step.parameters.tree.clone()),
            step.input.clone(),
        )
    } else {
        (next_step_id(existing)?, None, input)
    };
    let tree = match value {
        Some(value) => apply_darktable_control(previous_tree.as_ref(), &canonical, value)
            .map_err(|error| map_module_error(error, target, control))?,
        None => reset_darktable_control(previous_tree.as_ref(), &canonical)
            .map_err(|error| map_module_error(error, target, control))?,
    };
    let step = ProcessingStep {
        step_id: step_id.clone(),
        module,
        input,
        parameters: ProcessingParameterSnapshot::new(DARKTABLE_PARAMETER_VERSION, tree).map_err(
            |contract_error| {
                error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "invalid_value",
                    contract_error.to_string(),
                )
            },
        )?,
    };
    let mut steps = existing
        .map(|recipe| recipe.steps.clone())
        .unwrap_or_default();
    if let Some(index) = steps
        .iter()
        .position(|candidate| candidate.step_id == step_id)
    {
        steps[index] = step;
    } else {
        steps.push(step);
    }
    let recipe = ComposableEditRecipe {
        photo_id: photo_id.to_owned(),
        revision: existing
            .map(|recipe| recipe.revision.clone())
            .unwrap_or_else(|| "draft".into()),
        source_revision: source_revision.to_owned(),
        steps,
        current_step_id: Some(step_id),
    };
    recipe.validate().map_err(|contract_error| {
        error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_edit",
            contract_error.to_string(),
        )
    })?;
    Ok(recipe)
}

#[allow(clippy::too_many_arguments)]
async fn save(
    state: &HttpState,
    photo_id: String,
    request_id: String,
    expected_edit_revision: Option<String>,
    request_intent: Value,
    target: &str,
    control: &str,
    recipe: ComposableEditRecipe,
) -> Result<ComposableEditRecipeWriteOutcome, Response> {
    let mutation = SaveComposableEditRecipe {
        photo_id,
        request_id,
        expected_recipe_revision: expected_edit_revision,
        expected_source_revision: recipe.source_revision.clone(),
        request_intent: Some(request_intent),
        recipe,
        automatic_adjustment: None,
    };
    if let Some(outcome) = state
        .application
        .library
        .replay_composable_edit_recipe(mutation.clone())
        .await
        .map_err(|library_error| {
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage",
                library_error.to_string(),
            )
        })?
    {
        return Ok(outcome);
    }
    if let Err(admission) =
        crate::processing_recipe::validate_step_admission(state, &mutation.recipe).await
    {
        return Err(error_value(
            admission.status,
            admission.code,
            admission.message,
            json!({"target": target, "control": control}),
        ));
    }
    match state
        .application
        .library
        .save_composable_edit_recipe(mutation)
        .await
    {
        Ok(outcome) => Ok(outcome),
        Err(LibraryError::Persistence(storage_error)) => Err(error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage",
            storage_error.to_string(),
        )),
        Err(library_error) => Err(error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "processing_recipe",
            library_error.to_string(),
        )),
    }
}

fn outcome_response(
    photo_id: &str,
    read: &ComposableEditRecipeRead,
    outcome: ComposableEditRecipeWriteOutcome,
) -> Response {
    match outcome {
        ComposableEditRecipeWriteOutcome::Saved(recipe) => (
            StatusCode::CREATED,
            Json(json!({"outcome": "saved", "edit": state_json(photo_id, read, Some(&recipe))})),
        )
            .into_response(),
        ComposableEditRecipeWriteOutcome::Replayed(recipe) => (
            StatusCode::OK,
            Json(json!({"outcome": "replayed", "edit": state_json(photo_id, read, Some(&recipe))})),
        )
            .into_response(),
        ComposableEditRecipeWriteOutcome::Unchanged(recipe) => (
            StatusCode::OK,
            Json(
                json!({"outcome": "unchanged", "edit": state_json(photo_id, read, Some(&recipe))}),
            ),
        )
            .into_response(),
        ComposableEditRecipeWriteOutcome::Conflict(recipe) => error_value(
            StatusCode::CONFLICT,
            "edit_conflict",
            "The expected Edit revision is no longer current",
            json!({"edit": state_json(photo_id, read, recipe.as_ref())}),
        ),
        ComposableEditRecipeWriteOutcome::SourceChanged(recipe) => error_value(
            StatusCode::CONFLICT,
            "source_changed",
            "The guarded source revision is no longer current",
            json!({"edit": state_json(photo_id, read, recipe.as_ref())}),
        ),
        ComposableEditRecipeWriteOutcome::MissingPhoto => error(
            StatusCode::NOT_FOUND,
            "unknown_photo",
            "The Photo is not part of the persisted Library",
        ),
        ComposableEditRecipeWriteOutcome::Unavailable => error(
            StatusCode::CONFLICT,
            "source_unavailable",
            "The guarded Original source is not currently available",
        ),
        ComposableEditRecipeWriteOutcome::Invalid(reason) => error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_edit",
            reason.to_string(),
        ),
        ComposableEditRecipeWriteOutcome::RequestConflict => error(
            StatusCode::CONFLICT,
            "request_conflict",
            "The request identity was already used with a different payload",
        ),
        ComposableEditRecipeWriteOutcome::ReceiptExpired => error(
            StatusCode::CONFLICT,
            "receipt_expired",
            "The saved request identity has passed its reconciliation period",
        ),
    }
}

pub(crate) async fn get_edit(
    State(state): State<HttpState>,
    Path(photo_id): Path<String>,
) -> Response {
    let read = match read_photo(&state, &photo_id).await {
        Ok(read) => read,
        Err(response) => return response,
    };
    let mut projection = state_json(&photo_id, &read, read.recipe.as_ref());
    projection["recentOutputs"] = match recent_outputs(&state, &photo_id).await {
        Ok(outputs) => outputs,
        Err(response) => return response,
    };
    Json(projection).into_response()
}

pub(crate) async fn set_edit(
    State(state): State<HttpState>,
    Path(photo_id): Path<String>,
    Json(body): Json<SetBody>,
) -> Response {
    let read = match read_photo(&state, &photo_id).await {
        Ok(read) => read,
        Err(response) => return response,
    };
    let Some(source_revision) = read.current_source_revision.clone().or_else(|| {
        read.recipe
            .as_ref()
            .map(|recipe| recipe.source_revision.clone())
    }) else {
        return error(
            StatusCode::CONFLICT,
            "source_unavailable",
            "The Photo has no current source revision",
        );
    };
    if let Err(response) = parse_target(&body.target, &body.control) {
        return response;
    }
    let input = match resolve_input(&state, &photo_id, &source_revision, body.from.as_deref()).await
    {
        Ok(input) => input,
        Err(response) => return response,
    };
    let recipe = match build_recipe(
        &photo_id,
        &source_revision,
        read.recipe.as_ref(),
        &body.target,
        &body.control,
        Some(&body.value),
        input,
        body.from.is_some(),
    ) {
        Ok(recipe) => recipe,
        Err(response) => return response,
    };
    let request_intent = json!({
        "operation": "set",
        "expectedEditRevision": body.expected_edit_revision.clone(),
        "target": body.target.clone(),
        "control": body.control.clone(),
        "value": body.value.clone(),
        "from": body.from.clone(),
    });
    match save(
        &state,
        photo_id.clone(),
        body.request_id,
        body.expected_edit_revision,
        request_intent,
        &body.target,
        &body.control,
        recipe,
    )
    .await
    {
        Ok(outcome) => outcome_response(&photo_id, &read, outcome),
        Err(response) => response,
    }
}

pub(crate) async fn reset_edit(
    State(state): State<HttpState>,
    Path(photo_id): Path<String>,
    Json(body): Json<ResetBody>,
) -> Response {
    let read = match read_photo(&state, &photo_id).await {
        Ok(read) => read,
        Err(response) => return response,
    };
    if body.expected_edit_revision.is_none() {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "missing_revision",
            "Reset requires the latest observed Edit revision",
        );
    }
    let Some(source_revision) = read.current_source_revision.clone().or_else(|| {
        read.recipe
            .as_ref()
            .map(|recipe| recipe.source_revision.clone())
    }) else {
        return error(
            StatusCode::CONFLICT,
            "source_unavailable",
            "The Photo has no current source revision",
        );
    };
    let input = match resolve_input(&state, &photo_id, &source_revision, None).await {
        Ok(input) => input,
        Err(response) => return response,
    };
    let recipe = match build_recipe(
        &photo_id,
        &source_revision,
        read.recipe.as_ref(),
        &body.target,
        &body.control,
        None,
        input,
        false,
    ) {
        Ok(recipe) => recipe,
        Err(response) => return response,
    };
    let request_intent = json!({
        "operation": "reset",
        "expectedEditRevision": body.expected_edit_revision.clone(),
        "target": body.target.clone(),
        "control": body.control.clone(),
    });
    match save(
        &state,
        photo_id.clone(),
        body.request_id,
        body.expected_edit_revision,
        request_intent,
        &body.target,
        &body.control,
        recipe,
    )
    .await
    {
        Ok(outcome) => outcome_response(&photo_id, &read, outcome),
        Err(response) => response,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn original() -> ProcessingInput {
        ProcessingInput::Original {
            photo_id: "photo-1".into(),
            source_revision: "source-1".into(),
        }
    }

    #[test]
    fn stateful_set_updates_the_current_step_without_replacing_its_input() {
        let first = build_recipe(
            "photo-1",
            "source-1",
            None,
            "darktable.exposure",
            "ev",
            Some(&json!(0.5)),
            original(),
            false,
        )
        .expect("initial state");
        let step_id = first.current_step_id.clone().expect("current step");
        let updated = build_recipe(
            "photo-1",
            "source-1",
            Some(&first),
            "darktable.exposure",
            "ev",
            Some(&json!(0.75)),
            original(),
            false,
        )
        .expect("updated state");
        assert_eq!(updated.steps.len(), 1);
        assert_eq!(updated.current_step_id, Some(step_id));
        assert_eq!(updated.steps[0].input, first.steps[0].input);
        assert_eq!(
            updated.steps[0].parameters.tree["stack"][0]["params"]["exposure"],
            json!(0.75)
        );
    }

    #[test]
    fn reset_all_uses_the_control_reset_value() {
        let first = build_recipe(
            "photo-1",
            "source-1",
            None,
            "darktable.exposure",
            "ev",
            Some(&json!(0.75)),
            original(),
            false,
        )
        .expect("initial state");
        let reset = build_recipe(
            "photo-1",
            "source-1",
            Some(&first),
            "darktable.exposure",
            "all",
            None,
            original(),
            false,
        )
        .expect("reset state");
        assert_eq!(
            reset.steps[0].parameters.tree["stack"][0]["params"]["exposure"],
            json!(0.0)
        );
    }
}
