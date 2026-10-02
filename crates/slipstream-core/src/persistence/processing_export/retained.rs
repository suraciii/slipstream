use super::*;

fn validate_replay(request: &ReplayProcessingExport) -> Result<(), ProcessingExportRequestError> {
    let contract = ProcessingExportRequestError::Contract;
    validate_bounded_name(&request.photo_id, MAXIMUM_PHOTO_ID_BYTES).map_err(contract)?;
    validate_bounded_name(&request.request_id, MAXIMUM_EXPORT_REQUEST_ID_BYTES)
        .map_err(contract)?;
    validate_bounded_name(&request.expected_recipe_revision, MAXIMUM_REVISION_BYTES)
        .map_err(contract)?;
    validate_source_revision(&request.expected_source_revision).map_err(contract)
}

/// Resolve retained caller intent before consulting any mutable recipe or engine.
/// None means the identity has never received a durable decision.
pub(in crate::persistence) fn replay_processing_export(
    connection: &Connection,
    request: ReplayProcessingExport,
) -> Result<Option<ProcessingExportSubmitOutcome>, PersistenceError> {
    if let Err(error) = validate_replay(&request) {
        return Ok(Some(ProcessingExportSubmitOutcome::Invalid(error)));
    }
    if processing_export_expired(connection, &request.request_id)? {
        return Ok(Some(ProcessingExportSubmitOutcome::Expired));
    }
    if let Some(record) = read_processing_refusal_record(connection, &request.request_id)? {
        let refusal = parse_refusal_record(record)?;
        if refusal.photo_id != request.photo_id
            || refusal.step_id != request.step_id
            || refusal.recipe_revision != request.expected_recipe_revision
            || refusal.source_revision != request.expected_source_revision
        {
            return Ok(Some(ProcessingExportSubmitOutcome::RequestConflict));
        }
        return Ok(Some(ProcessingExportSubmitOutcome::Replayed(refusal)));
    }
    let Some(work) = read_processing_export_work(connection, &request.request_id)? else {
        return Ok(None);
    };
    let admission = &work.admission;
    if admission.photo_id != request.photo_id
        || admission.step_id != request.step_id
        || admission.recipe_revision != request.expected_recipe_revision
        || admission.source_revision != request.expected_source_revision
    {
        return Ok(Some(ProcessingExportSubmitOutcome::RequestConflict));
    }
    retained_work_outcome(connection, work).map(Some)
}

fn retained_work_outcome(
    connection: &Connection,
    work: ProcessingExportWork,
) -> Result<ProcessingExportSubmitOutcome, PersistenceError> {
    Ok(match work.state {
        ProcessingExportWorkState::Accepted | ProcessingExportWorkState::Executing => {
            ProcessingExportSubmitOutcome::Pending(work.admission)
        }
        ProcessingExportWorkState::Failed => ProcessingExportSubmitOutcome::FailureReplayed(work),
        ProcessingExportWorkState::Cancelled => {
            ProcessingExportSubmitOutcome::CancelledReplayed(work)
        }
        ProcessingExportWorkState::Succeeded => {
            let artifact = read_processing_artifact(
                connection,
                work.artifact_id
                    .as_ref()
                    .ok_or(PersistenceError::Storage)?
                    .as_str(),
            )?
            .ok_or(PersistenceError::Storage)?;
            ProcessingExportSubmitOutcome::ArtifactReplayed(artifact)
        }
    })
}

pub(in crate::persistence) fn list_processing_exports(
    connection: &Connection,
    photo_id: &str,
    now: u64,
) -> Result<ProcessingExportList, PersistenceError> {
    let mut works: Vec<_> = scan_processing_export_works(connection)?
        .into_iter()
        .filter(|work| {
            work.admission.photo_id == photo_id
                && work.retain_until.is_none_or(|deadline| now < deadline)
        })
        .collect();
    works.sort_by(|a, b| {
        b.accepted_at
            .cmp(&a.accepted_at)
            .then_with(|| b.admission.request_id.cmp(&a.admission.request_id))
    });
    let mut artifacts = Vec::new();
    for (key, value) in scan_metadata_records(connection, PROCESSING_ARTIFACT_PREFIX)? {
        let artifact = parse_artifact_record(parse_stored_value(&value)?)?;
        if key != processing_artifact_key(artifact.artifact_id.as_str()) {
            return Err(PersistenceError::Storage);
        }
        if artifact.photo_id != photo_id {
            continue;
        }
        let Some(retention) =
            read_processing_artifact_retention_record(connection, artifact.artifact_id.as_str())?
        else {
            continue;
        };
        if now >= retention.retain_until {
            continue;
        }
        let published_at = retention
            .retain_until
            .saturating_sub(PROCESSING_ARTIFACT_RETENTION_SECONDS);
        artifacts.push((published_at, artifact));
    }
    artifacts.sort_by(|(a_time, a), (b_time, b)| {
        b_time
            .cmp(a_time)
            .then_with(|| b.artifact_id.as_str().cmp(a.artifact_id.as_str()))
    });
    works.truncate(64);
    artifacts.truncate(64);
    Ok(ProcessingExportList {
        works,
        artifacts: artifacts
            .into_iter()
            .map(|(_, artifact)| artifact)
            .collect(),
    })
}

