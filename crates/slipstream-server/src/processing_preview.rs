//! Bounded Edit Preview execution, with a compatibility route for a selected
//! Processing Step.
//!
//! The route refuses a non-current step and never walks the recipe looking
//! for an implicitly usable predecessor. Each module executes directly at the
//! bounded Preview geometry, and the response carries the complete identity
//! needed to reject stale, superseded, or misidentified bytes.

use axum::{
    body::Body,
    extract::{Path, State},
    http::{Request, StatusCode, header::HeaderMap},
    response::Response,
};
use slipstream_core::{
    ProcessingGeometry, ProcessingImageContract, ProcessingInput, ProcessingModuleId,
    ProcessingParameterSnapshot, ProcessingPreviewIdentity, derivative::DISPLAY_TRANSFORM_VERSION,
};
use slipstream_processing::{
    local_preview::PREVIEW_LONG_EDGE,
    modules::{
        DARKTABLE_MODULE, ModuleAvailability, ModuleRegistry, Parameters, SPEKTRAFILM_MODULE,
    },
};
use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use crate::processing_policy::ProcessingModulePolicy;
use crate::{
    export_manager::ProcessingPreviewExecution,
    http::{
        CLI_CONTRACT_HEADER, HttpState, cli_error_empty, require_cli_contract, require_published,
        valid_id,
    },
};

#[derive(Clone, Copy, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum PreviewComparison {
    #[default]
    Current,
    Baseline,
}

impl PreviewComparison {
    fn as_str(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Baseline => "baseline",
        }
    }
}

fn preview_parameters<'a>(
    step: &'a slipstream_core::ProcessingStep,
    comparison: PreviewComparison,
) -> Result<Cow<'a, ProcessingParameterSnapshot>, String> {
    match comparison {
        PreviewComparison::Current => Ok(Cow::Borrowed(&step.parameters)),
        PreviewComparison::Baseline => {
            let registry =
                ModuleRegistry::new(ModuleAvailability::ready(), ModuleAvailability::ready());
            let description = registry
                .describe(step.module.as_str())
                .map_err(|error| error.message)?;
            let tree = description
                .parameter_schema
                .get("default")
                .filter(|tree| tree.is_object())
                .ok_or_else(|| "The module has no published default parameter tree".to_owned())?
                .clone();
            let parameters = Parameters {
                module: step.module.as_str().to_owned(),
                version: step.parameters.schema_version.clone(),
                tree,
            };
            registry
                .validate_parameters(&parameters)
                .map_err(|error| error.message)?;
            ProcessingParameterSnapshot::new(&parameters.version, parameters.tree)
                .map(Cow::Owned)
                .map_err(|error| error.to_string())
        }
    }
}

type PreviewIntentRegistry = Mutex<HashMap<String, (u64, Arc<AtomicBool>)>>;
static PREVIEW_INTENTS: LazyLock<PreviewIntentRegistry> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static NEXT_PREVIEW_INTENT: AtomicU64 = AtomicU64::new(0);

fn preview_intents() -> &'static PreviewIntentRegistry {
    &PREVIEW_INTENTS
}

fn begin_preview_intent(
    photo_id: &str,
    step_id: &str,
    comparison: PreviewComparison,
) -> (String, u64, Arc<AtomicBool>) {
    let key = format!("{photo_id}\0{step_id}\0{}", comparison.as_str());
    let generation = NEXT_PREVIEW_INTENT.fetch_add(1, Ordering::Relaxed);
    let token = Arc::new(AtomicBool::new(false));
    let mut intents = preview_intents()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some((_, previous)) = intents.insert(key.clone(), (generation, Arc::clone(&token))) {
        previous.store(true, Ordering::Release);
    }
    (key, generation, token)
}

fn current_preview_intent(key: &str, generation: u64, token: &AtomicBool) -> bool {
    !token.load(Ordering::Acquire)
        && preview_intents()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(key)
            .is_some_and(|(current, _)| *current == generation)
}

fn finish_preview_intent(key: &str, generation: u64) {
    let mut intents = preview_intents()
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if intents
        .get(key)
        .is_some_and(|(current, _)| *current == generation)
    {
        intents.remove(key);
    }
}

