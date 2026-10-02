use super::*;
#[tokio::test]
async fn qualified_decision_must_name_the_stored_parameter_schema() {
    let seeded = seeded();
    let recipe_revision = save_recipe(
        &seeded,
        "recipe-schema",
        None,
        original_step_input("raw-photo", &seeded.source),
    )
    .await;
    let mut mismatched =
        qualified_submission(&seeded, "export-schema", "develop-1", &recipe_revision);
    mismatched.adapter = ProcessingExportAdapterDecision::Qualified {
        adapter_version: "darktable-adapter-1".to_owned(),
        parameter_schema_version: "darktable-params-2".to_owned(),
    };
    assert!(matches!(
        seeded
            .persistence
            .submit_processing_export_receiver(mismatched, 1_000)
            .unwrap()
            .await
            .unwrap(),
        Err(PersistenceError::Storage)
    ));
    assert_eq!(acceptance_record_count(&seeded), 0);
}

/// One qualified submission admitted under the seeded recipe.
async fn admitted(seeded: &Seeded, request_id: &str) -> ProcessingExportAdmission {
    let current = seeded
        .persistence
        .composable_edit_recipe_receiver("raw-photo")
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    let recipe_revision = save_recipe(
        seeded,
        &format!("recipe-{request_id}"),
        current.as_ref().map(|recipe| recipe.revision.as_str()),
        original_step_input("raw-photo", &seeded.source),
    )
    .await;
    let ProcessingExportSubmitOutcome::Admitted(admission) = submit(
        seeded,
        qualified_submission(seeded, request_id, "develop-1", &recipe_revision),
    )
    .await
    else {
        panic!("qualified submission admits {request_id}");
    };
    admission
}

/// The artifact a validated execution of one admission publishes.
fn settled_artifact(admission: &ProcessingExportAdmission) -> ProcessingArtifact {
    let mut artifact = artifact();
    artifact.input.input = admission.input.clone();
    artifact.bundle_id = admission.bundle_id.clone();
    artifact.adapter_schema_version = admission.adapter_schema_version.clone();
    artifact.parameters = admission.parameters.clone();
    artifact
}

fn replay_of(seeded: &Seeded, admission: &ProcessingExportAdmission) -> SubmitProcessingExport {
    qualified_submission(
        seeded,
        &admission.request_id,
        "develop-1",
        &admission.recipe_revision,
    )
}

#[tokio::test]
async fn duplicate_live_submission_returns_pending_without_second_work() {
    let seeded = seeded();
    let admission = admitted(&seeded, "export-pending").await;
    // The duplicate of a still-live accepted request replays the
    // committed admission and starts no second execution.
    let ProcessingExportSubmitOutcome::Pending(pending) =
        submit(&seeded, replay_of(&seeded, &admission)).await
    else {
        panic!("a duplicate live request resolves to Pending");
    };
    assert_eq!(pending, admission);
    let ProcessingExportAttemptOutcome::Began(began) = seeded
        .persistence
        .begin_processing_export_attempt_receiver("export-pending", 1_500)
        .unwrap()
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("an accepted execution begins its first attempt");
    };
    assert_eq!(began.state, ProcessingExportWorkState::Executing);
    assert_eq!(
        began.attempt,
        Some(ProcessingExportAttempt {
            sequence: 1,
            began_at: 1_500
        })
    );
    // The duplicate of an executing request still resolves to Pending.
    let ProcessingExportSubmitOutcome::Pending(still) =
        submit(&seeded, replay_of(&seeded, &admission)).await
    else {
        panic!("a duplicate executing request resolves to Pending");
    };
    assert_eq!(still, admission);
    let unfinished = seeded
        .persistence
        .unfinished_processing_exports_receiver()
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unfinished.len(), 1);
    assert_eq!(unfinished[0].admission.request_id, "export-pending");
    assert_eq!(unfinished[0].state, ProcessingExportWorkState::Executing);
}

