use super::*;
// ---------------------------------------------------------------- real service

/// The two Originals of the shared fixture: JPEG sources only, with
/// processing explicitly the operator-disabled configuration, so the
/// deployment answers deterministically without an engine bundle. The
/// environment default is `auto`, which reports `bundle-unavailable` when
/// the bundle is missing — not the `disabled` condition asserted here.
fn real_service_fixture() -> (PathBuf, Config) {
    let base = temp_base("real-service");
    let originals = base.join("originals");
    let web = base.join("web");
    fs::create_dir(&originals).unwrap();
    fs::create_dir(originals.join("trip")).unwrap();
    fs::create_dir(&web).unwrap();
    fs::write(web.join("index.html"), b"<main>fixture</main>").unwrap();
    for name in ["one.JPG", "two.JPG"] {
        fs::write(originals.join("trip").join(name), jpeg_bytes()).unwrap();
    }
    let config = Config {
        library_root: originals,
        state_directory: base.join("state"),
        cache_directory: base.join("cache"),
        database_basename: "library.sqlite".to_owned(),
        host: "127.0.0.1".to_owned(),
        public_origin: "https://localhost".to_owned(),
        access_origins: vec!["https://localhost".to_owned()],
        port: 0,
        web_root: Some(web),
        processing: None,
        export_retained_output_bytes: None,
        metadata_supervisor: None,
    };
    (base, config)
}

