use super::*;

#[tokio::test]
async fn composable_recipe_route_persists_zero_and_selected_steps() {
    let (base, config) = prepare_fixture();
    approved_raw_fixture(&config.library_root.join("approved.ARW"));
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = configured_router(&application, config.web_root());
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .into_iter()
        .next()
        .unwrap();
    let uri = format!("/api/photos/{photo_id}/processing-recipe");
    let (_, current) = get_json(&router, &uri).await;
    let source_revision = current["sourceRevision"].as_str().unwrap().to_owned();
    assert!(current["recipe"].is_null());

    let body = serde_json::json!({
        "requestId": "composable-zero",
        "expectedRecipeRevision": null,
        "expectedSourceRevision": source_revision,
        "currentStepId": null,
        "steps": []
    });
    let response = post_json(&router, &uri, body.clone(), Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let saved = response_json(response).await;
    assert_eq!(saved["outcome"], "saved");
    assert_eq!(saved["recipe"]["steps"], serde_json::json!([]));

    let response = post_json(&router, &uri, body, Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await["outcome"], "replayed");

    let (_, read) = get_json(&router, &uri).await;
    let recipe_version = read["recipe"]["revision"].as_str().unwrap().to_owned();
    let source_revision = read["sourceRevision"].as_str().unwrap().to_owned();
    let step_body = serde_json::json!({
        "requestId": "composable-darktable",
        "expectedRecipeRevision": recipe_version,
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
    });
    let response = post_json(&router, &uri, step_body, Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let saved = response_json(response).await;
    assert_eq!(saved["recipe"]["currentStepId"], "develop-1");
    assert_eq!(saved["recipe"]["steps"][0]["module"], "darktable");

    let (status, body) = get_json(
        &router,
        &format!("/api/photos/{photo_id}/processing-preview/not-current"),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error"]["code"], "step_not_current");
    assert_eq!(body["error"]["effect"], "none");
    assert!(body["error"]["details"].is_object());

    let (status, body) = get_json(
        &router,
        &format!("/api/photos/{photo_id}/processing-preview/develop-1"),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["error"]["code"], "processing_unavailable");
    let stale_body = serde_json::json!({
        "requestId": "composable-stale-source",
        "expectedRecipeRevision": read["recipe"]["revision"],
        "expectedSourceRevision": "stale-source",
        "currentStepId": null,
        "steps": []
    });
    let response = post_json(&router, &uri, stale_body, Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json(response).await["error"]["code"],
        "source_changed"
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
