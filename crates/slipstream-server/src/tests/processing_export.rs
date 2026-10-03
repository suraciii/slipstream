//! Explicit composable Export and Processing Artifact route coverage. A
//! qualified darktable step over its bound Original executes through the
//! real engine bundle and settles an immutable Processing Artifact with a
//! durable lifecycle — accepted work, begun attempts, terminal failures,
//! explicit cancellation, and byte downloads under a finite lease — while
//! every unqualified pairing records the durable explicit refusal instead.

use super::*;
use crate::http::create_router_with_processing;
use sha2::Digest;
use slipstream_core::{
    ProcessingArtifact, ProcessingArtifactId, ProcessingArtifactPublication,
    ProcessingExportAdapterDecision, ProcessingExportSubmitOutcome, ProcessingGeometry,
    ProcessingImageContract, ProcessingInput, ProcessingInputEvidence, ProcessingModuleId,
    ProcessingParameterSnapshot, ProcessingStepId, SubmitProcessingExport,
};
use slipstream_processing::modules::{DARKTABLE_ADAPTER_VERSION, DARKTABLE_PARAMETER_VERSION};

/// One configured processing deployment with a live engine bundle and a
/// live retained-output owner: the minimum a selected composable step
/// needs to reach the adapter boundary and a qualified execution.
fn export_fixture() -> (PathBuf, Config) {
    let (base, mut config) = prepare_fixture();
    approved_raw_fixture(&config.library_root.join("approved.ARW"));
    let engine = FakePhotoEngine::install(&base);
    config.processing = Some(engine.processing_config());
    config.export_retained_output_bytes = Some(slipstream_core::MAXIMUM_EXPORT_BYTES);
    (base, config)
}

