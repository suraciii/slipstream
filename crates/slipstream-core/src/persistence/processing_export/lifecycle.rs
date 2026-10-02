use super::*;
/// Settles one admitted qualified composable Export: validates that the
/// artifact carries exactly the accepted admission's pinned provenance,
/// publishes the artifact insert-only, records the request's acceptance
/// receipt, records the work's terminal success, and installs the artifact's
/// finite retention — all in one write transaction. A replay of a settled
/// request returns the committed work unchanged; a request that already
/// reached a different terminal decision is refused by first-wins; and a
/// settlement without an accepted work record never publishes anything.
pub(in crate::persistence) fn settle_processing_export(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    artifact: ProcessingArtifact,
    request_id: &str,
    payload_digest: &str,
    now: u64,
) -> Result<ProcessingExportSettlement, PersistenceError> {
    artifact.validate().map_err(|_| PersistenceError::Storage)?;
    validate_bounded_name(request_id, MAXIMUM_EXPORT_REQUEST_ID_BYTES)
        .map_err(|_| PersistenceError::Storage)?;
    crate::processing::validate_digest(payload_digest).map_err(|_| PersistenceError::Storage)?;
    let artifact_id = artifact.artifact_id.as_str().to_owned();
    let record = artifact_record(&artifact);
    write_transaction(state, database_name, connection, |transaction| {
        let Some(mut work) = read_processing_export_work(transaction, request_id)? else {
            return Ok(ProcessingExportSettlement::Missing);
        };
        if work.admission.request_id != request_id
            || work.admission.payload_digest != payload_digest
        {
            return Ok(ProcessingExportSettlement::Conflict);
        }
        if let Some(existing) = read_processing_acceptance_record(transaction, request_id)? {
            return Ok(
                if existing.payload_digest == payload_digest
                    && existing.artifact_id == artifact_id
                    && work.state == ProcessingExportWorkState::Succeeded
                    && work.artifact_id.as_ref().map(ProcessingArtifactId::as_str)
                        == Some(artifact_id.as_str())
                {
                    ProcessingExportSettlement::Replayed(work)
                } else {
                    ProcessingExportSettlement::Conflict
                },
            );
        }
        match work.state {
            ProcessingExportWorkState::Failed | ProcessingExportWorkState::Cancelled => {
                // The first terminal decision wins: a failed or cancelled
                // request can never settle.
                return Ok(ProcessingExportSettlement::Terminal(work));
            }
            // A succeeded work record always carries its acceptance
            // receipt, which the replay above already resolved; a stored
            // pair without one is corrupt, never a settlement.
            ProcessingExportWorkState::Succeeded => return Err(PersistenceError::Storage),
            ProcessingExportWorkState::Accepted | ProcessingExportWorkState::Executing => {}
        }
        // The published provenance must be exactly the accepted admission:
        // execution by any other binding, step, module, pinned adapter,
        // captured parameters, or bundle never settles.
        if artifact.photo_id != work.admission.photo_id
            || artifact.step_id != work.admission.step_id
            || artifact.module != work.admission.module
            || artifact.adapter_schema_version != work.admission.adapter_schema_version
            || artifact.parameters != work.admission.parameters
            || artifact.bundle_id != work.admission.bundle_id
            || artifact.input.input != work.admission.input
        {
            return Ok(ProcessingExportSettlement::Conflict);
        }
        if let Some(published) = read_processing_artifact(transaction, &artifact_id)? {
            if !published.same_publication(&artifact) {
                // A different record already owns this artifact identity;
                // the work stays live and nothing is settled.
                return Ok(ProcessingExportSettlement::Conflict);
            }
        } else {
            write_metadata_value(
                transaction,
                &processing_artifact_key(&artifact_id),
                &serialize_record(&record)?,
            )?;
        }
        // The artifact's finite retention: the first settlement of an
        // artifact identity fixes its window; a later request settling the
        // identical artifact joins the existing window and is swept with it.
        let mut retention =
            match read_processing_artifact_retention_record(transaction, &artifact_id)? {
                Some(existing) => existing,
                None => ProcessingArtifactRetentionRecord {
                    artifact_id: artifact_id.clone(),
                    retain_until: now.saturating_add(PROCESSING_ARTIFACT_RETENTION_SECONDS),
                    requests: Vec::new(),
                },
            };
        if !retention
            .requests
            .iter()
            .any(|request| request == request_id)
        {
            retention.requests.push(request_id.to_owned());
        }
        work.state = ProcessingExportWorkState::Succeeded;
        work.artifact_id = Some(artifact.artifact_id.clone());
        work.terminal_at = Some(now);
        work.retain_until = Some(retention.retain_until);
        work.validate().map_err(|_| PersistenceError::Storage)?;
        write_metadata_value(
            transaction,
            &processing_export_work_key(request_id),
            &serialize_record(&work_record(&work))?,
        )?;
        write_metadata_value(
            transaction,
            &processing_artifact_retention_key(&artifact_id),
            &serialize_record(&retention)?,
        )?;
        let receipt = ProcessingExportAcceptanceRecord {
            photo_id: artifact.photo_id.clone(),
            request_id: request_id.to_owned(),
            payload_digest: payload_digest.to_owned(),
            artifact_id: artifact_id.clone(),
        };
        write_metadata_value(
            transaction,
            &processing_export_acceptance_key(request_id),
            &serialize_record(&receipt)?,
        )?;
        Ok(ProcessingExportSettlement::Settled(work))
    })
}

