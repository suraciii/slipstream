//! Historical Processing Artifact provenance and lifecycle remain readable.
//! Fixtures seed the retained persistence owner directly; generic darktable
//! routes refuse new execution and never serve as a historical fixture writer.

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

#[path = "processing_export_historical.rs"]
mod historical;
use historical::*;

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

#[path = "processing_export_download.rs"]
mod download;
#[path = "processing_export_publication.rs"]
mod publication;
#[path = "processing_export_retained.rs"]
mod retained;

#[tokio::test]
async fn submission_records_and_replays_the_explicit_adapter_refusal() {
    let (base, config) = export_fixture();
    let (application, router) = export_application(&config).await;
    let photo_id = first_photo_id(&application).await;
    let (_, source_revision) = save_selected_step(&application, &photo_id, "recipe-1").await;

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
        save_selected_film_step(&application, &photo_id, &source_revision, "recipe-2").await;
    seed_historical_refusal(
        &application,
        &photo_id,
        "export-1",
        &recipe_revision,
        &film_source,
    )
    .await;

    let body = submit_body("export-1", "film-1", &recipe_revision, &film_source);
    let (status, refused) = post_processing_export(&router, &photo_id, &body).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(refused["error"]["code"], "module_parameters_unavailable");
    assert_eq!(refused["error"]["effect"], "none");
    let refusal = &refused["error"]["details"]["refusal"];
    assert_eq!(refused["error"]["details"]["replayed"], true);
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
async fn generic_darktable_export_refuses_before_new_admission() {
    let (base, config) = export_fixture();
    let (application, router) = export_application(&config).await;
    let photo_id = first_photo_id(&application).await;
    let (recipe_revision, source_revision) =
        save_selected_step(&application, &photo_id, "recipe-1").await;

    for body in [
        submit_body(
            "export-new",
            "develop-1",
            &recipe_revision,
            &source_revision,
        ),
        submit_body(
            "export-stale",
            "develop-1",
            "stale-revision",
            &source_revision,
        ),
        submit_body(
            "export-other",
            "other-step",
            &recipe_revision,
            "stale-source",
        ),
    ] {
        let (status, refusal) = post_processing_export(&router, &photo_id, &body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(refusal["error"]["code"], "use_darktable_namespace");
        assert_eq!(refusal["error"]["effect"], "none");
    }
    assert_eq!(FakePhotoEngine::at(&base).runs(), 0);
    assert!(
        application
            .library
            .processing_export_work("export-new")
            .await
            .unwrap()
            .is_none()
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn historical_owner_keeps_recipe_source_and_step_admission_guards() {
    let (base, config) = export_fixture();
    let (application, _) = export_application(&config).await;
    let photo_id = first_photo_id(&application).await;
    let (revision, source) =
        save_selected_step(&application, &photo_id, "historical-guard-recipe").await;
    for (request, step, guard, source_guard, expected) in [
        (
            "stale-recipe",
            "develop-1",
            "stale",
            source.as_str(),
            "recipe",
        ),
        (
            "stale-source",
            "develop-1",
            revision.as_str(),
            "stale",
            "source",
        ),
        (
            "other-step",
            "other",
            revision.as_str(),
            source.as_str(),
            "step",
        ),
    ] {
        let outcome = application
            .library
            .submit_processing_export(
                SubmitProcessingExport {
                    photo_id: photo_id.clone(),
                    request_id: request.into(),
                    step_id: ProcessingStepId::new(step).unwrap(),
                    expected_recipe_revision: guard.into(),
                    expected_source_revision: source_guard.into(),
                    bundle_id: "c".repeat(64),
                    retained_output_bytes_max: u64::MAX,
                    adapter: ProcessingExportAdapterDecision::Qualified {
                        adapter_version: DARKTABLE_ADAPTER_VERSION.into(),
                        parameter_schema_version: DARKTABLE_PARAMETER_VERSION.into(),
                    },
                },
                1_000,
            )
            .await
            .unwrap();
        assert!(
            match expected {
                "recipe" => matches!(outcome, ProcessingExportSubmitOutcome::RecipeConflict(_)),
                "source" => matches!(outcome, ProcessingExportSubmitOutcome::SourceChanged(_)),
                _ => matches!(outcome, ProcessingExportSubmitOutcome::StepNotCurrent(_)),
            },
            "{outcome:?}"
        );
        assert!(
            application
                .library
                .processing_export_work(request)
                .await
                .unwrap()
                .is_none()
        );
    }
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
    let (revision, source) = save_selected_step(&application, &photo_id, "recipe-1").await;
    let artifact_id = settle_historical_export(
        &application,
        &photo_id,
        "export-provenance",
        &revision,
        &source,
    )
    .await;
    let (_, work) = export_status(&router, &photo_id, "export-provenance").await;
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
    let (_, source_revision) = save_selected_step(&application, &photo_id, "recipe-1").await;

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
    let (recipe_revision, _) =
        save_selected_film_step(&application, &photo_id, &source_revision, "recipe-2").await;
    seed_historical_refusal(
        &application,
        &photo_id,
        "export-film",
        &recipe_revision,
        &source_revision,
    )
    .await;

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
async fn historical_darktable_publication_reads_and_replays_without_execution() {
    let (base, config) = export_fixture();
    let engine = FakePhotoEngine::at(&base);
    let (application, router) = export_application(&config).await;
    let photo_id = first_photo_id(&application).await;
    let (recipe_revision, source_revision) =
        save_selected_step(&application, &photo_id, "recipe-1").await;

    let body = submit_body("export-1", "develop-1", &recipe_revision, &source_revision);
    settle_historical_export(
        &application,
        &photo_id,
        "export-1",
        &recipe_revision,
        &source_revision,
    )
    .await;
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
    assert_eq!(engine.runs(), 0, "historical seed does not execute");

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
        0,
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
async fn historical_export_failure_records_the_terminal_failure() {
    let (base, config) = export_fixture();
    let engine = FakePhotoEngine::at(&base);
    let (application, router) = export_application(&config).await;
    let photo_id = first_photo_id(&application).await;
    let (recipe_revision, source_revision) =
        save_selected_step(&application, &photo_id, "recipe-1").await;

    let body = submit_body(
        "export-failed",
        "develop-1",
        &recipe_revision,
        &source_revision,
    );
    seed_historical_export(
        &application,
        &photo_id,
        "export-failed",
        &recipe_revision,
        &source_revision,
    )
    .await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    application
        .library
        .begin_processing_export_attempt("export-failed", now)
        .await
        .unwrap();
    application
        .library
        .fail_processing_export("export-failed", "execution_failed", now)
        .await
        .unwrap();
    let (status, failed) = post_processing_export(&router, &photo_id, &body).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(failed["error"]["code"], "export_terminal");
    let receipt = &failed["error"]["details"]["receipt"];
    assert_eq!(receipt["state"], "failed");
    assert_eq!(receipt["failureReason"], "execution_failed");
    assert_eq!(receipt["requestId"], "export-failed");
    assert_eq!(engine.runs(), 0, "historical failure seed does not execute");

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
        0,
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
        save_selected_step(&application, &photo_id, "recipe-1").await;

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