async fn export_application(config: &Config) -> (Arc<Application>, Router) {
    let application = Application::open(config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = create_router_with_processing(
        Arc::clone(&application),
        crate::http::open_web_root(config.web_root()),
        config.processing.clone(),
    );
    application.access.seed_test_token();
    (application, router)
}

async fn first_photo_id(application: &Application) -> String {
    browse_photo_ids(application, BrowseSourceRequest::Library)
        .await
        .into_iter()
        .next()
        .unwrap()
}

/// Saves one selected darktable step bound to the Original and returns the
/// committed recipe revision plus the observed source revision.
async fn save_selected_step(router: &Router, photo_id: &str, request_id: &str) -> (String, String) {
    let uri = format!("/api/photos/{photo_id}/processing-recipe");
    let (_, current) = get_json(router, &uri).await;
    let source_revision = current["sourceRevision"].as_str().unwrap().to_owned();
    let recipe_revision = current["recipe"]["revision"].as_str().map(str::to_owned);
    let body = serde_json::json!({
        "requestId": request_id,
        "expectedRecipeRevision": recipe_revision,
        "expectedSourceRevision": source_revision,
        "currentStepId": "develop-1",
        "steps": [{
            "stepId": "develop-1",
            "module": "darktable",
            "input": {
                "kind": "original",
                "photoId": photo_id,
                "sourceRevision": source_revision
            },
            "parameters": {
                "schemaVersion": "darktable-params-1",
                "tree": {"stack": [], "output": {"format": "tiff", "precisionBits": 32, "colorSpace": "prophoto-rgb", "transferFunction": "linear"}}
            }
        }]
    });
    let response = post_json(router, &uri, body, Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let saved = response_json(response).await;
    (
        saved["recipe"]["revision"].as_str().unwrap().to_owned(),
        source_revision,
    )
}

/// Saves one selected SpektraFilm step bound to the published artifact and
/// returns the committed recipe revision plus the observed source revision.
async fn save_selected_film_step(
    router: &Router,
    photo_id: &str,
    source_revision: &str,
    request_id: &str,
) -> (String, String) {
    let uri = format!("/api/photos/{photo_id}/processing-recipe");
    let (_, current) = get_json(router, &uri).await;
    let body = serde_json::json!({
        "requestId": request_id,
        "expectedRecipeRevision": current["recipe"]["revision"],
        "expectedSourceRevision": source_revision,
        "currentStepId": "film-1",
        "steps": [{
            "stepId": "film-1",
            "module": "spektrafilm",
            "input": {
                "kind": "artifact",
                "artifactId": "artifact-a1",
                "contract": {
                    "format": "image/tiff",
                    "precision": "float32",
                    "colorSpace": "prophoto-rgb",
                    "transfer": "linear",
                    "geometry": {"width": 9504, "height": 6336},
                    "encoding": "deflate"
                }
            },
            "parameters": {
                "schemaVersion": "spektrafilm-params-1",
                "tree": slipstream_processing::modules::spektrafilm_default_tree()
            }
        }]
    });
    let response = post_json(router, &uri, body, Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let saved = response_json(response).await;
    (
        saved["recipe"]["revision"].as_str().unwrap().to_owned(),
        source_revision.to_owned(),
    )
}

fn submit_body(request_id: &str, step_id: &str, recipe_revision: &str, source: &str) -> String {
    serde_json::json!({
        "requestId": request_id,
        "stepId": step_id,
        "expectedRecipeRevision": recipe_revision,
        "expectedSourceRevision": source,
    })
    .to_string()
}

async fn post_processing_export(
    router: &Router,
    photo_id: &str,
    body: &str,
) -> (StatusCode, serde_json::Value) {
    let response = send(
        router,
        authenticated_request()
            .method("POST")
            .uri(format!("/api/photos/{photo_id}/processing-exports"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_owned()))
            .unwrap(),
    )
    .await;
    let status = response.status();
    (status, response_json(response).await)
}

async fn export_status(
    router: &Router,
    photo_id: &str,
    request_id: &str,
) -> (StatusCode, serde_json::Value) {
    get_json(
        router,
        &format!("/api/photos/{photo_id}/processing-exports/{request_id}"),
    )
    .await
}

async fn settled_export(router: &Router, photo_id: &str, request_id: &str) -> serde_json::Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (status, work) = export_status(router, photo_id, request_id).await;
            assert_eq!(status, StatusCode::OK);
            if matches!(
                work["state"].as_str(),
                Some("succeeded" | "failed" | "cancelled")
            ) {
                return work;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("accepted Export must reach a terminal state")
}

fn published_artifact(photo_id: &str, source_revision: &str) -> ProcessingArtifact {
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

#[path = "processing_export_publication.rs"]
mod publication;
#[path = "processing_export_retained.rs"]
mod retained;

#[tokio::test]
async fn submission_records_and_replays_the_explicit_adapter_refusal() {
    let (base, config) = export_fixture();
    let (application, router) = export_application(&config).await;
    let photo_id = first_photo_id(&application).await;
    let (_, source_revision) = save_selected_step(&router, &photo_id, "recipe-1").await;

    // Standalone SpektraFilm has no qualified adapter in this deployment,
    // so its artifact-bound step records the explicit refusal instead of
    // ever queueing work or publishing an artifact.
    let artifact = published_artifact(&photo_id, &source_revision);
    assert_eq!(
        application
            .library
            .publish_processing_artifact(artifact)
            .await
            .unwrap(),
        ProcessingArtifactPublication::Published
    );
    let (recipe_revision, film_source) =
        save_selected_film_step(&router, &photo_id, &source_revision, "recipe-2").await;

    let body = submit_body("export-1", "film-1", &recipe_revision, &film_source);
    let (status, refused) = post_processing_export(&router, &photo_id, &body).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(refused["error"]["code"], "module_parameters_unavailable");
    assert_eq!(refused["error"]["effect"], "none");
    let refusal = &refused["error"]["details"]["refusal"];
    assert_eq!(refused["error"]["details"]["replayed"], false);
    assert_eq!(refusal["photoId"], photo_id);
    assert_eq!(refusal["stepId"], "film-1");
    assert_eq!(refusal["recipeRevision"], recipe_revision);
    assert_eq!(refusal["sourceRevision"], film_source);
    assert_eq!(refusal["module"], "spektrafilm");
    assert_eq!(refusal["parameterSchemaVersion"], "spektrafilm-params-1");
    assert_eq!(
        refusal["parameterDigest"].as_str().map(str::len),
        Some(64),
        "the captured identity carries the canonical parameter digest"
    );
    assert_eq!(refusal["input"]["kind"], "artifact");
    assert_eq!(refusal["input"]["artifactId"], "artifact-a1");
    assert_eq!(refusal["bundleId"], "c".repeat(64));
    assert_eq!(refusal["reasonCode"], "module_parameters_unavailable");

    // A retry of the same request identity replays the committed refusal
    // unchanged instead of admitting the work again.
    let (status, replayed) = post_processing_export(&router, &photo_id, &body).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(replayed["error"]["code"], "module_parameters_unavailable");
    assert_eq!(replayed["error"]["details"]["replayed"], true);
    assert_eq!(
        replayed["error"]["details"]["refusal"], *refusal,
        "the replay carries the identical committed refusal"
    );

    // The same identity with any other payload is a conflict, never a
    // second recorded decision.
    let other = submit_body("export-1", "film-1", &recipe_revision, "stale-source");
    let (status, conflict) = post_processing_export(&router, &photo_id, &other).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["error"]["code"], "request_conflict");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn submission_guards_recipe_source_and_step_selection() {
    let (base, config) = export_fixture();
    let (application, router) = export_application(&config).await;
    let photo_id = first_photo_id(&application).await;
    let (recipe_revision, source_revision) =
        save_selected_step(&router, &photo_id, "recipe-1").await;

    let (status, body) = post_processing_export(
        &router,
        &photo_id,
        &submit_body("export-1", "develop-1", "stale-revision", &source_revision),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "recipe_conflict");

    let (status, body) = post_processing_export(
        &router,
        &photo_id,
        &submit_body("export-1", "develop-1", &recipe_revision, "stale-source"),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "source_changed");

    let (status, body) = post_processing_export(
        &router,
        &photo_id,
        &submit_body("export-1", "other-step", &recipe_revision, &source_revision),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "step_not_current");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn submission_without_a_recipe_names_the_missing_surface() {
    let (base, config) = export_fixture();
    let (application, router) = export_application(&config).await;
    let photo_id = first_photo_id(&application).await;

    let (status, body) = post_processing_export(
        &router,
        &photo_id,
        &submit_body("export-1", "develop-1", "missing", "missing"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "missing_recipe");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn artifact_route_reads_published_provenance_and_refuses_unknown_ids() {
    let (base, config) = export_fixture();
    let (application, router) = export_application(&config).await;
    let photo_id = first_photo_id(&application).await;
    let (revision, source) = save_selected_step(&router, &photo_id, "recipe-1").await;
    let submit = submit_body("export-provenance", "develop-1", &revision, &source);
    assert_eq!(
        post_processing_export(&router, &photo_id, &submit).await.0,
        StatusCode::ACCEPTED
    );
    let work = settled_export(&router, &photo_id, "export-provenance").await;
    assert_eq!(work["state"], "succeeded");
    let artifact_id = work["artifactId"].as_str().unwrap();
    let (status, body) =
        get_json(&router, &format!("/api/processing-artifacts/{artifact_id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["artifactId"], artifact_id);
    assert_eq!(body["photoId"], photo_id);
    assert_eq!(body["stepId"], "develop-1");
    assert_eq!(body["module"], "darktable");
    assert_eq!(body["parameters"], work["parameters"]);
    assert_eq!(body["input"]["binding"]["kind"], "original");
    let (status, body) = get_json(&router, "/api/processing-artifacts/artifact-none").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "unknown_artifact");
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn artifact_bound_step_export_captures_the_artifact_handoff() {
    let (base, config) = export_fixture();
    let (application, router) = export_application(&config).await;
    let photo_id = first_photo_id(&application).await;
    let (_, source_revision) = save_selected_step(&router, &photo_id, "recipe-1").await;

    let artifact = published_artifact(&photo_id, &source_revision);
    assert_eq!(
        application
            .library
            .publish_processing_artifact(artifact.clone())
            .await
            .unwrap(),
        ProcessingArtifactPublication::Published
    );

    // A downstream step bound to the exact published contract is admitted
    // through the handoff and stopped only at the adapter refusal.
    let uri = format!("/api/photos/{photo_id}/processing-recipe");
    let (_, current) = get_json(&router, &uri).await;
    let body = serde_json::json!({
        "requestId": "recipe-2",
        "expectedRecipeRevision": current["recipe"]["revision"],
        "expectedSourceRevision": source_revision,
        "currentStepId": "film-1",
        "steps": [{
            "stepId": "film-1",
            "module": "spektrafilm",
            "input": {
                "kind": "artifact",
                "artifactId": "artifact-a1",
                "contract": {
                    "format": "image/tiff",
                    "precision": "float32",
                    "colorSpace": "prophoto-rgb",
                    "transfer": "linear",
                    "geometry": {"width": 9504, "height": 6336},
                    "encoding": "deflate"
                }
            },
            "parameters": {
                "schemaVersion": "spektrafilm-params-1",
                "tree": slipstream_processing::modules::spektrafilm_default_tree()
            }
        }]
    });
    let response = post_json(&router, &uri, body, Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let saved = response_json(response).await;
    let recipe_revision = saved["recipe"]["revision"].as_str().unwrap().to_owned();

    let (status, refused) = post_processing_export(
        &router,
        &photo_id,
        &submit_body("export-film", "film-1", &recipe_revision, &source_revision),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(refused["error"]["code"], "module_parameters_unavailable");
    let refusal = &refused["error"]["details"]["refusal"];
    assert_eq!(refusal["module"], "spektrafilm");
    assert_eq!(refusal["input"]["kind"], "artifact");
    assert_eq!(refusal["input"]["artifactId"], "artifact-a1");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// One qualified darktable export executes end to end through the real
/// engine bundle: the frozen parameter tree crosses the development run,
/// the validated Development TIFF settles as an immutable Processing
/// Artifact bound to the staged source's byte evidence, and the same
/// request identity replays the committed artifact instead of
/// re-executing.
#[tokio::test]
async fn qualified_darktable_export_executes_publishes_and_replays() {
    let (base, config) = export_fixture();
    let engine = FakePhotoEngine::at(&base);
    let (application, router) = export_application(&config).await;
    let photo_id = first_photo_id(&application).await;
    let (recipe_revision, source_revision) =
        save_selected_step(&router, &photo_id, "recipe-1").await;

    let body = submit_body("export-1", "develop-1", &recipe_revision, &source_revision);
    let (status, accepted) = post_processing_export(&router, &photo_id, &body).await;
    assert_eq!(status, StatusCode::ACCEPTED, "submit body: {accepted}");
    assert_eq!(accepted["outcome"], "accepted");
    assert_eq!(accepted["receipt"]["state"], "accepted");
    let settled = settled_export(&router, &photo_id, "export-1").await;
    assert_eq!(settled["state"], "succeeded");
    let (status, created) = post_processing_export(&router, &photo_id, &body).await;
    assert_eq!(status, StatusCode::CREATED);
    let artifact = &created["artifact"];
    let artifact_id = artifact["artifactId"].as_str().unwrap().to_owned();
    assert!(artifact_id.starts_with("pa-"));
    assert_eq!(artifact["photoId"], photo_id);
    assert_eq!(artifact["stepId"], "develop-1");
    assert_eq!(artifact["module"], "darktable");
    assert_eq!(
        artifact["adapterSchemaVersion"],
        "darktable-adapter-1:darktable-params-1"
    );
    assert_eq!(
        artifact["parameters"]["schemaVersion"],
        "darktable-params-1"
    );
    assert_eq!(
        artifact["parameters"]["tree"]["stack"],
        serde_json::json!([])
    );
    assert_eq!(artifact["input"]["binding"]["kind"], "original");
    assert_eq!(artifact["input"]["binding"]["photoId"], photo_id);
    assert_eq!(
        artifact["input"]["sha256"].as_str().map(str::len),
        Some(64),
        "the artifact carries the staged source's verified byte evidence"
    );
    assert!(artifact["input"]["byteLength"].as_u64().unwrap() > 0);
    assert_eq!(artifact["outputContract"]["format"], "tiff");
    assert_eq!(artifact["outputContract"]["colorSpace"], "prophoto-rgb");
    assert_eq!(artifact["outputContract"]["transfer"], "linear");
    assert_eq!(artifact["bundleId"], "c".repeat(64));
    assert_eq!(
        artifact["sha256"].as_str().map(str::len),
        Some(64),
        "the published bytes carry their own digest"
    );
    assert!(artifact["byteLength"].as_u64().unwrap() > 0);
    assert_eq!(engine.runs(), 1, "exactly one engine attempt ran");

    // The settled work record is readable through the status route with
    // its terminal decision and the artifact it settled.
    let (status, work) = export_status(&router, &photo_id, "export-1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(work["state"], "succeeded");
    assert_eq!(work["requestId"], "export-1");
    assert_eq!(work["stepId"], "develop-1");
    assert_eq!(work["module"], "darktable");
    assert_eq!(work["artifactId"], artifact_id);
    assert_eq!(work["failureReason"], serde_json::Value::Null);
    assert!(
        work["terminalAt"].as_u64().unwrap() > 0,
        "the terminal decision carries its timestamp"
    );
    assert!(
        work["retainUntil"].as_u64().unwrap() > work["terminalAt"].as_u64().unwrap(),
        "the settled artifact carries its finite retention window"
    );
    assert_eq!(
        work["attempt"]["sequence"].as_u64(),
        Some(1),
        "the execution attempt is durable"
    );

    // The published artifact is readable through its provenance route and
    // resolves downstream bindings by its exact output contract.
    let (status, provenance) =
        get_json(&router, &format!("/api/processing-artifacts/{artifact_id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(provenance["artifactId"], artifact_id);
    assert_eq!(provenance["outputContract"], artifact["outputContract"]);

    // The same request identity replays the committed artifact without
    // touching the engine again.
    let (status, replayed) = post_processing_export(&router, &photo_id, &body).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(replayed["replayed"], true);
    assert_eq!(replayed["artifact"], *artifact);
    assert_eq!(
        engine.runs(),
        1,
        "the replay never started a second engine attempt"
    );

    // A settled request can no longer be cancelled: the first terminal
    // decision wins, and a lifecycle decision that finds `Succeeded`
    // replays the committed artifact instead of changing anything.
    let response = send(
        &router,
        authenticated_request()
            .method("POST")
            .uri(format!(
                "https://camera.local/api/photos/{photo_id}/processing-exports/export-1/cancel"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = response_json(response).await;
    assert_eq!(body["replayed"], true);
    assert_eq!(body["artifact"], *artifact);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// One admitted qualified export whose engine run fails records its
/// terminal failure durably — the bounded reason is the closed wire name —
/// and the committed record replays for the same identity without a
/// second engine attempt.
#[tokio::test]
async fn qualified_export_engine_failure_records_the_terminal_failure() {
    let (base, config) = export_fixture();
    let engine = FakePhotoEngine::at(&base);
    engine.fail_attempt(1, "the engine refused the development");
    let (application, router) = export_application(&config).await;
    let photo_id = first_photo_id(&application).await;
    let (recipe_revision, source_revision) =
        save_selected_step(&router, &photo_id, "recipe-1").await;

    let body = submit_body(
        "export-failed",
        "develop-1",
        &recipe_revision,
        &source_revision,
    );
    let (status, accepted) = post_processing_export(&router, &photo_id, &body).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(accepted["receipt"]["state"], "accepted");
    let work = settled_export(&router, &photo_id, "export-failed").await;
    assert_eq!(work["state"], "failed");
    let (status, failed) = post_processing_export(&router, &photo_id, &body).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(failed["error"]["code"], "export_terminal");
    let receipt = &failed["error"]["details"]["receipt"];
    assert_eq!(receipt["state"], "failed");
    assert_eq!(receipt["failureReason"], "execution_failed");
    assert_eq!(receipt["requestId"], "export-failed");
    assert_eq!(engine.runs(), 1, "exactly one engine attempt ran");

    // The committed terminal record is readable through the status route.
    let (status, work) = export_status(&router, &photo_id, "export-failed").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(work["state"], "failed");
    assert_eq!(work["failureReason"], "execution_failed");
    assert_eq!(work["artifactId"], serde_json::Value::Null);

    // The same identity replays the committed failure without a second
    // engine attempt, and can never settle an artifact.
    let (status, replayed) = post_processing_export(&router, &photo_id, &body).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(replayed["error"]["code"], "export_terminal");
    assert_eq!(replayed["error"]["details"]["replayed"], true);
    assert_eq!(replayed["error"]["details"]["receipt"]["state"], "failed");
    assert_eq!(
        engine.runs(),
        1,
        "the replay never started an engine attempt"
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// One still-live accepted work record answers a duplicate submission with
/// its committed receipt — never a second execution — and one explicit
/// cancellation records the first terminal decision durably.
#[tokio::test]
async fn live_work_replays_its_receipt_and_cancels_durably() {
    let (base, config) = export_fixture();
    let engine = FakePhotoEngine::at(&base);
    let (application, router) = export_application(&config).await;
    let photo_id = first_photo_id(&application).await;
    let (recipe_revision, source_revision) =
        save_selected_step(&router, &photo_id, "recipe-1").await;

    // One admitted-but-not-dispatched request — exactly the state a lost
    // response or an in-flight execution leaves behind — seeded through
    // the same serialized Library owner the route uses.
    let seed = |request_id: &str| SubmitProcessingExport {
        photo_id: photo_id.clone(),
        request_id: request_id.to_owned(),
        step_id: ProcessingStepId::new("develop-1").unwrap(),
        expected_recipe_revision: recipe_revision.clone(),
        expected_source_revision: source_revision.clone(),
        bundle_id: "c".repeat(64),
        retained_output_bytes_max: u64::MAX,
        adapter: ProcessingExportAdapterDecision::Qualified {
            adapter_version: DARKTABLE_ADAPTER_VERSION.to_owned(),
            parameter_schema_version: DARKTABLE_PARAMETER_VERSION.to_owned(),
        },
    };
    for request_id in ["export-live", "export-delete"] {
        let outcome = application
            .library
            .submit_processing_export(seed(request_id), 1_000)
            .await
            .unwrap();
        assert!(
            matches!(outcome, ProcessingExportSubmitOutcome::Admitted(_)),
            "the seeded request {request_id} must be admitted"
        );
    }

    // A duplicate of the live request replays its committed receipt and
    // starts no second execution.
    let body = submit_body(
        "export-live",
        "develop-1",
        &recipe_revision,
        &source_revision,
    );
    let (status, accepted) = post_processing_export(&router, &photo_id, &body).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let receipt = &accepted["receipt"];
    assert_eq!(receipt["requestId"], "export-live");
    assert_eq!(receipt["stepId"], "develop-1");
    assert_eq!(receipt["state"], "accepted");
    assert_eq!(accepted["outcome"], "replayed");
    assert_eq!(engine.runs(), 0, "no engine attempt ever started");

    // The status route reads the same committed receipt.
    let (status, work) = export_status(&router, &photo_id, "export-live").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(work["state"], "accepted");
    assert_eq!(work["attempt"], serde_json::Value::Null);

    // One explicit cancellation records the first terminal decision.
    let response = send(
        &router,
        authenticated_request()
            .method("POST")
            .uri(format!(
                "https://camera.local/api/photos/{photo_id}/processing-exports/export-live/cancel"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let cancelled = response_json(response).await;
    assert_eq!(cancelled["state"], "cancelled");
    assert_eq!(cancelled["requestId"], "export-live");
    assert_eq!(cancelled["artifactId"], serde_json::Value::Null);

    // The cancelled identity replays its committed terminal record and can
    // never execute.
    let (status, replayed) = post_processing_export(&router, &photo_id, &body).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(replayed["error"]["code"], "export_terminal");
    assert_eq!(
        replayed["error"]["details"]["receipt"]["state"],
        "cancelled"
    );
    assert_eq!(engine.runs(), 0, "no engine attempt ever started");

    // A second cancellation of the same identity answers with the
    // committed record — the first terminal decision wins.
    let (status, again) = export_status(&router, &photo_id, "export-live").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["state"], "cancelled");

    // The DELETE verb records the same cancellation for the second seeded
    // request.
    let response = send(
        &router,
        authenticated_request()
            .method("DELETE")
            .uri(format!(
                "https://camera.local/api/photos/{photo_id}/processing-exports/export-delete"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let deleted = response_json(response).await;
    assert_eq!(deleted["state"], "cancelled");
    assert_eq!(deleted["requestId"], "export-delete");

    // Unknown identities — never admitted, or scoped to another Photo —
    // answer the same closed refusal.
    let (status, body) = export_status(&router, &photo_id, "export-none").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "unknown_request");
    let response = send(
        &router,
        authenticated_request()
            .method("POST")
            .uri("https://camera.local/api/photos/unknown-photo/processing-exports/export-delete/cancel")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// One settled artifact's published bytes download under a finite lease
/// with the record's identity field for field in the response headers.
#[tokio::test]
async fn artifact_bytes_download_under_a_lease_with_provenance_headers() {
    let (base, config) = export_fixture();
    let (application, router) = export_application(&config).await;
    let photo_id = first_photo_id(&application).await;
    let (recipe_revision, source_revision) =
        save_selected_step(&router, &photo_id, "recipe-1").await;

    let body = submit_body(
        "export-bytes",
        "develop-1",
        &recipe_revision,
        &source_revision,
    );
    let (status, accepted) = post_processing_export(&router, &photo_id, &body).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    assert_eq!(accepted["receipt"]["state"], "accepted");
    assert_eq!(
        settled_export(&router, &photo_id, "export-bytes").await["state"],
        "succeeded"
    );
    let (status, created) = post_processing_export(&router, &photo_id, &body).await;
    assert_eq!(status, StatusCode::CREATED);
    let artifact = created["artifact"].clone();
    let artifact_id = artifact["artifactId"].as_str().unwrap().to_owned();
    let byte_length = artifact["byteLength"].as_u64().unwrap();
    let sha256 = artifact["sha256"].as_str().unwrap().to_owned();
    assert_eq!(
        artifact["filename"],
        format!("darktable-develop-1-{artifact_id}.tif")
    );
    let (_, settled) = export_status(&router, &photo_id, "export-bytes").await;
    let timestamp = |seconds: u64| {
        time::OffsetDateTime::from_unix_timestamp(seconds as i64)
            .unwrap()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap()
    };
    assert_eq!(
        artifact["publishedAt"],
        timestamp(settled["terminalAt"].as_u64().unwrap())
    );
    assert_eq!(
        artifact["expiresAt"],
        timestamp(settled["retainUntil"].as_u64().unwrap())
    );
    assert_eq!(artifact["orientation"], "top-left");
    assert_eq!(artifact["iccEmbedded"], true);
    let recipe_uri = format!("/api/photos/{photo_id}/processing-recipe");
    let (_, current) = get_json(&router, &recipe_uri).await;
    let mut steps = current["recipe"]["steps"].clone();
    steps[0]["parameters"]["tree"]["output"] = serde_json::json!({
        "format": "jpeg", "precisionBits": 8, "colorSpace": "srgb", "transferFunction": "srgb"
    });
    let response = post_json(
        &router,
        &recipe_uri,
        serde_json::json!({
            "requestId": "recipe-after-publication", "expectedRecipeRevision": recipe_revision,
            "expectedSourceRevision": source_revision, "currentStepId": "develop-1", "steps": steps
        }),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(artifact["sampleFormat"], "float32");
    let router = create_router_with_processing(
        Arc::clone(&application),
        crate::http::open_web_root(config.web_root()),
        None,
    );
    let (status, replayed) = post_processing_export(&router, &photo_id, &body).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(replayed["replayed"], true);
    assert_eq!(replayed["artifact"], artifact);

    let response = send(
        &router,
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/processing-artifacts/{artifact_id}/bytes"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers();
    assert_eq!(headers.get("content-type").unwrap(), "image/tiff");
    let header = |name: &str| headers.get(name).unwrap().to_str().unwrap();
    assert_eq!(header("content-length"), byte_length.to_string());
    assert_eq!(header("slipstream-artifact-id"), artifact_id);
    assert_eq!(header("slipstream-artifact-sha256"), sha256);
    assert_eq!(header("slipstream-artifact-module"), "darktable");
    assert_eq!(
        header("slipstream-artifact-filename"),
        artifact["filename"].as_str().unwrap()
    );
    assert_eq!(
        header("slipstream-artifact-published-at"),
        artifact["publishedAt"].as_str().unwrap()
    );
    assert_eq!(
        header("slipstream-artifact-expires-at"),
        artifact["expiresAt"].as_str().unwrap()
    );
    assert_eq!(header("slipstream-artifact-orientation"), "top-left");
    assert_eq!(header("slipstream-artifact-icc-embedded"), "true");
    assert_eq!(header("slipstream-artifact-sample-format"), "float32");
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(bytes.len() as u64, byte_length);

    let (status, retained) = get_json(
        &router,
        &format!("/api/photos/{photo_id}/processing-exports"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        retained["exports"]
            .as_array()
            .unwrap()
            .iter()
            .any(|work| work["requestId"] == "export-bytes" && work["state"] == "succeeded")
    );
    assert_eq!(retained["artifacts"], serde_json::json!([artifact.clone()]));
    use sha2::{Digest, Sha256};
    assert_eq!(format!("{:x}", Sha256::digest(&bytes)), sha256);

    // Unknown bytes identities answer the same closed refusal as the
    // provenance route.
    let (status, body) = get_json(&router, "/api/processing-artifacts/pa-none/bytes").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "unknown_artifact");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