/// Reads one durable composable Export work record in a single serialized
/// read. `None` means no qualified submission was ever accepted under the
/// request identity. A record that is malformed, oversized, or no longer
/// admissible is a storage error, never a partially parsed work record.
pub(in crate::persistence) fn read_processing_export_work(
    connection: &Connection,
    request_id: &str,
) -> Result<Option<ProcessingExportWork>, PersistenceError> {
    read_processing_export_work_record(connection, request_id)?
        .map(parse_work_record)
        .transpose()
}

/// Durably begins one execution attempt of an accepted composable Export:
/// the work becomes `Executing` under a fresh attempt sequence in one write
/// transaction. A terminal work record is returned unchanged — the first
/// terminal decision wins — and a missing request identity begins nothing.
pub(in crate::persistence) fn begin_processing_export_attempt(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    request_id: &str,
    now: u64,
) -> Result<ProcessingExportAttemptOutcome, PersistenceError> {
    validate_bounded_name(request_id, MAXIMUM_EXPORT_REQUEST_ID_BYTES)
        .map_err(|_| PersistenceError::Storage)?;
    write_transaction(state, database_name, connection, |transaction| {
        let Some(mut work) = read_processing_export_work(transaction, request_id)? else {
            return Ok(ProcessingExportAttemptOutcome::Missing);
        };
        if work.state.is_terminal() {
            return Ok(ProcessingExportAttemptOutcome::Terminal(work));
        }
        let sequence = work
            .attempt
            .map(|attempt| attempt.sequence)
            .unwrap_or(0)
            .saturating_add(1);
        work.attempt = Some(ProcessingExportAttempt {
            sequence,
            began_at: now,
        });
        work.state = ProcessingExportWorkState::Executing;
        write_metadata_value(
            transaction,
            &processing_export_work_key(request_id),
            &serialize_record(&work_record(&work))?,
        )?;
        Ok(ProcessingExportAttemptOutcome::Began(work))
    })
}

/// Records one terminal execution failure with a bounded reason code in a
/// single write transaction. The first terminal decision wins: a work
/// record that already succeeded, failed, or cancelled is returned
/// unchanged, and nothing is rewritten.
pub(in crate::persistence) fn fail_processing_export(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    request_id: &str,
    reason_code: &str,
    now: u64,
) -> Result<ProcessingExportFailureOutcome, PersistenceError> {
    if let Err(error) =
        validate_bounded_name(reason_code, crate::processing::MAXIMUM_CONTRACT_NAME_BYTES)
    {
        return Ok(ProcessingExportFailureOutcome::Invalid(error));
    }
    validate_bounded_name(request_id, MAXIMUM_EXPORT_REQUEST_ID_BYTES)
        .map_err(|_| PersistenceError::Storage)?;
    write_transaction(state, database_name, connection, |transaction| {
        let Some(mut work) = read_processing_export_work(transaction, request_id)? else {
            return Ok(ProcessingExportFailureOutcome::Missing);
        };
        if work.state.is_terminal() {
            return Ok(ProcessingExportFailureOutcome::Terminal(work));
        }
        work.state = ProcessingExportWorkState::Failed;
        work.failure_reason = Some(reason_code.to_owned());
        work.terminal_at = Some(now);
        work.retain_until = Some(now.saturating_add(PROCESSING_EXPORT_RECEIPT_RETENTION_SECONDS));
        write_metadata_value(
            transaction,
            &processing_export_work_key(request_id),
            &serialize_record(&work_record(&work))?,
        )?;
        Ok(ProcessingExportFailureOutcome::Failed(work))
    })
}

