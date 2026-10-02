//! Composable Export persistence: one immutable Processing Artifact record
//! per artifact identity, one durable explicit-refusal record per export
//! request identity, one durable work record per accepted request, and the
//! retention, lease, and expiry-tombstone rows that keep the surface
//! finite — all stored under namespaced `library_metadata` keys and
//! committed only through the serialized persistence owner.
//!
//! An artifact record is the published, validated Export result a later
//! step may select through an explicit artifact input binding; publication
//! is insert-only, so one artifact identity resolves to exactly one record
//! forever. A refusal record is the durable decision that one selected
//! current step was admitted for explicit Export and refused by the
//! deployment's adapter boundary before any artifact was published: a
//! replay of the same request identity returns the committed refusal, and
//! the same identity with any other payload is refused.
//!
//! A work record is the accepted receipt a qualified submission commits in
//! the same transaction that admits it: nothing may execute without one, a
//! duplicate live request replays `Pending`, the first terminal decision
//! wins, and a settlement publishes its artifact, records success, and
//! installs the artifact's finite retention atomically. Terminally failed
//! and cancelled records keep their replay answer for their own finite
//! receipt window. The sweep reclaims stale download leases, removes
//! expired artifacts with their retention and terminal work, removes
//! failed and cancelled receipts past their window, and tombstones their
//! request identities so they can never start new work. The legacy Export
//! tables in `export.rs` are a separate fixed surface that is neither read
//! nor written here.

use super::{
    DatabaseName, PersistenceError, StateDirectory,
    composable_recipe::read_composable_edit_recipe,
    edit_recipe::read_edit_recipe,
    owner::{random_uuid_v4, write_transaction},
};
use crate::processing::{
    MAXIMUM_EXPORT_REQUEST_ID_BYTES, MAXIMUM_LIVE_PROCESSING_EXPORTS,
    MAXIMUM_PARAMETER_SNAPSHOT_BYTES, MAXIMUM_PHOTO_ID_BYTES, MAXIMUM_REVISION_BYTES,
    PROCESSING_ARTIFACT_RETENTION_SECONDS, PROCESSING_EXPORT_RECEIPT_RETENTION_SECONDS,
    ProcessingArtifact, ProcessingArtifactId, ProcessingArtifactLeaseOutcome,
    ProcessingArtifactPublication, ProcessingExportAdapterDecision, ProcessingExportAdmission,
    ProcessingExportAttempt, ProcessingExportAttemptOutcome, ProcessingExportCancelOutcome,
    ProcessingExportFailureOutcome, ProcessingExportRefusal, ProcessingExportRequestError,
    ProcessingExportSettlement, ProcessingExportSubmitOutcome, ProcessingExportWork,
    ProcessingExportWorkState, ProcessingGeometry, ProcessingImageContract, ProcessingInput,
    ProcessingInputEvidence, ProcessingInputHandoffError, ProcessingModuleId,
    ProcessingParameterSnapshot, ProcessingStepId, SubmitProcessingExport, validate_bounded_name,
    validate_source_revision,
};
use crate::processing::{ProcessingExportList, ReplayProcessingExport, RetryProcessingExport};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;

/// The `library_metadata` key namespace of one published immutable
/// Processing Artifact: `processing_artifact:<artifact_id>`.
const PROCESSING_ARTIFACT_PREFIX: &str = "processing_artifact:";

/// The `library_metadata` key namespace of one settled qualified composable
/// Export receipt: `processing_export_acceptance:<request_id>`. Insert-only
/// exactly like the refusal receipt, so one request identity resolves to
/// exactly one settled outcome forever.
const PROCESSING_EXPORT_ACCEPTANCE_PREFIX: &str = "processing_export_acceptance:";

/// The `library_metadata` key namespace of one recorded explicit composable
/// Export refusal: `processing_export_refusal:<request_id>`.
const PROCESSING_EXPORT_REFUSAL_PREFIX: &str = "processing_export_refusal:";