fn hex_bytes(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn error(status: StatusCode, code: &'static str, message: &'static str) -> Response {
    cli_error_empty(status, code, message)
}

fn preview_failure(current: bool) -> Response {
    error(
        if current {
            StatusCode::SERVICE_UNAVAILABLE
        } else {
            StatusCode::CONFLICT
        },
        if current {
            "processing_unavailable"
        } else {
            "preview_superseded"
        },
        if current {
            "The selected Processing Step Preview could not be rendered"
        } else {
            "A newer Preview intent superseded this rendition"
        },
    )
}

fn output_contract(
    step: &slipstream_core::ProcessingStep,
) -> Result<ProcessingImageContract, String> {
    let module = step.module.as_str();
    let registry = ModuleRegistry::new(ModuleAvailability::ready(), ModuleAvailability::ready());
    let description = registry
        .describe(module)
        .map_err(|error| error.message.clone())?;
    let output = description
        .admitted_outputs
        .first()
        .ok_or_else(|| format!("module `{module}` has no admitted output"))?;
    let precision = if output.precision_bits >= 16 {
        format!("float{}", output.precision_bits)
    } else {
        format!("uint{}", output.precision_bits)
    };
    let geometry = match (&step.input, module) {
        (
            ProcessingInput::Artifact { contract, .. }
            | ProcessingInput::WorkspaceArtifact { contract, .. },
            SPEKTRAFILM_MODULE,
        ) => contract.geometry,
        _ => ProcessingGeometry::new(
            u32::try_from(output.width)
                .map_err(|_| "module output width is too large".to_owned())?,
            u32::try_from(output.height)
                .map_err(|_| "module output height is too large".to_owned())?,
        )
        .map_err(|error| error.to_string())?,
    };
    let encoding = match module {
        DARKTABLE_MODULE => "deflate",
        SPEKTRAFILM_MODULE => "quality-85-baseline",
        _ => return Err(format!("unknown module `{module}`")),
    };
    let contract = ProcessingImageContract {
        format: output.format.clone(),
        precision,
        color_space: output.color_space.clone(),
        transfer: output.transfer_function.clone(),
        geometry,
        encoding: encoding.to_owned(),
    };
    contract.validate().map_err(|error| error.to_string())?;
    Ok(contract)
}

fn preview_identity(
    step: &slipstream_core::ProcessingStep,
    parameters: &ProcessingParameterSnapshot,
    execution: &ProcessingPreviewExecution,
    bundle_id: &str,
) -> Result<ProcessingPreviewIdentity, String> {
    let module =
        ProcessingModuleId::new(step.module.as_str()).map_err(|error| error.to_string())?;
    let registry = ModuleRegistry::new(ModuleAvailability::ready(), ModuleAvailability::ready());
    let description = registry
        .describe(step.module.as_str())
        .map_err(|error| error.message.clone())?;
    let identity = ProcessingPreviewIdentity {
        input: execution.input.clone(),
        module,
        adapter_schema_version: format!(
            "{}:{}",
            description.id.adapter_version, parameters.schema_version
        ),
        parameter_digest: parameters.canonical_digest(),
        output_contract: output_contract(step)?,
        bundle_id: bundle_id.to_owned(),
        geometry: ProcessingGeometry::new(PREVIEW_LONG_EDGE, PREVIEW_LONG_EDGE)
            .map_err(|error| error.to_string())?,
        display_conversion: Some(DISPLAY_TRANSFORM_VERSION.to_owned()),
        invocation_digest: None,
    };
    identity.validate().map_err(|error| error.to_string())?;
    Ok(identity)
}

fn insert_header(headers: &mut HeaderMap, name: &'static str, value: String) {
    headers.insert(name, value.parse().expect("valid Preview identity header"));
}

fn ready_response(
    execution: ProcessingPreviewExecution,
    identity: &ProcessingPreviewIdentity,
    photo_id: &str,
    step_id: &str,
    recipe: &slipstream_core::ComposableEditRecipe,
    bundle_id: &str,
    comparison: PreviewComparison,
) -> Response {
    let mut response = Response::new(Body::from(execution.bytes));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(
        "content-type",
        "image/png".parse().expect("valid content type"),
    );
    headers.insert(
        "cache-control",
        "no-store".parse().expect("valid cache control"),
    );
    insert_header(
        headers,
        "slipstream-processing-preview-comparison",
        comparison.as_str().to_owned(),
    );
    insert_header(
        headers,
        "slipstream-processing-preview-input-sha256",
        identity.input.sha256.clone(),
    );
    insert_header(
        headers,
        "slipstream-processing-preview-input-byte-length",
        identity.input.byte_length.to_string(),
    );
    insert_header(
        headers,
        "slipstream-processing-preview-photo-id",
        photo_id.to_owned(),
    );
    insert_header(
        headers,
        "slipstream-processing-preview-step-id",
        step_id.to_owned(),
    );
    insert_header(
        headers,
        "slipstream-processing-preview-source-revision",
        hex_bytes(&recipe.source_revision),
    );
    insert_header(
        headers,
        "slipstream-processing-preview-recipe-revision",
        recipe.revision.clone(),
    );
    // The public Edit State surface names this guard `editRevision`. Keep the
    // recipe header as a compatibility alias for older clients.
    insert_header(
        headers,
        "slipstream-processing-preview-edit-revision",
        recipe.revision.clone(),
    );
    insert_header(
        headers,
        "slipstream-processing-preview-sha256",
        execution.rendition.sha256.clone(),
    );
    insert_header(
        headers,
        "slipstream-processing-preview-width",
        execution.rendition.width.to_string(),
    );
    insert_header(
        headers,
        "slipstream-processing-preview-height",
        execution.rendition.height.to_string(),
    );
    insert_header(
        headers,
        "slipstream-processing-preview-bundle-id",
        bundle_id.to_owned(),
    );
    insert_header(
        headers,
        "slipstream-processing-preview-geometry",
        PREVIEW_LONG_EDGE.to_string(),
    );
    insert_header(
        headers,
        "slipstream-processing-preview-module",
        identity.module.as_str().to_owned(),
    );
    insert_header(
        headers,
        "slipstream-processing-preview-adapter-schema-version",
        identity.adapter_schema_version.clone(),
    );
    insert_header(
        headers,
        "slipstream-processing-preview-parameter-digest",
        identity.parameter_digest.clone(),
    );
    insert_header(
        headers,
        "slipstream-processing-preview-output-contract",
        identity.output_contract.canonical_digest(),
    );
    insert_header(
        headers,
        "slipstream-processing-preview-display-conversion",
        identity
            .display_conversion
            .as_deref()
            .unwrap_or("scene-referred")
            .to_owned(),
    );
    insert_header(
        headers,
        "slipstream-processing-preview-identity",
        identity.digest(),
    );
    response
}

async fn recipe_still_current(
    state: &HttpState,
    photo_id: &str,
    recipe_revision: &str,
    step_id: &str,
    requires_original: bool,
    source_revision: &str,
) -> bool {
    let Some(current_recipe) = crate::processing_recipe::read_recipe(state, photo_id)
        .await
        .ok()
        .flatten()
        .and_then(|read| read.recipe)
    else {
        return false;
    };
    if current_recipe.revision != recipe_revision
        || current_recipe
            .current_step_id
            .as_ref()
            .map(|id| id.as_str())
            != Some(step_id)
    {
        return false;
    }
    if requires_original {
        return state
            .application
            .library
            .edit_recipe_surface(photo_id)
            .await
            .ok()
            .flatten()
            .and_then(|(_, edit)| edit.current_source_revision)
            .is_some_and(|current| current == source_revision);
    }
    true
}

/// `GET /api/photos/{id}/processing-preview/{step_id}`.
pub(crate) async fn get_processing_preview(
    State(state): State<HttpState>,
    Path((photo_id, step_id)): Path<(String, String)>,
    request: Request<Body>,
) -> Response {
    if request.headers().contains_key(CLI_CONTRACT_HEADER)
        && let Err(response) = require_cli_contract(&request)
    {
        return *response;
    }
    let comparison = match request.uri().query() {
        None | Some("") | Some("comparison=current") => PreviewComparison::Current,
        Some("comparison=baseline") => PreviewComparison::Baseline,
        _ => {
            return error(
                StatusCode::BAD_REQUEST,
                "invalid_comparison",
                "Preview comparison must be current or baseline",
            );
        }
    };
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
    let Some(recipe) = (match crate::processing_recipe::read_recipe(&state, &photo_id).await {
        Ok(read) => read.and_then(|read| read.recipe),
        Err(_) => {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "storage",
                "The current Edit State could not be read",
            );
        }
    }) else {
        return error(
            StatusCode::NOT_FOUND,
            "missing_recipe",
            "Save an Edit State before requesting an Edit Preview",
        );
    };
    if recipe.current_step_id.as_ref().map(|id| id.as_str()) != Some(step_id.as_str()) {
        return error(
            StatusCode::CONFLICT,
            "step_not_current",
            "The Edit Preview is bounded to the current Edit State",
        );
    }
    let Some(step) = recipe
        .steps
        .iter()
        .find(|step| step.step_id.as_str() == step_id)
    else {
        return error(
            StatusCode::NOT_FOUND,
            "unknown_step",
            "The Processing Step is unknown",
        );
    };
    if step.module.as_str() == DARKTABLE_MODULE {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "use_darktable_namespace",
            "Use the darktable namespace to execute darktable state",
        );
    }
    let requires_original = matches!(step.input, ProcessingInput::Original { .. });
    let Some(exports) = state.application.exports.as_ref().cloned() else {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "processing_unavailable",
            "Processing is not configured for this deployment",
        );
    };
    let preview_parameters = match preview_parameters(step, comparison) {
        Ok(parameters) => parameters,
        Err(_) => {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "baseline_unavailable",
                "The module's published default parameters are unavailable",
            );
        }
    };

    let (execution, bundle_id) = match step.module.as_str() {
        SPEKTRAFILM_MODULE => {
            let parameters = Parameters {
                module: step.module.as_str().to_owned(),
                version: preview_parameters.schema_version.clone(),
                tree: preview_parameters.tree.clone(),
            };
            let (intent_key, generation, cancellation) =
                begin_preview_intent(&photo_id, &step_id, comparison);
            let rendered = exports
                .render_film_preview(&step.input, parameters, cancellation.clone())
                .await;
            let current = current_preview_intent(&intent_key, generation, &cancellation);
            finish_preview_intent(&intent_key, generation);
            let execution = match rendered {
                Ok(execution) if current => execution,
                Ok(_) => return preview_failure(false),
                Err(_) => return preview_failure(current),
            };
            let bundle_id = state
                .processing
                .as_ref()
                .map(|processing| {
                    ProcessingModulePolicy::new(processing).bundle_id(SPEKTRAFILM_MODULE)
                })
                .unwrap_or_default();
            (execution, bundle_id)
        }
        _ => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "unknown_module",
                "The current Edit State names an unknown Processing Engine",
            );
        }
    };

    if !recipe_still_current(
        &state,
        &photo_id,
        &recipe.revision,
        &step_id,
        requires_original,
        &recipe.source_revision,
    )
    .await
    {
        return error(
            StatusCode::CONFLICT,
            "preview_stale",
            "The Preview no longer matches the selected Processing Step",
        );
    }
    let identity = match preview_identity(step, &preview_parameters, &execution, &bundle_id) {
        Ok(identity) => identity,
        Err(_) => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_settings",
                "The current Edit State has no valid Edit Preview identity",
            );
        }
    };
    let workspace_id = match &step.input {
        ProcessingInput::WorkspaceArtifact { workspace_id, .. } => Some(workspace_id.clone()),
        _ => state
            .application
            .workspaces
            .lookup_photo(&photo_id)
            .ok()
            .flatten()
            .map(|workspace| workspace.id),
    };
    if let Some(workspace_id) = workspace_id {
        let provenance = serde_json::json!({
            "kind": "preview",
            "handle": {"photoId":photo_id,"stepId":step_id,"recipeRevision":recipe.revision,"identity":identity.digest()},
            "input": crate::processing_export::input_json(&step.input),
            "module": step.module.as_str(), "adapterSchemaVersion":identity.adapter_schema_version,
            "bundleId":bundle_id,"parameterDigest":identity.parameter_digest,
            "sha256":execution.rendition.sha256,"byteLength":execution.bytes.len(),
            "outputContract":{"format":"image/png","precision":"uint8","colorSpace":"srgb","transfer":"srgb","geometry":{"width":execution.rendition.width,"height":execution.rendition.height},"encoding":"deflate"},
            "detail":{"fullResolution":false,"scale":f64::from(execution.rendition.width)/f64::from(identity.output_contract.geometry.width)},
            "inputSha256":identity.input.sha256,"inputByteLength":identity.input.byte_length,
            "comparison":comparison.as_str(),"displayConversion":identity.display_conversion,
        });
        if exports
            .retain_processing_preview(&workspace_id, &execution.bytes, provenance)
            .await
            .is_err()
        {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "output_unavailable",
                "The captured Preview could not be retained in its Workspace",
            );
        }
    }
    ready_response(
        execution, &identity, &photo_id, &step_id, &recipe, &bundle_id, comparison,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paired_intents_stay_current_until_their_own_mode_is_superseded() {
        let (current_key, current_generation, current_token) = begin_preview_intent(
            "paired-intents-test",
            "selected",
            PreviewComparison::Current,
        );
        let (baseline_key, baseline_generation, baseline_token) = begin_preview_intent(
            "paired-intents-test",
            "selected",
            PreviewComparison::Baseline,
        );
        assert!(current_preview_intent(
            &current_key,
            current_generation,
            &current_token
        ));
        assert!(current_preview_intent(
            &baseline_key,
            baseline_generation,
            &baseline_token
        ));

        let (new_key, new_generation, new_token) = begin_preview_intent(
            "paired-intents-test",
            "selected",
            PreviewComparison::Current,
        );
        assert!(!current_preview_intent(
            &current_key,
            current_generation,
            &current_token
        ));
        assert!(current_preview_intent(
            &baseline_key,
            baseline_generation,
            &baseline_token
        ));
        assert!(current_preview_intent(&new_key, new_generation, &new_token));
        finish_preview_intent(&new_key, new_generation);
        finish_preview_intent(&baseline_key, baseline_generation);
    }
}
