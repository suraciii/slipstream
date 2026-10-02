//! Composable Edit Recipe persistence: one guarded recipe record per Photo
//! plus one durable save receipt per request identity, both stored under
//! namespaced `library_metadata` keys and committed only through the
//! serialized persistence owner.
//!
//! A stored recipe is the complete caller-controlled composition: zero or
//! more individually admitted Processing Steps and, whenever steps exist,
//! the caller's selected current step. Steps are stored in canonical
//! `step_id` order so equal recipes serialize to equal bytes; that order is
//! a serialization detail only, never a pipeline order, and steps stay
//! addressed by `step_id`. The fixed single-module recipe in
//! `edit_recipe.rs` is a separate surface that is neither read nor written
//! here.

use super::owner::Command;
use super::{
    DatabaseName, PersistenceError, StateDirectory,
    edit_recipe::read_edit_recipe,
    owner::{random_uuid_v4, write_transaction},
};
use crate::processing::{
    ComposableEditRecipe, ComposableEditRecipeWriteOutcome, ComposableRecipeRequestError,
    MAXIMUM_PARAMETER_SNAPSHOT_BYTES, MAXIMUM_PHOTO_ID_BYTES, MAXIMUM_RECIPE_STEPS,
    MAXIMUM_REVISION_BYTES, ProcessingArtifactId, ProcessingContractError, ProcessingGeometry,
    ProcessingImageContract, ProcessingInput, ProcessingModuleId, ProcessingParameterSnapshot,
    ProcessingStep, ProcessingStepId, SaveComposableEditRecipe, validate_bounded_name,
    validate_revision,
};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;

/// The owner-facing request surface of the composable recipe records: one
/// receiver per command so the central dispatch in `owner.rs` stays the
/// single serialized writer.
impl super::owner::Persistence {
    pub(crate) fn composable_edit_recipe_receiver(
        &self,
        photo_id: &str,
    ) -> Result<
        oneshot::Receiver<Result<Option<ComposableEditRecipe>, PersistenceError>>,
        PersistenceError,
    > {
        let (send, receive) = oneshot::channel();
        self.submit(Command::ReadComposableEditRecipe {
            photo_id: photo_id.to_owned(),
            reply: send,
        })?;
        Ok(receive)
    }

    pub(crate) fn save_composable_edit_recipe_receiver(
        &self,
        mutation: SaveComposableEditRecipe,
    ) -> Result<
        oneshot::Receiver<Result<ComposableEditRecipeWriteOutcome, PersistenceError>>,
        PersistenceError,
    > {
        let (send, receive) = oneshot::channel();
        self.submit(Command::SaveComposableEditRecipe(mutation, send))?;
        Ok(receive)
    }
}

/// The `library_metadata` key namespace of one Photo's saved composable
/// recipe: `composable_edit_recipe:<photo_id>`.
const COMPOSABLE_EDIT_RECIPE_PREFIX: &str = "composable_edit_recipe:";

// Save receipts stay durable at this internal boundary, exactly like the
// fixed recipe receipts: without an explicit expiry contract, deleting a
// receipt could let an old request identity be reused for a different save.
const COMPOSABLE_EDIT_RECIPE_RECEIPT_PREFIX: &str = "composable_edit_recipe_receipt:";

/// Longest admitted byte length of one composable save request identity.
const MAXIMUM_COMPOSABLE_RECIPE_REQUEST_ID_BYTES: usize = 128;

/// Largest admitted serialized byte length of one stored recipe record or
/// save receipt. Every step is already individually bounded by the contract
/// vocabulary, so this bound keeps one metadata value finite without ever
/// clamping an admitted recipe; a value past it is a storage error, never a
/// truncated write.
const MAXIMUM_COMPOSABLE_RECIPE_RECORD_BYTES: usize =
    MAXIMUM_RECIPE_STEPS * MAXIMUM_PARAMETER_SNAPSHOT_BYTES + 65_536;

/// The strict stored shape of one composable recipe. Unknown fields are
/// refused, and every reconstructed value is revalidated before use.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ComposableRecipeRecord {
    photo_id: String,
    revision: String,
    source_revision: String,
    steps: Vec<ComposableStepRecord>,
    current_step_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ComposableStepRecord {
    step_id: String,
    module: String,
    input: ComposableInputRecord,
    parameters: ComposableParametersRecord,
}