/// The `library_metadata` key namespace of one durable composable Export
/// work record: `processing_export_work:<request_id>`. The record is written
/// as `accepted` in the same transaction that admits a qualified submission,
/// carries the begun attempt and the first terminal decision, and is removed
/// only together with its expired settled artifact.
const PROCESSING_EXPORT_WORK_PREFIX: &str = "processing_export_work:";

/// The `library_metadata` key namespace of one published Processing
/// Artifact's finite retention: `processing_artifact_retention:<artifact_id>`.
const PROCESSING_ARTIFACT_RETENTION_PREFIX: &str = "processing_artifact_retention:";

/// The `library_metadata` key namespace of one Processing Artifact download
/// lease: `processing_artifact_lease:<lease_id>`.
const PROCESSING_ARTIFACT_LEASE_PREFIX: &str = "processing_artifact_lease:";

/// The `library_metadata` key namespace of one expired composable Export
/// request identity: `processing_export_expired:<request_id>`. The tombstone
/// is written only when a settled artifact's retention expired, and it keeps
/// the request from ever starting new work.
const PROCESSING_EXPORT_EXPIRED_PREFIX: &str = "processing_export_expired:";

/// A download lease whose liveness anchor is older than this window is crash
/// debris: the sweep reclaims it, and a stream still running must have kept
/// renewing its lease to hold the artifact against expiry cleanup.
const PROCESSING_ARTIFACT_LEASE_STALE_SECONDS: u64 = 24 * 60 * 60;

/// Largest admitted serialized byte length of one stored artifact record or
/// refusal record. Every component is already individually bounded by the
/// contract vocabulary, so this bound keeps one metadata value finite
/// without ever clamping an admitted record; a value past it is a storage
/// error, never a truncated write.
const MAXIMUM_PROCESSING_EXPORT_RECORD_BYTES: usize = MAXIMUM_PARAMETER_SNAPSHOT_BYTES
    + 2 * crate::processing::MAXIMUM_SOURCE_REVISION_BYTES * 6
    + 65_536;

/// The strict stored shape of one published Processing Artifact. Unknown
/// fields are refused, and every reconstructed value is revalidated before
/// use.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProcessingArtifactRecord {
    artifact_id: String,
    photo_id: String,
    step_id: String,
    module: String,
    adapter_schema_version: String,
    parameters: SnapshotRecord,
    input: EvidenceRecord,
    output_contract: ContractRecord,
    bundle_id: String,
    sha256: String,
    byte_length: u64,
}