/// Cancels one admitted composable Export in a single write transaction.
/// The first terminal decision wins: a work record that already settled,
/// failed, or cancelled is returned unchanged, and nothing is rewritten.
pub(in crate::persistence) fn cancel_processing_export(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    request_id: &str,
    now: u64,
) -> Result<ProcessingExportCancelOutcome, PersistenceError> {
    validate_bounded_name(request_id, MAXIMUM_EXPORT_REQUEST_ID_BYTES)
        .map_err(|_| PersistenceError::Storage)?;
    write_transaction(state, database_name, connection, |transaction| {
        let Some(mut work) = read_processing_export_work(transaction, request_id)? else {
            return Ok(ProcessingExportCancelOutcome::Missing);
        };
        if work.state.is_terminal() {
            return Ok(ProcessingExportCancelOutcome::Terminal(work));
        }
        work.state = ProcessingExportWorkState::Cancelled;
        work.terminal_at = Some(now);
        work.retain_until = Some(now.saturating_add(PROCESSING_EXPORT_RECEIPT_RETENTION_SECONDS));
        write_metadata_value(
            transaction,
            &processing_export_work_key(request_id),
            &serialize_record(&work_record(&work))?,
        )?;
        Ok(ProcessingExportCancelOutcome::Cancelled(work))
    })
}

/// Every live composable Export work record — accepted or executing — for
/// restart reconciliation, ordered by acceptance time and request identity.
pub(in crate::persistence) fn unfinished_processing_exports(
    connection: &Connection,
) -> Result<Vec<ProcessingExportWork>, PersistenceError> {
    let mut works = scan_processing_export_works(connection)?;
    works.retain(|work| !work.state.is_terminal());
    works.sort_by(|left, right| {
        left.accepted_at
            .cmp(&right.accepted_at)
            .then_with(|| left.admission.request_id.cmp(&right.admission.request_id))
    });
    Ok(works)
}

/// Removes expired Processing Artifacts and terminal receipts. Stale
/// download leases are reclaimed first; then every artifact whose finite
/// retention has passed and that no live lease holds loses its artifact
/// record and retention, its settled work records and acceptance receipts
/// are removed, and their request identities are tombstoned so they can
/// never start new work. Finally every terminally failed or cancelled work
/// record past its own finite receipt window is removed and its request
/// identity tombstoned the same way. Returns the expired artifact identities
/// whose files the caller must delete after the transaction commits; an
/// uncertainty about a settlement never deletes anything.
pub(in crate::persistence) fn sweep_processing_export_expiry(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    now: u64,
) -> Result<Vec<String>, PersistenceError> {
    write_transaction(state, database_name, connection, |transaction| {
        let stale_before = now.saturating_sub(PROCESSING_ARTIFACT_LEASE_STALE_SECONDS);
        for lease in scan_processing_artifact_leases(transaction)? {
            if lease.created_at < stale_before {
                delete_metadata_value(
                    transaction,
                    &processing_artifact_lease_key(&lease.lease_id),
                )?;
            }
        }
        let held_artifacts = scan_processing_artifact_leases(transaction)?
            .into_iter()
            .map(|lease| lease.artifact_id)
            .collect::<Vec<String>>();
        let mut expired = Vec::new();
        for retention in scan_processing_artifact_retentions(transaction)? {
            if retention.retain_until > now || held_artifacts.contains(&retention.artifact_id) {
                continue;
            }
            let artifact_id = retention.artifact_id.clone();
            for request_id in &retention.requests {
                match read_processing_export_work(transaction, request_id)? {
                    Some(work)
                        if work.state == ProcessingExportWorkState::Succeeded
                            && work.artifact_id.as_ref().map(ProcessingArtifactId::as_str)
                                == Some(artifact_id.as_str()) =>
                    {
                        delete_metadata_value(
                            transaction,
                            &processing_export_work_key(request_id),
                        )?;
                        delete_metadata_value(
                            transaction,
                            &processing_export_acceptance_key(request_id),
                        )?;
                        write_metadata_value(
                            transaction,
                            &processing_export_expired_key(request_id),
                            request_id,
                        )?;
                    }
                    // A retention row exists only for work a settlement
                    // recorded as succeeded; a live record under it is
                    // corrupt, and nothing is deleted on an uncertain
                    // settlement.
                    Some(_) => return Err(PersistenceError::Storage),
                    None => {
                        write_metadata_value(
                            transaction,
                            &processing_export_expired_key(request_id),
                            request_id,
                        )?;
                    }
                }
            }
            delete_metadata_value(transaction, &processing_artifact_key(&artifact_id))?;
            delete_metadata_value(
                transaction,
                &processing_artifact_retention_key(&artifact_id),
            )?;
            expired.push(artifact_id);
        }
        // Terminally failed and cancelled receipts keep their replay answer
        // for their own finite window; past it the record is removed and the
        // request identity is tombstoned so it can never start new work.
        // Succeeded records are removed only with their expired artifact
        // above, never here.
        for work in scan_processing_export_works(transaction)? {
            if work.state == ProcessingExportWorkState::Succeeded || !work.state.is_terminal() {
                continue;
            }
            match work.retain_until {
                Some(deadline) if deadline <= now => {}
                _ => continue,
            }
            delete_metadata_value(
                transaction,
                &processing_export_work_key(&work.admission.request_id),
            )?;
            write_metadata_value(
                transaction,
                &processing_export_expired_key(&work.admission.request_id),
                &work.admission.request_id,
            )?;
        }
        Ok(expired)
    })
}