/// The stored input binding, domain-separated per variant exactly like
/// [`ProcessingInput`]: an Original and an artifact can never share a
/// stored shape.
#[derive(Clone, Debug, Deserialize, Serialize)]
enum ComposableInputRecord {
    Original(ComposableOriginalRecord),
    Artifact(ComposableArtifactRecord),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ComposableOriginalRecord {
    photo_id: String,
    source_revision: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ComposableArtifactRecord {
    artifact_id: String,
    contract: ComposableContractRecord,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ComposableContractRecord {
    format: String,
    precision: String,
    color_space: String,
    transfer: String,
    width: u32,
    height: u32,
    encoding: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ComposableParametersRecord {
    schema_version: String,
    tree: Value,
}

/// The durable receipt of one committed save request. A replay of the same
/// request identity with the same payload digest returns exactly the
/// committed recipe; the same identity with any other payload is refused.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ComposableRecipeReceipt {
    photo_id: String,
    payload_digest: String,
    outcome: ComposableReceiptOutcome,
    recipe: ComposableRecipeRecord,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
enum ComposableReceiptOutcome {
    Saved,
    Unchanged,
}

/// The digest payload of one save request. It covers every field the caller
/// controls except the recipe revision, which the commit mints, so two
/// equal intents digest equally while any changed step, module, input,
/// parameter, current step, Photo identity, or guard digests differently.
#[derive(Serialize)]
struct SavePayload<'a> {
    kind: &'static str,
    photo_id: &'a str,
    expected_recipe_revision: &'a Option<String>,
    expected_source_revision: &'a str,
    steps: &'a [ComposableStepRecord],
    current_step_id: &'a Option<String>,
}

/// Reads one Photo's saved composable recipe in a single serialized read.
/// `None` means no composable recipe has been saved for the Photo; the fixed
/// recipe surface is not consulted. A record that is malformed, oversized,
/// or no longer admissible is a storage error, never a partially parsed
/// recipe.
pub(super) fn read_composable_edit_recipe(
    connection: &Connection,
    photo_id: &str,
) -> Result<Option<ComposableEditRecipe>, PersistenceError> {
    let value = connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [composable_recipe_key(photo_id)],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    value
        .map(|value| {
            parse_stored_value::<ComposableRecipeRecord>(&value).and_then(parse_recipe_record)
        })
        .transpose()
}

/// Saves one complete composable recipe behind both guards. The whole
/// decision — receipt replay, Photo existence, source availability, the
/// published source revision, the stored recipe revision, and the commit —
/// happens inside one write transaction on the serialized owner, so the
/// source revision a save is guarded against can never come from a
/// different committed read than the stored recipe it is compared to.
pub(super) fn save_composable_edit_recipe(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    mutation: SaveComposableEditRecipe,
) -> Result<ComposableEditRecipeWriteOutcome, PersistenceError> {
    if let Err(error) = validate_save_request(&mutation) {
        return Ok(ComposableEditRecipeWriteOutcome::Invalid(error));
    }
    let submitted = canonical_recipe(&mutation.recipe);
    let payload_digest = save_payload_digest(&mutation, &submitted)?;
    write_transaction(state, database_name, connection, |transaction| {
        if let Some(receipt) = read_composable_recipe_receipt(transaction, &mutation.request_id)? {
            if receipt.photo_id != mutation.photo_id || receipt.payload_digest != payload_digest {
                return Ok(ComposableEditRecipeWriteOutcome::RequestConflict);
            }
            let recipe = parse_recipe_record(receipt.recipe)?;
            return Ok(match receipt.outcome {
                ComposableReceiptOutcome::Saved => {
                    ComposableEditRecipeWriteOutcome::Replayed(recipe)
                }
                ComposableReceiptOutcome::Unchanged => {
                    ComposableEditRecipeWriteOutcome::Unchanged(recipe)
                }
            });
        }
        // One serialized read: the Photo's support facts and the stored
        // composable recipe come from the same committed state, so a scan
        // publication between two reads can never be mixed into one guard
        // decision.
        let Some(facts) = read_edit_recipe(transaction, &mutation.photo_id)? else {
            return Ok(ComposableEditRecipeWriteOutcome::MissingPhoto);
        };
        let stored = read_composable_edit_recipe(transaction, &mutation.photo_id)?;
        if !facts.source_available || facts.current_source_revision.is_none() {
            // No confirmed source facts are bound to the observed Original:
            // either it is unavailable or no Capture fact has published a
            // revision for it, so no guarded save exists.
            return Ok(ComposableEditRecipeWriteOutcome::Unavailable);
        }
        if facts.current_source_revision.as_deref()
            != Some(mutation.expected_source_revision.as_str())
        {
            return Ok(ComposableEditRecipeWriteOutcome::SourceChanged(stored));
        }
        if stored.as_ref().map(|recipe| recipe.revision.as_str())
            != mutation.expected_recipe_revision.as_deref()
        {
            return Ok(ComposableEditRecipeWriteOutcome::Conflict(stored));
        }
        if let Some(committed) = &stored
            && same_committed_content(committed, &submitted)
        {
            write_composable_recipe_receipt(
                transaction,
                &mutation.request_id,
                &ComposableRecipeReceipt {
                    photo_id: mutation.photo_id.clone(),
                    payload_digest,
                    outcome: ComposableReceiptOutcome::Unchanged,
                    recipe: recipe_record(committed),
                },
            )?;
            return Ok(ComposableEditRecipeWriteOutcome::Unchanged(
                committed.clone(),
            ));
        }
        let mut committed = submitted;
        committed.revision = random_uuid_v4()?;
        write_composable_recipe_record(transaction, &mutation.photo_id, &committed)?;
        write_composable_recipe_receipt(
            transaction,
            &mutation.request_id,
            &ComposableRecipeReceipt {
                photo_id: mutation.photo_id.clone(),
                payload_digest,
                outcome: ComposableReceiptOutcome::Saved,
                recipe: recipe_record(&committed),
            },
        )?;
        Ok(ComposableEditRecipeWriteOutcome::Saved(committed))
    })
}

/// Admits one save request: a bounded request identity and Photo identity,
/// bounded expected revisions, a complete valid recipe, one Photo identity
/// shared by the request and the recipe, and a recipe bound to exactly the
/// guarded source revision. A zero-step recipe with no current step is
/// admissible.
fn validate_save_request(
    mutation: &SaveComposableEditRecipe,
) -> Result<(), ComposableRecipeRequestError> {
    let contract = |error: ProcessingContractError| ComposableRecipeRequestError::Contract(error);
    validate_bounded_name(
        &mutation.request_id,
        MAXIMUM_COMPOSABLE_RECIPE_REQUEST_ID_BYTES,
    )
    .map_err(contract)?;
    validate_bounded_name(&mutation.photo_id, MAXIMUM_PHOTO_ID_BYTES).map_err(contract)?;
    validate_revision(&mutation.expected_source_revision).map_err(contract)?;
    if let Some(expected) = &mutation.expected_recipe_revision {
        validate_bounded_name(expected, MAXIMUM_REVISION_BYTES).map_err(contract)?;
    }
    mutation.recipe.validate().map_err(contract)?;
    if mutation.recipe.photo_id != mutation.photo_id {
        return Err(ComposableRecipeRequestError::PhotoMismatch);
    }
    if mutation.recipe.source_revision != mutation.expected_source_revision {
        return Err(ComposableRecipeRequestError::SourceRevisionMismatch);
    }
    Ok(())
}

/// One recipe in canonical form: identical in every field, with the steps
/// ordered by `step_id`. Canonical form is what is digested, stored, and
/// compared, so two callers that compose the same step set in different
/// list orders save one recipe, never two.
fn canonical_recipe(recipe: &ComposableEditRecipe) -> ComposableEditRecipe {
    let mut canonical = recipe.clone();
    canonical
        .steps
        .sort_by(|left, right| left.step_id.as_str().cmp(right.step_id.as_str()));
    canonical
}

/// Whether two recipes carry the same committed content: the same source
/// binding, the same current step, and the same steps. The revision is
/// excluded because the commit owns it.
fn same_committed_content(stored: &ComposableEditRecipe, submitted: &ComposableEditRecipe) -> bool {
    stored.source_revision == submitted.source_revision
        && stored.current_step_id == submitted.current_step_id
        && stored.steps == submitted.steps
}

/// The canonical payload digest of one save request, over the canonical
/// step order.
fn save_payload_digest(
    mutation: &SaveComposableEditRecipe,
    submitted: &ComposableEditRecipe,
) -> Result<String, PersistenceError> {
    let record = recipe_record(submitted);
    let payload = SavePayload {
        kind: "composable-edit-recipe-save-v1",
        photo_id: &mutation.photo_id,
        expected_recipe_revision: &mutation.expected_recipe_revision,
        expected_source_revision: &mutation.expected_source_revision,
        steps: &record.steps,
        current_step_id: &record.current_step_id,
    };
    let bytes = serde_json::to_vec(&payload).map_err(|_| PersistenceError::Storage)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn composable_recipe_key(photo_id: &str) -> String {
    format!("{COMPOSABLE_EDIT_RECIPE_PREFIX}{photo_id}")
}

fn composable_recipe_receipt_key(request_id: &str) -> String {
    format!("{COMPOSABLE_EDIT_RECIPE_RECEIPT_PREFIX}{request_id}")
}

/// Reads and strictly parses one stored receipt. A malformed or oversized
/// receipt is a storage error, never a silent miss: a receipt that cannot be
/// parsed cannot prove a replay.
fn read_composable_recipe_receipt(
    connection: &Connection,
    request_id: &str,
) -> Result<Option<ComposableRecipeReceipt>, PersistenceError> {
    let value = connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [composable_recipe_receipt_key(request_id)],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    value
        .map(|value| {
            let receipt: ComposableRecipeReceipt =
                parse_stored_value::<ComposableRecipeReceipt>(&value)?;
            if receipt.photo_id.is_empty() {
                return Err(PersistenceError::Storage);
            }
            validate_digest_hex(&receipt.payload_digest)?;
            Ok(receipt)
        })
        .transpose()
}

fn write_composable_recipe_record(
    transaction: &Transaction<'_>,
    photo_id: &str,
    recipe: &ComposableEditRecipe,
) -> Result<(), PersistenceError> {
    write_metadata_value(
        transaction,
        &composable_recipe_key(photo_id),
        &serialize_record(&recipe_record(recipe))?,
    )
}

fn write_composable_recipe_receipt(
    transaction: &Transaction<'_>,
    request_id: &str,
    receipt: &ComposableRecipeReceipt,
) -> Result<(), PersistenceError> {
    write_metadata_value(
        transaction,
        &composable_recipe_receipt_key(request_id),
        &serialize_record(receipt)?,
    )
}

fn write_metadata_value(
    transaction: &Transaction<'_>,
    key: &str,
    value: &str,
) -> Result<(), PersistenceError> {
    transaction
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES(?,?)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        )
        .map_err(|_| PersistenceError::Storage)?;
    Ok(())
}

/// Serializes one record within the metadata value bound. A value past the
/// bound is refused as a storage error rather than written oversized or
/// truncated; admitted recipes cannot reach it.
fn serialize_record<T: Serialize>(record: &T) -> Result<String, PersistenceError> {
    let serialized = serde_json::to_string(record).map_err(|_| PersistenceError::Storage)?;
    if serialized.len() > MAXIMUM_COMPOSABLE_RECIPE_RECORD_BYTES {
        return Err(PersistenceError::Storage);
    }
    Ok(serialized)
}

/// Parses one stored metadata value strictly: bounded, unknown fields
/// refused, and the reconstructed record revalidated end to end.
fn parse_stored_value<T: for<'de> Deserialize<'de>>(value: &str) -> Result<T, PersistenceError> {
    if value.len() > MAXIMUM_COMPOSABLE_RECIPE_RECORD_BYTES {
        return Err(PersistenceError::Storage);
    }
    serde_json::from_str(value).map_err(|_| PersistenceError::Storage)
}

fn validate_digest_hex(value: &str) -> Result<(), PersistenceError> {
    crate::processing::validate_digest(value).map_err(|_| PersistenceError::Storage)
}

/// The stored shape of one recipe: a complete copy in canonical `step_id`
/// order, so equal step sets serialize to equal bytes whatever list order
/// the caller composed them in.
fn recipe_record(recipe: &ComposableEditRecipe) -> ComposableRecipeRecord {
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
    }
}

/// Rebuilds one recipe from its stored shape. Every identity is
/// reconstructed through its admitting constructor and the whole recipe is
/// revalidated, so a record that no longer passes the contract vocabulary is
/// a storage error, never a silently degraded recipe.
fn parse_recipe_record(
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

#[cfg(test)]
#[path = "composable_recipe_tests.rs"]
mod tests;
