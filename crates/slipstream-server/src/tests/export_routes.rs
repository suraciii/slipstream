// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Export routes
// ---------------------------------------------------------------------------
use super::*;

use crate::export_manager::development_tiff_decode::{stored_zlib, write_development_tiff};
use crate::http::create_router_with_processing;
use sha2::{Digest, Sha256};
use std::time::Duration;

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

async fn seed_historical_export(
    application: &Application,
    photo_id: &str,
    recipe_revision: &str,
    source: &str,
) -> String {
    let outcome = application
        .library
        .submit_export(slipstream_core::ExportSubmission {
            request_id: "historical-fixture".to_owned(),
            photo_id: photo_id.to_owned(),
            source_profile_id: "sony-ilce-7rm5-arw".to_owned(),
            workload: "development-tiff".to_owned(),
            policy_id: "b".repeat(64),
            bundle_id: "c".repeat(64),
            expected_recipe_revision: recipe_revision.to_owned(),
            expected_source_revision: source.to_owned(),
            exposure_range: slipstream_core::ExportExposureRange {
                minimum_milli_ev: 0,
                maximum_milli_ev: 1000,
            },
            retained_output_bytes_max: 64 * 1024 * 1024 * 1024,
        })
        .await
        .unwrap();
    let slipstream_core::ExportSubmitOutcome::Created(record) = outcome else {
        panic!("historical fixture failed: {outcome:?}")
    };
    record.id
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

/// Restart reconciliation validates an already-published artifact from
/// disk instead of running a second engine attempt, and resolves the
/// Export from it.
#[tokio::test]
async fn export_restart_recovers_an_already_published_artifact() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let engine = export_engine(&base);
    let (application, router) = export_application(&base, &config).await;
    let manager = Arc::clone(application.exports.as_ref().unwrap());
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let export_id =
        seed_historical_export(&application, &photo_id, &recipe.revision, &source_revision).await;

    // The previous process renamed the validated file into place and
    // crashed before committing: the record still looks running with its
    // attempt, and the durable publication claim is spent.
    let artifact_bytes = valid_development_tiff();
    let path = manager
        .artifact_path_for_workload(&export_id, "development-tiff")
        .unwrap();
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
    // Recovery adopts the interrupted process's durable publication claim.
    manager.reconcile_after_restart();
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
    assert_eq!(
        engine.runs(),
        0,
        "historical recovery must never launch an engine"
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// P1/P2: disk recovery must adopt a published artifact only for the
/// attempt that published it; a stale file from a superseded attempt is
/// never the live attempt's output.
#[tokio::test]
async fn export_recovery_ignores_a_file_from_a_superseded_attempt() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let (application, router) = export_application(&base, &config).await;
    let manager = Arc::clone(application.exports.as_ref().unwrap());
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let export_id =
        seed_historical_export(&application, &photo_id, &recipe.revision, &source_revision).await;
    application.library.cancel_export(&export_id).await.unwrap();

    // A stale file survives from the cancelled attempt, and the record is
    // left looking like an interrupted retry of a previous process. Only
    // the stale file could make this Export succeed.
    let stale_bytes = valid_development_tiff();
    let stale_digest = format!("{:x}", Sha256::digest(&stale_bytes));
    let path = manager
        .artifact_path_for_workload(&export_id, "development-tiff")
        .unwrap();
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
    let path = manager
        .artifact_path_for_workload(&export_id, "development-tiff")
        .unwrap();
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
    let source = current_source_revision(&application, &photo_id).await;
    let step_id = slipstream_core::ProcessingStepId::new("xmp-step").unwrap();
    let intended = slipstream_core::ComposableEditRecipe {
        photo_id: photo_id.clone(), revision: "draft".into(), source_revision: source.clone(),
        current_step_id: Some(step_id.clone()),
        steps: vec![slipstream_core::ProcessingStep {
            step_id,
            module: slipstream_core::ProcessingModuleId::new("darktable").unwrap(),
            input: slipstream_core::ProcessingInput::Original { photo_id: photo_id.clone(), source_revision: source.clone() },
            parameters: slipstream_core::ProcessingParameterSnapshot::new("darktable-params-1", serde_json::json!({"stack":[
                {"operation":"exposure","multiPriority":0,"enabled":true,"params":{"mode":"EXPOSURE_MODE_MANUAL","exposure":0.5}},
                {"operation":"temperature","multiPriority":0,"enabled":true,"params":{"temperatureKelvin":6500,"tintMilli":-12}}
            ]})).unwrap(),
        }],
    };
    let outcome = application
        .library
        .save_composable_edit_recipe(slipstream_core::SaveComposableEditRecipe {
            photo_id: photo_id.clone(),
            request_id: "save-xmp".into(),
            expected_recipe_revision: None,
            expected_source_revision: source.clone(),
            recipe: intended,
            automatic_adjustment: None,
        })
        .await
        .unwrap();
    let slipstream_core::ComposableEditRecipeWriteOutcome::Saved(recipe) = outcome else {
        panic!("XMP recipe not saved: {outcome:?}")
    };
    let uri = format!("https://camera.local/api/photos/{photo_id}/edit-state-exports");
    let body = serde_json::json!({"requestId":"xmp-snapshot", "expectedRecipeVersion":recipe.revision, "expectedSourceRevision":recipe.source_revision});
    let created = post_export_body(&router, uri.clone(), body.clone()).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = response_json(created).await;
    assert_eq!(
        created["parameterSupport"]["standard"],
        serde_json::json!(["Exposure2012"])
    );
    assert_eq!(
        created["parameterSupport"]["unsupported"],
        serde_json::json!(["Arbitrary darktable controls"])
    );
    assert!(
        created["parameterSupport"]["slipstream"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == "RecipeSnapshot")
    );
    assert!(
        created["parameterSupport"]["slipstream"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name == "RecipeSnapshotEncoding")
    );
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
    let mut changed = recipe.clone();
    changed.steps[0].module = slipstream_core::ProcessingModuleId::new("spektrafilm").unwrap();
    changed.steps[0].parameters = slipstream_core::ProcessingParameterSnapshot::new(
        "spektrafilm-params-1",
        serde_json::json!({}),
    )
    .unwrap();
    let changed = application
        .library
        .save_composable_edit_recipe(slipstream_core::SaveComposableEditRecipe {
            photo_id: photo_id.clone(),
            request_id: "save-xmp-later".into(),
            expected_recipe_revision: Some(recipe.revision.clone()),
            expected_source_revision: source,
            recipe: changed,
            automatic_adjustment: None,
        })
        .await
        .unwrap();
    let slipstream_core::ComposableEditRecipeWriteOutcome::Saved(changed) = changed else {
        panic!("changed XMP recipe not saved")
    };
    let refused = post_export_body(
        &router,
        uri.clone(),
        serde_json::json!({
            "requestId":"xmp-wrong-module", "expectedRecipeVersion":changed.revision,
            "expectedSourceRevision":changed.source_revision
        }),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        error_code(&response_json(refused).await),
        "unsupported_module"
    );
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
    let retained_download = send(
        &router,
        authenticated_request()
            .uri(&artifact_uri)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(retained_download.status(), StatusCode::OK);
    assert_eq!(
        axum::body::to_bytes(retained_download.into_body(), usize::MAX)
            .await
            .unwrap(),
        bytes
    );
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
    // Earlier acknowledged XMP bytes carry no complete recipe snapshot.
    // Reads and replays must not advertise a newer generator's properties.
    let mut historical = std::str::from_utf8(&bytes).unwrap().to_owned();
    let start = historical.find("<slip:RecipeSnapshot>").unwrap();
    let end = historical.find("</slip:RecipeSnapshot>").unwrap() + "</slip:RecipeSnapshot>".len();
    historical.replace_range(start..end, "");
    historical = historical.replace(
        "<slip:RecipeSnapshotEncoding>json-utf8</slip:RecipeSnapshotEncoding>",
        "",
    );
    let historical_digest = format!("{:x}", Sha256::digest(historical.as_bytes()));
    let connection =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE xmp_exports SET document=?,byte_length=?,sha256=? WHERE id=?",
            rusqlite::params![
                historical.as_bytes(),
                historical.len() as i64,
                historical_digest,
                export_id
            ],
        )
        .unwrap();
    drop(connection);
    let replay = post_export_body(&router, uri.clone(), body.clone()).await;
    assert_eq!(replay.status(), StatusCode::OK);
    let historical_record = response_json(replay).await;
    let support = historical_record["parameterSupport"]["slipstream"]
        .as_array()
        .unwrap();
    assert!(!support.iter().any(|name| name == "RecipeSnapshot"));
    assert!(!support.iter().any(|name| name == "RecipeSnapshotEncoding"));
    assert!(support.iter().any(|name| name == "StepId"));
    assert_eq!(historical_record["artifact"]["sha256"], historical_digest);
    let (status, listed) = get_json(
        &router,
        &format!("/api/photos/{photo_id}/edit-state-exports"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listed["exports"], serde_json::json!([historical_record]));
    let download = send(
        &router,
        authenticated_request()
            .uri(&artifact_uri)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(download.status(), StatusCode::OK);
    assert_eq!(
        download.headers()["slipstream-artifact-sha256"],
        historical_digest
    );
    assert_eq!(
        axum::body::to_bytes(download.into_body(), usize::MAX)
            .await
            .unwrap()
            .as_ref(),
        historical.as_bytes()
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

#[tokio::test]
async fn historical_queued_work_settles_interrupted_without_engine_execution() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "historical-recipe", None, 0.5).await;
    let source = current_source_revision(&application, &photo_id).await;
    let export_id =
        seed_historical_export(&application, &photo_id, &recipe.revision, &source).await;
    application
        .exports
        .as_ref()
        .unwrap()
        .reconcile_after_restart();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let terminal = loop {
        let record = get_export(&router, &export_id).await;
        if record["state"] == "failed" {
            break record;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "historical queued work did not settle: {record}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(
        terminal["failureReason"],
        "the attempt was interrupted by a restart"
    );
    assert_eq!(terminal["terminalOutcome"], "failed");
    assert_eq!(export_engine(&base).runs(), 0);
    assert!(
        application
            .library
            .export(&export_id)
            .await
            .unwrap()
            .unwrap()
            .attempt
            .is_none()
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