/// Acquires one Processing Artifact download lease: the artifact must be
/// published and, when it carries a finite retention, that retention must
/// not have passed. The lease holds the artifact against expiry cleanup
/// until it is released or its liveness anchor goes stale.
pub(in crate::persistence) fn acquire_processing_artifact_lease(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    artifact_id: &str,
    now: u64,
) -> Result<ProcessingArtifactLeaseOutcome, PersistenceError> {
    validate_bounded_name(artifact_id, crate::processing::MAXIMUM_ARTIFACT_ID_BYTES)
        .map_err(|_| PersistenceError::Storage)?;
    write_transaction(state, database_name, connection, |transaction| {
        let Some(artifact) = read_processing_artifact(transaction, artifact_id)? else {
            return Ok(ProcessingArtifactLeaseOutcome::Unknown);
        };
        if let Some(retention) =
            read_processing_artifact_retention_record(transaction, artifact_id)?
            && retention.retain_until <= now
        {
            return Ok(ProcessingArtifactLeaseOutcome::Expired);
        }
        let lease_id = format!("lease-{}", random_uuid_v4()?);
        let record = ProcessingArtifactLeaseRecord {
            lease_id: lease_id.clone(),
            artifact_id: artifact_id.to_owned(),
            created_at: now,
        };
        write_metadata_value(
            transaction,
            &processing_artifact_lease_key(&lease_id),
            &serialize_record(&record)?,
        )?;
        Ok(ProcessingArtifactLeaseOutcome::Acquired {
            lease_id,
            artifact: Box::new(artifact),
        })
    })
}

/// Refreshes one download lease's liveness anchor; `false` means the lease
/// is gone and the stream must stop renewing.
pub(in crate::persistence) fn renew_processing_artifact_lease(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    lease_id: &str,
    now: u64,
) -> Result<bool, PersistenceError> {
    write_transaction(state, database_name, connection, |transaction| {
        let Some(mut record) = read_processing_artifact_lease_record(transaction, lease_id)? else {
            return Ok(false);
        };
        record.created_at = now;
        write_metadata_value(
            transaction,
            &processing_artifact_lease_key(lease_id),
            &serialize_record(&record)?,
        )?;
        Ok(true)
    })
}

/// Releases one download lease once its stream has settled; `false` means
/// the lease was already gone.
pub(in crate::persistence) fn release_processing_artifact_lease(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    lease_id: &str,
) -> Result<bool, PersistenceError> {
    write_transaction(state, database_name, connection, |transaction| {
        let changed = transaction
            .execute(
                "DELETE FROM library_metadata WHERE key=?",
                [processing_artifact_lease_key(lease_id)],
            )
            .map_err(|_| PersistenceError::Storage)?;
        Ok(changed == 1)
    })
}
