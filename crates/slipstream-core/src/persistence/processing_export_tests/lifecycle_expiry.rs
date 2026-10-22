use super::lifecycle::{admitted, settled_artifact};
use super::*;
/// Writes one workspace publication row directly, so a test can observe the
/// shared expiry decision against a retained publication.
fn retain_in_workspace(seeded: &Seeded, artifact_id: &str, published_at: u64) {
    let mut connection = Connection::open(&seeded.path).unwrap();
    let transaction = connection.transaction().unwrap();
    write_metadata_value(
        &transaction,
        &processing_workspace_artifact_key(artifact_id),
        &published_at.to_string(),
    )
    .unwrap();
    transaction.commit().unwrap();
}

/// The shared expiry predicate: an artifact with no retention, a retention
/// deadline still ahead, or a retained workspace publication is not
/// expired; only a passed deadline with no publication is.
#[tokio::test]
async fn artifact_retention_expired_requires_a_passed_deadline_and_no_publication() {
    let seeded = seeded();
    let admission = admitted(&seeded, "export-predicate").await;
    let artifact = settled_artifact(&admission);
    let artifact_id = artifact.artifact_id.as_str().to_owned();
    let retain_until = 2_000 + PROCESSING_ARTIFACT_RETENTION_SECONDS;
    let connection = Connection::open(&seeded.path).unwrap();

    // No retention record was ever installed: not expired.
    assert!(!artifact_retention_expired(&connection, &artifact_id, 2_000).unwrap());
    assert!(!artifact_retention_expired(&connection, &artifact_id, retain_until + 1).unwrap());

    settle_at(
        &seeded,
        artifact.clone(),
        "export-predicate",
        &admission.payload_digest,
        2_000,
    )
    .await;

    // Before the deadline passes, and exactly on it, the artifact is live.
    assert!(!artifact_retention_expired(&connection, &artifact_id, retain_until - 1).unwrap());
    assert!(artifact_retention_expired(&connection, &artifact_id, retain_until).unwrap());
    assert!(artifact_retention_expired(&connection, &artifact_id, retain_until + 1).unwrap());

    // One durable workspace publication reprieves it.
    retain_in_workspace(&seeded, &artifact_id, 1_000);
    assert!(!artifact_retention_expired(&connection, &artifact_id, retain_until + 1).unwrap());
}

/// The shared expiry predicate drives the input handoff: a passed retention
/// refuses an artifact-input submission, and a retained workspace
/// publication keeps the same submission admitted past the deadline.
#[tokio::test]
async fn artifact_input_handoff_follows_the_shared_expiry_decision() {
    let seeded = seeded();
    // Install the upstream artifact's finite retention at 1_000 through a
    // qualified admission whose settlement publishes the upstream artifact
    // itself. The upstream admission is settled before the artifact-input
    // recipe is saved, so the current recipe stays the one the handoff reads.
    let upstream_admission = admitted(&seeded, "export-upstream-expiry").await;
    let upstream = settled_artifact(&upstream_admission);
    settle_at(
        &seeded,
        upstream.clone(),
        "export-upstream-expiry",
        &upstream_admission.payload_digest,
        1_000,
    )
    .await;
    let input = ProcessingInput::Artifact {
        artifact_id: upstream.artifact_id.clone(),
        contract: upstream.output_contract.clone(),
    };
    let recipe_revision = save_recipe(
        &seeded,
        "recipe-expired-input",
        Some(&upstream_admission.recipe_revision),
        input,
    )
    .await;

    // Before the upstream retention deadline the handoff is admitted; the
    // admission is a plain refusal (no qualified adapter), so the request
    // identity holds no work. Each submission below uses a fresh request
    // identity so the handoff guard runs again instead of replaying the
    // committed refusal.
    assert!(matches!(
        submit(
            &seeded,
            submission(
                &seeded,
                "export-expired-live",
                "develop-1",
                &recipe_revision
            )
        )
        .await,
        ProcessingExportSubmitOutcome::Refused(_)
    ));

    // The retention installed at 1_000 refuses the handoff once it passed.
    let retain_until = 1_000 + PROCESSING_ARTIFACT_RETENTION_SECONDS;
    assert_eq!(
        submit_at(
            &seeded,
            submission(
                &seeded,
                "export-expired-refused",
                "develop-1",
                &recipe_revision
            ),
            retain_until
        )
        .await,
        ProcessingExportSubmitOutcome::IncompatibleInput(
            ProcessingInputHandoffError::ArtifactMissing
        )
    );

    // A retained workspace publication keeps the same artifact input live
    // past the deadline, so the same submission is refused for another reason.
    retain_in_workspace(&seeded, upstream.artifact_id.as_str(), 1_000);
    let ProcessingExportSubmitOutcome::Refused(_) = submit_at(
        &seeded,
        submission(
            &seeded,
            "export-expired-reprieved",
            "develop-1",
            &recipe_revision,
        ),
        retain_until + 1,
    )
    .await
    else {
        panic!("a retained workspace publication keeps the artifact input admitted");
    };
}