async fn wait_until_idle(server: &str) {
    let client = reqwest::Client::builder()
        .add_root_certificate(common::test_certificate())
        .build()
        .unwrap();
    for _ in 0..400 {
        let response = client
            .get(format!("{server}/api/status"))
            .header("Slipstream-CLI-Contract", "1")
            .bearer_auth(common::ACCESS_TOKEN)
            .send()
            .await
            .unwrap();
        let result: Value = response.json().await.unwrap();
        if result["scan"]["state"] == "idle" {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("fixture Library did not become idle");
}

/// Discovery reports each peer's availability and parameter schema even
/// when processing is disabled in this deployment.
#[tokio::test]
async fn the_real_service_module_discovery_preserves_peer_refusals() {
    let (base, config) = real_service_fixture();
    let server = common::start_authenticated_server(config).await;
    wait_until_idle(&server.url).await;
    let (exit, report) = command(&server.url, &["processing", "modules"]).await;
    assert_eq!(exit, 0);
    assert_eq!(
        report["data"]["contractVersion"],
        "slipstream-module-contract-1"
    );
    let modules = report["data"]["modules"].as_array().unwrap();
    assert_eq!(modules.len(), 2);
    for module in modules {
        assert_eq!(module["availability"]["state"], "unavailable");
        assert!(module["parameterSchema"].is_object());
        assert!(module["id"]["adapterVersion"].is_string());
    }
    server.close().await.unwrap();
    fs::remove_dir_all(base).unwrap();
}

/// The composable recipe reads, saves, and replays through the real
/// authenticated service: an absent recipe is a successful read, a zero-step
/// save commits the caller's exact guards, the same request identity
/// confirms its retained receipt, and the next read observes the committed
/// recipe with its server-assigned revision.
#[tokio::test]
async fn the_real_service_round_trips_the_composable_recipe() {
    let (base, config) = real_service_fixture();
    let server = common::start_authenticated_server(config).await;
    wait_until_idle(&server.url).await;
    let (exit, page) = command(&server.url, &["photos", "list", "--limit", "60"]).await;
    assert_eq!(exit, 0);
    let photo_id = page["data"]["items"][0]["id"].as_str().unwrap().to_owned();

    let (exit, read) = command(
        &server.url,
        &["photos", "processing-recipe", "get", &photo_id],
    )
    .await;
    assert_eq!(exit, 0, "{read}");
    assert_eq!(read["data"]["photoId"], photo_id);
    assert_eq!(read["data"]["recipe"], Value::Null);
    let source_revision = read["data"]["sourceRevision"].as_str().unwrap().to_owned();
    assert!(!source_revision.is_empty());

    let save = json!({
        "requestId": "composable-real-1",
        "expectedRecipeRevision": Value::Null,
        "expectedSourceRevision": source_revision,
        "currentStepId": null,
        "steps": [],
    });
    let input = base.join("composable.json");
    fs::write(&input, save.to_string()).unwrap();
    let (exit, saved) = command(
        &server.url,
        &[
            "photos",
            "processing-recipe",
            "save",
            &photo_id,
            "--input",
            input.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(exit, 0, "{saved}");
    assert_eq!(saved["data"]["outcome"], "saved");
    assert_eq!(saved["data"]["requestId"], "composable-real-1");
    assert_eq!(saved["data"]["sourceRevision"], source_revision);
    let committed = saved["data"]["recipeVersion"].as_str().unwrap().to_owned();
    assert!(!committed.is_empty());
    assert_eq!(saved["data"]["recipe"]["revision"], committed.as_str());
    assert_eq!(saved["data"]["recipe"]["steps"], json!([]));

    // The same request identity replays its retained receipt.
    let (exit, replayed) = command(
        &server.url,
        &[
            "photos",
            "processing-recipe",
            "save",
            &photo_id,
            "--input",
            input.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(exit, 0, "{replayed}");
    assert_eq!(replayed["data"]["outcome"], "replayed");
    assert_eq!(replayed["data"]["recipeVersion"], committed.as_str());

    let (exit, reread) = command(
        &server.url,
        &["photos", "processing-recipe", "get", &photo_id],
    )
    .await;
    assert_eq!(exit, 0, "{reread}");
    assert_eq!(reread["data"]["recipe"]["revision"], committed.as_str());
    assert_eq!(reread["data"]["recipe"]["steps"], json!([]));
    server.close().await.unwrap();
    fs::remove_dir_all(base).unwrap();
}

/// A composable save guarded by a stale source revision is a confirmed
/// conflict: exit 4 with the service's refusal and no unknown outcome.
#[tokio::test]
async fn the_real_service_refuses_a_stale_composable_source_guard() {
    let (base, config) = real_service_fixture();
    let server = common::start_authenticated_server(config).await;
    wait_until_idle(&server.url).await;
    let (exit, page) = command(&server.url, &["photos", "list", "--limit", "60"]).await;
    assert_eq!(exit, 0);
    let photo_id = page["data"]["items"][0]["id"].as_str().unwrap().to_owned();
    let input = base.join("stale.json");
    fs::write(
        &input,
        json!({
            "requestId": "composable-stale",
            "expectedRecipeRevision": null,
            "expectedSourceRevision": "stale-source-revision",
            "currentStepId": null,
            "steps": [],
        })
        .to_string(),
    )
    .unwrap();
    let (exit, refusal) = command(
        &server.url,
        &[
            "photos",
            "processing-recipe",
            "save",
            &photo_id,
            "--input",
            input.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(exit, 4, "{refusal}");
    assert_eq!(refusal["error"]["code"], "source_changed");
    assert_eq!(refusal["error"]["effect"], "none");
    server.close().await.unwrap();
    fs::remove_dir_all(base).unwrap();
}

/// The Processing Preview is bounded to the recipe's selected current step:
/// a non-current step is a confirmed conflict, the current step of a saved
/// recipe is refused explicitly while no qualified adapter is configured,
/// and neither refusal creates the destination file.
#[tokio::test]
async fn the_real_service_bounds_the_processing_preview_to_the_selected_step() {
    let (base, config) = real_service_fixture();
    let server = common::start_authenticated_server(config).await;
    wait_until_idle(&server.url).await;
    let (exit, page) = command(&server.url, &["photos", "list", "--limit", "60"]).await;
    assert_eq!(exit, 0);
    let photo_id = page["data"]["items"][0]["id"].as_str().unwrap().to_owned();
    let (exit, read) = command(
        &server.url,
        &["photos", "processing-recipe", "get", &photo_id],
    )
    .await;
    assert_eq!(exit, 0);
    let source_revision = read["data"]["sourceRevision"].as_str().unwrap().to_owned();

    // Save one darktable step bound to the guarded Original and select it.
    let input = base.join("step.json");
    fs::write(
        &input,
        json!({
            "requestId": "composable-step",
            "expectedRecipeRevision": null,
            "expectedSourceRevision": source_revision,
            "currentStepId": "develop-1",
            "steps": [{
                "stepId": "develop-1",
                "module": "darktable",
                "input": {
                    "kind": "original",
                    "photoId": photo_id,
                    "sourceRevision": source_revision,
                },
                "parameters": {
                    "schemaVersion": "darktable-params-1",
                    "tree": {"stack": [], "output": {
                        "format": "tiff",
                        "precisionBits": 32,
                        "colorSpace": "prophoto-rgb",
                        "transferFunction": "linear"
                    }},
                },
            }],
        })
        .to_string(),
    )
    .unwrap();
    let (exit, saved) = command(
        &server.url,
        &[
            "photos",
            "processing-recipe",
            "save",
            &photo_id,
            "--input",
            input.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(exit, 0, "{saved}");

    let destination = base.join("preview.jpg");
    let (exit, refusal) = command(
        &server.url,
        &[
            "photos",
            "processing-preview",
            &photo_id,
            "--step",
            "not-a-step",
            "--file",
            destination.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(exit, 4, "{refusal}");
    assert_eq!(refusal["error"]["code"], "step_not_current");
    assert_eq!(refusal["error"]["effect"], "none");
    assert!(!destination.exists());

    // The selected current step passes the step bound and is refused
    // explicitly — never delegated to a legacy preview — while this
    // deployment has no processing runtime.
    let (exit, refusal) = command(
        &server.url,
        &[
            "photos",
            "processing-preview",
            &photo_id,
            "--step",
            "develop-1",
            "--file",
            destination.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(exit, 6, "{refusal}");
    assert_eq!(refusal["error"]["code"], "processing_unavailable");
    assert_eq!(refusal["error"]["effect"], "none");
    assert!(!destination.exists());
    server.close().await.unwrap();
    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn the_real_service_lists_no_exports_or_artifacts_before_any_submission() {
    let (base, config) = real_service_fixture();
    let server = common::start_authenticated_server(config).await;
    wait_until_idle(&server.url).await;
    let (exit, page) = command(&server.url, &["photos", "list", "--limit", "60"]).await;
    assert_eq!(exit, 0, "{page}");
    let photo_id = page["data"]["items"][0]["id"].as_str().unwrap();
    let (exit, listed) =
        command(&server.url, &["photos", "processing-export-list", photo_id]).await;
    assert_eq!(exit, 0, "{listed}");
    assert_eq!(listed["data"]["photoId"], photo_id);
    assert_eq!(listed["data"]["exports"], json!([]));
    assert_eq!(listed["data"]["artifacts"], json!([]));
    assert_eq!(listed["data"]["historicalExports"], json!([]));
    server.close().await.unwrap();
    fs::remove_dir_all(base).unwrap();
}