/// The stored input evidence, domain-separated per binding variant exactly
/// like [`ProcessingInputEvidence`]: an Original and an artifact can never
/// share a stored shape.
#[derive(Clone, Debug, Deserialize, Serialize)]
enum EvidenceRecord {
    Original(OriginalEvidenceRecord),
    Artifact(ArtifactEvidenceRecord),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OriginalEvidenceRecord {
    photo_id: String,
    source_revision: String,
    sha256: String,
    byte_length: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ArtifactEvidenceRecord {
    artifact_id: String,
    contract: ContractRecord,
    sha256: String,
    byte_length: u64,
}

/// The stored input binding of one refusal record, domain-separated per
/// variant exactly like [`ProcessingInput`].
#[derive(Clone, Debug, Deserialize, Serialize)]
enum InputRecord {
    Original(OriginalRecord),
    Artifact(ArtifactRecord),
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OriginalRecord {
    photo_id: String,
    source_revision: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ArtifactRecord {
    artifact_id: String,
    contract: ContractRecord,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ContractRecord {
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
struct SnapshotRecord {
    schema_version: String,
    tree: Value,
}

/// The stored shape of one recorded explicit composable Export refusal.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProcessingExportRefusalRecord {
    photo_id: String,
    request_id: String,
    payload_digest: String,
    step_id: String,
    recipe_revision: String,
    source_revision: String,
    module: String,
    parameter_schema_version: String,
    parameter_digest: String,
    input: InputRecord,
    bundle_id: String,
    reason_code: String,
}

/// The digest payload of one composable Export submission. It covers every
/// field the caller controls plus the adapter decision's identity — the
/// refusal reason code, or the qualified adapter and parameter-schema
/// versions — so two equal intents digest equally while any changed step,
/// guard, or decision digests differently. Deployment state such as the
/// processing bundle is deliberately excluded, so a bundle change cannot
/// break a replay.
#[derive(Serialize)]
struct SubmitPayload<'a> {
    kind: &'static str,
    photo_id: &'a str,
    step_id: &'a str,
    expected_recipe_revision: &'a str,
    expected_source_revision: &'a str,
    decision: &'a str,
}

/// The stored receipt of one settled qualified composable Export: the
/// request identity, the payload digest it settled, and the immutable
/// artifact identity that execution published. Insert-only, exactly like
/// the refusal record it mirrors.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProcessingExportAcceptanceRecord {
    photo_id: String,
    request_id: String,
    payload_digest: String,
    artifact_id: String,
}

/// The stored shape of one durable composable Export work record: the
/// accepted admission plus its lifecycle state, begun attempt, and first
/// terminal decision. Unknown fields are refused, and every reconstructed
/// value is revalidated before use.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProcessingExportWorkRecord {
    photo_id: String,
    request_id: String,
    payload_digest: String,
    step_id: String,
    recipe_revision: String,
    source_revision: String,
    module: String,
    adapter_schema_version: String,
    parameters: SnapshotRecord,
    input: InputRecord,
    bundle_id: String,
    state: String,
    accepted_at: u64,
    attempt: Option<AttemptRecord>,
    artifact_id: Option<String>,
    failure_reason: Option<String>,
    terminal_at: Option<u64>,
    retain_until: Option<u64>,
}

/// The stored shape of one durably begun execution attempt.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AttemptRecord {
    sequence: u64,
    began_at: u64,
}

/// The stored shape of one published Processing Artifact's finite retention:
/// the deadline and every request identity whose settlement published the
/// artifact, so the expiry sweep can remove exactly those terminal work
/// records and tombstone their requests.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProcessingArtifactRetentionRecord {
    artifact_id: String,
    retain_until: u64,
    requests: Vec<String>,
}

/// The stored shape of one Processing Artifact download lease: its renewal
/// anchor holds the artifact against expiry cleanup until the lease is
/// released or goes stale.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProcessingArtifactLeaseRecord {
    lease_id: String,
    artifact_id: String,
    created_at: u64,
}

/// Reads one published Processing Artifact in a single serialized read.
/// `None` means no artifact was ever published under the identity. A record
/// that is malformed, oversized, or no longer admissible is a storage
/// error, never a partially parsed artifact.
pub(super) fn read_processing_artifact(
    connection: &Connection,
    artifact_id: &str,
) -> Result<Option<ProcessingArtifact>, PersistenceError> {
    let value = connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [processing_artifact_key(artifact_id)],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    value
        .map(|value| {
            parse_stored_value::<ProcessingArtifactRecord>(&value).and_then(parse_artifact_record)
        })
        .transpose()
}

/// Publishes one validated Processing Artifact insert-only. The whole
/// decision — the identity's emptiness, the equality of any existing
/// record, and the insert — happens inside one write transaction on the
/// serialized owner, so a concurrent publication of the same identity can
/// never be mixed into one immutability decision.
pub(super) fn publish_processing_artifact(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    artifact: ProcessingArtifact,
) -> Result<ProcessingArtifactPublication, PersistenceError> {
    artifact.validate().map_err(|_| PersistenceError::Storage)?;
    let record = artifact_record(&artifact);
    write_transaction(state, database_name, connection, |transaction| {
        let existing = read_processing_artifact(transaction, artifact.artifact_id.as_str())?;
        if let Some(published) = existing {
            return Ok(if published.same_publication(&artifact) {
                ProcessingArtifactPublication::Replayed
            } else {
                ProcessingArtifactPublication::IdentityConflict
            });
        }
        write_metadata_value(
            transaction,
            &processing_artifact_key(artifact.artifact_id.as_str()),
            &serialize_record(&record)?,
        )?;
        Ok(ProcessingArtifactPublication::Published)
    })
}

