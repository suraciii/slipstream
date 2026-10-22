//! Canonical retained recipe encoding and strict reconstruction.
use super::*;

/// Serializes one recipe record within the composable-recipe metadata value
/// bound. A value past the bound is refused as a storage error rather than
/// written oversized or truncated; admitted recipes cannot reach it.
pub(super) fn serialize_record<T: Serialize>(record: &T) -> Result<String, PersistenceError> {
    crate::persistence::retained_record::serialize_record_within_bound(
        record,
        MAXIMUM_COMPOSABLE_RECIPE_RECORD_BYTES,
    )
}

/// Parses one stored metadata value strictly within the composable-recipe
/// bound: bounded, unknown fields refused, and the reconstructed record
/// revalidated end to end.
pub(super) fn parse_stored_value<T: for<'de> Deserialize<'de>>(
    value: &str,
) -> Result<T, PersistenceError> {
    crate::persistence::retained_record::parse_stored_value_within_bound(
        value,
        MAXIMUM_COMPOSABLE_RECIPE_RECORD_BYTES,
    )
}

pub(super) fn validate_digest_hex(value: &str) -> Result<(), PersistenceError> {
    crate::processing::validate_digest(value).map_err(|_| PersistenceError::Storage)
}

/// Encodes a validated recipe using the existing canonical retained record format.
pub fn processing_recipe_encode(recipe: &ComposableEditRecipe) -> Result<Value, PersistenceError> {
    recipe.validate().map_err(|_| PersistenceError::Storage)?;
    serde_json::to_value(recipe_record(recipe)).map_err(|_| PersistenceError::Storage)
}

/// Decodes the bounded strict retained record format and revalidates its recipe.
pub fn processing_recipe_decode(snapshot: Value) -> Result<ComposableEditRecipe, PersistenceError> {
    let snapshot = serde_json::to_string(&snapshot).map_err(|_| PersistenceError::Storage)?;
    parse_stored_value(&snapshot).and_then(parse_recipe_record)
}

/// The stored shape of one recipe: a complete copy in canonical `step_id`
/// order, so equal step sets serialize to equal bytes whatever list order
/// the caller composed them in.
pub(super) fn recipe_record(recipe: &ComposableEditRecipe) -> ComposableRecipeRecord {
    let mut steps: Vec<ComposableStepRecord> = recipe.steps.iter().map(step_record).collect();
    steps.sort_by(|left, right| left.step_id.cmp(&right.step_id));
    ComposableRecipeRecord {
        photo_id: recipe.photo_id.clone(),
        revision: recipe.revision.clone(),
        source_revision: recipe.source_revision.clone(),
        current_step_id: recipe
            .current_step_id
            .as_ref()
            .map(|step_id| step_id.as_str().to_owned()),
        steps,
    }
}

fn step_record(step: &ProcessingStep) -> ComposableStepRecord {
    ComposableStepRecord {
        step_id: step.step_id.as_str().to_owned(),
        module: step.module.as_str().to_owned(),
        input: input_record(&step.input),
        parameters: ComposableParametersRecord {
            schema_version: step.parameters.schema_version.clone(),
            tree: step.parameters.tree.clone(),
        },
    }
}

fn input_record(input: &ProcessingInput) -> ComposableInputRecord {
    match input {
        ProcessingInput::Original {
            photo_id,
            source_revision,
        } => ComposableInputRecord::Original(ComposableOriginalRecord {
            photo_id: photo_id.clone(),
            source_revision: source_revision.clone(),
        }),
        ProcessingInput::Artifact {
            artifact_id,
            contract,
        } => ComposableInputRecord::Artifact(ComposableArtifactRecord {
            artifact_id: artifact_id.as_str().to_owned(),
            contract: ComposableContractRecord {
                format: contract.format.clone(),
                precision: contract.precision.clone(),
                color_space: contract.color_space.clone(),
                transfer: contract.transfer.clone(),
                width: contract.geometry.width,
                height: contract.geometry.height,
                encoding: contract.encoding.clone(),
            },
        }),
        ProcessingInput::WorkspaceArtifact {
            workspace_id,
            artifact_id,
            revision,
            contract,
        } => ComposableInputRecord::WorkspaceArtifact(ComposableWorkspaceArtifactRecord {
            workspace_id: workspace_id.clone(),
            artifact_id: artifact_id.clone(),
            revision: revision.clone(),
            contract: ComposableContractRecord {
                format: contract.format.clone(),
                precision: contract.precision.clone(),
                color_space: contract.color_space.clone(),
                transfer: contract.transfer.clone(),
                width: contract.geometry.width,
                height: contract.geometry.height,
                encoding: contract.encoding.clone(),
            },
        }),
    }
}