/// Re-admit only the retained captured snapshot; the current recipe is irrelevant.
pub(in crate::persistence) fn retry_processing_export(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    request: RetryProcessingExport,
    now: u64,
) -> Result<ProcessingExportSubmitOutcome, PersistenceError> {
    for (value, bound) in [
        (&request.photo_id, MAXIMUM_PHOTO_ID_BYTES),
        (&request.request_id, MAXIMUM_EXPORT_REQUEST_ID_BYTES),
        (
            &request.previous_request_id,
            MAXIMUM_EXPORT_REQUEST_ID_BYTES,
        ),
    ] {
        if let Err(error) = validate_bounded_name(value, bound) {
            return Ok(ProcessingExportSubmitOutcome::Invalid(
                ProcessingExportRequestError::Contract(error),
            ));
        }
    }
    if request.request_id == request.previous_request_id {
        return Ok(ProcessingExportSubmitOutcome::RequestConflict);
    }
    write_transaction(state, database_name, connection, |transaction| {
        if let Some(outcome) = replay_processing_export_retry(
            transaction,
            &request.photo_id,
            &request.previous_request_id,
            &request.request_id,
        )? {
            return Ok(outcome);
        }
        if let Err(error) = validate_bounded_name(
            &request.bundle_id,
            crate::processing::MAXIMUM_CONTRACT_NAME_BYTES,
        ) {
            return Ok(ProcessingExportSubmitOutcome::Invalid(
                ProcessingExportRequestError::Contract(error),
            ));
        }
        if let Err(error) = request.adapter.validate() {
            return Ok(ProcessingExportSubmitOutcome::Invalid(
                ProcessingExportRequestError::Contract(error),
            ));
        }
        if processing_export_expired(transaction, &request.request_id)?
            || processing_export_expired(transaction, &request.previous_request_id)?
        {
            return Ok(ProcessingExportSubmitOutcome::Expired);
        }
        let Some(previous) =
            read_processing_export_work(transaction, &request.previous_request_id)?
        else {
            return Ok(ProcessingExportSubmitOutcome::RequestConflict);
        };
        if previous.admission.photo_id != request.photo_id
            || !matches!(
                previous.state,
                ProcessingExportWorkState::Failed | ProcessingExportWorkState::Cancelled
            )
        {
            return Ok(ProcessingExportSubmitOutcome::RequestConflict);
        }
        if previous
            .retain_until
            .is_some_and(|deadline| now >= deadline)
        {
            return Ok(ProcessingExportSubmitOutcome::Expired);
        }
        let bytes = serde_json::to_vec(&(
            "processing-export-retry-v1",
            &request.previous_request_id,
            &previous.admission.payload_digest,
        ))
        .map_err(|_| PersistenceError::Storage)?;
        let digest = format!("{:x}", Sha256::digest(bytes));
        if let Some(existing) = read_processing_export_work(transaction, &request.request_id)? {
            if existing.admission.photo_id != request.photo_id
                || existing.admission.payload_digest != digest
            {
                return Ok(ProcessingExportSubmitOutcome::RequestConflict);
            }
            return retained_work_outcome(transaction, existing);
        }
        if read_processing_refusal_record(transaction, &request.request_id)?.is_some()
            || read_processing_acceptance_record(transaction, &request.request_id)?.is_some()
        {
            return Ok(ProcessingExportSubmitOutcome::RequestConflict);
        }
        let mut admission = previous.admission.clone();
        if request.bundle_id != admission.bundle_id {
            return Ok(ProcessingExportSubmitOutcome::Unavailable);
        }
        match &request.adapter {
            ProcessingExportAdapterDecision::Qualified {
                adapter_version,
                parameter_schema_version,
            } if format!("{adapter_version}:{parameter_schema_version}")
                == admission.adapter_schema_version => {}
            _ => return Ok(ProcessingExportSubmitOutcome::Unavailable),
        }
        match &admission.input {
            ProcessingInput::Original {
                photo_id,
                source_revision,
            } => {
                let Some(facts) = read_edit_recipe(transaction, photo_id)? else {
                    return Ok(ProcessingExportSubmitOutcome::MissingPhoto);
                };
                if !facts.source_available || facts.current_source_revision.is_none() {
                    return Ok(ProcessingExportSubmitOutcome::Unavailable);
                }
                if facts.current_source_revision.as_ref() != Some(source_revision) {
                    return Ok(ProcessingExportSubmitOutcome::SourceChanged(None));
                }
                if photo_id != &admission.photo_id || source_revision != &admission.source_revision
                {
                    return Ok(ProcessingExportSubmitOutcome::SourceChanged(None));
                }
            }
            ProcessingInput::Artifact {
                artifact_id,
                contract,
            } => {
                let Some(artifact) = read_processing_artifact(transaction, artifact_id.as_str())?
                else {
                    return Ok(ProcessingExportSubmitOutcome::IncompatibleInput(
                        ProcessingInputHandoffError::ArtifactMissing,
                    ));
                };
                if read_processing_artifact_retention_record(transaction, artifact_id.as_str())?
                    .is_some_and(|record| now >= record.retain_until)
                {
                    return Ok(ProcessingExportSubmitOutcome::IncompatibleInput(
                        ProcessingInputHandoffError::ArtifactMissing,
                    ));
                }
                if artifact.output_contract != *contract {
                    return Ok(ProcessingExportSubmitOutcome::IncompatibleInput(
                        ProcessingInputHandoffError::ArtifactContractMismatch,
                    ));
                }
            }
        }
        if live_processing_export_count(transaction)? >= MAXIMUM_LIVE_PROCESSING_EXPORTS {
            return Ok(ProcessingExportSubmitOutcome::ReservationFull);
        }
        if reserved_processing_output_bytes(transaction)?
            .saturating_add(crate::MAXIMUM_EXPORT_BYTES)
            > request.retained_output_bytes_max
        {
            return Ok(ProcessingExportSubmitOutcome::RetainedOutputFull);
        }
        admission.request_id = request.request_id.clone();
        admission.payload_digest = digest;
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
            &processing_export_work_key(&request.request_id),
            &serialize_record(&work_record(&work))?,
        )?;
        write_metadata_value(
            transaction,
            &format!("processing_export_retry:{}", request.request_id),
            &serialize_record(&(
                request.photo_id.clone(),
                request.previous_request_id.clone(),
            ))?,
        )?;
        Ok(ProcessingExportSubmitOutcome::Admitted(admission))
    })
}