/// Submits one selected current Processing Step for explicit Export behind
/// both guards. The whole decision — expiry tombstones, decision replays,
/// live-work deduplication and reservation, Photo existence, source
/// availability, the published source revision, the stored recipe revision
/// and current step, the concrete input handoff check, and the commit —
/// happens inside one write transaction on the serialized owner, so the
/// recipe a decision is captured against can never come from a different
/// committed read than the source revision it is guarded by. A qualified
/// admission commits its accepted work record — the receipt execution must
/// present before any artifact is published — in this same transaction, so
/// a duplicate live request resolves to `Pending` and never starts a second
/// execution.
pub(super) fn submit_processing_export(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    mutation: SubmitProcessingExport,
    now: u64,
) -> Result<ProcessingExportSubmitOutcome, PersistenceError> {
    if let Err(error) = validate_submit_request(&mutation) {
        return Ok(ProcessingExportSubmitOutcome::Invalid(error));
    }
    let decision = decision_identity(&mutation.adapter);
    let payload_digest = submit_payload_digest(&mutation, &decision)?;
    write_transaction(state, database_name, connection, |transaction| {
        if processing_export_expired(transaction, &mutation.request_id)? {
            // The request once settled an artifact whose finite retention
            // expired: the identity can never start new work, and no newer
            // result resolves in its place.
            return Ok(ProcessingExportSubmitOutcome::Expired);
        }
        if let Some(record) = read_processing_refusal_record(transaction, &mutation.request_id)? {
            if record.photo_id != mutation.photo_id
                || record.payload_digest != payload_digest
                || record.request_id != mutation.request_id
            {
                return Ok(ProcessingExportSubmitOutcome::RequestConflict);
            }
            return Ok(ProcessingExportSubmitOutcome::Replayed(
                parse_refusal_record(record)?,
            ));
        }
        if let Some(record) = read_processing_acceptance_record(transaction, &mutation.request_id)?
        {
            if record.photo_id != mutation.photo_id
                || record.payload_digest != payload_digest
                || record.request_id != mutation.request_id
            {
                return Ok(ProcessingExportSubmitOutcome::RequestConflict);
            }
            let artifact = read_processing_artifact(transaction, &record.artifact_id)?
                .ok_or(PersistenceError::Storage)?;
            return Ok(ProcessingExportSubmitOutcome::ArtifactReplayed(artifact));
        }
        if let Some(record) = read_processing_export_work_record(transaction, &mutation.request_id)?
        {
            if record.photo_id != mutation.photo_id
                || record.payload_digest != payload_digest
                || record.request_id != mutation.request_id
            {
                return Ok(ProcessingExportSubmitOutcome::RequestConflict);
            }
            let work = parse_work_record(record)?;
            return Ok(match work.state {
                ProcessingExportWorkState::Accepted | ProcessingExportWorkState::Executing => {
                    ProcessingExportSubmitOutcome::Pending(work.admission)
                }
                ProcessingExportWorkState::Failed => {
                    ProcessingExportSubmitOutcome::FailureReplayed(work)
                }
                ProcessingExportWorkState::Cancelled => {
                    ProcessingExportSubmitOutcome::CancelledReplayed(work)
                }
                // A succeeded work record always carries its acceptance
                // receipt, which the replay above already resolved; a
                // stored pair without one is corrupt, never an export.
                ProcessingExportWorkState::Succeeded => return Err(PersistenceError::Storage),
            });
        }
        // Every replay decision precedes the reservation, so a recorded
        // decision always resolves even when the live-work reservation is
        // full.
        if live_processing_export_count(transaction)? >= MAXIMUM_LIVE_PROCESSING_EXPORTS {
            return Ok(ProcessingExportSubmitOutcome::ReservationFull);
        }
        // One serialized read: the Photo's support facts, the stored
        // composable recipe, and every artifact consulted by the input
        // handoff below come from the same committed state, so no scan
        // publication or concurrent publication can change them
        // mid-submission.
        let Some(facts) = read_edit_recipe(transaction, &mutation.photo_id)? else {
            return Ok(ProcessingExportSubmitOutcome::MissingPhoto);
        };
        let Some(stored) = read_composable_edit_recipe(transaction, &mutation.photo_id)? else {
            return Ok(ProcessingExportSubmitOutcome::MissingRecipe);
        };
        if stored.source_revision != mutation.expected_source_revision {
            return Ok(ProcessingExportSubmitOutcome::SourceChanged(Some(stored)));
        }
        if let Some(current_source_revision) = facts.current_source_revision.as_deref()
            && current_source_revision != mutation.expected_source_revision
        {
            return Ok(ProcessingExportSubmitOutcome::SourceChanged(Some(stored)));
        }
        if stored.revision != mutation.expected_recipe_revision {
            return Ok(ProcessingExportSubmitOutcome::RecipeConflict(Some(stored)));
        }
        if stored.current_step_id.as_ref() != Some(&mutation.step_id) {
            return Ok(ProcessingExportSubmitOutcome::StepNotCurrent(
                stored.current_step_id.clone(),
            ));
        }
        let Some(step) = stored
            .steps
            .iter()
            .find(|step| step.step_id == mutation.step_id)
        else {
            // A validated recipe's current step is always one of its steps;
            // a stored record where it is not is corrupt, never an export.
            return Err(PersistenceError::Storage);
        };
        match &step.input {
            ProcessingInput::Original {
                photo_id,
                source_revision,
            } => {
                if !facts.source_available || facts.current_source_revision.is_none() {
                    return Ok(ProcessingExportSubmitOutcome::Unavailable);
                }
                if photo_id != &mutation.photo_id {
                    return Ok(ProcessingExportSubmitOutcome::IncompatibleInput(
                        ProcessingInputHandoffError::OriginalPhotoMismatch,
                    ));
                }
                if source_revision != &mutation.expected_source_revision {
                    return Ok(ProcessingExportSubmitOutcome::IncompatibleInput(
                        ProcessingInputHandoffError::OriginalSourceStale,
                    ));
                }
            }
            ProcessingInput::Artifact {
                artifact_id,
                contract,
            } => {
                let Some(published) = read_processing_artifact(transaction, artifact_id.as_str())?
                else {
                    return Ok(ProcessingExportSubmitOutcome::IncompatibleInput(
                        ProcessingInputHandoffError::ArtifactMissing,
                    ));
                };
                if read_processing_artifact_retention_record(transaction, artifact_id.as_str())?
                    .is_some_and(|retention| now >= retention.retain_until)
                {
                    return Ok(ProcessingExportSubmitOutcome::IncompatibleInput(
                        ProcessingInputHandoffError::ArtifactMissing,
                    ));
                }
                if &published.output_contract != contract {
                    return Ok(ProcessingExportSubmitOutcome::IncompatibleInput(
                        ProcessingInputHandoffError::ArtifactContractMismatch,
                    ));
                }
            }
        }
        match &mutation.adapter {
            ProcessingExportAdapterDecision::NoQualifiedAdapter { reason_code } => {
                let record = ProcessingExportRefusalRecord {
                    photo_id: mutation.photo_id.clone(),
                    request_id: mutation.request_id.clone(),
                    payload_digest,
                    step_id: mutation.step_id.as_str().to_owned(),
                    recipe_revision: stored.revision.clone(),
                    source_revision: stored.source_revision.clone(),
                    module: step.module.as_str().to_owned(),
                    parameter_schema_version: step.parameters.schema_version.clone(),
                    parameter_digest: step.parameters.canonical_digest(),
                    input: input_record(&step.input),
                    bundle_id: mutation.bundle_id.clone(),
                    reason_code: reason_code.clone(),
                };
                write_metadata_value(
                    transaction,
                    &processing_export_refusal_key(&mutation.request_id),
                    &serialize_record(&record)?,
                )?;
                Ok(ProcessingExportSubmitOutcome::Refused(
                    parse_refusal_record(record)?,
                ))
            }
            ProcessingExportAdapterDecision::Qualified {
                adapter_version,
                parameter_schema_version,
            } => {
                let reserved = reserved_processing_output_bytes(transaction)?;
                if reserved.saturating_add(crate::MAXIMUM_EXPORT_BYTES)
                    > mutation.retained_output_bytes_max
                {
                    return Ok(ProcessingExportSubmitOutcome::RetainedOutputFull);
                }
                if parameter_schema_version != &step.parameters.schema_version {
                    // The qualified decision must name the exact stored
                    // parameter-schema version; anything else is an
                    // admission contract violation, never a silent
                    // execution under another schema.
                    return Err(PersistenceError::Storage);
                }
                let admission = ProcessingExportAdmission {
                    photo_id: mutation.photo_id.clone(),
                    request_id: mutation.request_id.clone(),
                    payload_digest,
                    step_id: mutation.step_id.clone(),
                    recipe_revision: stored.revision.clone(),
                    source_revision: stored.source_revision.clone(),
                    module: step.module.clone(),
                    adapter_schema_version: format!("{adapter_version}:{parameter_schema_version}"),
                    parameters: step.parameters.clone(),
                    input: step.input.clone(),
                    bundle_id: mutation.bundle_id.clone(),
                };
                admission
                    .validate()
                    .map_err(|_| PersistenceError::Storage)?;
                // The accepted work record is the receipt execution must
                // present before any artifact is published: it is committed
                // in this same transaction, so a lost response can never
                // admit a second execution under the same request identity.
                let work = ProcessingExportWork {
                    admission: admission.clone(),
                    state: ProcessingExportWorkState::Accepted,
                    accepted_at: now,
                    attempt: None,
                    artifact_id: None,
                    failure_reason: None,
                    terminal_at: None,
                    retain_until: None,
                };
                write_metadata_value(
                    transaction,
                    &processing_export_work_key(&mutation.request_id),
                    &serialize_record(&work_record(&work))?,
                )?;
                Ok(ProcessingExportSubmitOutcome::Admitted(admission))
            }
        }
    })
}

