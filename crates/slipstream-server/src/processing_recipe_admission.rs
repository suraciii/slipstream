use super::*;
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
pub(crate) struct AdmissionError {
    pub(crate) code: &'static str,
    pub(crate) status: StatusCode,
    pub(crate) message: String,
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

pub(crate) async fn validate_step_admission(
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
    if requires_original && (!read.source_available || current_source_revision.is_none()) {
        return Err(AdmissionError::source_changed(
            "the selected Original input's source revision is unavailable",
        ));
    }
    if requires_original
        && let Some(current_source_revision) = current_source_revision
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
            ProcessingInput::WorkspaceArtifact {
                workspace_id,
                artifact_id,
                revision,
                contract,
            } => {
                let admitted = state
                    .application
                    .workspaces
                    .admit_input(workspace_id, artifact_id, revision)
                    .map_err(|e| AdmissionError::invalid(e.to_string()))?;
                if admitted["workspaceId"] != *workspace_id
                    || admitted["artifactId"] != *artifact_id
                    || admitted["revision"] != *revision
                    || admitted["imageContract"] != contract_json(contract)
                {
                    return Err(AdmissionError::source_changed(
                        "The selected workspace image revision or actual contract changed",
                    ));
                }
                let actual = module_image_contract(contract).ok_or_else(|| {
                    AdmissionError::invalid("Workspace image precision is not admitted")
                })?;
                registry
                    .admit_input_contract(step.module.as_str(), &actual)
                    .map_err(|e| AdmissionError::invalid(e.message))?;
            }
        }
    }
    Ok(())
}