#[tokio::test]
async fn terminal_decisions_are_first_wins_and_replay() {
    let seeded = seeded();
    let admission = admitted(&seeded, "export-fail").await;
    seeded
        .persistence
        .begin_processing_export_attempt_receiver("export-fail", 1_500)
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    let ProcessingExportFailureOutcome::Failed(failed) = seeded
        .persistence
        .fail_processing_export_receiver("export-fail", "adapter_execution_error", 1_600)
        .unwrap()
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("an executing request records its terminal failure");
    };
    assert_eq!(failed.state, ProcessingExportWorkState::Failed);
    assert_eq!(
        failed.failure_reason.as_deref(),
        Some("adapter_execution_error")
    );
    assert_eq!(failed.terminal_at, Some(1_600));
    assert_eq!(
        failed.retain_until,
        Some(1_600 + PROCESSING_EXPORT_RECEIPT_RETENTION_SECONDS)
    );
    assert_eq!(failed.attempt.map(|attempt| attempt.sequence), Some(1));
    // Every later decision loses to the recorded failure.
    assert!(matches!(
        seeded
            .persistence
            .cancel_processing_export_receiver("export-fail", 1_700)
            .unwrap()
            .await
            .unwrap()
            .unwrap(),
        ProcessingExportCancelOutcome::Terminal(_)
    ));
    assert!(matches!(
        seeded
            .persistence
            .begin_processing_export_attempt_receiver("export-fail", 1_700)
            .unwrap()
            .await
            .unwrap()
            .unwrap(),
        ProcessingExportAttemptOutcome::Terminal(_)
    ));
    assert!(matches!(
        settle(
            &seeded,
            settled_artifact(&admission),
            "export-fail",
            &admission.payload_digest
        )
        .await,
        ProcessingExportSettlement::Terminal(_)
    ));
    let ProcessingExportSubmitOutcome::FailureReplayed(replayed) =
        submit(&seeded, replay_of(&seeded, &admission)).await
    else {
        panic!("a failed request replays its failure");
    };
    assert_eq!(replayed, failed);
    // An unbounded reason code is refused before any record changes.
    assert!(matches!(
        seeded
            .persistence
            .fail_processing_export_receiver("export-fail", " reason", 1_800)
            .unwrap()
            .await
            .unwrap()
            .unwrap(),
        ProcessingExportFailureOutcome::Invalid(_)
    ));

    // A cancellation recorded before settlement wins the same way.
    let admission = admitted(&seeded, "export-cancel").await;
    let ProcessingExportCancelOutcome::Cancelled(cancelled) = seeded
        .persistence
        .cancel_processing_export_receiver("export-cancel", 1_600)
        .unwrap()
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("an accepted request cancels");
    };
    assert_eq!(cancelled.state, ProcessingExportWorkState::Cancelled);
    assert_eq!(cancelled.terminal_at, Some(1_600));
    assert_eq!(
        cancelled.retain_until,
        Some(1_600 + PROCESSING_EXPORT_RECEIPT_RETENTION_SECONDS)
    );
    assert!(matches!(
        seeded
            .persistence
            .fail_processing_export_receiver("export-cancel", "adapter_execution_error", 1_700)
            .unwrap()
            .await
            .unwrap()
            .unwrap(),
        ProcessingExportFailureOutcome::Terminal(_)
    ));
    let ProcessingExportSubmitOutcome::CancelledReplayed(replayed) =
        submit(&seeded, replay_of(&seeded, &admission)).await
    else {
        panic!("a cancelled request replays its cancellation");
    };
    assert_eq!(replayed, cancelled);

    // Unknown request identities resolve to Missing.
    assert!(matches!(
        seeded
            .persistence
            .begin_processing_export_attempt_receiver("export-none", 1_000)
            .unwrap()
            .await
            .unwrap()
            .unwrap(),
        ProcessingExportAttemptOutcome::Missing
    ));
    assert!(matches!(
        seeded
            .persistence
            .fail_processing_export_receiver("export-none", "adapter_execution_error", 1_000)
            .unwrap()
            .await
            .unwrap()
            .unwrap(),
        ProcessingExportFailureOutcome::Missing
    ));
    assert!(matches!(
        seeded
            .persistence
            .cancel_processing_export_receiver("export-none", 1_000)
            .unwrap()
            .await
            .unwrap()
            .unwrap(),
        ProcessingExportCancelOutcome::Missing
    ));
    assert!(matches!(
        settle(
            &seeded,
            settled_artifact(&admission),
            "export-none",
            &admission.payload_digest
        )
        .await,
        ProcessingExportSettlement::Missing
    ));
}

#[tokio::test]
async fn expired_requests_never_reexecute() {
    let seeded = seeded();
    let admission = admitted(&seeded, "export-expired").await;
    let artifact = settled_artifact(&admission);
    let ProcessingExportSettlement::Settled(work) = settle_at(
        &seeded,
        artifact.clone(),
        "export-expired",
        &admission.payload_digest,
        2_000,
    )
    .await
    else {
        panic!("the accepted execution settles");
    };
    let retain_until = 2_000 + PROCESSING_ARTIFACT_RETENTION_SECONDS;
    assert_eq!(work.retain_until, Some(retain_until));
    // Before the window passes nothing is removed.
    let before = seeded
        .persistence
        .sweep_processing_export_expiry_receiver(retain_until - 1)
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert!(before.is_empty());
    let expired = seeded
        .persistence
        .sweep_processing_export_expiry_receiver(retain_until)
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(expired, vec![artifact.artifact_id.as_str().to_owned()]);
    assert_eq!(
        read_artifact(&seeded, artifact.artifact_id.as_str()).await,
        None
    );
    assert!(
        seeded
            .persistence
            .read_processing_export_work_receiver("export-expired")
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .is_none()
    );
    assert_eq!(acceptance_record_count(&seeded), 0);
    assert!(matches!(
        submit(&seeded, replay_of(&seeded, &admission)).await,
        ProcessingExportSubmitOutcome::Expired
    ));
}

