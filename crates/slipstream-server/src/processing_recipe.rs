//! Caller-controlled composable Edit Recipe HTTP surface (Issue #496).
//!
//! Stores complete caller-owned Processing Steps independently of execution
//! availability, with explicit source rebinding and no implicit conversion.

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use slipstream_core::{
    ComposableEditRecipe, ComposableEditRecipeWriteOutcome, LibraryError, ProcessingArtifactId,
    ProcessingGeometry, ProcessingImageContract, ProcessingInput, ProcessingModuleId,
    ProcessingParameterSnapshot, ProcessingStep, ProcessingStepId, RebindComposableEditRecipe,
    SaveComposableEditRecipe,
};
use slipstream_processing::modules::{
    ImageContract, ModuleAvailability, ModuleRegistry, Parameters,
};

use crate::http::HttpState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SaveBody {
    request_id: String,
    expected_recipe_revision: Option<String>,
    expected_source_revision: String,
    current_step_id: Option<String>,
    steps: Vec<StepBody>,
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
    current_source_revision: Option<String>,
    source_available: bool,
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
    }
}

fn recipe_from_body(photo_id: &str, body: SaveBody) -> Result<ComposableEditRecipe, String> {
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
    let recipe = ComposableEditRecipe {
        photo_id: photo_id.to_owned(),
        revision,
        source_revision: body.expected_source_revision,
        current_step_id,
        steps,
    };
    recipe.validate().map_err(|error| error.to_string())?;
    Ok(recipe)
}

fn module_image_contract(contract: &ProcessingImageContract) -> Option<ImageContract> {
    let format = contract
        .format
        .strip_prefix("image/")
        .unwrap_or(&contract.format)
        .to_owned();
    let precision_bits = contract
        .precision
        .strip_prefix("uint")
        .or_else(|| contract.precision.strip_prefix("float"))
        .and_then(|value| value.parse::<u8>().ok())?;
    Some(ImageContract {
        format,
        color_space: contract.color_space.clone(),
        transfer_function: contract.transfer.clone(),
        precision_bits,
        width: u64::from(contract.geometry.width),
        height: u64::from(contract.geometry.height),
    })
}

#[derive(Debug)]
struct AdmissionError {
    code: &'static str,
    status: StatusCode,
    message: String,
}

impl AdmissionError {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: "incompatible_input",
            status: StatusCode::UNPROCESSABLE_ENTITY,
            message: message.into(),
        }
    }

    fn source_changed(message: impl Into<String>) -> Self {
        Self {
            code: "source_changed",
            status: StatusCode::CONFLICT,
            message: message.into(),
        }
    }
}

