use super::*;
use crate::persistence::Persistence;
use crate::persistence::test_support::*;
use crate::processing::{
    ComposableEditRecipe, ComposableEditRecipeWriteOutcome, ProcessingStep,
    SaveComposableEditRecipe,
};
use crate::source_revision;
use std::path::PathBuf;

struct Seeded {
    _tree: TempTree,
    _root: crate::LibraryRoot,
    persistence: Persistence,
    path: PathBuf,
    source: String,
}

/// One seeded RAW Photo with a published source revision behind the
/// real serialized owner, mirroring the composable-recipe fixture.
fn seeded() -> Seeded {
    let (tree, library, state, name, path) = fixture();
    seed(
        &path,
        include_str!("../../../../compatibility/sqlite/schema-v6.sql"),
    );
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
            [library.canonical_path().to_str().unwrap()],
        )
        .unwrap();
    add_recipe_test_photo(
        &connection,
        RecipeTestPhoto {
            original_id: "raw-original",
            photo_id: "raw-photo",
            relative_path: "shoot/one.ARW",
            kind: "raw",
            available: true,
            size: 17,
            mtime_ms: 1_000.0,
        },
    );
    let source = source_revision("shoot/one.ARW", 17, 1_000.0).unwrap();
    connection
            .execute(
                "UPDATE original_files SET capture_metadata_state='missing',capture_source_revision=? WHERE id='raw-original'",
                [format!("{}\0fixture-device\0fixture-inode", source)],
            )
            .unwrap();
    drop(connection);
    let persistence = Persistence::open(
        state,
        name,
        library.canonical_path().to_string_lossy().into_owned(),
    )
    .unwrap();
    Seeded {
        _tree: tree,
        _root: library,
        persistence,
        path,
        source,
    }
}

fn original_step_input(photo_id: &str, source_revision: &str) -> ProcessingInput {
    ProcessingInput::Original {
        photo_id: photo_id.to_owned(),
        source_revision: source_revision.to_owned(),
    }
}

fn artifact() -> ProcessingArtifact {
    ProcessingArtifact {
        artifact_id: ProcessingArtifactId::new("artifact-a1").unwrap(),
        photo_id: "raw-photo".to_owned(),
        step_id: ProcessingStepId::new("develop-1").unwrap(),
        module: ProcessingModuleId::new("darktable").unwrap(),
        adapter_schema_version: "darktable-adapter-1".to_owned(),
        parameters: ProcessingParameterSnapshot::new(
            "darktable-params-1",
            serde_json::json!({"stack": []}),
        )
        .unwrap(),
        input: ProcessingInputEvidence::new(
            original_step_input("raw-photo", "fixture-source"),
            &"a".repeat(64),
            4096,
        )
        .unwrap(),
        output_contract: ProcessingImageContract {
            format: "image/tiff".to_owned(),
            precision: "uint16".to_owned(),
            color_space: "prophoto-rgb".to_owned(),
            transfer: "linear".to_owned(),
            geometry: ProcessingGeometry::new(64, 48).unwrap(),
            encoding: "deflate".to_owned(),
        },
        bundle_id: "b".repeat(64),
        sha256: "c".repeat(64),
        byte_length: 12_288,
    }
}

fn no_adapter_decision() -> ProcessingExportAdapterDecision {
    ProcessingExportAdapterDecision::NoQualifiedAdapter {
        reason_code: "module_parameters_unavailable".to_owned(),
    }
}

async fn publish(seeded: &Seeded, artifact: ProcessingArtifact) -> ProcessingArtifactPublication {
    seeded
        .persistence
        .publish_processing_artifact_receiver(artifact)
        .unwrap()
        .await
        .unwrap()
        .unwrap()
}

async fn read_artifact(seeded: &Seeded, artifact_id: &str) -> Option<ProcessingArtifact> {
    seeded
        .persistence
        .read_processing_artifact_receiver(artifact_id)
        .unwrap()
        .await
        .unwrap()
        .unwrap()
}

