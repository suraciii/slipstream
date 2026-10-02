use super::*;
/// Admits one submission request: a bounded request identity, Photo
/// identity, expected revisions, and bundle identity, plus a valid adapter
/// decision.
pub(super) fn validate_submit_request(
    mutation: &SubmitProcessingExport,
) -> Result<(), ProcessingExportRequestError> {
    let contract = ProcessingExportRequestError::Contract;
    validate_bounded_name(&mutation.request_id, MAXIMUM_EXPORT_REQUEST_ID_BYTES)
        .map_err(contract)?;
    validate_bounded_name(&mutation.photo_id, MAXIMUM_PHOTO_ID_BYTES).map_err(contract)?;
    validate_bounded_name(&mutation.expected_recipe_revision, MAXIMUM_REVISION_BYTES)
        .map_err(contract)?;
    validate_revision(&mutation.expected_source_revision).map_err(contract)?;
    validate_bounded_name(
        &mutation.bundle_id,
        crate::processing::MAXIMUM_CONTRACT_NAME_BYTES,
    )
    .map_err(contract)?;
    mutation
        .adapter
        .validate()
        .map_err(ProcessingExportRequestError::Contract)?;
    Ok(())
}

/// The canonical payload digest of one submission request.
pub(super) fn submit_payload_digest(
    mutation: &SubmitProcessingExport,
    decision: &str,
) -> Result<String, PersistenceError> {
    let payload = SubmitPayload {
        kind: "processing-export-submit-v1",
        photo_id: &mutation.photo_id,
        step_id: mutation.step_id.as_str(),
        expected_recipe_revision: &mutation.expected_recipe_revision,
        expected_source_revision: &mutation.expected_source_revision,
        decision,
    };
    let bytes = serde_json::to_vec(&payload).map_err(|_| PersistenceError::Storage)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub(super) fn processing_artifact_key(artifact_id: &str) -> String {
    format!("{PROCESSING_ARTIFACT_PREFIX}{artifact_id}")
}

pub(super) fn processing_export_refusal_key(request_id: &str) -> String {
    format!("{PROCESSING_EXPORT_REFUSAL_PREFIX}{request_id}")
}

pub(super) fn processing_export_acceptance_key(request_id: &str) -> String {
    format!("{PROCESSING_EXPORT_ACCEPTANCE_PREFIX}{request_id}")
}

pub(super) fn processing_export_work_key(request_id: &str) -> String {
    format!("{PROCESSING_EXPORT_WORK_PREFIX}{request_id}")
}

pub(super) fn processing_artifact_retention_key(artifact_id: &str) -> String {
    format!("{PROCESSING_ARTIFACT_RETENTION_PREFIX}{artifact_id}")
}

pub(super) fn processing_artifact_lease_key(lease_id: &str) -> String {
    format!("{PROCESSING_ARTIFACT_LEASE_PREFIX}{lease_id}")
}

pub(super) fn processing_export_expired_key(request_id: &str) -> String {
    format!("{PROCESSING_EXPORT_EXPIRED_PREFIX}{request_id}")
}

/// Whether one composable Export request identity is tombstoned as expired.
/// A stored tombstone that names another identity is corruption, never a
/// silent miss.
pub(super) fn processing_export_expired(
    connection: &Connection,
    request_id: &str,
) -> Result<bool, PersistenceError> {
    let stored = connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [processing_export_expired_key(request_id)],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    match stored {
        Some(value) if value == request_id => Ok(true),
        Some(_) => Err(PersistenceError::Storage),
        None => Ok(false),
    }
}

/// Reads and strictly parses one stored work record. A malformed or
/// oversized record is a storage error, never a silent miss: a record that
/// cannot be parsed cannot prove a replay.
pub(super) fn read_processing_export_work_record(
    connection: &Connection,
    request_id: &str,
) -> Result<Option<ProcessingExportWorkRecord>, PersistenceError> {
    let value = connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [processing_export_work_key(request_id)],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    value
        .map(|value| {
            let record: ProcessingExportWorkRecord = parse_stored_value(&value)?;
            if record.photo_id.is_empty() || record.request_id != request_id {
                return Err(PersistenceError::Storage);
            }
            crate::processing::validate_digest(&record.payload_digest)
                .map_err(|_| PersistenceError::Storage)?;
            Ok(record)
        })
        .transpose()
}

/// Reads and strictly parses one stored artifact retention record. A
/// malformed or oversized record is a storage error, never a silent miss.
pub(super) fn read_processing_artifact_retention_record(
    connection: &Connection,
    artifact_id: &str,
) -> Result<Option<ProcessingArtifactRetentionRecord>, PersistenceError> {
    let value = connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [processing_artifact_retention_key(artifact_id)],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    value
        .map(|value| {
            let record: ProcessingArtifactRetentionRecord = parse_stored_value(&value)?;
            if record.artifact_id != artifact_id
                || record.requests.iter().any(|request| request.is_empty())
            {
                return Err(PersistenceError::Storage);
            }
            validate_bounded_name(
                &record.artifact_id,
                crate::processing::MAXIMUM_ARTIFACT_ID_BYTES,
            )
            .map_err(|_| PersistenceError::Storage)?;
            Ok(record)
        })
        .transpose()
}

/// Reads and strictly parses one stored download lease record. A malformed
/// or oversized record is a storage error, never a silent miss.
pub(super) fn read_processing_artifact_lease_record(
    connection: &Connection,
    lease_id: &str,
) -> Result<Option<ProcessingArtifactLeaseRecord>, PersistenceError> {
    let value = connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [processing_artifact_lease_key(lease_id)],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    value
        .map(|value| {
            let record: ProcessingArtifactLeaseRecord = parse_stored_value(&value)?;
            if record.lease_id != lease_id || record.lease_id.is_empty() {
                return Err(PersistenceError::Storage);
            }
            validate_bounded_name(
                &record.artifact_id,
                crate::processing::MAXIMUM_ARTIFACT_ID_BYTES,
            )
            .map_err(|_| PersistenceError::Storage)?;
            Ok(record)
        })
        .transpose()
}

/// Reads and strictly parses every stored work record, binding each record
/// to its key. A malformed record under the namespace is a storage error,
/// never a silently skipped entry.
pub(super) fn scan_processing_export_works(
    connection: &Connection,
) -> Result<Vec<ProcessingExportWork>, PersistenceError> {
    scan_metadata_records(connection, PROCESSING_EXPORT_WORK_PREFIX)?
        .into_iter()
        .map(|(key, value)| {
            let record: ProcessingExportWorkRecord = parse_stored_value(&value)?;
            let work = parse_work_record(record)?;
            if key != processing_export_work_key(&work.admission.request_id) {
                return Err(PersistenceError::Storage);
            }
            Ok(work)
        })
        .collect()
}

/// Reads and strictly parses every stored artifact retention record, each
/// bound to its key.
pub(super) fn scan_processing_artifact_retentions(
    connection: &Connection,
) -> Result<Vec<ProcessingArtifactRetentionRecord>, PersistenceError> {
    scan_metadata_records(connection, PROCESSING_ARTIFACT_RETENTION_PREFIX)?
        .into_iter()
        .map(|(key, value)| {
            let record: ProcessingArtifactRetentionRecord = parse_stored_value(&value)?;
            if key != processing_artifact_retention_key(&record.artifact_id)
                || record.requests.iter().any(|request| request.is_empty())
            {
                return Err(PersistenceError::Storage);
            }
            validate_bounded_name(
                &record.artifact_id,
                crate::processing::MAXIMUM_ARTIFACT_ID_BYTES,
            )
            .map_err(|_| PersistenceError::Storage)?;
            Ok(record)
        })
        .collect()
}

/// Reads and strictly parses every stored download lease record, each bound
/// to its key.
pub(super) fn scan_processing_artifact_leases(
    connection: &Connection,
) -> Result<Vec<ProcessingArtifactLeaseRecord>, PersistenceError> {
    scan_metadata_records(connection, PROCESSING_ARTIFACT_LEASE_PREFIX)?
        .into_iter()
        .map(|(key, value)| {
            let record: ProcessingArtifactLeaseRecord = parse_stored_value(&value)?;
            if key != processing_artifact_lease_key(&record.lease_id) || record.lease_id.is_empty()
            {
                return Err(PersistenceError::Storage);
            }
            validate_bounded_name(
                &record.artifact_id,
                crate::processing::MAXIMUM_ARTIFACT_ID_BYTES,
            )
            .map_err(|_| PersistenceError::Storage)?;
            Ok(record)
        })
        .collect()
}

/// Reads every key and value under one metadata-key namespace, ordered by
/// key for a deterministic scan.
pub(super) fn scan_metadata_records(
    connection: &Connection,
    prefix: &str,
) -> Result<Vec<(String, String)>, PersistenceError> {
    let mut statement = connection
        .prepare("SELECT key,value FROM library_metadata WHERE key LIKE ? ORDER BY key")
        .map_err(|_| PersistenceError::Storage)?;
    statement
        .query_map([format!("{prefix}%")], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)
}

/// The number of live — accepted or executing — composable Export work
/// records, read inside a caller's transaction.
pub(super) fn live_processing_export_count(
    connection: &Connection,
) -> Result<usize, PersistenceError> {
    Ok(scan_processing_export_works(connection)?
        .into_iter()
        .filter(|work| !work.state.is_terminal())
        .count())
}

/// Conservatively counts every retained composable artifact and every live
/// work record before admitting another bounded output.
pub(super) fn reserved_processing_output_bytes(
    connection: &Connection,
) -> Result<u64, PersistenceError> {
    let artifacts = scan_metadata_records(connection, PROCESSING_ARTIFACT_PREFIX)?;
    let retained = artifacts
        .into_iter()
        .filter(|(key, _)| !key.starts_with(PROCESSING_ARTIFACT_RETENTION_PREFIX))
        .map(|(_, value)| {
            let record = parse_stored_value::<ProcessingArtifactRecord>(&value)?;
            Ok(record.byte_length)
        })
        .collect::<Result<Vec<_>, PersistenceError>>()?;
    let live = live_processing_export_count(connection)?;
    Ok(retained
        .into_iter()
        .fold(0_u64, u64::saturating_add)
        .saturating_add(
            u64::try_from(live)
                .unwrap_or(u64::MAX)
                .saturating_mul(crate::MAXIMUM_EXPORT_BYTES),
        ))
}

/// Reads and strictly parses one stored acceptance receipt. A malformed or
/// oversized record is a storage error, never a silent miss.
pub(super) fn read_processing_acceptance_record(
    connection: &Connection,
    request_id: &str,
) -> Result<Option<ProcessingExportAcceptanceRecord>, PersistenceError> {
    let value = connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [processing_export_acceptance_key(request_id)],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    value
        .map(|value| {
            let record: ProcessingExportAcceptanceRecord =
                parse_stored_value::<ProcessingExportAcceptanceRecord>(&value)?;
            if record.photo_id.is_empty() || record.request_id != request_id {
                return Err(PersistenceError::Storage);
            }
            crate::processing::validate_digest(&record.payload_digest)
                .map_err(|_| PersistenceError::Storage)?;
            validate_bounded_name(
                &record.artifact_id,
                crate::processing::MAXIMUM_ARTIFACT_ID_BYTES,
            )
            .map_err(|_| PersistenceError::Storage)?;
            Ok(record)
        })
        .transpose()
}

/// Reads and strictly parses one stored refusal record. A malformed or
/// oversized record is a storage error, never a silent miss: a record that
/// cannot be parsed cannot prove a replay.
pub(super) fn read_processing_refusal_record(
    connection: &Connection,
    request_id: &str,
) -> Result<Option<ProcessingExportRefusalRecord>, PersistenceError> {
    let value = connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [processing_export_refusal_key(request_id)],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    value
        .map(|value| {
            let record: ProcessingExportRefusalRecord =
                parse_stored_value::<ProcessingExportRefusalRecord>(&value)?;
            if record.photo_id.is_empty() || record.request_id != request_id {
                return Err(PersistenceError::Storage);
            }
            crate::processing::validate_digest(&record.payload_digest)
                .map_err(|_| PersistenceError::Storage)?;
            Ok(record)
        })
        .transpose()
}

pub(super) fn write_metadata_value(
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

pub(super) fn delete_metadata_value(
    transaction: &Transaction<'_>,
    key: &str,
) -> Result<(), PersistenceError> {
    transaction
        .execute("DELETE FROM library_metadata WHERE key=?", [key])
        .map_err(|_| PersistenceError::Storage)?;
    Ok(())
}

/// Serializes one record within the metadata value bound. A value past the
/// bound is refused as a storage error rather than written oversized or
/// truncated; admitted records cannot reach it.
pub(super) fn serialize_record<T: Serialize>(record: &T) -> Result<String, PersistenceError> {
    let serialized = serde_json::to_string(record).map_err(|_| PersistenceError::Storage)?;
    if serialized.len() > MAXIMUM_PROCESSING_EXPORT_RECORD_BYTES {
        return Err(PersistenceError::Storage);
    }
    Ok(serialized)
}

/// Parses one stored metadata value strictly: bounded, unknown fields
/// refused.
pub(super) fn parse_stored_value<T: for<'de> Deserialize<'de>>(
    value: &str,
) -> Result<T, PersistenceError> {
    if value.len() > MAXIMUM_PROCESSING_EXPORT_RECORD_BYTES {
        return Err(PersistenceError::Storage);
    }
    serde_json::from_str(value).map_err(|_| PersistenceError::Storage)
}

pub(super) fn contract_record(contract: &ProcessingImageContract) -> ContractRecord {
    ContractRecord {
        format: contract.format.clone(),
        precision: contract.precision.clone(),
        color_space: contract.color_space.clone(),
        transfer: contract.transfer.clone(),
        width: contract.geometry.width,
        height: contract.geometry.height,
        encoding: contract.encoding.clone(),
    }
}

pub(super) fn parse_contract_record(
    record: ContractRecord,
) -> Result<ProcessingImageContract, PersistenceError> {
    let geometry = ProcessingGeometry::new(record.width, record.height)
        .map_err(|_| PersistenceError::Storage)?;
    let contract = ProcessingImageContract {
        format: record.format,
        precision: record.precision,
        color_space: record.color_space,
        transfer: record.transfer,
        geometry,
        encoding: record.encoding,
    };
    contract.validate().map_err(|_| PersistenceError::Storage)?;
    Ok(contract)
}

pub(super) fn input_record(input: &ProcessingInput) -> InputRecord {
    match input {
        ProcessingInput::Original {
            photo_id,
            source_revision,
        } => InputRecord::Original(OriginalRecord {
            photo_id: photo_id.clone(),
            source_revision: source_revision.clone(),
        }),
        ProcessingInput::Artifact {
            artifact_id,
            contract,
        } => InputRecord::Artifact(ArtifactRecord {
            artifact_id: artifact_id.as_str().to_owned(),
            contract: contract_record(contract),
        }),
    }
}

pub(super) fn parse_input_record(record: InputRecord) -> Result<ProcessingInput, PersistenceError> {
    let input = match record {
        InputRecord::Original(record) => ProcessingInput::Original {
            photo_id: record.photo_id,
            source_revision: record.source_revision,
        },
        InputRecord::Artifact(record) => ProcessingInput::Artifact {
            artifact_id: ProcessingArtifactId::new(&record.artifact_id)
                .map_err(|_| PersistenceError::Storage)?,
            contract: parse_contract_record(record.contract)?,
        },
    };
    input.validate().map_err(|_| PersistenceError::Storage)?;
    Ok(input)
}

pub(super) fn evidence_record(evidence: &ProcessingInputEvidence) -> EvidenceRecord {
    match &evidence.input {
        ProcessingInput::Original {
            photo_id,
            source_revision,
        } => EvidenceRecord::Original(OriginalEvidenceRecord {
            photo_id: photo_id.clone(),
            source_revision: source_revision.clone(),
            sha256: evidence.sha256.clone(),
            byte_length: evidence.byte_length,
        }),
        ProcessingInput::Artifact {
            artifact_id,
            contract,
        } => EvidenceRecord::Artifact(ArtifactEvidenceRecord {
            artifact_id: artifact_id.as_str().to_owned(),
            contract: contract_record(contract),
            sha256: evidence.sha256.clone(),
            byte_length: evidence.byte_length,
        }),
    }
}

pub(super) fn parse_evidence_record(
    record: EvidenceRecord,
) -> Result<ProcessingInputEvidence, PersistenceError> {
    let evidence = match record {
        EvidenceRecord::Original(record) => ProcessingInputEvidence::new(
            ProcessingInput::Original {
                photo_id: record.photo_id,
                source_revision: record.source_revision,
            },
            &record.sha256,
            record.byte_length,
        )
        .map_err(|_| PersistenceError::Storage)?,
        EvidenceRecord::Artifact(record) => ProcessingInputEvidence::new(
            ProcessingInput::Artifact {
                artifact_id: ProcessingArtifactId::new(&record.artifact_id)
                    .map_err(|_| PersistenceError::Storage)?,
                contract: parse_contract_record(record.contract)?,
            },
            &record.sha256,
            record.byte_length,
        )
        .map_err(|_| PersistenceError::Storage)?,
    };
    Ok(evidence)
}

pub(super) fn snapshot_record(snapshot: &ProcessingParameterSnapshot) -> SnapshotRecord {
    SnapshotRecord {
        schema_version: snapshot.schema_version.clone(),
        tree: snapshot.tree.clone(),
    }
}

pub(super) fn parse_snapshot_record(
    record: SnapshotRecord,
) -> Result<ProcessingParameterSnapshot, PersistenceError> {
    ProcessingParameterSnapshot::new(&record.schema_version, record.tree)
        .map_err(|_| PersistenceError::Storage)
}

pub(super) fn artifact_record(artifact: &ProcessingArtifact) -> ProcessingArtifactRecord {
    ProcessingArtifactRecord {
        artifact_id: artifact.artifact_id.as_str().to_owned(),
        photo_id: artifact.photo_id.clone(),
        step_id: artifact.step_id.as_str().to_owned(),
        module: artifact.module.as_str().to_owned(),
        adapter_schema_version: artifact.adapter_schema_version.clone(),
        parameters: snapshot_record(&artifact.parameters),
        input: evidence_record(&artifact.input),
        output_contract: contract_record(&artifact.output_contract),
        bundle_id: artifact.bundle_id.clone(),
        sha256: artifact.sha256.clone(),
        byte_length: artifact.byte_length,
    }
}

/// Rebuilds one artifact from its stored shape. Every identity is
/// reconstructed through its admitting constructor and the whole record is
/// revalidated, so a record that no longer passes the contract vocabulary
/// is a storage error, never a silently degraded artifact.
pub(super) fn parse_artifact_record(
    record: ProcessingArtifactRecord,
) -> Result<ProcessingArtifact, PersistenceError> {
    let artifact = ProcessingArtifact {
        artifact_id: ProcessingArtifactId::new(&record.artifact_id)
            .map_err(|_| PersistenceError::Storage)?,
        photo_id: record.photo_id,
        step_id: ProcessingStepId::new(&record.step_id).map_err(|_| PersistenceError::Storage)?,
        module: ProcessingModuleId::new(&record.module).map_err(|_| PersistenceError::Storage)?,
        adapter_schema_version: record.adapter_schema_version,
        parameters: parse_snapshot_record(record.parameters)?,
        input: parse_evidence_record(record.input)?,
        output_contract: parse_contract_record(record.output_contract)?,
        bundle_id: record.bundle_id,
        sha256: record.sha256,
        byte_length: record.byte_length,
    };
    artifact.validate().map_err(|_| PersistenceError::Storage)?;
    Ok(artifact)
}

/// Rebuilds one refusal from its stored shape, revalidated end to end.
pub(super) fn parse_refusal_record(
    record: ProcessingExportRefusalRecord,
) -> Result<ProcessingExportRefusal, PersistenceError> {
    let refusal = ProcessingExportRefusal {
        photo_id: record.photo_id,
        request_id: record.request_id,
        payload_digest: record.payload_digest,
        step_id: ProcessingStepId::new(&record.step_id).map_err(|_| PersistenceError::Storage)?,
        recipe_revision: record.recipe_revision,
        source_revision: record.source_revision,
        module: ProcessingModuleId::new(&record.module).map_err(|_| PersistenceError::Storage)?,
        parameter_schema_version: record.parameter_schema_version,
        parameter_digest: record.parameter_digest,
        input: parse_input_record(record.input)?,
        bundle_id: record.bundle_id,
        reason_code: record.reason_code,
    };
    refusal.validate().map_err(|_| PersistenceError::Storage)?;
    Ok(refusal)
}

/// The stored shape of one durable work record.
pub(super) fn work_record(work: &ProcessingExportWork) -> ProcessingExportWorkRecord {
    let admission = &work.admission;
    ProcessingExportWorkRecord {
        photo_id: admission.photo_id.clone(),
        request_id: admission.request_id.clone(),
        payload_digest: admission.payload_digest.clone(),
        step_id: admission.step_id.as_str().to_owned(),
        recipe_revision: admission.recipe_revision.clone(),
        source_revision: admission.source_revision.clone(),
        module: admission.module.as_str().to_owned(),
        adapter_schema_version: admission.adapter_schema_version.clone(),
        parameters: snapshot_record(&admission.parameters),
        input: input_record(&admission.input),
        bundle_id: admission.bundle_id.clone(),
        state: work.state.as_str().to_owned(),
        accepted_at: work.accepted_at,
        attempt: work.attempt.map(|attempt| AttemptRecord {
            sequence: attempt.sequence,
            began_at: attempt.began_at,
        }),
        artifact_id: work.artifact_id.as_ref().map(|id| id.as_str().to_owned()),
        failure_reason: work.failure_reason.clone(),
        terminal_at: work.terminal_at,
        retain_until: work.retain_until,
    }
}

/// Rebuilds one work record from its stored shape. Every identity is
/// reconstructed through its admitting constructor, the whole record is
/// revalidated, and the state-dependent shape invariants — exactly which
/// lifecycle facts each state may carry — are enforced, so a record that no
/// longer passes the contract vocabulary is a storage error, never a
/// silently degraded work record.
pub(super) fn parse_work_record(
    record: ProcessingExportWorkRecord,
) -> Result<ProcessingExportWork, PersistenceError> {
    let admission = ProcessingExportAdmission {
        photo_id: record.photo_id,
        request_id: record.request_id,
        payload_digest: record.payload_digest,
        step_id: ProcessingStepId::new(&record.step_id).map_err(|_| PersistenceError::Storage)?,
        recipe_revision: record.recipe_revision,
        source_revision: record.source_revision,
        module: ProcessingModuleId::new(&record.module).map_err(|_| PersistenceError::Storage)?,
        adapter_schema_version: record.adapter_schema_version,
        parameters: parse_snapshot_record(record.parameters)?,
        input: parse_input_record(record.input)?,
        bundle_id: record.bundle_id,
    };
    admission
        .validate()
        .map_err(|_| PersistenceError::Storage)?;
    let state = ProcessingExportWorkState::parse(&record.state).ok_or(PersistenceError::Storage)?;
    let artifact_id = record
        .artifact_id
        .as_deref()
        .map(ProcessingArtifactId::new)
        .transpose()
        .map_err(|_| PersistenceError::Storage)?;
    let work = ProcessingExportWork {
        admission,
        state,
        accepted_at: record.accepted_at,
        attempt: record.attempt.map(|attempt| ProcessingExportAttempt {
            sequence: attempt.sequence,
            began_at: attempt.began_at,
        }),
        artifact_id,
        failure_reason: record.failure_reason,
        terminal_at: record.terminal_at,
        retain_until: record.retain_until,
    };
    work.validate().map_err(|_| PersistenceError::Storage)?;
    let shape_valid = match work.state {
        ProcessingExportWorkState::Accepted => {
            work.attempt.is_none()
                && work.artifact_id.is_none()
                && work.failure_reason.is_none()
                && work.terminal_at.is_none()
                && work.retain_until.is_none()
        }
        ProcessingExportWorkState::Executing => {
            work.attempt.is_some()
                && work.artifact_id.is_none()
                && work.failure_reason.is_none()
                && work.terminal_at.is_none()
                && work.retain_until.is_none()
        }
        ProcessingExportWorkState::Succeeded => {
            work.artifact_id.is_some()
                && work.failure_reason.is_none()
                && work.terminal_at.is_some()
                && work.retain_until.is_some()
        }
        ProcessingExportWorkState::Failed => {
            work.failure_reason.is_some()
                && work.artifact_id.is_none()
                && work.terminal_at.is_some()
                && work.retain_until.is_some()
        }
        ProcessingExportWorkState::Cancelled => {
            work.artifact_id.is_none()
                && work.failure_reason.is_none()
                && work.terminal_at.is_some()
                && work.retain_until.is_some()
        }
    };
    if !shape_valid {
        return Err(PersistenceError::Storage);
    }
    Ok(work)
}