async fn validate_step_admission(
    state: &HttpState,
    recipe: &ComposableEditRecipe,
) -> Result<(), AdmissionError> {
    let Some(read) = state
        .application
        .library
        .composable_edit_recipe_read(&recipe.photo_id)
        .await
        .map_err(|error| AdmissionError::invalid(error.to_string()))?
    else {
        return Err(AdmissionError::invalid(
            "The Photo is not part of the persisted Library",
        ));
    };
    let current_source_revision = read.current_source_revision.as_deref();
    let requires_original = recipe
        .steps
        .iter()
        .any(|step| matches!(step.input, ProcessingInput::Original { .. }));
    if requires_original && current_source_revision.is_none() {
        return Err(AdmissionError::source_changed(
            "the selected Original input's source revision is unavailable",
        ));
    }
    if let Some(current_source_revision) = current_source_revision
        && recipe.source_revision != current_source_revision
    {
        return Err(AdmissionError::source_changed(
            "the expected source revision is no longer current",
        ));
    }
    // Parameter shape is a module contract, not a product-availability fact:
    // an unavailable runtime can still validate a saved caller-owned recipe.
    let registry = ModuleRegistry::new(ModuleAvailability::ready(), ModuleAvailability::ready());
    for step in &recipe.steps {
        let parameters = Parameters {
            module: step.module.as_str().to_owned(),
            version: step.parameters.schema_version.clone(),
            tree: step.parameters.tree.clone(),
        };
        registry
            .validate_saved_parameters(&parameters)
            .map_err(|error| AdmissionError::invalid(error.message))?;
        match &step.input {
            ProcessingInput::Original {
                photo_id,
                source_revision,
            } => {
                if photo_id != &recipe.photo_id
                    || source_revision != &recipe.source_revision
                    || current_source_revision != Some(recipe.source_revision.as_str())
                {
                    return Err(AdmissionError::source_changed(
                        "an Original input is bound to a stale Photo or source revision",
                    ));
                }
                if step.module.as_str() != slipstream_processing::modules::DARKTABLE_MODULE {
                    return Err(AdmissionError::invalid(
                        "standalone SpektraFilm requires an explicitly selected compatible artifact input",
                    ));
                }
            }
            ProcessingInput::Artifact {
                artifact_id,
                contract,
            } => {
                let Some(core_contract) = module_image_contract(contract) else {
                    return Err(AdmissionError::invalid(
                        "artifact image precision is not admitted",
                    ));
                };
                // An artifact binding resolves only against the immutable
                // Processing Artifact store: the legacy Export records are
                // a fixed single-module surface that must never stand in
                // for a selected composable step's artifact.
                let Some(published) = state
                    .application
                    .library
                    .processing_artifact(artifact_id.as_str())
                    .await
                    .map_err(|error| AdmissionError::invalid(error.to_string()))?
                else {
                    return Err(AdmissionError::invalid(
                        "processing artifact is missing or expired",
                    ));
                };
                if &published.output_contract != contract {
                    return Err(AdmissionError::source_changed(
                        "processing artifact image contract is stale",
                    ));
                }
                registry
                    .admit_input_contract(step.module.as_str(), &core_contract)
                    .map_err(|error| AdmissionError::invalid(error.message))?;
            }
        }
    }
    Ok(())
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

pub(crate) fn recipe_json(recipe: &ComposableEditRecipe) -> Value {
    let registry = ModuleRegistry::new(ModuleAvailability::ready(), ModuleAvailability::ready());
    let execution_refusals: Vec<Value> = recipe
        .steps
        .iter()
        .filter_map(|step| {
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

fn outcome_response(outcome: ComposableEditRecipeWriteOutcome) -> Response {
    match outcome {
        ComposableEditRecipeWriteOutcome::Saved(recipe) => (
            StatusCode::CREATED,
            Json(json!({
                "outcome": "saved",
                "recipe": recipe_json(&recipe),
                "recipeVersion": recipe.revision,
                "sourceRevision": recipe.source_revision,
            })),
        )
            .into_response(),
        ComposableEditRecipeWriteOutcome::Replayed(recipe) => (
            StatusCode::OK,
            Json(json!({
                "outcome": "replayed",
                "recipe": recipe_json(&recipe),
                "recipeVersion": recipe.revision,
                "sourceRevision": recipe.source_revision,
            })),
        )
            .into_response(),
        ComposableEditRecipeWriteOutcome::Unchanged(recipe) => (
            StatusCode::OK,
            Json(json!({
                "outcome": "unchanged",
                "recipe": recipe_json(&recipe),
                "recipeVersion": recipe.revision,
                "sourceRevision": recipe.source_revision,
            })),
        )
            .into_response(),
        ComposableEditRecipeWriteOutcome::Conflict(recipe) => error_value(
            StatusCode::CONFLICT,
            "recipe_conflict",
            "The expected composable recipe revision is no longer current",
            recipe.map(|value| recipe_json(&value)),
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
    let Some(read) = (match state
        .application
        .library
        .composable_edit_recipe_read(&photo_id)
        .await
    {
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
        photo_id,
        source_revision: read.current_source_revision.clone().unwrap_or_default(),
        current_source_revision: read.current_source_revision,
        source_available: read.source_available,
        recipe: read.recipe.as_ref().map(recipe_json),
    })
    .into_response()
}

pub(crate) async fn post_composable_edit_recipe(
    State(state): State<HttpState>,
    Path(photo_id): Path<String>,
    Json(body): Json<SaveBody>,
) -> Response {
    let request_id = body.request_id.clone();
    let expected_recipe_revision = body.expected_recipe_revision.clone();
    let expected_source_revision = body.expected_source_revision.clone();
    let recipe = match recipe_from_body(&photo_id, body) {
        Ok(value) => value,
        Err(message) => {
            return error(StatusCode::UNPROCESSABLE_ENTITY, "invalid_recipe", message);
        }
    };
    let mutation = SaveComposableEditRecipe {
        photo_id,
        request_id,
        expected_recipe_revision,
        expected_source_revision,
        recipe,
    };
    match state
        .application
        .library
        .replay_composable_edit_recipe(mutation.clone())
        .await
    {
        Ok(Some(outcome)) => return outcome_response(outcome),
        Ok(None) => {}
        Err(library_error) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage",
                library_error.to_string(),
            );
        }
    }
    if let Err(admission) = validate_step_admission(&state, &mutation.recipe).await {
        return error(admission.status, admission.code, admission.message);
    }
    match state
        .application
        .library
        .save_composable_edit_recipe(mutation)
        .await
    {
        Ok(outcome) => outcome_response(outcome),
        Err(LibraryError::Persistence(storage_error)) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage",
            storage_error.to_string(),
        ),
        Err(library_error) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "processing_recipe",
            library_error.to_string(),
        ),
    }
}

pub(crate) async fn post_composable_edit_recipe_rebind(
    State(state): State<HttpState>,
    Path(photo_id): Path<String>,
    Json(body): Json<RebindBody>,
) -> Response {
    let mutation = RebindComposableEditRecipe {
        photo_id,
        request_id: body.request_id,
        expected_recipe_revision: body.expected_recipe_revision,
        new_source_revision: body.new_source_revision,
    };
    match state
        .application
        .library
        .rebind_composable_edit_recipe(mutation)
        .await
    {
        Ok(outcome) => outcome_response(outcome),
        Err(library_error) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "storage",
            library_error.to_string(),
        ),
    }
}
