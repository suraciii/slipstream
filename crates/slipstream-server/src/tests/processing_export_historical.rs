//! Typed persistence fixtures for retained historical Processing Exports.
use super::*;
/// Seed historical intent through its retained persistence owner, without a
/// generic HTTP darktable write or execution.
pub(super) async fn save_selected_step(
    application: &Application,
    photo_id: &str,
    request_id: &str,
) -> (String, String) {
    let read = application
        .library
        .composable_edit_recipe_read(photo_id)
        .await
        .unwrap()
        .unwrap();
    let source = read
        .current_source_revision
        .expect("fixture Original source revision");
    let step_id = ProcessingStepId::new("develop-1").unwrap();
    let recipe = slipstream_core::ComposableEditRecipe {
        photo_id: photo_id.to_owned(),
        revision: "historical".into(),
        source_revision: source.clone(),
        current_step_id: Some(step_id.clone()),
        steps: vec![slipstream_core::ProcessingStep {
            step_id,
            module: ProcessingModuleId::new("darktable").unwrap(),
            input: ProcessingInput::Original {
                photo_id: photo_id.to_owned(),
                source_revision: source.clone(),
            },
            parameters: ProcessingParameterSnapshot::new(
                "darktable-params-1",
                serde_json::json!({"stack":[], "output":{
                    "format":"tiff", "precisionBits":32, "colorSpace":"prophoto-rgb", "transferFunction":"linear"
                }}),
            ).unwrap(),
        }],
    };
    let recipe = slipstream_core::processing_recipe_decode(
        slipstream_core::processing_recipe_encode(&recipe).unwrap(),
    )
    .unwrap();
    let outcome = application
        .library
        .save_composable_edit_recipe(slipstream_core::SaveComposableEditRecipe {
            photo_id: photo_id.to_owned(),
            request_id: request_id.to_owned(),
            expected_recipe_revision: read.recipe.map(|recipe| recipe.revision),
            expected_source_revision: source.clone(),
            request_intent: None,
            recipe,
            automatic_adjustment: None,
        })
        .await
        .unwrap();
    let recipe = seeded_recipe(outcome);
    (recipe.revision, source)
}

/// Saves one selected SpektraFilm step bound to the published artifact and
/// returns the committed recipe revision plus the observed source revision.
pub(super) async fn save_selected_film_step(
    application: &Application,
    photo_id: &str,
    source_revision: &str,
    request_id: &str,
) -> (String, String) {
    let current = application
        .library
        .composable_edit_recipe_read(photo_id)
        .await
        .unwrap()
        .unwrap();
    let step_id = ProcessingStepId::new("film-1").unwrap();
    let recipe = slipstream_core::ComposableEditRecipe {
        photo_id: photo_id.to_owned(),
        revision: "historical-film".into(),
        source_revision: source_revision.to_owned(),
        current_step_id: Some(step_id.clone()),
        steps: vec![slipstream_core::ProcessingStep {
            step_id,
            module: ProcessingModuleId::new("spektrafilm").unwrap(),
            input: ProcessingInput::Artifact {
                artifact_id: ProcessingArtifactId::new("artifact-a1").unwrap(),
                contract: ProcessingImageContract {
                    format: "image/tiff".into(),
                    precision: "float32".into(),
                    color_space: "prophoto-rgb".into(),
                    transfer: "linear".into(),
                    geometry: ProcessingGeometry::new(9504, 6336).unwrap(),
                    encoding: "deflate".into(),
                },
            },
            parameters: ProcessingParameterSnapshot::new(
                "spektrafilm-params-1",
                slipstream_processing::modules::spektrafilm_default_tree(),
            )
            .unwrap(),
        }],
    };
    let recipe = slipstream_core::processing_recipe_decode(
        slipstream_core::processing_recipe_encode(&recipe).unwrap(),
    )
    .unwrap();
    let outcome = application
        .library
        .save_composable_edit_recipe(slipstream_core::SaveComposableEditRecipe {
            photo_id: photo_id.to_owned(),
            request_id: request_id.to_owned(),
            expected_recipe_revision: current.recipe.map(|r| r.revision),
            expected_source_revision: source_revision.to_owned(),
            request_intent: None,
            recipe,
            automatic_adjustment: None,
        })
        .await
        .unwrap();
    let recipe = seeded_recipe(outcome);
    (recipe.revision, source_revision.to_owned())
}