pub(in crate::persistence) fn read_processing_artifact_retention(
    connection: &Connection,
    artifact_id: &str,
) -> Result<Option<crate::processing::ProcessingArtifactRetention>, PersistenceError> {
    Ok(
        read_processing_artifact_retention_record(connection, artifact_id)?.map(|record| {
            crate::processing::ProcessingArtifactRetention {
                published_at_unix_seconds: record
                    .retain_until
                    .saturating_sub(PROCESSING_ARTIFACT_RETENTION_SECONDS),
                expires_at_unix_seconds: record.retain_until,
            }
        }),
    )
}

/// Reconcile a retry before inspecting current deployment or its expired parent.
pub(in crate::persistence) fn replay_processing_export_retry(
    connection: &Connection,
    photo_id: &str,
    previous_request_id: &str,
    request_id: &str,
) -> Result<Option<ProcessingExportSubmitOutcome>, PersistenceError> {
    for (value, bound) in [
        (photo_id, MAXIMUM_PHOTO_ID_BYTES),
        (previous_request_id, MAXIMUM_EXPORT_REQUEST_ID_BYTES),
        (request_id, MAXIMUM_EXPORT_REQUEST_ID_BYTES),
    ] {
        if let Err(error) = validate_bounded_name(value, bound) {
            return Ok(Some(ProcessingExportSubmitOutcome::Invalid(
                ProcessingExportRequestError::Contract(error),
            )));
        }
    }
    if request_id == previous_request_id {
        return Ok(Some(ProcessingExportSubmitOutcome::RequestConflict));
    }
    if processing_export_expired(connection, request_id)? {
        return Ok(Some(ProcessingExportSubmitOutcome::Expired));
    }
    let Some(work) = read_processing_export_work(connection, request_id)? else {
        if read_processing_refusal_record(connection, request_id)?.is_some()
            || read_processing_acceptance_record(connection, request_id)?.is_some()
        {
            return Ok(Some(ProcessingExportSubmitOutcome::RequestConflict));
        }
        if processing_export_expired(connection, previous_request_id)? {
            return Ok(Some(ProcessingExportSubmitOutcome::Expired));
        }
        return Ok(None);
    };
    let value: Option<String> = connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [format!("processing_export_retry:{request_id}")],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    let Some(value) = value else {
        return Ok(Some(ProcessingExportSubmitOutcome::RequestConflict));
    };
    let lineage: (String, String) = parse_stored_value(&value)?;
    if lineage.0 != photo_id
        || lineage.1 != previous_request_id
        || work.admission.photo_id != photo_id
    {
        return Ok(Some(ProcessingExportSubmitOutcome::RequestConflict));
    }
    retained_work_outcome(connection, work).map(Some)
}
