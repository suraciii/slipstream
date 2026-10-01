// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Export routes
// ---------------------------------------------------------------------------
use super::*;

use crate::export_manager::development_tiff_decode::{stored_zlib, write_development_tiff};
use crate::http::create_router_with_processing;
use sha2::{Digest, Sha256};
use std::time::Duration;

mod admission;

fn tiff_bytes(tags: &[(u16, u16, Vec<u8>)]) -> Vec<u8> {
    let data_start = 8 + 2 + tags.len() * 12 + 4;
    let mut bytes = b"II*\0\x08\0\0\0".to_vec();
    bytes.extend_from_slice(&(tags.len() as u16).to_le_bytes());
    let mut data = Vec::new();
    for (tag, field_type, value) in tags {
        bytes.extend_from_slice(&tag.to_le_bytes());
        bytes.extend_from_slice(&field_type.to_le_bytes());
        let count = if *field_type == 3 || *field_type == 5 {
            1
        } else {
            value.len() as u32
        };
        bytes.extend_from_slice(&count.to_le_bytes());
        if value.len() <= 4 {
            let mut inline = [0_u8; 4];
            inline[..value.len()].copy_from_slice(value);
            bytes.extend_from_slice(&inline);
        } else {
            bytes.extend_from_slice(&((data_start + data.len()) as u32).to_le_bytes());
            data.extend_from_slice(value);
        }
    }
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&data);
    bytes
}

/// Writes one RAW fixture whose TIFF header carries the camera identity
/// so the capture inspection classifies it against the approved profile.
fn raw_fixture_with_camera(path: &std::path::Path, make: &[u8], model: &[u8]) {
    let bytes = tiff_bytes(&[(0x010f, 2, make.to_vec()), (0x0110, 2, model.to_vec())]);
    fs::write(path, bytes).unwrap();
}

/// Builds the shared Export fixture: a RAW Photo carrying an approved
/// camera identity, a JPEG-only Photo, and a configured processing
/// deployment with the given retained-output allowance. The scripted
/// local engine bundle below the fixture base succeeds by default; tests
/// script it through [`export_engine`].
fn export_fixture(allowance: Option<u64>) -> (PathBuf, Config) {
    let (base, mut config) = prepare_populated_fixture();
    raw_fixture_with_camera(
        &config.library_root.join("pair.ARW"),
        b"SONY\0",
        b"ILCE-7RM5\0\0\0",
    );
    let engine = FakePhotoEngine::install(&base);
    config.processing = Some(engine.processing_config());
    config.export_retained_output_bytes = allowance;
    (base, config)
}

/// The scripted engine one export fixture runs against.
fn export_engine(base: &Path) -> FakePhotoEngine {
    FakePhotoEngine::at(base)
}