async fn submit(
    seeded: &Seeded,
    mutation: SubmitProcessingExport,
) -> ProcessingExportSubmitOutcome {
    submit_at(seeded, mutation, 1_000).await
}

async fn submit_at(
    seeded: &Seeded,
    mutation: SubmitProcessingExport,
    now: u64,
) -> ProcessingExportSubmitOutcome {
    seeded
        .persistence
        .submit_processing_export_receiver(mutation, now)
        .unwrap()
        .await
        .unwrap()
        .unwrap()
}

async fn save_recipe(
    seeded: &Seeded,
    request_id: &str,
    expected: Option<&str>,
    input: ProcessingInput,
) -> String {
    let recipe = ComposableEditRecipe {
        photo_id: "raw-photo".to_owned(),
        revision: "caller-snapshot".to_owned(),
        source_revision: seeded.source.clone(),
        current_step_id: Some(ProcessingStepId::new("develop-1").unwrap()),
        steps: vec![ProcessingStep {
            step_id: ProcessingStepId::new("develop-1").unwrap(),
            module: ProcessingModuleId::new("darktable").unwrap(),
            input,
            parameters: ProcessingParameterSnapshot::new(
                "darktable-params-1",
                serde_json::json!({"stack": [], "output": {"format": "tiff", "precisionBits": 32, "colorSpace": "prophoto-rgb", "transferFunction": "linear"}}),
            )
            .unwrap(),
        }],
    };
    let mutation = SaveComposableEditRecipe {
        photo_id: "raw-photo".to_owned(),
        request_id: request_id.to_owned(),
        expected_recipe_revision: expected.map(str::to_owned),
        expected_source_revision: seeded.source.clone(),
        recipe,
    };
    let outcome = seeded
        .persistence
        .save_composable_edit_recipe_receiver(mutation)
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    match outcome {
        ComposableEditRecipeWriteOutcome::Saved(committed)
        | ComposableEditRecipeWriteOutcome::Unchanged(committed) => committed.revision,
        other => panic!("recipe save should commit: {other:?}"),
    }
}