pub(super) fn published_artifact(photo_id: &str, source_revision: &str) -> ProcessingArtifact {
    ProcessingArtifact {
        artifact_id: ProcessingArtifactId::new("artifact-a1").unwrap(),
        photo_id: photo_id.to_owned(),
        step_id: ProcessingStepId::new("develop-1").unwrap(),
        module: ProcessingModuleId::new("darktable").unwrap(),
        adapter_schema_version: "darktable-adapter-1".to_owned(),
        parameters: ProcessingParameterSnapshot::new(
            "darktable-params-1",
            serde_json::json!({"stack": []}),
        )
        .unwrap(),
        input: ProcessingInputEvidence::new(
            ProcessingInput::Original {
                photo_id: photo_id.to_owned(),
                source_revision: source_revision.to_owned(),
            },
            &"a".repeat(64),
            4096,
        )
        .unwrap(),
        output_contract: ProcessingImageContract {
            format: "image/tiff".to_owned(),
            precision: "float32".to_owned(),
            color_space: "prophoto-rgb".to_owned(),
            transfer: "linear".to_owned(),
            geometry: ProcessingGeometry::new(9504, 6336).unwrap(),
            encoding: "deflate".to_owned(),
        },
        bundle_id: "c".repeat(64),
        sha256: "d".repeat(64),
        byte_length: 12_288,
    }
}

pub(super) async fn seed_historical_export(
    application: &Application,
    photo_id: &str,
    request_id: &str,
    revision: &str,
    source: &str,
) -> slipstream_core::ProcessingExportAdmission {
    let outcome = application
        .library
        .submit_processing_export(
            SubmitProcessingExport {
                photo_id: photo_id.to_owned(),
                request_id: request_id.to_owned(),
                step_id: ProcessingStepId::new("develop-1").unwrap(),
                expected_recipe_revision: revision.to_owned(),
                expected_source_revision: source.to_owned(),
                bundle_id: "c".repeat(64),
                retained_output_bytes_max: u64::MAX,
                adapter: ProcessingExportAdapterDecision::Qualified {
                    adapter_version: DARKTABLE_ADAPTER_VERSION.to_owned(),
                    parameter_schema_version: DARKTABLE_PARAMETER_VERSION.to_owned(),
                },
            },
            1_000,
        )
        .await
        .unwrap();
    let ProcessingExportSubmitOutcome::Admitted(admission) = outcome else {
        panic!("historical admission: {outcome:?}")
    };
    admission
}

pub(super) async fn settle_historical_export(
    application: &Application,
    photo_id: &str,
    request_id: &str,
    revision: &str,
    source: &str,
) -> String {
    let admission =
        seed_historical_export(application, photo_id, request_id, revision, source).await;
    application
        .library
        .begin_processing_export_attempt(request_id, 1_001)
        .await
        .unwrap();
    let artifact_id = format!("pa-{request_id}");
    let bytes = valid_development_tiff_fixture();
    let manager = application.exports.as_ref().unwrap();
    let path = manager
        .artifact_path_for_module(&artifact_id, "darktable")
        .unwrap();
    fs::write(path, &bytes).unwrap();
    let claim = manager.claim_retained_publication(&artifact_id, request_id);
    let mut artifact = published_artifact(photo_id, source);
    artifact.artifact_id = ProcessingArtifactId::new(&artifact_id).unwrap();
    artifact.parameters = admission.parameters.clone();
    artifact.adapter_schema_version = admission.adapter_schema_version.clone();
    artifact.sha256 = format!("{:x}", sha2::Sha256::digest(&bytes));
    artifact.byte_length = bytes.len() as u64;
    artifact.output_contract.format = "tiff".to_owned();
    artifact.output_contract.geometry = ProcessingGeometry::new(2, 1).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let outcome = application
        .library
        .settle_processing_export(artifact, request_id, &admission.payload_digest, now)
        .await
        .unwrap();
    assert!(
        matches!(
            outcome,
            slipstream_core::ProcessingExportSettlement::Settled(_)
        ),
        "{outcome:?}"
    );
    drop(claim);
    artifact_id
}

pub(super) async fn seed_historical_refusal(
    application: &Application,
    photo_id: &str,
    request_id: &str,
    revision: &str,
    source: &str,
) {
    let outcome = application
        .library
        .submit_processing_export(
            SubmitProcessingExport {
                photo_id: photo_id.to_owned(),
                request_id: request_id.to_owned(),
                step_id: ProcessingStepId::new("film-1").unwrap(),
                expected_recipe_revision: revision.to_owned(),
                expected_source_revision: source.to_owned(),
                bundle_id: "c".repeat(64),
                retained_output_bytes_max: u64::MAX,
                adapter: ProcessingExportAdapterDecision::NoQualifiedAdapter {
                    reason_code: "module_parameters_unavailable".to_owned(),
                },
            },
            1_000,
        )
        .await
        .unwrap();
    assert!(
        matches!(outcome, ProcessingExportSubmitOutcome::Refused(_)),
        "{outcome:?}"
    );
}

fn seeded_recipe(
    outcome: slipstream_core::ComposableEditRecipeWriteOutcome,
) -> slipstream_core::ComposableEditRecipe {
    use slipstream_core::ComposableEditRecipeWriteOutcome;
    match outcome {
        ComposableEditRecipeWriteOutcome::Saved(recipe)
        | ComposableEditRecipeWriteOutcome::Unchanged(recipe)
        | ComposableEditRecipeWriteOutcome::Replayed(recipe) => recipe,
        other => panic!("historical recipe seed: {other:?}"),
    }
}