/// The digest identity of one adapter decision: the refusal reason code,
/// or the qualified adapter and parameter-schema versions.
fn decision_identity(decision: &ProcessingExportAdapterDecision) -> String {
    match decision {
        ProcessingExportAdapterDecision::NoQualifiedAdapter { reason_code } => {
            format!("refused:{reason_code}")
        }
        ProcessingExportAdapterDecision::Qualified {
            adapter_version,
            parameter_schema_version,
        } => format!("qualified:{adapter_version}:{parameter_schema_version}"),
    }
}

mod lifecycle;
pub(super) use lifecycle::*;

mod retained;
pub(super) use retained::*;

mod records;
use records::*;

/// The processing source facts of one Photo — its Original kind and joint
/// availability — read inside a caller's transaction.
pub(super) fn photo_processing_source(
    connection: &Connection,
    photo_id: &str,
) -> Result<Option<(crate::OriginalKind, bool)>, PersistenceError> {
    connection
        .query_row(
            "SELECT o.kind,o.available,p.available FROM photos p
             JOIN original_files o ON o.id=p.original_id WHERE p.id=?",
            [photo_id],
            |row| {
                Ok((
                    super::scan::parse_kind(&row.get::<_, String>(0)?)?,
                    row.get::<_, i64>(1)? != 0 && row.get::<_, i64>(2)? != 0,
                ))
            },
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)
}

mod owner;

#[cfg(test)]
#[path = "processing_export_tests.rs"]
mod tests;