async fn export_application(_base: &Path, config: &Config) -> (Arc<Application>, Router) {
    let application = Application::open(config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = create_router_with_processing(
        Arc::clone(&application),
        open_web_root(config.web_root()),
        config.processing.clone(),
    );
    application.access.seed_test_token();
    (application, router)
}

fn photo_id_for(config: &Config, relative_path: &str) -> String {
    let connection =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    connection
        .query_row(
            "SELECT p.id FROM photos p JOIN original_files o ON p.original_id = o.id
                 WHERE o.relative_path = ?1",
            [relative_path],
            |row| row.get::<_, String>(0),
        )
        .unwrap()
}

async fn post_export_body(router: &Router, uri: String, body: serde_json::Value) -> Response<Body> {
    send(
        router,
        authenticated_request()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

async fn submit_export_body(
    router: &Router,
    photo_id: &str,
    body: serde_json::Value,
) -> Response<Body> {
    post_export_body(
        router,
        format!("https://camera.local/api/photos/{photo_id}/exports"),
        body,
    )
    .await
}

async fn submit_export_request(
    router: &Router,
    photo_id: &str,
    request_id: &str,
    recipe_version: &str,
    source_revision: &str,
) -> Response<Body> {
    submit_export_body(
        router,
        photo_id,
        serde_json::json!({
            "requestId": request_id,
            "expectedRecipeVersion": recipe_version,
            "expectedSourceRevision": source_revision,
            "target": "development-tiff",
        }),
    )
    .await
}

async fn get_export(router: &Router, export_id: &str) -> serde_json::Value {
    response_json(
        send(
            router,
            authenticated_request()
                .uri(format!("https://camera.local/api/exports/{export_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await
}

async fn get_export_response(router: &Router, export_id: &str) -> Response<Body> {
    send(
        router,
        authenticated_request()
            .uri(format!("https://camera.local/api/exports/{export_id}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

async fn download_artifact(router: &Router, export_id: &str) -> Response<Body> {
    send(
        router,
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/exports/{export_id}/artifact"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

async fn current_source_revision(application: &Application, photo_id: &str) -> String {
    application
        .library
        .edit_recipe(photo_id)
        .await
        .unwrap()
        .expect("the Photo must exist")
        .current_source_revision
        .expect("settled Photo must have a published source revision")
}

async fn save_recipe(
    application: &Application,
    photo_id: &str,
    request_id: &str,
    expected_recipe_version: Option<String>,
    exposure_ev: f64,
) -> slipstream_core::EditRecipe {
    let expected_source_revision = current_source_revision(application, photo_id).await;
    let outcome = application
        .library
        .save_edit_recipe(slipstream_core::SaveEditRecipe {
            photo_id: photo_id.to_owned(),
            request_id: request_id.to_owned(),
            expected_recipe_version,
            expected_source_revision,
            settings: slipstream_core::EditRecipeSettings {
                exposure_ev,
                white_balance: slipstream_core::WhiteBalanceIntent::AsShot,
            },
        })
        .await
        .unwrap();
    match outcome {
        slipstream_core::EditRecipeWriteOutcome::Saved(recipe)
        | slipstream_core::EditRecipeWriteOutcome::Unchanged(recipe) => recipe,
        other => panic!("the recipe must save: {other:?}"),
    }
}

async fn wait_for_state(router: &Router, export_id: &str, state: &str) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let record = get_export(router, export_id).await;
        if record["state"] == state {
            return record;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the Export must reach {state}: {record}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn retry_export(router: &Router, export_id: &str, request_id: &str) -> Response<Body> {
    post_export_body(
        router,
        format!("https://camera.local/api/exports/{export_id}/retry"),
        serde_json::json!({ "requestId": request_id }),
    )
    .await
}

async fn cancel_export(router: &Router, export_id: &str) -> Response<Body> {
    send(
        router,
        authenticated_request()
            .method("POST")
            .uri(format!(
                "https://camera.local/api/exports/{export_id}/cancel"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

fn error_code(body: &serde_json::Value) -> &serde_json::Value {
    &body["error"]["code"]
}

/// One valid Development TIFF the fake engine writes as its output.
fn valid_development_tiff() -> Vec<u8> {
    let path = std::env::temp_dir().join(format!(
        "export-artifact-fixture-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    write_development_tiff(&path, &stored_zlib(&[0_u8; 2 * 3 * 4]));
    let bytes = fs::read(&path).unwrap();
    let _ = fs::remove_file(&path);
    bytes
}

#[tokio::test]
async fn export_submit_shape_is_refused_before_any_state_change() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;

    // A target outside the closed set is refused before acceptance.
    let wrong_target = submit_export_body(
        &router,
        &photo_id,
        serde_json::json!({
            "requestId": "request-target",
            "expectedRecipeVersion": recipe.revision,
            "expectedSourceRevision": source_revision,
            "target": "finished-jpeg",
        }),
    )
    .await;
    assert_eq!(wrong_target.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        error_code(&response_json(wrong_target).await),
        "invalid_settings"
    );

    // The refused submission created nothing: the same identity is still
    // fresh and is accepted as new work.
    let accepted = submit_export_request(
        &router,
        &photo_id,
        "request-target",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(accepted.status(), StatusCode::CREATED);

    // Unknown fields, missing fields, wrong types, and malformed request
    // identities are all refused with the one closed shape code.
    let unknown_field = submit_export_body(
        &router,
        &photo_id,
        serde_json::json!({
            "requestId": "request-unknown-field",
            "expectedRecipeVersion": recipe.revision,
            "expectedSourceRevision": source_revision,
            "target": "development-tiff",
            "extra": true,
        }),
    )
    .await;
    assert_eq!(unknown_field.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let missing_field = submit_export_body(
        &router,
        &photo_id,
        serde_json::json!({
            "requestId": "request-missing-field",
            "expectedSourceRevision": source_revision,
            "target": "development-tiff",
        }),
    )
    .await;
    assert_eq!(missing_field.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let wrong_type = submit_export_body(
        &router,
        &photo_id,
        serde_json::json!({
            "requestId": "request-wrong-type",
            "expectedRecipeVersion": 7,
            "expectedSourceRevision": source_revision,
            "target": "development-tiff",
        }),
    )
    .await;
    assert_eq!(wrong_type.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        error_code(&response_json(wrong_type).await),
        "invalid_settings"
    );

    for bad_identity in ["", "bad identity", &"x".repeat(129)] {
        let refused = submit_export_body(
            &router,
            &photo_id,
            serde_json::json!({
                "requestId": bad_identity,
                "expectedRecipeVersion": recipe.revision,
                "expectedSourceRevision": source_revision,
                "target": "development-tiff",
            }),
        )
        .await;
        assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            error_code(&response_json(refused).await),
            "invalid_settings"
        );
    }

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn export_submit_accepts_replays_and_keeps_identity_after_later_writes() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let first = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;

    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &first.revision,
        &source_revision,
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = response_json(created).await;
    assert!(created["exportId"].as_str().unwrap().starts_with("exp-"));
    assert_eq!(created["state"], "queued");
    assert_eq!(created["target"], "development-tiff");
    assert_eq!(created["recipeVersion"], first.revision);
    assert_eq!(created["sourceRevision"], source_revision);
    // Retention runs through the active operation, so both expiries are
    // null while the Export is queued or running.
    assert!(created["receiptExpiresAt"].is_null());
    assert!(created["artifactExpiresAt"].is_null());

    // Repeating the identity and payload returns the same body with 200.
    let replayed = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &first.revision,
        &source_revision,
    )
    .await;
    assert_eq!(replayed.status(), StatusCode::OK);
    let replayed = response_json(replayed).await;
    assert_eq!(replayed["exportId"], created["exportId"]);
    assert_eq!(replayed["recipeVersion"], first.revision);

    // A later recipe write must not be overwritten by replaying the older
    // request identity: the Export keeps its original snapshot.
    let second = save_recipe(
        &application,
        &photo_id,
        "save-2",
        Some(first.revision.clone()),
        0.75,
    )
    .await;
    assert_ne!(second.revision, first.revision);
    let stale_replay = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &first.revision,
        &source_revision,
    )
    .await;
    assert_eq!(stale_replay.status(), StatusCode::OK);
    let stale_record = response_json(stale_replay).await;
    assert_eq!(stale_record["exportId"], created["exportId"]);
    assert_eq!(stale_record["recipeVersion"], first.revision);

    // A different payload under the accepted identity conflicts.
    let conflict = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &second.revision,
        &source_revision,
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(
        error_code(&response_json(conflict).await),
        "export_conflict"
    );

    // A new request identity against the current recipe starts new work.
    let fresh = submit_export_request(
        &router,
        &photo_id,
        "request-2",
        &second.revision,
        &source_revision,
    )
    .await;
    assert_eq!(fresh.status(), StatusCode::CREATED);
    let fresh_record = response_json(fresh).await;
    assert_ne!(fresh_record["exportId"], created["exportId"]);
    assert_eq!(fresh_record["recipeVersion"], second.revision);

    let listed = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{photo_id}/exports"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    let entries = listed["exports"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    // Retention order is newest first, so the Export just submitted heads
    // the list even when both share a whole-second creation time.
    assert_eq!(entries[0]["exportId"], fresh_record["exportId"]);
    assert_eq!(entries[1]["exportId"], created["exportId"]);
    for entry in entries {
        assert!(entry["exportId"].is_string());
        assert!(entry["state"].is_string());
        assert_eq!(entry["target"], "development-tiff");
    }

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn export_submit_reports_every_owner_refusal_code() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let jpeg_id = photo_id_for(&config, "pair.JPG");

    let unknown =
        submit_export_request(&router, "missing-photo", "request-unknown", "rev", "source").await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    assert_eq!(error_code(&response_json(unknown).await), "unknown_photo");

    // JPEG-only Photos cannot take the development-tiff workload.
    let jpeg = submit_export_request(&router, &jpeg_id, "request-jpeg", "rev", "source").await;
    assert_eq!(jpeg.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(error_code(&response_json(jpeg).await), "unsupported_photo");

    // A RAW Photo without a saved recipe cannot start an Export.
    let missing =
        submit_export_request(&router, &photo_id, "request-norecipe", "rev", "source").await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(error_code(&response_json(missing).await), "missing_recipe");

    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;

    // A stale recipe version is a conflict.
    let stale_recipe = submit_export_request(
        &router,
        &photo_id,
        "request-stale-recipe",
        "no-such-revision",
        &source_revision,
    )
    .await;
    assert_eq!(stale_recipe.status(), StatusCode::CONFLICT);
    assert_eq!(
        error_code(&response_json(stale_recipe).await),
        "recipe_conflict"
    );

    // A stale source revision is a conflict.
    let stale_source = submit_export_request(
        &router,
        &photo_id,
        "request-stale-source",
        &recipe.revision,
        "stale-source-revision",
    )
    .await;
    assert_eq!(stale_source.status(), StatusCode::CONFLICT);
    assert_eq!(
        error_code(&response_json(stale_source).await),
        "source_changed"
    );

    // Settings outside the approved range are invalid input.
    let outside = save_recipe(
        &application,
        &photo_id,
        "save-2",
        Some(recipe.revision.clone()),
        2.0,
    )
    .await;
    let invalid = submit_export_request(
        &router,
        &photo_id,
        "request-invalid",
        &outside.revision,
        &source_revision,
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        error_code(&response_json(invalid).await),
        "invalid_settings"
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn export_submit_reports_requires_rebind_for_a_stale_recipe_binding() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;

    // A new scan publishes the changed source facts and keeps the saved
    // recipe bound to the prior revision until explicit rebind.
    let original = config.library_root.join("pair.ARW");
    let mut bytes = fs::read(&original).unwrap();
    bytes.push(0);
    fs::write(&original, bytes).unwrap();
    application.rescan().await.unwrap();
    let new_source_revision = current_source_revision(&application, &photo_id).await;
    assert_ne!(new_source_revision, recipe.source_revision);

    // The expected revision is current but the stored recipe is bound to
    // the old source: only an explicit rebind may adopt it.
    let refused = submit_export_request(
        &router,
        &photo_id,
        "request-rebind",
        &recipe.revision,
        &new_source_revision,
    )
    .await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    assert_eq!(error_code(&response_json(refused).await), "requires_rebind");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn export_submit_reports_retained_output_capacity_before_acceptance() {
    let (base, config) = export_fixture(Some(slipstream_core::MAXIMUM_EXPORT_BYTES));
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;

    let first = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(first.status(), StatusCode::CREATED);

    // The in-flight Export reserves the worst-case artifact size, so the
    // allowance cannot admit a second one before acceptance.
    let second = submit_export_request(
        &router,
        &photo_id,
        "request-2",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        error_code(&response_json(second).await),
        "retained_output_full"
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A fresh Film identity is refused before any state change even while the
/// local engine is healthy and admits the Development target: the engine
/// qualifies only Development, so a full-resolution Film Export records no
/// Export and no receipt.
#[tokio::test]
async fn unqualified_film_export_refuses_without_a_receipt() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;

    // The same healthy engine admits the Development target, so the Film
    // refusal below is the qualification gate, not availability.
    let developed = submit_export_request(
        &router,
        &photo_id,
        "develop-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(developed.status(), StatusCode::CREATED);

    let request = serde_json::json!({
        "requestId": "unqualified-film",
        "expectedRecipeVersion": recipe.revision,
        "expectedSourceRevision": source_revision,
        "target": "film-jpeg",
    });
    let refused = submit_export_body(&router, &photo_id, request.clone()).await;
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        error_code(&response_json(refused).await),
        "processing_unavailable"
    );
    // The refusal recorded no Export: only the Development admission lists.
    let listed = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{photo_id}/exports"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    let exports = listed["exports"].as_array().unwrap();
    assert_eq!(exports.len(), 1);
    assert_eq!(exports[0]["target"], "development-tiff");
    // And no receipt: the same identity is refused again, not replayed.
    let replay = submit_export_body(&router, &photo_id, request).await;
    assert_eq!(replay.status(), StatusCode::SERVICE_UNAVAILABLE);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// An unusable local bundle refuses the submission before acceptance, so
/// no Export, receipt, or capacity reservation is consumed and the identity
/// stays fresh for a deployment whose bundle is available again.
#[tokio::test]
async fn export_submission_refuses_before_acceptance_when_the_bundle_is_unavailable() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));

    // The deployment's bundle is invalid, so the application opens with
    // processing configured but unavailable; the Library stays usable.
    let mut unavailable = config.clone();
    let mut processing = unavailable.processing.clone().unwrap();
    processing.failure = Some("bundle-unavailable");
    unavailable.processing = Some(processing);
    let (application, router) = export_application(&base, &unavailable).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;

    let blocked = submit_export_request(
        &router,
        &photo_id,
        "request-blocked",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(blocked.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        error_code(&response_json(blocked).await),
        "processing_unavailable"
    );

    // Nothing was accepted: the listing is empty and no engine ran.
    let listed = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{photo_id}/exports"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(listed["exports"].as_array().unwrap().len(), 0);
    assert_eq!(export_engine(&base).runs(), 0);
    application.shutdown().await.unwrap();

    // The same state directory under a deployment with a valid bundle
    // accepts the still-fresh identity as new work.
    let (application, router) = export_application(&base, &config).await;
    let accepted = submit_export_request(
        &router,
        &photo_id,
        "request-blocked",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(accepted.status(), StatusCode::CREATED);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn export_routes_report_processing_unavailable_without_configuration() {
    // Neither processing nor an allowance is configured.
    let (base, config) = prepare_populated_fixture();
    let (application, router) = export_application(&base, &config).await;
    let body = serde_json::json!({
        "requestId": "r",
        "expectedRecipeVersion": "rev",
        "expectedSourceRevision": "src",
        "target": "development-tiff"
    });
    let submitted = submit_export_body(&router, "whatever", body).await;
    assert_eq!(submitted.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        error_code(&response_json(submitted).await),
        "processing_unavailable"
    );
    let read = send(
        &router,
        authenticated_request()
            .uri("https://camera.local/api/exports/missing")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(read.status(), StatusCode::NOT_FOUND);
    assert_eq!(error_code(&response_json(read).await), "unknown_export");
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);

    // Processing configured without an allowance refuses the same way.
    let (base, mut config) = prepare_populated_fixture();
    raw_fixture_with_camera(
        &config.library_root.join("pair.ARW"),
        b"SONY\0",
        b"ILCE-7RM5\0\0\0",
    );
    config.processing = Some(unresolved_processing_config());
    let (application, router) = export_application(&base, &config).await;
    let body = serde_json::json!({
        "requestId": "r",
        "expectedRecipeVersion": "rev",
        "expectedSourceRevision": "src",
        "target": "development-tiff"
    });
    let submitted = submit_export_body(&router, "whatever", body).await;
    assert_eq!(submitted.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        error_code(&response_json(submitted).await),
        "processing_unavailable"
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn export_unknown_identity_is_reported_on_every_route() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let (_application, router) = export_application(&base, &config).await;

    // A missing record maps to unknown_export on every route.
    let response = get_export_response(&router, "no-such-export").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(error_code(&response_json(response).await), "unknown_export");

    let retried = retry_export(&router, "no-such-export", "retry-unknown").await;
    assert_eq!(retried.status(), StatusCode::NOT_FOUND);
    assert_eq!(error_code(&response_json(retried).await), "unknown_export");

    let cancelled = cancel_export(&router, "no-such-export").await;
    assert_eq!(cancelled.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        error_code(&response_json(cancelled).await),
        "unknown_export"
    );

    let artifact = download_artifact(&router, "no-such-export").await;
    assert_eq!(artifact.status(), StatusCode::NOT_FOUND);
    assert_eq!(error_code(&response_json(artifact).await), "unknown_export");

    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn export_cancel_settles_once_and_retry_rearms_within_retention() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let engine = export_engine(&base);
    // The first engine run holds a live attempt open until released; the
    // retry's second run fails with an actionable engine error.
    engine.hang_attempt(1);
    engine.fail_attempt(2, "engine-failed");
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = response_json(created).await;
    let export_id = created["exportId"].as_str().unwrap().to_owned();

    // The attempt really runs against the local engine.
    let running = wait_for_state(&router, &export_id, "running").await;
    assert_eq!(running["terminalOutcome"], serde_json::Value::Null);

    // Cancellation settles exactly once against the actual completion
    // state and tears the live engine attempt down.
    let cancelled = cancel_export(&router, &export_id).await;
    assert_eq!(cancelled.status(), StatusCode::OK);
    let cancelled_record = response_json(cancelled).await;
    assert_eq!(cancelled_record["exportId"], created["exportId"]);
    assert_eq!(cancelled_record["state"], "cancelled");
    assert_eq!(cancelled_record["terminalOutcome"], "cancelled");
    assert_eq!(cancelled_record.as_object().unwrap().len(), 3);

    // A repeated cancellation returns the unchanged terminal record.
    let repeated = cancel_export(&router, &export_id).await;
    assert_eq!(repeated.status(), StatusCode::OK);
    assert_eq!(response_json(repeated).await, cancelled_record);

    // Replaying the original submission returns the terminal snapshot
    // with the disclosed receipt expiry.
    let replayed = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(replayed.status(), StatusCode::OK);
    let replayed = response_json(replayed).await;
    assert_eq!(replayed["state"], "cancelled");
    assert!(replayed["receiptExpiresAt"].is_string());
    assert!(replayed["artifactExpiresAt"].is_null());

    // Retry re-arms the retained snapshot under the caller's new request
    // identity and returns the admitted attempt.
    let retried = retry_export(&router, &export_id, "retry-1").await;
    assert_eq!(retried.status(), StatusCode::ACCEPTED);
    let retried_record = response_json(retried).await;
    assert_eq!(retried_record["exportId"], created["exportId"]);
    assert_eq!(retried_record["state"], "queued", "{retried_record}");
    assert_eq!(retried_record.as_object().unwrap().len(), 2);

    // Replaying the retry identity resolves to the Export without
    // starting another attempt.
    let replayed_retry = retry_export(&router, &export_id, "retry-1").await;
    assert_eq!(replayed_retry.status(), StatusCode::ACCEPTED);
    assert_eq!(response_json(replayed_retry).await["state"], "queued");

    // Without a completable engine result the re-armed attempt fails
    // with an actionable reason and stays retriable.
    let failed = wait_for_state(&router, &export_id, "failed").await;
    let failure_reason = failed["failureReason"].as_str().unwrap().to_owned();
    assert!(failure_reason.contains("engine-failed"), "{failure_reason}");
    assert_eq!(failed["terminalOutcome"], "failed");
    assert!(failed["receiptExpiresAt"].is_string());

    let retry_failed = retry_export(&router, &export_id, "retry-2").await;
    assert_eq!(retry_failed.status(), StatusCode::ACCEPTED);
    assert_eq!(response_json(retry_failed).await["state"], "queued");

    // A retry of a queued Export is refused as a conflict.
    let conflict = retry_export(&router, &export_id, "retry-3").await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(
        error_code(&response_json(conflict).await),
        "export_conflict"
    );

    // A retry identity consumed by another Export conflicts.
    let second = submit_export_request(
        &router,
        &photo_id,
        "request-2",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(second.status(), StatusCode::CREATED);
    let second_id = response_json(second).await["exportId"]
        .as_str()
        .unwrap()
        .to_owned();
    // The second Export cannot start while the first attempt owns the
    // serialized engine slot, so cancel it before retrying with the
    // shared identity.
    assert_eq!(
        cancel_export(&router, &second_id).await.status(),
        StatusCode::OK
    );
    let shared = retry_export(&router, &second_id, "retry-2").await;
    assert_eq!(shared.status(), StatusCode::CONFLICT);
    assert_eq!(error_code(&response_json(shared).await), "request_conflict");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn export_cancellation_settles_a_completion_that_finished_before_it() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let engine = export_engine(&base);
    engine.hang_attempt(1);
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    let export_id = response_json(created).await["exportId"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_for_state(&router, &export_id, "running").await;

    // The engine completed and the attempt settled before the
    // cancellation landed: cancellation must return the actual terminal
    // result, and the validated artifact stays published, never undone.
    engine.release();
    wait_for_state(&router, &export_id, "succeeded").await;
    let cancelled = cancel_export(&router, &export_id).await;
    assert_eq!(cancelled.status(), StatusCode::OK);
    let settled = response_json(cancelled).await;
    assert_eq!(settled["state"], "succeeded");
    assert_eq!(settled["terminalOutcome"], "succeeded");

    let inspected = get_export(&router, &export_id).await;
    assert_eq!(inspected["state"], "succeeded");
    assert_eq!(inspected["artifact"]["stage"], "develop");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn export_expiry_reports_expired_identities_and_reclaims_records() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    let export_id = response_json(created).await["exportId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        cancel_export(&router, &export_id).await.status(),
        StatusCode::OK
    );

    // A passed retention window refuses the retry with the explicit
    // expired outcome even before the sweep reclaims the record.
    let connection =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    connection
        .execute("UPDATE exports SET retain_until = 1", [])
        .unwrap();
    let expired_retry = retry_export(&router, &export_id, "retry-expired").await;
    assert_eq!(expired_retry.status(), StatusCode::GONE);
    assert_eq!(
        error_code(&response_json(expired_retry).await),
        "export_expired"
    );

    // Force the retention window to pass and run the sweep.
    drop(connection);
    let manager = Arc::clone(application.exports.as_ref().unwrap());
    manager.sweep_expiry().await;

    // The record is gone and the identity refuses new work as expired.
    let read = get_export_response(&router, &export_id).await;
    assert_eq!(read.status(), StatusCode::NOT_FOUND);
    let replayed = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(replayed.status(), StatusCode::GONE);
    assert_eq!(error_code(&response_json(replayed).await), "export_expired");
    // The swept identity still reports its expiry on retry.
    let retried = retry_export(&router, &export_id, "retry-swept").await;
    assert_eq!(retried.status(), StatusCode::GONE);
    assert_eq!(error_code(&response_json(retried).await), "export_expired");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn export_download_headers_match_the_inspect_artifact_object() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let (application, router) = export_application(&base, &config).await;
    let engine = export_engine(&base);
    engine.hang_attempt(1);
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    let export_id = response_json(created).await["exportId"]
        .as_str()
        .unwrap()
        .to_owned();

    // While the attempt is live the download conflicts.
    wait_for_state(&router, &export_id, "running").await;
    let live = download_artifact(&router, &export_id).await;
    assert_eq!(live.status(), StatusCode::CONFLICT);
    assert_eq!(error_code(&response_json(live).await), "export_conflict");
    engine.release();

    // The engine writes a genuinely valid Development TIFF; the service
    // validates, publishes, and settles exactly once.
    let artifact_bytes = valid_development_tiff();
    let settled = wait_for_state(&router, &export_id, "succeeded").await;

    let icc: &[u8] =
        include_bytes!("../../../slipstream-core/assets/prophoto-linear-g10-darktable.icc");
    let artifact = &settled["artifact"];
    assert_eq!(artifact["exportId"], settled["exportId"]);
    assert_eq!(artifact["target"], "development-tiff");
    assert_eq!(artifact["stage"], "develop");
    assert_eq!(artifact["contentType"], "image/tiff");
    assert_eq!(artifact["width"], 2);
    assert_eq!(artifact["height"], 1);
    assert_eq!(
        artifact["profileIdentity"],
        format!("{:x}", Sha256::digest(icc))
    );
    assert_eq!(artifact["byteLength"], artifact_bytes.len() as u64);
    assert_eq!(
        artifact["sha256"],
        format!("{:x}", Sha256::digest(&artifact_bytes))
    );
    assert!(artifact["expiresAt"].is_string());
    assert_eq!(settled["terminalOutcome"], "succeeded");
    assert!(settled["failureReason"].is_null());
    assert!(settled["receiptExpiresAt"].is_string());

    let download = download_artifact(&router, &export_id).await;
    assert_eq!(download.status(), StatusCode::OK);
    let headers = download.headers();
    // The closed typed metadata framing is response headers; every header
    // equals the artifact object field for field.
    assert_eq!(headers["slipstream-artifact-export-id"], export_id);
    assert_eq!(headers["slipstream-artifact-target"], "development-tiff");
    assert_eq!(headers["slipstream-artifact-stage"], "develop");
    assert_eq!(headers["slipstream-artifact-content-type"], "image/tiff");
    assert_eq!(headers["slipstream-artifact-width"], "2");
    assert_eq!(headers["slipstream-artifact-height"], "1");
    assert_eq!(
        headers["slipstream-artifact-profile-identity"],
        artifact["profileIdentity"].as_str().unwrap()
    );
    assert_eq!(
        headers["slipstream-artifact-byte-length"],
        artifact["byteLength"].as_u64().unwrap().to_string()
    );
    assert_eq!(
        headers["slipstream-artifact-sha256"],
        artifact["sha256"].as_str().unwrap()
    );
    assert_eq!(
        headers["slipstream-artifact-expires-at"],
        artifact["expiresAt"].as_str().unwrap()
    );
    assert_eq!(headers["content-type"], "image/tiff");
    assert_eq!(
        headers["content-length"],
        artifact["byteLength"].as_u64().unwrap().to_string()
    );
    let body = axum::body::to_bytes(download.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(&body[..], &artifact_bytes[..]);

    // The lease is released once the response stream settles.
    let connection =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let leases = connection
            .query_row(
                "SELECT COUNT(*) FROM export_download_leases WHERE export_id = ?1",
                [&export_id],
                |row| row.get::<_, u32>(0),
            )
            .unwrap();
        if leases == 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the download lease must be released after the stream settles"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // A row whose validated metadata is incomplete can never serve a
    // download: the artifact object and the headers would not exist.
    connection
        .execute("UPDATE exports SET artifact_width = NULL", [])
        .unwrap();
    drop(connection);
    let incomplete = download_artifact(&router, &export_id).await;
    assert_eq!(incomplete.status(), StatusCode::NOT_FOUND);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn export_without_a_retained_artifact_refuses_its_download() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let engine = export_engine(&base);
    engine.fail_attempt(1, "engine-failed");
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    let export_id = response_json(created).await["exportId"]
        .as_str()
        .unwrap()
        .to_owned();

    // The attempt fails without a valid output; the download refuses with
    // the one terminal no-artifact code.
    let failed = wait_for_state(&router, &export_id, "failed").await;
    assert_eq!(failed["artifact"], serde_json::Value::Null);
    let missing = download_artifact(&router, &export_id).await;
    assert_eq!(missing.status(), StatusCode::CONFLICT);
    assert_eq!(
        error_code(&response_json(missing).await),
        "output_unavailable"
    );

    // After the artifact retention passes the download reports expiry.
    let connection =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE exports SET state='succeeded', artifact_size=?1, artifact_sha256=?2,
                   artifact_expires_at=1, artifact_width=2, artifact_height=1,
                   artifact_profile_identity=?3, settled_at=1, retain_until=?4 WHERE id=?5",
            rusqlite::params![22_i64, "f".repeat(64), "e".repeat(64), 2, export_id,],
        )
        .unwrap();
    drop(connection);
    let expired = download_artifact(&router, &export_id).await;
    assert_eq!(expired.status(), StatusCode::GONE);
    assert_eq!(
        error_code(&response_json(expired).await),
        "artifact_expired"
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// An engine that writes its output and then dies before completing the
/// protocol fails the attempt: a partial or unacknowledged result is never
/// published, and no artifact leaks from the failed attempt.
#[tokio::test]
async fn export_engine_death_after_writing_output_publishes_nothing() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let engine = export_engine(&base);
    engine.die_after_writing();
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    let export_id = response_json(created).await["exportId"]
        .as_str()
        .unwrap()
        .to_owned();

    let failed = wait_for_state(&router, &export_id, "failed").await;
    assert_eq!(failed["artifact"], serde_json::Value::Null);
    let download = download_artifact(&router, &export_id).await;
    assert_eq!(download.status(), StatusCode::CONFLICT);
    assert_eq!(
        error_code(&response_json(download).await),
        "output_unavailable"
    );
    // The written bytes never survived as a published artifact file.
    let manager = Arc::clone(application.exports.as_ref().unwrap());
    assert!(manager.artifact_path(&export_id).is_none_or(|path| {
        !path.exists() || fs::read(&path).unwrap() != valid_development_tiff()
    }));

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// Restart reconciliation validates an already-published artifact from
/// disk instead of running a second engine attempt, and resolves the
/// Export from it.
#[tokio::test]
async fn export_restart_recovers_an_already_published_artifact() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let engine = export_engine(&base);
    engine.hang_attempt(1);
    let (application, router) = export_application(&base, &config).await;
    let manager = Arc::clone(application.exports.as_ref().unwrap());
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    let export_id = response_json(created).await["exportId"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_for_state(&router, &export_id, "running").await;

    // The previous process renamed the validated file into place and
    // crashed before committing: the record still looks running with its
    // attempt, and the durable publication claim is spent.
    let artifact_bytes = valid_development_tiff();
    let path = manager.artifact_path(&export_id).unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, &artifact_bytes).unwrap();
    let incarnation = "a".repeat(32);
    let connection =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE exports SET state='running', attempt_incarnation=?1,
                   attempt_sequence=1 WHERE id=?2",
            rusqlite::params![incarnation, export_id],
        )
        .unwrap();
    drop(connection);
    // The crashed process had durably claimed this attempt's publication
    // just before renaming the validated file into place.
    application
        .library
        .claim_export_publication(&export_id, &incarnation, 1)
        .await
        .unwrap();
    // The crashed process's reconciliation runs now; it serializes behind
    // the live attempt of this process, which observes its superseded
    // record once the engine is released and aborts without publishing.
    manager.reconcile_after_restart();
    engine.release();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let settled = get_export(&router, &export_id).await;
        if settled["state"] == "succeeded" {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "reconciliation must resolve the record: {settled}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // The recovery adopted exactly the published bytes.
    let settled = get_export(&router, &export_id).await;
    assert_eq!(
        settled["artifact"]["byteLength"],
        artifact_bytes.len() as u64
    );
    assert_eq!(
        settled["artifact"]["sha256"],
        format!("{:x}", Sha256::digest(&artifact_bytes))
    );
    // The recovery must not have asked the engine for a second attempt.
    assert_eq!(engine.runs(), 1, "recovery must not run a second attempt");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A previously accepted Film identity must remain inspectable after Film is
/// withdrawn, even though a new Film submission can no longer be admitted.
/// The engine is healthy, so the replay proves receipt resolution precedes
/// the qualification gate.
#[tokio::test]
async fn unqualified_film_still_replays_an_existing_receipt() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let source_revision = current_source_revision(&application, &photo_id).await;
    let recipe = save_recipe(&application, &photo_id, "save-film", None, 0.5).await;
    let identity = "previous-film-request";
    let digest = slipstream_core::export_submission_payload_digest(
        "film-jpeg",
        &recipe.revision,
        &source_revision,
    );
    let export_id = "exp-prior-film";
    let recipe_digest = slipstream_core::ExportRecipePayload::capture(
        &slipstream_core::EditRecipeSettings {
            exposure_ev: 0.5,
            white_balance: slipstream_core::WhiteBalanceIntent::AsShot,
        },
        slipstream_core::ExportExposureRange {
            minimum_milli_ev: i64::MIN,
            maximum_milli_ev: i64::MAX,
        },
    )
    .unwrap()
    .digest();
    let connection =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    connection
        .execute(
            "INSERT INTO exports(id,photo_id,target,state,recipe_revision,exposure_ev,
                   white_balance_mode,source_revision,source_profile_id,source_kind,
                   recipe_digest,policy_id,bundle_id,workload,created_at,outcome,retain_until)
             VALUES(?1,?2,'film-jpeg','failed',?3,0.5,'as-shot',?4,
                    'sony-ilce-7rm5-arw','raw',?5,?6,?7,'film-jpeg',?8,
                    'the attempt was interrupted by a restart',?9)",
            rusqlite::params![
                export_id,
                photo_id,
                recipe.revision,
                source_revision,
                recipe_digest,
                "b".repeat(64),
                "c".repeat(64),
                1_800_000_000_i64,
                1_900_000_000_i64,
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES(?1,?2)",
            rusqlite::params![
                format!("export_receipt:{photo_id}\0{identity}"),
                serde_json::json!({
                    "payload_digest": digest,
                    "export_id": export_id,
                    "created_at": 1_800_000_000_u64,
                    "settled_at": null
                })
                .to_string(),
            ],
        )
        .unwrap();
    drop(connection);
    let body = serde_json::json!({
        "requestId": identity,
        "expectedRecipeVersion": recipe.revision,
        "expectedSourceRevision": source_revision,
        "target": "film-jpeg",
    });
    let replay = submit_export_body(&router, &photo_id, body).await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(response_json(replay).await["exportId"], export_id);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A recorded submission replays with 200 even while the local bundle is
/// unavailable or after its source has become unreadable; a different
/// payload under the recorded identity still conflicts.
#[tokio::test]
async fn export_replay_resolves_without_development_availability() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let export_id = response_json(created).await["exportId"].clone();
    application.shutdown().await.unwrap();

    // The same state directory under a deployment whose bundle is
    // unavailable still resolves the recorded identity: receipt
    // resolution precedes any admission.
    let mut unavailable = config.clone();
    let mut processing = unavailable.processing.clone().unwrap();
    processing.failure = Some("bundle-unavailable");
    unavailable.processing = Some(processing);
    let (application, router) = export_application(&base, &unavailable).await;
    let replayed = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(replayed.status(), StatusCode::OK);
    assert_eq!(response_json(replayed).await["exportId"], export_id);

    // A different payload under the recorded identity still conflicts.
    let conflict = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        "other-revision",
        &source_revision,
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(
        error_code(&response_json(conflict).await),
        "export_conflict"
    );

    // The same replay resolves after the source itself disappears:
    // receipt resolution precedes any source classification and a replay
    // never touches the source.
    fs::remove_file(config.library_root.join("pair.ARW")).unwrap();
    let unreadable = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(
        unreadable.status(),
        StatusCode::OK,
        "a recorded identity must replay even when its source is unreadable: {}",
        response_json(unreadable).await
    );
    assert_eq!(response_json(unreadable).await["exportId"], export_id);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// P2-1: Export request identities are scoped per Photo; the same
/// otherwise-valid identity on another Photo starts its own Export.
#[tokio::test]
async fn export_request_identities_are_scoped_per_photo() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    raw_fixture_with_camera(
        &config.library_root.join("second.ARW"),
        b"SONY\0",
        b"ILCE-7RM5\0\0\0",
    );
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let second_id = photo_id_for(&config, "second.ARW");
    let first = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let second = save_recipe(&application, &second_id, "save-2", None, 0.5).await;
    let first_source = current_source_revision(&application, &photo_id).await;
    let second_source = current_source_revision(&application, &second_id).await;

    let first_created = submit_export_request(
        &router,
        &photo_id,
        "shared-request",
        &first.revision,
        &first_source,
    )
    .await;
    assert_eq!(first_created.status(), StatusCode::CREATED);
    let second_created = submit_export_request(
        &router,
        &second_id,
        "shared-request",
        &second.revision,
        &second_source,
    )
    .await;
    assert_eq!(second_created.status(), StatusCode::CREATED);
    assert_ne!(
        response_json(first_created).await["exportId"],
        response_json(second_created).await["exportId"]
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// P1: the submission digest covers only the caller's payload, so a
/// legitimate deployment bundle change cannot turn an identical replay
/// into a conflict.
#[tokio::test]
async fn export_replay_survives_a_deployment_bundle_change() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let export_id = response_json(created).await["exportId"].clone();

    // A redeploy swaps the processing bundle identity; the policy stays
    // as it was.
    let mut redeployed = unresolved_processing_config();
    redeployed.bundle_sha256 = "d".repeat(64);
    let router_after = create_router_with_processing(
        Arc::clone(&application),
        open_web_root(config.web_root()),
        Some(redeployed),
    );
    let replayed = submit_export_request(
        &router_after,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(
        replayed.status(),
        StatusCode::OK,
        "an identical caller payload must replay across a bundle change: {}",
        response_json(replayed).await
    );
    assert_eq!(response_json(replayed).await["exportId"], export_id);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// P1/P2: disk recovery must adopt a published artifact only for the
/// attempt that published it; a stale file from a superseded attempt is
/// never the live attempt's output.
#[tokio::test]
async fn export_recovery_ignores_a_file_from_a_superseded_attempt() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let engine = export_engine(&base);
    engine.hang_attempt(1);
    let (application, router) = export_application(&base, &config).await;
    let manager = Arc::clone(application.exports.as_ref().unwrap());
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    let export_id = response_json(created).await["exportId"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_for_state(&router, &export_id, "running").await;
    cancel_export(&router, &export_id).await;
    wait_for_state(&router, &export_id, "cancelled").await;

    // A stale file survives from the cancelled attempt, and the record is
    // left looking like an interrupted retry of a previous process. Only
    // the stale file could make this Export succeed.
    let stale_bytes = valid_development_tiff();
    let stale_digest = format!("{:x}", Sha256::digest(&stale_bytes));
    let path = manager.artifact_path(&export_id).unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, &stale_bytes).unwrap();
    let connection =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE exports SET state='running', attempt_incarnation=?1,
                   attempt_sequence=2 WHERE id=?2",
            rusqlite::params!["a".repeat(32), export_id],
        )
        .unwrap();
    drop(connection);

    manager.reconcile_after_restart();
    let settled = loop {
        let settled = get_export(&router, &export_id).await;
        if settled["state"] == "failed" {
            break settled;
        }
        assert_ne!(
            settled["state"], "succeeded",
            "reconciliation must not adopt the superseded artifact: {settled}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let recorded_sha = settled["artifact"]["sha256"].as_str().unwrap_or("absent");
    assert_ne!(
        recorded_sha, stale_digest,
        "recovery must not adopt a superseded attempt's file as the retry's output"
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// P2: the download lease holds and keeps renewing until the response
/// body drains or is dropped, not merely until the producer reaches end
/// of file.
#[tokio::test]
async fn export_download_holds_its_lease_until_the_body_drains() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let (application, router) = export_application(&base, &config).await;
    let manager = Arc::clone(application.exports.as_ref().unwrap());
    manager.set_lease_renewal_interval(Duration::from_millis(150));
    let photo_id = photo_id_for(&config, "pair.ARW");
    let export_id = "exp-lease-drain-check".to_owned();
    let artifact_bytes = vec![9_u8; 64 * 1024];
    let digest = format!("{:x}", Sha256::digest(&artifact_bytes));
    let recipe_digest = slipstream_core::ExportRecipePayload::capture(
        &slipstream_core::EditRecipeSettings {
            exposure_ev: 0.0,
            white_balance: slipstream_core::WhiteBalanceIntent::AsShot,
        },
        slipstream_core::ExportExposureRange {
            minimum_milli_ev: i64::MIN,
            maximum_milli_ev: i64::MAX,
        },
    )
    .unwrap()
    .digest();
    let path = manager.artifact_path(&export_id).unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, &artifact_bytes).unwrap();
    let connection =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    connection
        .execute(
            "INSERT INTO exports(id,photo_id,target,state,recipe_revision,exposure_ev,
                   white_balance_mode,source_revision,source_profile_id,source_kind,
                   recipe_digest,policy_id,bundle_id,workload,created_at,outcome,
                   artifact_size,artifact_sha256,artifact_expires_at,artifact_width,
                   artifact_height,artifact_profile_identity,settled_at,retain_until)
                 VALUES(?1,?2,'development-tiff','succeeded','rev',0.0,'as-shot','src',
                   'profile','raw',?3,?4,?5,'development-tiff',1,NULL,?6,?7,?8,2,1,?9,
                   1800000000,1900000000)",
            rusqlite::params![
                export_id,
                photo_id,
                recipe_digest,
                "b".repeat(64),
                "c".repeat(64),
                artifact_bytes.len() as i64,
                digest,
                1_900_000_000_i64,
                "e".repeat(64),
            ],
        )
        .unwrap();
    drop(connection);

    let download = download_artifact(&router, &export_id).await;
    assert_eq!(download.status(), StatusCode::OK);
    // The producer has certainly reached end of file by now, but the
    // response body was never consumed: the lease must still exist.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let held = {
        let connection =
            rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
        connection
            .query_row(
                "SELECT COUNT(*) FROM export_download_leases WHERE export_id = ?1",
                [&export_id],
                |row| row.get::<_, u32>(0),
            )
            .unwrap()
    };
    assert_eq!(
        held, 1,
        "an undrained response body must keep its download lease"
    );

    // A live stream keeps its lease renewed, so the staleness sweep
    // cannot reclaim it while the body is still undrained.
    let first_renewal: i64 = {
        let connection =
            rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
        connection
            .query_row(
                "SELECT created_at FROM export_download_leases WHERE export_id = ?1",
                [&export_id],
                |row| row.get(0),
            )
            .unwrap()
    };
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let renewed_at: i64 = {
        let connection =
            rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
        connection
            .query_row(
                "SELECT created_at FROM export_download_leases WHERE export_id = ?1",
                [&export_id],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert!(
        renewed_at > first_renewal,
        "a live stream must keep its lease renewed: {first_renewal} then {renewed_at}"
    );
    drop(download);

    // Dropping the body settles the stream and releases the lease.
    let connection =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let leases = connection
            .query_row(
                "SELECT COUNT(*) FROM export_download_leases WHERE export_id = ?1",
                [&export_id],
                |row| row.get::<_, u32>(0),
            )
            .unwrap();
        if leases == 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the download lease must be released after the body is dropped"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    drop(connection);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

async fn edit_preview_request(router: &Router, photo_id: &str) -> Response<Body> {
    send(
        router,
        authenticated_request()
            .uri(format!(
                "http://camera.local/api/photos/{photo_id}/edit-preview/develop"
            ))
            .header("slipstream-cli-contract", "1")
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

async fn edit_preview_settings_request(
    router: &Router,
    photo_id: &str,
    settings: &str,
) -> Response<Body> {
    send(
        router,
        authenticated_request()
            .uri(format!(
                "http://camera.local/api/photos/{photo_id}/edit-preview/develop?settings={settings}"
            ))
            .header("slipstream-cli-contract", "1")
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

/// The Edit Preview derives the `develop` rendition from the retained
/// Development TIFF of a succeeded Export while that Export's captured
/// identity is the current one, then admits a preview-class render after a
/// later recipe makes the retained result stale.
#[tokio::test]
async fn edit_preview_derives_from_the_retained_development_tiff_of_an_export() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let engine = export_engine(&base);
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    let export_id = response_json(created).await["exportId"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_for_state(&router, &export_id, "succeeded").await;

    // The published Development TIFF is the retained Development Result of
    // exactly this identity, so the develop rendition derives from it.
    let preview = edit_preview_request(&router, &photo_id).await;
    assert_eq!(preview.status(), StatusCode::OK);
    let headers = preview.headers();
    assert_eq!(headers["slipstream-edit-preview-stage"], "develop");
    assert_eq!(
        headers["slipstream-edit-preview-recipe-version"],
        recipe.revision
    );
    assert_eq!(
        headers["slipstream-edit-preview-source-revision"],
        crate::queries::hex_encode(source_revision.as_bytes())
    );
    assert_eq!(headers["slipstream-edit-preview-width"], "2");
    assert_eq!(headers["slipstream-edit-preview-height"], "1");
    assert_eq!(headers["content-type"], "image/jpeg");
    let declared_sha256 = headers["slipstream-edit-preview-sha256"]
        .to_str()
        .unwrap()
        .to_owned();
    let body = axum::body::to_bytes(preview.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(&body[..2], &[0xff, 0xd8], "the rendition is a JPEG");
    assert_eq!(
        declared_sha256,
        format!("{:x}", Sha256::digest(&body)),
        "the rendition's content digest is the one it declares"
    );

    // A later recipe is another identity. The retained Development TIFF is
    // no longer current, so the service admits a preview-class render
    // through the same processing workload instead of refusing.
    let second = save_recipe(
        &application,
        &photo_id,
        "save-2",
        Some(recipe.revision.clone()),
        0.25,
    )
    .await;
    assert_ne!(second.revision, recipe.revision);
    let admitted = edit_preview_request(&router, &photo_id).await;
    assert_eq!(admitted.status(), StatusCode::ACCEPTED);
    let payload = response_json(admitted).await;
    assert!(
        payload["state"] == "queued" || payload["state"] == "running",
        "preview admission state is queued or running: {payload}"
    );
    assert_eq!(payload["stage"], "develop");
    let repeated = edit_preview_request(&router, &photo_id).await;
    assert_eq!(repeated.status(), StatusCode::ACCEPTED);
    let repeated_payload = response_json(repeated).await;
    assert_eq!(repeated_payload["state"], "running");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let response = edit_preview_request(&router, &photo_id).await;
        if response.status() == StatusCode::OK {
            break;
        }
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert!(
            tokio::time::Instant::now() < deadline,
            "preview-class render did not become a rendition"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // One durable and one coalesced preview attempt reached the engine.
    engine.wait_for_runs(2).await;
    assert_eq!(engine.runs(), 2, "one Export and one coalesced render");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// The baseline comparison selector serves the as-shot/baseline
/// development from the retained result captured at exactly those
/// settings, independently of the saved recipe revision, and it keeps
/// serving while the saved recipe moves on.
#[tokio::test]
async fn edit_preview_serves_the_baseline_comparison_independently_of_the_recipe() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    // The saved recipe is the processing baseline itself: 0 EV, as-shot.
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.0).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    let export_id = response_json(created).await["exportId"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_for_state(&router, &export_id, "succeeded").await;

    let current = edit_preview_request(&router, &photo_id).await;
    assert_eq!(current.status(), StatusCode::OK);
    let current_sha = current.headers()["slipstream-edit-preview-sha256"]
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        current.headers()["slipstream-edit-preview-settings"],
        "current"
    );
    assert_eq!(
        current.headers()["slipstream-edit-preview-recipe-version"],
        recipe.revision
    );

    // The comparison is the baseline development: the same retained
    // result, named as the baseline rather than as the saved recipe.
    let baseline = edit_preview_settings_request(&router, &photo_id, "baseline").await;
    assert_eq!(baseline.status(), StatusCode::OK);
    assert_eq!(
        baseline.headers()["slipstream-edit-preview-settings"],
        "baseline"
    );
    assert_eq!(
        baseline.headers()["slipstream-edit-preview-recipe-version"],
        "",
        "a baseline rendition is not a saved recipe's"
    );
    assert_eq!(
        baseline.headers()["slipstream-edit-preview-sha256"],
        current_sha,
        "the baseline of a baseline recipe is the same development"
    );

    // A later edit moves the current identity. The comparison is
    // unchanged — it still resolves from the retained baseline result —
    // while the current rendition is no longer retained and is admitted
    // as its own render work.
    let second = save_recipe(
        &application,
        &photo_id,
        "save-2",
        Some(recipe.revision.clone()),
        0.5,
    )
    .await;
    assert_ne!(second.revision, recipe.revision);
    let baseline = edit_preview_settings_request(&router, &photo_id, "baseline").await;
    assert_eq!(baseline.status(), StatusCode::OK);
    assert_eq!(
        baseline.headers()["slipstream-edit-preview-sha256"],
        current_sha
    );
    let admitted = edit_preview_request(&router, &photo_id).await;
    assert_eq!(admitted.status(), StatusCode::ACCEPTED);
    let payload = response_json(admitted).await;
    assert!(
        payload["state"] == "queued" || payload["state"] == "running",
        "the edited recipe is new render work: {payload}"
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A stored white-balance mode the closed execution payload cannot
/// represent is retained intent, not render work: the route refuses it
/// before admitting an attempt that could never produce a result, so a
/// polling client is told the stage is unavailable instead of being handed
/// queued work that fails and is re-admitted forever.
#[tokio::test]
async fn edit_preview_refuses_a_recipe_the_closed_payload_cannot_execute() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let engine = export_engine(&base);
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let source_revision = current_source_revision(&application, &photo_id).await;
    let outcome = application
        .library
        .save_edit_recipe(slipstream_core::SaveEditRecipe {
            photo_id: photo_id.clone(),
            request_id: "save-temperature-tint".to_owned(),
            expected_recipe_version: None,
            expected_source_revision: source_revision,
            settings: slipstream_core::EditRecipeSettings {
                exposure_ev: 0.25,
                white_balance: slipstream_core::WhiteBalanceIntent::TemperatureTint {
                    temperature_kelvin: 6_500,
                    tint_milli: -12,
                },
            },
        })
        .await
        .unwrap();
    assert!(
        matches!(
            outcome,
            slipstream_core::EditRecipeWriteOutcome::Saved(_)
                | slipstream_core::EditRecipeWriteOutcome::Unchanged(_)
        ),
        "an adjustable white balance is accepted as editing intent: {outcome:?}"
    );

    let refused = edit_preview_request(&router, &photo_id).await;
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    let payload = response_json(refused).await;
    assert_eq!(payload["error"]["code"], "processing_unavailable");
    assert_eq!(payload["error"]["details"]["stage"], "develop");
    assert_eq!(
        payload["error"]["details"]["reason"],
        "recipe-not-representable"
    );
    assert_eq!(
        engine.runs(),
        0,
        "a refused identity starts no processing attempt"
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A preview-class attempt that fails is released, not remembered: the
/// next request admits a new attempt instead of reporting `running` for
/// work that no longer exists, and the failed attempt's ephemeral output
/// is never served as a rendition.
#[tokio::test]
async fn edit_preview_re_admits_after_a_failed_render() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let engine = export_engine(&base);
    engine.fail_attempt(1, "engine-failed");
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    save_recipe(&application, &photo_id, "save-1", None, 0.4).await;

    let admitted = edit_preview_request(&router, &photo_id).await;
    assert_eq!(admitted.status(), StatusCode::ACCEPTED);
    assert_eq!(response_json(admitted).await["state"], "queued");

    // The route keeps answering the admission while the attempt is live,
    // and once it has failed the next request admits a new one.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let response = edit_preview_request(&router, &photo_id).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        if response_json(response).await["state"] == "queued" {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "a failed render must be released for a new attempt"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // The new attempt reaches the engine from a background task, so the
    // count is observed rather than assumed.
    engine.wait_for_runs(2).await;
    assert_eq!(
        engine.runs(),
        2,
        "the failed attempt is re-admitted as new work"
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A newer intent supersedes a live render: the engine attempt is
/// cancelled, so the stale attempt neither occupies the serialized
/// processing slot nor publishes, and the new identity is admitted as
/// its own engine attempt.
#[tokio::test]
async fn edit_preview_supersedes_a_live_render_with_the_newer_intent() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let engine = export_engine(&base);
    engine.hang_attempt(1);
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let first = save_recipe(&application, &photo_id, "save-1", None, 0.2).await;
    let admitted = edit_preview_request(&router, &photo_id).await;
    assert_eq!(admitted.status(), StatusCode::ACCEPTED);

    // The first attempt is live on the engine before the newer intent
    // arrives, so the newer intent has to release it.
    engine.wait_for_runs(1).await;

    let second = save_recipe(
        &application,
        &photo_id,
        "save-2",
        Some(first.revision.clone()),
        0.6,
    )
    .await;
    assert_ne!(second.revision, first.revision);
    let superseded = edit_preview_request(&router, &photo_id).await;
    assert_eq!(superseded.status(), StatusCode::ACCEPTED);
    assert_eq!(response_json(superseded).await["state"], "queued");

    // The superseded attempt is cancelled — its held engine run is torn
    // down — and the freed slot admits the newer identity as a second
    // engine attempt, which then serves its rendition.
    engine.release();
    engine.wait_for_runs(2).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let response = edit_preview_request(&router, &photo_id).await;
        if response.status() == StatusCode::OK {
            assert_eq!(
                response.headers()["slipstream-edit-preview-recipe-version"],
                second.revision
            );
            break;
        }
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert!(
            tokio::time::Instant::now() < deadline,
            "the newer intent must become the served rendition"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn xmp_snapshot_download_and_replay_do_not_need_processing_or_original() {
    let (base, config) = prepare_populated_fixture();
    raw_fixture_with_camera(
        &config.library_root.join("pair.ARW"),
        b"SONY\0",
        b"ILCE-7RM5\0\0\0",
    );
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-xmp", None, 0.5).await;
    let uri = format!("https://camera.local/api/photos/{photo_id}/edit-state-exports");
    let body = serde_json::json!({"requestId":"xmp-snapshot", "expectedRecipeVersion":recipe.revision, "expectedSourceRevision":recipe.source_revision});
    let created = post_export_body(&router, uri.clone(), body.clone()).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = response_json(created).await;
    let export_id = created["exportId"].as_str().unwrap().to_owned();
    let artifact_uri = format!("{uri}/{export_id}/artifact");
    let download = send(
        &router,
        authenticated_request()
            .uri(&artifact_uri)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(download.status(), StatusCode::OK);
    assert_eq!(download.headers()["content-type"], "application/rdf+xml");
    assert_eq!(
        download.headers()["slipstream-artifact-filename"],
        created["artifact"]["filename"].as_str().unwrap()
    );
    assert_eq!(
        download.headers()["slipstream-artifact-sha256"],
        created["artifact"]["sha256"].as_str().unwrap()
    );
    let bytes = axum::body::to_bytes(download.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        bytes.len() as u64,
        created["artifact"]["byteLength"].as_u64().unwrap()
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(&bytes)),
        created["artifact"]["sha256"].as_str().unwrap()
    );
    assert!(
        std::str::from_utf8(&bytes)
            .unwrap()
            .contains("<crs:Exposure2012>0.5</crs:Exposure2012>")
    );
    save_recipe(
        &application,
        &photo_id,
        "save-xmp-later",
        Some(recipe.revision),
        1.0,
    )
    .await;
    fs::remove_file(config.library_root.join("pair.ARW")).unwrap();
    let replay = post_export_body(&router, uri.clone(), body.clone()).await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(response_json(replay).await, created);
    let listed = response_json(
        send(
            &router,
            authenticated_request()
                .uri(&uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(listed["exports"], serde_json::json!([created]));
    let mut conflicting = body.clone();
    conflicting["expectedRecipeVersion"] = serde_json::json!("different");
    let conflict = post_export_body(&router, uri.clone(), conflicting).await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(
        error_code(&response_json(conflict).await),
        "export_conflict"
    );
    for request_id in ["contains space", "bad/slash", "nonascii-ñ"] {
        let mut invalid = body.clone();
        invalid["requestId"] = serde_json::json!(request_id);
        assert_eq!(
            post_export_body(&router, uri.clone(), invalid)
                .await
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let mut stale = body.clone();
    stale["requestId"] = serde_json::json!("new-request");
    assert_eq!(
        post_export_body(&router, uri.clone(), stale).await.status(),
        StatusCode::CONFLICT
    );
    let connection =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE xmp_exports SET expires_at=created_at WHERE id=?",
            [&export_id],
        )
        .unwrap();
    drop(connection);
    let expired = post_export_body(&router, uri, body).await;
    assert_eq!(expired.status(), StatusCode::GONE);
    assert_eq!(error_code(&response_json(expired).await), "export_expired");
    assert_eq!(
        send(
            &router,
            authenticated_request()
                .uri(&artifact_uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .status(),
        StatusCode::GONE
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
