use super::*;
use crate::http::create_router_with_processing;

#[tokio::test]
async fn configured_server_returns_independent_current_and_baseline_previews() {
    let (base, mut config) = prepare_fixture();
    approved_raw_fixture(&config.library_root.join("approved.ARW"));
    let engine = FakePhotoEngine::install(&base);
    config.processing = Some(engine.processing_config());
    config.export_retained_output_bytes = Some(64 * 1024 * 1024);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = create_router_with_processing(
        Arc::clone(&application),
        crate::http::open_web_root(config.web_root()),
        config.processing.clone(),
    );
    application.access.seed_test_token();
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .into_iter()
        .next()
        .unwrap();
    let recipe_uri = format!("/api/photos/{photo_id}/processing-recipe");
    let (_, current) = get_json(&router, &recipe_uri).await;
    let source_revision = current["sourceRevision"].as_str().unwrap().to_owned();
    let registry = slipstream_processing::modules::ModuleRegistry::new(
        slipstream_processing::modules::ModuleAvailability::ready(),
        slipstream_processing::modules::ModuleAvailability::ready(),
    );
    let description = registry.describe("darktable").unwrap();
    let default_tree = description.parameter_schema["default"].clone();
    let mut current_tree = default_tree.clone();
    current_tree["stack"][0]["params"]["exposure"] = serde_json::json!(1.0);
    let current_parameters = slipstream_core::ProcessingParameterSnapshot::new(
        "darktable-params-1",
        current_tree.clone(),
    )
    .unwrap();
    let baseline_parameters =
        slipstream_core::ProcessingParameterSnapshot::new("darktable-params-1", default_tree)
            .unwrap();
    let response = post_json(
        &router,
        &recipe_uri,
        serde_json::json!({
            "requestId": "preview-identity",
            "expectedRecipeRevision": null,
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
                    "tree": current_tree
                }
            }]
        }),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let saved = response_json(response).await;
    let preview_uri = format!("/api/photos/{photo_id}/processing-preview/develop-1");
    let (preview, baseline) = tokio::join!(
        send(
            &router,
            authenticated_request()
                .uri(&preview_uri)
                .body(Body::empty())
                .unwrap(),
        ),
        send(
            &router,
            authenticated_request()
                .uri(format!("{preview_uri}?comparison=baseline"))
                .body(Body::empty())
                .unwrap(),
        ),
    );
    assert_eq!(baseline.status(), StatusCode::OK);
    assert_eq!(
        preview.headers()["slipstream-processing-preview-comparison"],
        "current"
    );
    assert_eq!(
        baseline.headers()["slipstream-processing-preview-comparison"],
        "baseline"
    );
    assert_eq!(
        preview.headers()["slipstream-processing-preview-parameter-digest"],
        current_parameters.canonical_digest()
    );
    assert_eq!(
        baseline.headers()["slipstream-processing-preview-parameter-digest"],
        baseline_parameters.canonical_digest()
    );
    assert_ne!(
        preview.headers()["slipstream-processing-preview-identity"],
        baseline.headers()["slipstream-processing-preview-identity"]
    );
    for header in [
        "slipstream-processing-preview-photo-id",
        "slipstream-processing-preview-step-id",
        "slipstream-processing-preview-source-revision",
        "slipstream-processing-preview-recipe-revision",
        "slipstream-processing-preview-input-sha256",
        "slipstream-processing-preview-input-byte-length",
        "slipstream-processing-preview-module",
        "slipstream-processing-preview-adapter-schema-version",
        "slipstream-processing-preview-bundle-id",
        "slipstream-processing-preview-geometry",
        "slipstream-processing-preview-output-contract",
        "slipstream-processing-preview-display-conversion",
        "slipstream-processing-preview-width",
        "slipstream-processing-preview-height",
    ] {
        assert_eq!(
            preview.headers()[header],
            baseline.headers()[header],
            "{header}"
        );
    }
    assert_eq!(
        baseline.headers()["slipstream-processing-preview-recipe-revision"],
        saved["recipe"]["revision"].as_str().unwrap()
    );
    assert_eq!(preview.status(), StatusCode::OK);
    assert_eq!(preview.headers()["content-type"], "image/png");
    assert_eq!(
        preview.headers()["slipstream-processing-preview-module"],
        "darktable"
    );
    assert_eq!(
        preview.headers()["slipstream-processing-preview-adapter-schema-version"],
        "darktable-adapter-1:darktable-params-1"
    );
    assert_eq!(
        preview.headers()["slipstream-processing-preview-display-conversion"],
        slipstream_core::DISPLAY_TRANSFORM_VERSION
    );
    assert_eq!(
        preview.headers()["slipstream-processing-preview-parameter-digest"]
            .to_str()
            .unwrap()
            .len(),
        64
    );
    assert_eq!(
        preview.headers()["slipstream-processing-preview-identity"]
            .to_str()
            .unwrap()
            .len(),
        64
    );
    assert_eq!(
        preview.headers()["slipstream-processing-preview-geometry"],
        "1224"
    );
    assert_eq!(
        preview.headers()["slipstream-processing-preview-source-revision"],
        crate::queries::hex_encode(source_revision.as_bytes())
    );
    assert_eq!(saved["recipe"]["currentStepId"], "develop-1");
    let bytes = axum::body::to_bytes(preview.into_body(), 64 * 1024)
        .await
        .unwrap()
        .to_vec();
    assert_eq!(&bytes[0..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(u32::from_be_bytes(bytes[16..20].try_into().unwrap()), 1224);
    assert_eq!(u32::from_be_bytes(bytes[20..24].try_into().unwrap()), 1224);
    let baseline_bytes = axum::body::to_bytes(baseline.into_body(), 64 * 1024)
        .await
        .unwrap();
    assert_eq!(&baseline_bytes[0..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(
        u32::from_be_bytes(baseline_bytes[16..20].try_into().unwrap()),
        1224
    );
    assert_eq!(
        u32::from_be_bytes(baseline_bytes[20..24].try_into().unwrap()),
        1224
    );

    let refused = send(
        &router,
        authenticated_request()
            .uri(format!(
                "/api/photos/{photo_id}/processing-preview/not-current?comparison=baseline"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json(refused).await["error"]["code"],
        "step_not_current"
    );
    let (_, unchanged) = get_json(&router, &recipe_uri).await;
    assert_eq!(unchanged["recipe"], saved["recipe"]);
    let invalid = send(
        &router,
        authenticated_request()
            .uri(format!("{preview_uri}?comparison=previous"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(invalid).await["error"]["code"],
        "invalid_comparison"
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
