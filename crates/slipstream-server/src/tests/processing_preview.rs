use super::*;
use crate::http::create_router_with_processing;

#[tokio::test]
async fn configured_server_returns_bounded_preview_identity() {
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
                    "tree": {"stack": []}
                }
            }]
        }),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let saved = response_json(response).await;
    let preview = send(
        &router,
        authenticated_request()
            .uri(format!(
                "/api/photos/{photo_id}/processing-preview/develop-1"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
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

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