#[tokio::test]
async fn artifact_bound_submission_survives_unavailable_original() {
    let seeded = seeded();
    let upstream = artifact();
    publish(&seeded, upstream.clone()).await;
    let recipe = ComposableEditRecipe {
        photo_id: "raw-photo".to_owned(),
        revision: "artifact-recipe".to_owned(),
        source_revision: seeded.source.clone(),
        current_step_id: Some(ProcessingStepId::new("film-1").unwrap()),
        steps: vec![ProcessingStep {
            step_id: ProcessingStepId::new("film-1").unwrap(),
            module: ProcessingModuleId::new("spektrafilm").unwrap(),
            input: ProcessingInput::Artifact {
                artifact_id: upstream.artifact_id.clone(),
                contract: upstream.output_contract.clone(),
            },
            parameters: ProcessingParameterSnapshot::new(
                "spektrafilm-params-1",
                serde_json::json!({}),
            )
            .unwrap(),
        }],
    };
    let saved = seeded
        .persistence
        .save_composable_edit_recipe_receiver(SaveComposableEditRecipe {
            photo_id: "raw-photo".to_owned(),
            request_id: "artifact-recipe-request".to_owned(),
            expected_recipe_revision: None,
            expected_source_revision: seeded.source.clone(),
            recipe,
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    let ComposableEditRecipeWriteOutcome::Saved(saved) = saved else {
        panic!("artifact recipe should save while its Original is available");
    };
    Connection::open(&seeded.path)
        .unwrap()
        .execute(
            "UPDATE original_files SET available=0 WHERE id='raw-original'",
            [],
        )
        .unwrap();
    Connection::open(&seeded.path)
        .unwrap()
        .execute("UPDATE photos SET available=0 WHERE id='raw-photo'", [])
        .unwrap();
    let outcome = submit(
        &seeded,
        SubmitProcessingExport {
            photo_id: "raw-photo".to_owned(),
            request_id: "artifact-export".to_owned(),
            step_id: ProcessingStepId::new("film-1").unwrap(),
            expected_recipe_revision: saved.revision,
            expected_source_revision: seeded.source.clone(),
            bundle_id: "b".repeat(64),
            retained_output_bytes_max: u64::MAX,
            adapter: no_adapter_decision(),
        },
    )
    .await;
    assert!(matches!(outcome, ProcessingExportSubmitOutcome::Refused(_)));
}

#[tokio::test]
async fn qualified_submission_refuses_when_retained_output_is_full() {
    let seeded = seeded();
    let upstream = artifact();
    publish(&seeded, upstream.clone()).await;
    let recipe = ComposableEditRecipe {
        photo_id: "raw-photo".to_owned(),
        revision: "retained-full-recipe".to_owned(),
        source_revision: seeded.source.clone(),
        current_step_id: Some(ProcessingStepId::new("film-1").unwrap()),
        steps: vec![ProcessingStep {
            step_id: ProcessingStepId::new("film-1").unwrap(),
            module: ProcessingModuleId::new("spektrafilm").unwrap(),
            input: ProcessingInput::Artifact {
                artifact_id: upstream.artifact_id.clone(),
                contract: upstream.output_contract.clone(),
            },
            parameters: ProcessingParameterSnapshot::new(
                "spektrafilm-params-1",
                serde_json::json!({}),
            )
            .unwrap(),
        }],
    };
    let saved = seeded
        .persistence
        .save_composable_edit_recipe_receiver(SaveComposableEditRecipe {
            photo_id: "raw-photo".to_owned(),
            request_id: "retained-full-recipe-request".to_owned(),
            expected_recipe_revision: None,
            expected_source_revision: seeded.source.clone(),
            recipe,
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    let ComposableEditRecipeWriteOutcome::Saved(saved) = saved else {
        panic!("retained-output recipe should save");
    };
    let outcome = submit(
        &seeded,
        SubmitProcessingExport {
            photo_id: "raw-photo".to_owned(),
            request_id: "retained-full-export".to_owned(),
            step_id: ProcessingStepId::new("film-1").unwrap(),
            expected_recipe_revision: saved.revision,
            expected_source_revision: seeded.source.clone(),
            bundle_id: "b".repeat(64),
            retained_output_bytes_max: 0,
            adapter: ProcessingExportAdapterDecision::Qualified {
                adapter_version: "spektrafilm-adapter-1".to_owned(),
                parameter_schema_version: "spektrafilm-params-1".to_owned(),
            },
        },
    )
    .await;
    assert_eq!(outcome, ProcessingExportSubmitOutcome::RetainedOutputFull);
    assert_eq!(acceptance_record_count(&seeded), 0);
}

fn submission(
    seeded: &Seeded,
    request_id: &str,
    step_id: &str,
    recipe_revision: &str,
) -> SubmitProcessingExport {
    SubmitProcessingExport {
        photo_id: "raw-photo".to_owned(),
        request_id: request_id.to_owned(),
        step_id: ProcessingStepId::new(step_id).unwrap(),
        expected_recipe_revision: recipe_revision.to_owned(),
        expected_source_revision: seeded.source.clone(),
        bundle_id: "b".repeat(64),
        retained_output_bytes_max: u64::MAX,
        adapter: no_adapter_decision(),
    }
}

fn refusal_record_count(seeded: &Seeded) -> i64 {
    Connection::open(&seeded.path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM library_metadata WHERE key LIKE 'processing_export_refusal:%'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

fn acceptance_record_count(seeded: &Seeded) -> i64 {
    Connection::open(&seeded.path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM library_metadata WHERE key LIKE 'processing_export_acceptance:%'",
            [],
            |row| row.get(0),
        )
        .unwrap()
}

#[tokio::test]
async fn artifact_publication_is_insert_only_and_replays_identically() {
    let seeded = seeded();
    let first = artifact();
    assert_eq!(
        publish(&seeded, first.clone()).await,
        ProcessingArtifactPublication::Published
    );
    assert_eq!(
        publish(&seeded, first.clone()).await,
        ProcessingArtifactPublication::Replayed
    );
    let mut different = first.clone();
    different.sha256 = "d".repeat(64);
    assert_eq!(
        publish(&seeded, different).await,
        ProcessingArtifactPublication::IdentityConflict
    );
    assert_eq!(read_artifact(&seeded, "artifact-a1").await, Some(first));
    assert_eq!(read_artifact(&seeded, "artifact-none").await, None);
}

#[tokio::test]
async fn malformed_artifact_record_is_a_storage_error() {
    let seeded = seeded();
    let first = artifact();
    publish(&seeded, first).await;
    Connection::open(&seeded.path)
        .unwrap()
        .execute(
            "UPDATE library_metadata SET value=? WHERE key=?",
            params![
                serde_json::json!({"artifact_id": "artifact-a1"}).to_string(),
                processing_artifact_key("artifact-a1")
            ],
        )
        .unwrap();
    let refused = seeded
        .persistence
        .read_processing_artifact_receiver("artifact-a1")
        .unwrap()
        .await
        .unwrap();
    assert!(matches!(refused, Err(PersistenceError::Storage)));
}

#[tokio::test]
async fn refusal_replays_by_request_identity_and_conflicts_on_other_payloads() {
    let seeded = seeded();
    let recipe_revision = save_recipe(
        &seeded,
        "recipe-1",
        None,
        original_step_input("raw-photo", &seeded.source),
    )
    .await;
    let mutation = submission(&seeded, "export-request-1", "develop-1", &recipe_revision);
    let ProcessingExportSubmitOutcome::Refused(refused) = submit(&seeded, mutation.clone()).await
    else {
        panic!("first submission records the explicit refusal");
    };
    assert_eq!(refused.photo_id, "raw-photo");
    assert_eq!(refused.recipe_revision, recipe_revision);
    assert_eq!(refused.source_revision, seeded.source);
    assert_eq!(refused.module.as_str(), "darktable");
    assert_eq!(refused.reason_code, "module_parameters_unavailable");
    assert_eq!(refused.bundle_id, "b".repeat(64));
    assert_eq!(
        refused.input,
        original_step_input("raw-photo", &seeded.source)
    );
    crate::processing::validate_digest(&refused.parameter_digest).unwrap();
    let ProcessingExportSubmitOutcome::Replayed(replayed) = submit(&seeded, mutation.clone()).await
    else {
        panic!("same identity and payload replays the refusal");
    };
    assert_eq!(replayed, refused);
    let mut other = mutation;
    other.step_id = ProcessingStepId::new("develop-2").unwrap();
    assert!(matches!(
        submit(&seeded, other).await,
        ProcessingExportSubmitOutcome::RequestConflict
    ));
}

#[tokio::test]
async fn submission_guards_recipe_revision_source_and_current_step() {
    let seeded = seeded();
    let recipe_revision = save_recipe(
        &seeded,
        "recipe-1",
        None,
        original_step_input("raw-photo", &seeded.source),
    )
    .await;
    let mut stale_source = submission(&seeded, "r-source", "develop-1", &recipe_revision);
    stale_source.expected_source_revision = "stale-source".to_owned();
    assert!(matches!(
        submit(&seeded, stale_source).await,
        ProcessingExportSubmitOutcome::SourceChanged(Some(_))
    ));
    assert!(matches!(
        submit(
            &seeded,
            submission(&seeded, "r-recipe", "develop-1", "stale-revision")
        )
        .await,
        ProcessingExportSubmitOutcome::RecipeConflict(Some(_))
    ));
    assert!(matches!(
        submit(
            &seeded,
            submission(&seeded, "r-step", "not-current", &recipe_revision)
        )
        .await,
        ProcessingExportSubmitOutcome::StepNotCurrent(Some(_))
    ));
    assert!(matches!(
        submit(
            &seeded,
            submission(&seeded, "r-ok", "develop-1", &recipe_revision)
        )
        .await,
        ProcessingExportSubmitOutcome::Refused(_)
    ));
}

#[tokio::test]
async fn submission_without_a_recipe_or_photo_refuses_before_any_record() {
    let seeded = seeded();
    assert!(matches!(
        submit(
            &seeded,
            submission(&seeded, "r-no-recipe", "develop-1", "missing")
        )
        .await,
        ProcessingExportSubmitOutcome::MissingRecipe
    ));
    let mut unknown_photo = submission(&seeded, "r-no-photo", "develop-1", "missing");
    unknown_photo.photo_id = "unknown-photo".to_owned();
    assert!(matches!(
        submit(&seeded, unknown_photo).await,
        ProcessingExportSubmitOutcome::MissingPhoto
    ));
    let mut invalid = submission(&seeded, "r-invalid", "develop-1", "missing");
    invalid.adapter = ProcessingExportAdapterDecision::NoQualifiedAdapter {
        reason_code: " ".to_owned(),
    };
    assert!(matches!(
        submit(&seeded, invalid).await,
        ProcessingExportSubmitOutcome::Invalid(_)
    ));
    assert_eq!(refusal_record_count(&seeded), 0);
}

#[tokio::test]
async fn artifact_input_handoff_requires_the_published_contract() {
    let seeded = seeded();
    let published = artifact();
    publish(&seeded, published.clone()).await;
    let mut mismatched = published.output_contract.clone();
    mismatched.precision = "float32".to_owned();
    let mut expected_revision: Option<String> = None;
    for (name, artifact_id, contract, expected) in [
        (
            "missing",
            ProcessingArtifactId::new("artifact-none").unwrap(),
            published.output_contract.clone(),
            ProcessingInputHandoffError::ArtifactMissing,
        ),
        (
            "mismatched",
            published.artifact_id.clone(),
            mismatched,
            ProcessingInputHandoffError::ArtifactContractMismatch,
        ),
    ] {
        let recipe_revision = save_recipe(
            &seeded,
            &format!("recipe-{name}"),
            expected_revision.as_deref(),
            ProcessingInput::Artifact {
                artifact_id,
                contract,
            },
        )
        .await;
        expected_revision = Some(recipe_revision.clone());
        assert_eq!(
            submit(
                &seeded,
                submission(
                    &seeded,
                    &format!("export-{name}"),
                    "develop-1",
                    &recipe_revision
                )
            )
            .await,
            ProcessingExportSubmitOutcome::IncompatibleInput(expected)
        );
    }
}

#[tokio::test]
async fn artifact_handoff_accepts_the_exact_published_contract() {
    let seeded = seeded();
    let published = artifact();
    publish(&seeded, published.clone()).await;
    let recipe_revision = save_recipe(
        &seeded,
        "recipe-artifact",
        None,
        ProcessingInput::Artifact {
            artifact_id: published.artifact_id.clone(),
            contract: published.output_contract.clone(),
        },
    )
    .await;
    let ProcessingExportSubmitOutcome::Refused(refused) = submit(
        &seeded,
        submission(&seeded, "export-artifact", "develop-1", &recipe_revision),
    )
    .await
    else {
        panic!("a compatible artifact input is admitted up to the adapter refusal");
    };
    assert_eq!(
        refused.input,
        ProcessingInput::Artifact {
            artifact_id: published.artifact_id.clone(),
            contract: published.output_contract.clone(),
        }
    );
    assert_eq!(
        refused.parameter_schema_version,
        published.parameters.schema_version
    );
}

fn qualified_submission(
    seeded: &Seeded,
    request_id: &str,
    step_id: &str,
    recipe_revision: &str,
) -> SubmitProcessingExport {
    SubmitProcessingExport {
        adapter: ProcessingExportAdapterDecision::Qualified {
            adapter_version: "darktable-adapter-1".to_owned(),
            parameter_schema_version: "darktable-params-1".to_owned(),
        },
        ..submission(seeded, request_id, step_id, recipe_revision)
    }
}

async fn settle(
    seeded: &Seeded,
    artifact: ProcessingArtifact,
    request_id: &str,
    payload_digest: &str,
) -> ProcessingExportSettlement {
    settle_at(seeded, artifact, request_id, payload_digest, 2_000).await
}

async fn settle_at(
    seeded: &Seeded,
    artifact: ProcessingArtifact,
    request_id: &str,
    payload_digest: &str,
    now: u64,
) -> ProcessingExportSettlement {
    seeded
        .persistence
        .settle_processing_export_receiver(artifact, request_id, payload_digest, now)
        .unwrap()
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn qualified_submission_admits_settles_and_replays_the_artifact() {
    let seeded = seeded();
    let recipe_revision = save_recipe(
        &seeded,
        "recipe-qualified",
        None,
        original_step_input("raw-photo", &seeded.source),
    )
    .await;
    let mutation = qualified_submission(&seeded, "export-qualified", "develop-1", &recipe_revision);
    let ProcessingExportSubmitOutcome::Admitted(admission) =
        submit(&seeded, mutation.clone()).await
    else {
        panic!("a qualified decision admits the selected step");
    };
    assert_eq!(admission.photo_id, "raw-photo");
    assert_eq!(admission.request_id, "export-qualified");
    assert_eq!(admission.recipe_revision, recipe_revision);
    assert_eq!(admission.source_revision, seeded.source);
    assert_eq!(admission.module.as_str(), "darktable");
    assert_eq!(
        admission.adapter_schema_version,
        "darktable-adapter-1:darktable-params-1"
    );
    assert_eq!(admission.parameters.schema_version, "darktable-params-1");
    assert_eq!(
        admission.input,
        original_step_input("raw-photo", &seeded.source)
    );
    admission.validate().unwrap();
    // Admission records neither refusal nor acceptance receipt; the
    // accepted work record it commits is the receipt for execution.
    assert_eq!(refusal_record_count(&seeded), 0);
    assert_eq!(acceptance_record_count(&seeded), 0);

    let mut artifact = artifact();
    artifact.input.input = admission.input.clone();
    artifact.bundle_id = admission.bundle_id.clone();
    artifact.adapter_schema_version = admission.adapter_schema_version.clone();
    artifact.parameters = admission.parameters.clone();
    artifact.sha256 = "e".repeat(64);
    let ProcessingExportSettlement::Settled(work) = settle(
        &seeded,
        artifact.clone(),
        &admission.request_id,
        &admission.payload_digest,
    )
    .await
    else {
        panic!("an accepted execution settles its validated artifact");
    };
    assert_eq!(work.state, ProcessingExportWorkState::Succeeded);
    assert_eq!(work.admission, admission);
    assert_eq!(work.artifact_id, Some(artifact.artifact_id.clone()));
    assert_eq!(work.terminal_at, Some(2_000));
    assert_eq!(
        work.retain_until,
        Some(2_000 + PROCESSING_ARTIFACT_RETENTION_SECONDS)
    );
    // A replayed settlement returns the committed work unchanged.
    assert!(matches!(
        settle(
            &seeded,
            artifact.clone(),
            &admission.request_id,
            &admission.payload_digest
        )
        .await,
        ProcessingExportSettlement::Replayed(_)
    ));
    // The settled request replays its committed artifact unchanged,
    // and the same identity under any other payload conflicts.
    let ProcessingExportSubmitOutcome::ArtifactReplayed(replayed) =
        submit(&seeded, mutation.clone()).await
    else {
        panic!("a settled request replays its artifact");
    };
    assert_eq!(replayed, artifact);
    let mut other = mutation;
    other.step_id = ProcessingStepId::new("develop-2").unwrap();
    assert!(matches!(
        submit(&seeded, other).await,
        ProcessingExportSubmitOutcome::RequestConflict
    ));
    // A different payload digest cannot settle the same request.
    assert!(matches!(
        settle(&seeded, artifact, "export-qualified", &"f".repeat(64)).await,
        ProcessingExportSettlement::Conflict
    ));
}

#[path = "processing_export_tests/lifecycle.rs"]
mod lifecycle;