#[tokio::test]
async fn artifact_leases_are_finite_and_hold_against_the_sweep() {
    let seeded = seeded();
    let admission = admitted(&seeded, "export-lease").await;
    let artifact = settled_artifact(&admission);
    settle_at(
        &seeded,
        artifact.clone(),
        "export-lease",
        &admission.payload_digest,
        2_000,
    )
    .await;
    let retain_until = 2_000 + PROCESSING_ARTIFACT_RETENTION_SECONDS;
    // A fresh lease holds the artifact past its retention deadline.
    let ProcessingArtifactLeaseOutcome::Acquired {
        lease_id,
        artifact: leased,
    } = seeded
        .persistence
        .acquire_processing_artifact_lease_receiver(artifact.artifact_id.as_str(), retain_until - 1)
        .unwrap()
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("a retained artifact leases for download");
    };
    assert_eq!(*leased, artifact);
    let held = seeded
        .persistence
        .sweep_processing_export_expiry_receiver(retain_until)
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert!(held.is_empty());
    assert!(
        read_artifact(&seeded, artifact.artifact_id.as_str())
            .await
            .is_some()
    );
    // A lease is never granted past the retention deadline.
    assert!(matches!(
        seeded
            .persistence
            .acquire_processing_artifact_lease_receiver(artifact.artifact_id.as_str(), retain_until)
            .unwrap()
            .await
            .unwrap()
            .unwrap(),
        ProcessingArtifactLeaseOutcome::Expired
    ));
    // Renewal refreshes the liveness anchor; release frees the hold.
    assert!(
        seeded
            .persistence
            .renew_processing_artifact_lease_receiver(&lease_id, retain_until)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
    );
    assert!(
        seeded
            .persistence
            .release_processing_artifact_lease_receiver(&lease_id)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
    );
    assert!(
        !seeded
            .persistence
            .renew_processing_artifact_lease_receiver(&lease_id, retain_until)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
    );
    let expired = seeded
        .persistence
        .sweep_processing_export_expiry_receiver(retain_until)
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(expired, vec![artifact.artifact_id.as_str().to_owned()]);
    // An unknown artifact identity never leases.
    assert!(matches!(
        seeded
            .persistence
            .acquire_processing_artifact_lease_receiver("artifact-none", 1_000)
            .unwrap()
            .await
            .unwrap()
            .unwrap(),
        ProcessingArtifactLeaseOutcome::Unknown
    ));
}

#[tokio::test]
async fn expired_terminal_receipts_are_swept_and_tombstoned() {
    let seeded = seeded();
    let failed_admission = admitted(&seeded, "export-fail-expired").await;
    seeded
        .persistence
        .begin_processing_export_attempt_receiver("export-fail-expired", 1_500)
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    seeded
        .persistence
        .fail_processing_export_receiver("export-fail-expired", "adapter_execution_error", 1_600)
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    let cancelled_admission = admitted(&seeded, "export-cancel-expired").await;
    seeded
        .persistence
        .cancel_processing_export_receiver("export-cancel-expired", 1_700)
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    let deadline = 1_700 + PROCESSING_EXPORT_RECEIPT_RETENTION_SECONDS;
    // Before the window passes both receipts still replay.
    let early = seeded
        .persistence
        .sweep_processing_export_expiry_receiver(
            1_600 + PROCESSING_EXPORT_RECEIPT_RETENTION_SECONDS - 1,
        )
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert!(early.is_empty());
    assert!(
        seeded
            .persistence
            .read_processing_export_work_receiver("export-fail-expired")
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .is_some()
    );
    // Past the window both terminal receipts are removed, their request
    // identities are tombstoned, and neither can start new work.
    let swept = seeded
        .persistence
        .sweep_processing_export_expiry_receiver(deadline)
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert!(swept.is_empty());
    assert!(
        seeded
            .persistence
            .read_processing_export_work_receiver("export-fail-expired")
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .is_none()
    );
    assert!(
        seeded
            .persistence
            .read_processing_export_work_receiver("export-cancel-expired")
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        submit(&seeded, replay_of(&seeded, &failed_admission)).await,
        ProcessingExportSubmitOutcome::Expired
    ));
    assert!(matches!(
        submit(&seeded, replay_of(&seeded, &cancelled_admission)).await,
        ProcessingExportSubmitOutcome::Expired
    ));
}