/// Rebuilds one recipe from its stored shape. Every identity is
/// reconstructed through its admitting constructor and the whole recipe is
/// revalidated, so a record that no longer passes the contract vocabulary is
/// a storage error, never a silently degraded recipe.
pub(super) fn parse_recipe_record(
    record: ComposableRecipeRecord,
) -> Result<ComposableEditRecipe, PersistenceError> {
    let storage = |_| PersistenceError::Storage;
    let mut steps = Vec::with_capacity(record.steps.len());
    for step in record.steps {
        let input = match step.input {
            ComposableInputRecord::Original(original) => ProcessingInput::Original {
                photo_id: original.photo_id,
                source_revision: original.source_revision,
            },
            ComposableInputRecord::Artifact(artifact) => ProcessingInput::Artifact {
                artifact_id: ProcessingArtifactId::new(&artifact.artifact_id).map_err(storage)?,
                contract: ProcessingImageContract {
                    format: artifact.contract.format,
                    precision: artifact.contract.precision,
                    color_space: artifact.contract.color_space,
                    transfer: artifact.contract.transfer,
                    geometry: ProcessingGeometry::new(
                        artifact.contract.width,
                        artifact.contract.height,
                    )
                    .map_err(storage)?,
                    encoding: artifact.contract.encoding,
                },
            },
            ComposableInputRecord::WorkspaceArtifact(artifact) => {
                ProcessingInput::WorkspaceArtifact {
                    workspace_id: artifact.workspace_id,
                    artifact_id: artifact.artifact_id,
                    revision: artifact.revision,
                    contract: ProcessingImageContract {
                        format: artifact.contract.format,
                        precision: artifact.contract.precision,
                        color_space: artifact.contract.color_space,
                        transfer: artifact.contract.transfer,
                        geometry: ProcessingGeometry::new(
                            artifact.contract.width,
                            artifact.contract.height,
                        )
                        .map_err(storage)?,
                        encoding: artifact.contract.encoding,
                    },
                }
            }
        };
        steps.push(ProcessingStep {
            step_id: ProcessingStepId::new(&step.step_id).map_err(storage)?,
            module: ProcessingModuleId::new(&step.module).map_err(storage)?,
            input,
            parameters: ProcessingParameterSnapshot::new(
                &step.parameters.schema_version,
                step.parameters.tree,
            )
            .map_err(storage)?,
        });
    }
    let recipe = ComposableEditRecipe {
        photo_id: record.photo_id,
        revision: record.revision,
        source_revision: record.source_revision,
        current_step_id: match record.current_step_id {
            Some(step_id) => Some(ProcessingStepId::new(&step_id).map_err(storage)?),
            None => None,
        },
        steps,
    };
    recipe.validate().map_err(storage)?;
    Ok(recipe)
}
/// The canonical payload digest of one save request, over the canonical
/// step order. Stateful facade requests use their stable command intent
/// instead of a recipe reconstructed from mutable state.
pub(super) fn save_payload_digest(
    mutation: &SaveComposableEditRecipe,
    submitted: &ComposableEditRecipe,
) -> Result<String, PersistenceError> {
    if let Some(intent) = mutation.request_intent.as_ref() {
        let payload = StatefulSavePayload {
            kind: "stateful-edit-save-v1",
            photo_id: &mutation.photo_id,
            intent,
        };
        let bytes = serde_json::to_vec(&payload).map_err(|_| PersistenceError::Storage)?;
        return Ok(format!("{:x}", Sha256::digest(bytes)));
    }
    let record = recipe_record(submitted);
    let digest_record = mutation
        .automatic_adjustment
        .as_ref()
        .and_then(|value| value.original_recipe.as_deref())
        .map(recipe_record)
        .unwrap_or_else(|| record.clone());
    let automatic_adjustment =
        mutation
            .automatic_adjustment
            .as_ref()
            .map(|value| SaveAutomaticAdjustment {
                step_id: value.step_id.as_str(),
                operation: &value.operation,
                multi_priority: value.multi_priority,
                instruction: &value.instruction,
            });
    let payload = SavePayload {
        kind: "composable-edit-recipe-save-v1",
        photo_id: &mutation.photo_id,
        expected_recipe_revision: &mutation.expected_recipe_revision,
        expected_source_revision: &mutation.expected_source_revision,
        steps: &digest_record.steps,
        current_step_id: &digest_record.current_step_id,
        automatic_adjustment,
    };
    let bytes = serde_json::to_vec(&payload).map_err(|_| PersistenceError::Storage)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}
