//! Retained non-darktable adapters project canonical workspace tool state.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use slipstream_core::{
    AutomaticAdjustment, ComposableEditRecipe, ComposableEditRecipeWriteOutcome,
    ProcessingArtifactId, ProcessingGeometry, ProcessingImageContract, ProcessingInput,
    ProcessingModuleId, ProcessingParameterSnapshot, ProcessingStep, ProcessingStepId,
    SaveComposableEditRecipe,
};

use slipstream_processing::modules::{
    ImageContract, ModuleAvailability, ModuleRegistry, Parameters,
};

use crate::http::HttpState;
#[path = "processing_recipe_admission.rs"]
mod admission;
#[path = "processing_recipe_state.rs"]
mod film_state;
pub(crate) use admission::validate_step_admission;

pub(crate) fn inspect_film_state(path: &std::path::Path) -> Result<Value, String> {
    film_state::inspect(path)
}

pub(crate) async fn read_recipe(
    state: &HttpState,
    photo_id: &str,
) -> Result<Option<slipstream_core::ComposableEditRecipeRead>, String> {
    let mut read = state
        .application
        .library
        .composable_edit_recipe_read(photo_id)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(read) = read.as_mut()
        && !read.source_available
    {
        // Library facts retain the last location revision for historical identity.
        // Adapter reads disclose only currently available live source evidence;
        // the saved recipe and its captured input guard remain untouched.
        read.current_source_revision = None;
    }
    if let Some(read) = read.as_mut()
        && let Some(workspace) = state
            .application
            .workspaces
            .lookup_photo(photo_id)
            .map_err(|e| e.to_string())?
    {
        let directory = state
            .application
            .workspaces
            .directory(&workspace.id)
            .map_err(|e| e.to_string())?;
        match directory.child(".edits/spektrafilm/adapter") {
            Ok(_) => {
                let owner = film_state::FilmState::open(&directory)?;
                if let Some(recipe) = owner.recipe()? {
                    read.recipe = Some(recipe);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(read)
}

/// Resolves historical intent only against its exact already protected source.
/// The versioned tool importer owns conversion; retained database bytes stay intact.
pub(crate) async fn legacy_darktable_input(
    state: &HttpState,
    photo_id: &str,
    step_id: &str,
) -> Result<(Value, Value), String> {
    let read = state
        .application
        .library
        .composable_edit_recipe_read(photo_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "The Photo is not part of the Library".to_owned())?;
    let recipe = read
        .recipe
        .ok_or_else(|| "No retained processing recipe exists".to_owned())?;
    let step = recipe
        .steps
        .iter()
        .find(|s| s.step_id.as_str() == step_id)
        .ok_or_else(|| "The retained step is missing".to_owned())?;
    if step.module.as_str() != slipstream_processing::modules::DARKTABLE_MODULE {
        return Err("The retained step is not darktable".into());
    }
    let ProcessingInput::Original {
        photo_id: bound_photo,
        source_revision,
    } = &step.input
    else {
        return Err("Legacy darktable migration requires its captured Original identity; select and review an explicit input through the darktable namespace".into());
    };
    if bound_photo != photo_id || source_revision.is_empty() {
        return Err("Legacy darktable source identity is invalid".into());
    }
    let input = state
        .application
        .workspaces
        .prepared_input(photo_id, source_revision)
        .map_err(|e| format!("The exact historical Original capture is unavailable: {e}"))?;
    let legacy = json!({"module":step.module.as_str(),"schemaVersion":step.parameters.schema_version,"tree":step.parameters.tree,"stepId":step.step_id.as_str(),"sourceRevision":source_revision,"recipeRevision":recipe.revision});
    Ok((input, legacy))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SaveBody {
    request_id: String,
    expected_recipe_revision: Option<String>,
    expected_source_revision: String,
    current_step_id: Option<String>,
    steps: Vec<StepBody>,
    automatic_adjustment: Option<AutomaticAdjustmentBody>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AutomaticAdjustmentBody {
    step_id: String,
    operation: String,
    multi_priority: i64,
    instruction: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RebindBody {
    request_id: String,
    expected_recipe_revision: String,
    new_source_revision: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StepBody {
    step_id: String,
    module: String,
    input: InputBody,
    parameters: ParametersBody,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ParametersBody {
    schema_version: String,
    tree: Value,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum InputBody {
    #[serde(rename_all = "camelCase")]
    Original {
        photo_id: String,
        source_revision: String,
    },
    #[serde(rename_all = "camelCase")]
    Artifact {
        artifact_id: String,
        contract: ImageContractBody,
    },
    #[serde(rename_all = "camelCase")]
    WorkspaceArtifact {
        workspace_id: String,
        artifact_id: String,
        revision: String,
        contract: ImageContractBody,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImageContractBody {
    format: String,
    precision: String,
    color_space: String,
    transfer: String,
    geometry: GeometryBody,
    encoding: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GeometryBody {
    width: u32,
    height: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecipeResponse {
    photo_id: String,
    source_revision: String,
    web_url: String,
    recipe: Option<Value>,
}

fn contract(body: ImageContractBody) -> Result<ProcessingImageContract, String> {
    let geometry = ProcessingGeometry::new(body.geometry.width, body.geometry.height)
        .map_err(|error| error.to_string())?;
    let contract = ProcessingImageContract {
        format: body.format,
        precision: body.precision,
        color_space: body.color_space,
        transfer: body.transfer,
        geometry,
        encoding: body.encoding,
    };
    contract.validate().map_err(|error| error.to_string())?;
    Ok(contract)
}

fn input(body: InputBody) -> Result<ProcessingInput, String> {
    match body {
        InputBody::Original {
            photo_id,
            source_revision,
        } => Ok(ProcessingInput::Original {
            photo_id,
            source_revision,
        }),
        InputBody::Artifact {
            artifact_id,
            contract: image,
        } => Ok(ProcessingInput::Artifact {
            artifact_id: ProcessingArtifactId::new(&artifact_id)
                .map_err(|error| error.to_string())?,
            contract: contract(image)?,
        }),
        InputBody::WorkspaceArtifact {
            workspace_id,
            artifact_id,
            revision,
            contract: image,
        } => Ok(ProcessingInput::WorkspaceArtifact {
            workspace_id,
            artifact_id,
            revision,
            contract: contract(image)?,
        }),
    }
}

fn recipe_from_body(
    photo_id: &str,
    body: SaveBody,
) -> Result<(ComposableEditRecipe, Option<AutomaticAdjustment>), String> {
    let revision = body
        .expected_recipe_revision
        .clone()
        .unwrap_or_else(|| "draft".to_owned());
    let steps = body
        .steps
        .into_iter()
        .map(|step| {
            Ok(ProcessingStep {
                step_id: ProcessingStepId::new(&step.step_id).map_err(|error| error.to_string())?,
                module: ProcessingModuleId::new(&step.module).map_err(|error| error.to_string())?,
                input: input(step.input)?,
                parameters: ProcessingParameterSnapshot::new(
                    &step.parameters.schema_version,
                    step.parameters.tree,
                )
                .map_err(|error| error.to_string())?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let current_step_id = body
        .current_step_id
        .as_deref()
        .map(|id| ProcessingStepId::new(id).map_err(|error| error.to_string()))
        .transpose()?;
    let automatic_adjustment = body
        .automatic_adjustment
        .map(|auto| {
            if !matches!(auto.operation.as_str(), "exposure" | "channelmixerrgb") {
                return Err("automaticAdjustment.operation is not qualified".to_owned());
            }
            if !(0..=128).contains(&auto.multi_priority) {
                return Err("automaticAdjustment.multiPriority is outside 0..=128".to_owned());
            }
            if !auto.instruction.is_object() {
                return Err("automaticAdjustment.instruction must be an object".to_owned());
            }
            let step_id =
                ProcessingStepId::new(&auto.step_id).map_err(|error| error.to_string())?;
            Ok::<_, String>(AutomaticAdjustment::new(
                step_id,
                auto.operation,
                auto.multi_priority,
                auto.instruction,
            ))
        })
        .transpose()?;
    let recipe = ComposableEditRecipe {
        photo_id: photo_id.to_owned(),
        revision,
        source_revision: body.expected_source_revision,
        current_step_id,
        steps,
    };
    recipe.validate().map_err(|error| error.to_string())?;
    Ok((recipe, automatic_adjustment))
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
        ProcessingInput::WorkspaceArtifact {
            workspace_id,
            artifact_id,
            revision,
            contract,
        } => {
            json!({"kind":"workspace-artifact","workspaceId":workspace_id,"artifactId":artifact_id,"revision":revision,"contract":contract_json(contract)})
        }
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

pub(crate) fn recipe_json(recipe: &ComposableEditRecipe) -> Value {
    let registry = ModuleRegistry::new(ModuleAvailability::ready(), ModuleAvailability::ready());
    let execution_refusals: Vec<Value> = recipe
        .steps
        .iter()
        .filter_map(|step| {
            if step.module.as_str()==slipstream_processing::modules::DARKTABLE_MODULE {
                return Some(json!({"stepId":step.step_id.as_str(),"code":"unsupported_state","message":"Retained darktable state remains readable; review and migrate through the full darktable namespace to execute it"}));
            }
            let parameters = Parameters {
                module: step.module.as_str().to_owned(),
                version: step.parameters.schema_version.clone(),
                tree: step.parameters.tree.clone(),
            };
            registry
                .validate_parameters(&parameters)
                .err()
                .map(|refusal| {
                    json!({
                        "stepId": step.step_id.as_str(),
                        "code": refusal.code,
                        "message": refusal.message,
                    })
                })
        })
        .collect();
    json!({
        "photoId": recipe.photo_id,
        "revision": recipe.revision,
        "sourceRevision": recipe.source_revision,
        "currentStepId": recipe.current_step_id.as_ref().map(ProcessingStepId::as_str),
        "executionRefusals": execution_refusals,
        "steps": recipe.steps.iter().map(|step| json!({
            "stepId": step.step_id.as_str(),
            "module": step.module.as_str(),
            "input": input_json(&step.input),
            "parameters": {
                "schemaVersion": step.parameters.schema_version,
                "tree": &step.parameters.tree,
            },
        })).collect::<Vec<_>>(),
    })
}

fn error_value(
    status: StatusCode,
    code: &'static str,
    message: impl Into<String>,
    details: Option<Value>,
) -> Response {
    crate::http::cli_error(status, code, message, details.unwrap_or_else(|| json!({})))
        .into_response()
}

fn error(status: StatusCode, code: &'static str, message: impl Into<String>) -> Response {
    error_value(status, code, message, None)
}

fn outcome_response(
    outcome: ComposableEditRecipeWriteOutcome,
    request_id: &str,
    source_observation: &str,
) -> Response {
    match outcome {
        ComposableEditRecipeWriteOutcome::Saved(recipe) => (
            StatusCode::CREATED,
            Json(json!({
                "photoId": recipe.photo_id,
                "requestId": request_id,
                "webUrl": format!("/photos/{}/edit",recipe.photo_id),
                "outcome": "saved",
                "recipe": recipe_json(&recipe),
                "recipeVersion": recipe.revision,
                "sourceRevision": source_observation,
            })),
        )
            .into_response(),
        ComposableEditRecipeWriteOutcome::Replayed(recipe) => (
            StatusCode::OK,
            Json(json!({
                "photoId": recipe.photo_id,
                "requestId": request_id,
                "webUrl": format!("/photos/{}/edit",recipe.photo_id),
                "outcome": "replayed",
                "recipe": recipe_json(&recipe),
                "recipeVersion": recipe.revision,
                "sourceRevision": source_observation,
            })),
        )
            .into_response(),
        ComposableEditRecipeWriteOutcome::Unchanged(recipe) => (
            StatusCode::OK,
            Json(json!({
                "photoId": recipe.photo_id,
                "requestId": request_id,
                "webUrl": format!("/photos/{}/edit",recipe.photo_id),
                "outcome": "unchanged",
                "recipe": recipe_json(&recipe),
                "recipeVersion": recipe.revision,
                "sourceRevision": source_observation,
            })),
        )
            .into_response(),
        ComposableEditRecipeWriteOutcome::Conflict(recipe) => error_value(
            StatusCode::CONFLICT,
            "conflict",
            "The expected composable recipe revision is no longer current",
            Some(
                json!({"currentRecipeVersion":recipe.as_ref().map(|r|&r.revision),"currentSourceRevision":recipe.as_ref().map(|r|&r.source_revision)}),
            ),
        ),
        ComposableEditRecipeWriteOutcome::SourceChanged(recipe) => error_value(
            StatusCode::CONFLICT,
            "source_changed",
            "The guarded source revision is no longer current",
            recipe.map(|value| recipe_json(&value)),
        ),
        ComposableEditRecipeWriteOutcome::MissingPhoto => error(
            StatusCode::NOT_FOUND,
            "unknown_photo",
            "The Photo is not part of the persisted Library",
        ),
        ComposableEditRecipeWriteOutcome::Unavailable => error(
            StatusCode::CONFLICT,
            "source_unavailable",
            "The guarded source revision is not currently available",
        ),
        ComposableEditRecipeWriteOutcome::Invalid(reason) => error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_recipe",
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

pub(crate) async fn get_composable_edit_recipe(
    State(state): State<HttpState>,
    Path(photo_id): Path<String>,
) -> Response {
    let Some(read) = (match read_recipe(&state, &photo_id).await {
        Ok(value) => value,
        Err(library_error) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage",
                library_error.to_string(),
            );
        }
    }) else {
        return error(
            StatusCode::NOT_FOUND,
            "unknown_photo",
            "The Photo is not part of the persisted Library",
        );
    };
    Json(RecipeResponse {
        photo_id: photo_id.clone(),
        source_revision: read.current_source_revision.clone().unwrap_or_default(),
        web_url: format!("/photos/{photo_id}/edit"),
        recipe: read.recipe.as_ref().map(recipe_json),
    })
    .into_response()
}

pub(crate) async fn post_composable_edit_recipe(
    State(state): State<HttpState>,
    Path(photo_id): Path<String>,
    Json(body): Json<SaveBody>,
) -> Response {
    save_body(state, photo_id, body, None).await
}

async fn save_body(
    state: HttpState,
    photo_id: String,
    body: SaveBody,
    request_intent: Option<Value>,
) -> Response {
    let request_id = body.request_id.clone();
    let expected_recipe_revision = body.expected_recipe_revision.clone();
    let expected_source_revision = body.expected_source_revision.clone();
    let (recipe, automatic_adjustment) = match recipe_from_body(&photo_id, body) {
        Ok(value) => value,
        Err(message) => {
            return error(StatusCode::UNPROCESSABLE_ENTITY, "invalid_recipe", message);
        }
    };
    let mut mutation = SaveComposableEditRecipe {
        photo_id,
        request_id,
        expected_recipe_revision,
        expected_source_revision,
        request_intent: None,
        recipe,
        automatic_adjustment,
    };
    match state
        .application
        .library
        .replay_composable_edit_recipe(mutation.clone())
        .await
    {
        Ok(Some(outcome)) => {
            return outcome_response(
                outcome,
                &mutation.request_id,
                &mutation.expected_source_revision,
            );
        }
        Ok(None) => (),
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, "storage", e.to_string()),
    }
    if mutation.automatic_adjustment.is_some() {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "unsupported_action",
            "Generic automatic actions are unavailable for this adapter; use the full darktable namespace for darktable actions",
        );
    }
    if mutation.request_id.is_empty()
        || mutation.request_id.len() > 128
        || !mutation
            .request_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Request identity is invalid",
        );
    }
    let read = match state
        .application
        .library
        .composable_edit_recipe_read(&mutation.photo_id)
        .await
    {
        Ok(Some(read)) => read,
        Ok(None) => {
            return error(
                StatusCode::NOT_FOUND,
                "unknown_photo",
                "The Photo is not part of the Library",
            );
        }
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, "storage", e.to_string()),
    };
    let workspace = match state
        .application
        .workspaces
        .lookup_photo(&mutation.photo_id)
    {
        Ok(Some(workspace)) => workspace,
        Ok(None) => {
            return error(
                StatusCode::CONFLICT,
                "workspace_unavailable",
                "Open the Photo Workspace before saving an edit",
            );
        }
        Err(e) => return error(StatusCode::CONFLICT, "workspace_unavailable", e.to_string()),
    };
    let directory = match state.application.workspaces.directory(&workspace.id) {
        Ok(d) => d,
        Err(e) => return error(StatusCode::CONFLICT, "workspace_unavailable", e.to_string()),
    };
    let mut owner = match film_state::FilmState::open(&directory) {
        Ok(s) => s,
        Err(e) => return error(StatusCode::CONFLICT, "state_unavailable", e),
    };
    let intent=request_intent.unwrap_or_else(||json!({"kind":"save","expectedRecipeRevision":mutation.expected_recipe_revision,"expectedSourceRevision":mutation.expected_source_revision,"recipe":recipe_json(&mutation.recipe)}));
    match owner.replay(&mutation.request_id, &intent) {
        Ok(Some(outcome)) => {
            return outcome_response(
                outcome,
                &mutation.request_id,
                &mutation.expected_source_revision,
            );
        }
        Ok(None) => (),
        Err(e) => return error(StatusCode::CONFLICT, "state_unavailable", e),
    };
    let current = match owner.recipe() {
        Ok(Some(recipe)) => Some(recipe),
        Ok(None) => read.recipe,
        Err(e) => return error(StatusCode::CONFLICT, "state_unavailable", e),
    };
    if mutation.expected_recipe_revision.as_deref() != current.as_ref().map(|r| r.revision.as_str())
    {
        return outcome_response(
            ComposableEditRecipeWriteOutcome::Conflict(current),
            &mutation.request_id,
            &mutation.expected_source_revision,
        );
    }
    if let Err(e) = film_state::preserve_retained(current.as_ref(), &mutation.recipe) {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "unsupported_state", e);
    }
    if mutation
        .recipe
        .current_step()
        .is_some_and(|s| s.module.as_str() == slipstream_processing::modules::DARKTABLE_MODULE)
        && current.as_ref().and_then(|r| r.current_step_id.as_ref())
            != mutation.recipe.current_step_id.as_ref()
    {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "unsupported_state",
            "Generic writes cannot change darktable selection; use the full darktable namespace",
        );
    }
    let mut supported = mutation.recipe.clone();
    supported.steps.retain(|s| {
        s.module.as_str() == slipstream_processing::modules::SPEKTRAFILM_MODULE
            && current
                .as_ref()
                .and_then(|r| r.steps.iter().find(|old| old.step_id == s.step_id))
                != Some(s)
    });
    let mut input_leases = Vec::new();
    for step in &mut supported.steps {
        if let ProcessingInput::WorkspaceArtifact {
            workspace_id,
            artifact_id,
            revision,
            ..
        } = &step.input
        {
            if workspace_id != &workspace.id {
                return error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "incompatible_input",
                    "Film input must be selected from the same Photo Workspace",
                );
            }
            let (lease, admitted) =
                match state
                    .application
                    .workspaces
                    .lease_input(workspace_id, artifact_id, revision)
                {
                    Ok(value) => value,
                    Err(e) => {
                        return error(StatusCode::CONFLICT, "artifact_changed", e.to_string());
                    }
                };
            let actual: ImageContractBody =
                match serde_json::from_value(admitted["imageContract"].clone()) {
                    Ok(value) => value,
                    Err(e) => {
                        return error(
                            StatusCode::UNPROCESSABLE_ENTITY,
                            "incompatible_input",
                            e.to_string(),
                        );
                    }
                };
            let actual = match contract(actual) {
                Ok(value) => value,
                Err(e) => return error(StatusCode::UNPROCESSABLE_ENTITY, "incompatible_input", e),
            };
            let identity = |field: &str| {
                admitted[field]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            };
            let (Some(captured_workspace), Some(captured_artifact), Some(captured_revision)) = (
                identity("workspaceId"),
                identity("artifactId"),
                identity("revision"),
            ) else {
                return error(
                    StatusCode::CONFLICT,
                    "artifact_changed",
                    "Protected input admission has no complete binding",
                );
            };
            if captured_workspace != workspace.id {
                return error(
                    StatusCode::CONFLICT,
                    "artifact_changed",
                    "Protected input admission changed the workspace",
                );
            }
            step.input = ProcessingInput::WorkspaceArtifact {
                workspace_id: captured_workspace,
                artifact_id: captured_artifact,
                revision: captured_revision,
                contract: actual,
            };
            mutation
                .recipe
                .steps
                .iter_mut()
                .find(|saved| saved.step_id == step.step_id)
                .expect("supported step belongs to submitted recipe")
                .input = step.input.clone();
            input_leases.push(lease);
        }
    }
    if let Err(admission) = validate_step_admission(&state, &supported).await {
        return error(admission.status, admission.code, admission.message);
    }
    match owner.settle(&mutation, current, intent) {
        Ok(outcome) => outcome_response(
            outcome,
            &mutation.request_id,
            &mutation.expected_source_revision,
        ),
        Err(e) => error(StatusCode::CONFLICT, "state_conflict", e),
    }
}

pub(crate) async fn post_composable_edit_recipe_rebind(
    State(state): State<HttpState>,
    Path(photo_id): Path<String>,
    Json(body): Json<RebindBody>,
) -> Response {
    let historical = slipstream_core::RebindComposableEditRecipe {
        photo_id: photo_id.clone(),
        request_id: body.request_id.clone(),
        expected_recipe_revision: body.expected_recipe_revision.clone(),
        new_source_revision: body.new_source_revision.clone(),
    };
    match state
        .application
        .library
        .replay_composable_edit_recipe_rebind(historical)
        .await
    {
        Ok(Some(outcome)) => {
            return outcome_response(outcome, &body.request_id, &body.new_source_revision);
        }
        Ok(None) => (),
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, "storage", e.to_string()),
    }
    let intent = json!({"kind":"rebind","expectedRecipeRevision":body.expected_recipe_revision,"newSourceRevision":body.new_source_revision});
    match state.application.workspaces.lookup_photo(&photo_id) {
        Ok(Some(workspace)) => {
            let directory = match state.application.workspaces.directory(&workspace.id) {
                Ok(v) => v,
                Err(e) => {
                    return error(StatusCode::CONFLICT, "workspace_unavailable", e.to_string());
                }
            };
            let owner = match film_state::FilmState::open(&directory) {
                Ok(v) => v,
                Err(e) => return error(StatusCode::CONFLICT, "state_unavailable", e),
            };
            match owner.replay(&body.request_id, &intent) {
                Ok(Some(outcome)) => {
                    return outcome_response(outcome, &body.request_id, &body.new_source_revision);
                }
                Ok(None) => (),
                Err(e) => return error(StatusCode::CONFLICT, "state_unavailable", e),
            };
        }
        Ok(None) => (),
        Err(e) => return error(StatusCode::CONFLICT, "workspace_unavailable", e.to_string()),
    }
    let read = match read_recipe(&state, &photo_id).await {
        Ok(Some(read)) => read,
        Ok(None) => {
            return error(
                StatusCode::NOT_FOUND,
                "unknown_photo",
                "The Photo is not part of the Library",
            );
        }
        Err(e) => return error(StatusCode::CONFLICT, "state_unavailable", e),
    };
    let Some(mut recipe) = read.recipe else {
        return error(
            StatusCode::CONFLICT,
            "recipe_conflict",
            "No saved recipe is available",
        );
    };
    if recipe.revision != body.expected_recipe_revision {
        return outcome_response(
            ComposableEditRecipeWriteOutcome::Conflict(Some(recipe)),
            &body.request_id,
            &body.new_source_revision,
        );
    }
    if !recipe.steps.iter().any(|s| {
        s.module.as_str() == slipstream_processing::modules::SPEKTRAFILM_MODULE
            && matches!(s.input, ProcessingInput::Original { .. })
    }) {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "unsupported_state",
            "No supported Original-bound state is eligible; darktable rebinding uses the full darktable namespace",
        );
    }
    let admitted = match state
        .application
        .workspaces
        .prepared_input(&photo_id, &body.new_source_revision)
    {
        Ok(v) => v,
        Err(e) => return error(StatusCode::CONFLICT, "source_unavailable", e.to_string()),
    };
    let parsed: ImageContractBody = match serde_json::from_value(admitted["imageContract"].clone())
    {
        Ok(v) => v,
        Err(e) => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "incompatible_input",
                e.to_string(),
            );
        }
    };
    let image = match contract(parsed) {
        Ok(v) => v,
        Err(e) => return error(StatusCode::UNPROCESSABLE_ENTITY, "incompatible_input", e),
    };
    for step in &mut recipe.steps {
        if step.module.as_str() == slipstream_processing::modules::SPEKTRAFILM_MODULE
            && matches!(step.input, ProcessingInput::Original { .. })
        {
            step.input = ProcessingInput::WorkspaceArtifact {
                workspace_id: admitted["workspaceId"].as_str().unwrap_or_default().into(),
                artifact_id: admitted["artifactId"].as_str().unwrap_or_default().into(),
                revision: admitted["revision"].as_str().unwrap_or_default().into(),
                contract: image.clone(),
            };
        }
    }
    let wire = recipe_json(&recipe);
    let body: SaveBody = match serde_json::from_value(
        json!({"requestId":body.request_id,"expectedRecipeRevision":body.expected_recipe_revision,"expectedSourceRevision":body.new_source_revision,"currentStepId":wire["currentStepId"],"steps":wire["steps"]}),
    ) {
        Ok(v) => v,
        Err(e) => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_recipe",
                e.to_string(),
            );
        }
    };
    save_body(state, photo_id, body, Some(intent)).await
}
