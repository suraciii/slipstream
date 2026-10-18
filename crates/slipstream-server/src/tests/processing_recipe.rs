use super::*;

/// Discover a completed TIFF through the same workspace API used by a caller,
/// then explicitly select its protected capture rather than the live path.
pub(super) fn completed_film_binding(
    application: &Application,
    photo_id: &str,
) -> serde_json::Value {
    let workspace = application.workspaces.open_photo(photo_id).unwrap();
    let workspace_id = workspace["workspaceId"].as_str().unwrap();
    let directory = application.workspaces.directory(workspace_id).unwrap();
    fs::write(
        directory.path().join("independent.tif"),
        valid_development_tiff_fixture(),
    )
    .unwrap();
    let files = application
        .workspaces
        .files(Some(workspace_id), None, 60, None)
        .unwrap();
    let selected = files["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["relativePath"] == "independent.tif")
        .unwrap();
    assert_eq!(selected["state"], "ready");
    assert!(!selected["imageContract"].is_null());
    let (lease, captured) = application
        .workspaces
        .lease_input(
            workspace_id,
            selected["artifactId"].as_str().unwrap(),
            selected["revision"].as_str().unwrap(),
        )
        .unwrap();
    drop(lease);
    serde_json::json!({
        "kind": "workspace-artifact", "workspaceId": workspace_id,
        "artifactId": captured["artifactId"], "revision": captured["revision"],
        "contract": captured["imageContract"],
    })
}

pub(super) fn film_save(
    binding: serde_json::Value,
    source: serde_json::Value,
    request: &str,
) -> serde_json::Value {
    serde_json::json!({
        "requestId": request, "expectedRecipeRevision": null,
        "expectedSourceRevision": source, "currentStepId": "film",
        "steps": [{
            "stepId": "film", "module": "spektrafilm", "input": binding,
            "parameters": {
                "schemaVersion": slipstream_processing::modules::SPEKTRAFILM_PARAMETER_VERSION,
                "tree": slipstream_processing::modules::spektrafilm_default_tree(),
            },
        }],
    })
}

#[tokio::test]
async fn film_save_captures_external_tiff_and_replays_original_selection() {
    let (base, config) = prepare_fixture();
    approved_raw_fixture(&config.library_root.join("approved.ARW"));
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .remove(0);
    let workspace = application.workspaces.open_photo(&photo_id).unwrap();
    let workspace_id = workspace["workspaceId"].as_str().unwrap();
    let directory = application.workspaces.directory(workspace_id).unwrap();
    let input_path = directory.path().join("external.tif");
    fs::write(&input_path, valid_development_tiff_fixture()).unwrap();
    let files = application
        .workspaces
        .files(Some(workspace_id), None, 60, None)
        .unwrap();
    let selected = files["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["relativePath"] == "external.tif")
        .unwrap();
    let binding = serde_json::json!({"kind":"workspace-artifact","workspaceId":workspace_id,"artifactId":selected["artifactId"],"revision":selected["revision"],"contract":selected["imageContract"]});
    let router = configured_router(&application, config.web_root());
    let uri = format!("/api/photos/{photo_id}/processing-recipe");
    let (_, read) = get_json(&router, &uri).await;
    let body = film_save(
        binding.clone(),
        read["sourceRevision"].clone(),
        "external-film-capture",
    );
    let response = post_json(&router, &uri, body.clone(), Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let saved = response_json(response).await;
    let captured = &saved["recipe"]["steps"][0]["input"];
    assert_ne!(captured["artifactId"], binding["artifactId"]);
    assert_eq!(captured["contract"], binding["contract"]);
    let (_, read) = get_json(&router, &uri).await;
    assert_eq!(read["recipe"], saved["recipe"]);
    fs::remove_file(input_path).unwrap();
    let response = post_json(&router, &uri, body, Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await["recipe"], saved["recipe"]);
    application.shutdown().await.unwrap();
    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn workspace_film_save_uses_empty_live_source_and_persists_canonical_state() {
    let (base, config) = prepare_fixture();
    let original = config.library_root.join("approved.ARW");
    approved_raw_fixture(&original);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .remove(0);
    let binding = completed_film_binding(&application, &photo_id);
    fs::remove_file(original).unwrap();
    application.rescan().await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = configured_router(&application, config.web_root());
    let uri = format!("/api/photos/{photo_id}/processing-recipe");
    let (status, read) = get_json(&router, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read["sourceRevision"], "");
    assert!(read["recipe"].is_null());
    let body = film_save(
        binding.clone(),
        read["sourceRevision"].clone(),
        "workspace-film-save",
    );
    let response = post_json(&router, &uri, body.clone(), Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let saved = response_json(response).await;
    assert_eq!(saved["sourceRevision"], "");
    assert_eq!(saved["recipe"]["sourceRevision"], "");
    assert_eq!(saved["recipe"]["steps"][0]["input"], binding);
    let (_, after) = get_json(&router, &uri).await;
    assert_eq!(after["recipe"], saved["recipe"]);
    let workspace_id = binding["workspaceId"].as_str().unwrap();
    let directory = application.workspaces.directory(workspace_id).unwrap();
    let canonical = crate::processing_recipe::inspect_film_state(
        &directory
            .path()
            .join(crate::workspace::layout::EDITS)
            .join("spektrafilm")
            .join("adapter"),
    )
    .unwrap();
    assert_eq!(canonical["recipe"], saved["recipe"]);
    assert_eq!(canonical["recipe"]["steps"][0]["module"], "spektrafilm");
    let response = post_json(&router, &uri, body.clone(), Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let replay = response_json(response).await;
    assert_eq!(replay["outcome"], "replayed");
    assert_eq!(replay["recipe"], saved["recipe"]);
    application.shutdown().await.unwrap();
    drop(router);
    drop(application);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = configured_router(&application, config.web_root());
    let (_, reopened) = get_json(&router, &uri).await;
    assert_eq!(reopened["sourceRevision"], "");
    assert_eq!(reopened["recipe"], saved["recipe"]);
    let response = post_json(&router, &uri, body, Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let replay = response_json(response).await;
    assert_eq!(replay["outcome"], "replayed");
    assert_eq!(replay["sourceRevision"], "");
    assert_eq!(replay["recipe"], saved["recipe"]);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn workspace_film_missing_input_refuses_initial_save_without_canonical_state() {
    let (base, config) = prepare_fixture();
    approved_raw_fixture(&config.library_root.join("approved.ARW"));
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .remove(0);
    let mut binding = completed_film_binding(&application, &photo_id);
    binding["artifactId"] = serde_json::json!("missing-artifact");
    let router = configured_router(&application, config.web_root());
    let uri = format!("/api/photos/{photo_id}/processing-recipe");
    let (_, read) = get_json(&router, &uri).await;
    let response = post_json(
        &router,
        &uri,
        film_save(
            binding,
            read["sourceRevision"].clone(),
            "missing-workspace-input",
        ),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let refusal = response_json(response).await;
    assert_eq!(refusal["error"]["code"], "artifact_changed");
    assert_eq!(refusal["error"]["effect"], "none");
    let (_, after) = get_json(&router, &uri).await;
    assert!(after["recipe"].is_null());
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn settled_film_save_replays_after_later_state_and_bound_bytes_disappear() {
    let (base, config) = prepare_fixture();
    let original = config.library_root.join("approved.ARW");
    approved_raw_fixture(&original);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .remove(0);
    let binding = completed_film_binding(&application, &photo_id);
    let router = configured_router(&application, config.web_root());
    let uri = format!("/api/photos/{photo_id}/processing-recipe");
    let (_, read) = get_json(&router, &uri).await;
    let body = film_save(
        binding.clone(),
        read["sourceRevision"].clone(),
        "settled-film-save",
    );
    let response = post_json(&router, &uri, body.clone(), Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let settled = response_json(response).await;
    let remove = serde_json::json!({
        "requestId":"remove-film", "expectedRecipeRevision":settled["recipe"]["revision"],
        "expectedSourceRevision":read["sourceRevision"], "currentStepId":null, "steps":[],
    });
    let response = post_json(&router, &uri, remove, Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let newer = response_json(response).await;
    assert_ne!(newer["recipe"]["revision"], settled["recipe"]["revision"]);
    let input = application
        .workspaces
        .inspect(
            binding["workspaceId"].as_str().unwrap(),
            binding["artifactId"].as_str().unwrap(),
        )
        .unwrap();
    fs::remove_file(input["path"].as_str().unwrap()).unwrap();
    fs::remove_file(original).unwrap();
    application.rescan().await.unwrap();
    wait_for_scan_settled(&application).await;
    let response = post_json(&router, &uri, body.clone(), Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let replay = response_json(response).await;
    assert_eq!(replay["outcome"], "replayed");
    assert_eq!(replay["recipe"], settled["recipe"]);
    assert_eq!(replay["sourceRevision"], body["expectedSourceRevision"]);
    let (_, current) = get_json(&router, &uri).await;
    assert_eq!(current["sourceRevision"], "");
    assert_eq!(current["recipe"]["sourceRevision"], read["sourceRevision"]);
    assert_eq!(current["recipe"], newer["recipe"]);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn generic_darktable_save_refuses_without_creating_canonical_state() {
    let (base, config) = prepare_fixture();
    approved_raw_fixture(&config.library_root.join("approved.ARW"));
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .remove(0);
    application.workspaces.open_photo(&photo_id).unwrap();
    let router = configured_router(&application, config.web_root());
    let uri = format!("/api/photos/{photo_id}/processing-recipe");
    let (_, read) = get_json(&router, &uri).await;
    let body = serde_json::json!({
        "requestId":"generic-darktable-refusal", "expectedRecipeRevision":null,
        "expectedSourceRevision":read["sourceRevision"], "currentStepId":"darktable",
        "steps":[{"stepId":"darktable", "module":"darktable",
            "input":{"kind":"original", "photoId":photo_id, "sourceRevision":read["sourceRevision"]},
            "parameters":{"schemaVersion":"darktable-params-1", "tree":{"stack":[]}},
        }],
    });
    let response = post_json(&router, &uri, body, Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let refusal = response_json(response).await;
    assert_eq!(refusal["error"]["code"], "unsupported_state");
    assert_eq!(refusal["error"]["effect"], "none");
    let (_, after) = get_json(&router, &uri).await;
    assert!(after["recipe"].is_null());
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn fixed_processing_http_paths_are_retired() {
    let (base, config) = prepare_fixture();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = configured_router(&application, config.web_root());
    for uri in [
        "/api/processing/capability",
        "/api/photos/photo/edit-recipe",
        "/api/photos/photo/edit-recipe/rebind",
        "/api/photos/photo/edit-preview/develop",
    ] {
        let response = send(
            &router,
            authenticated_request()
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
    }
    let response = post_json(
        &router,
        "/api/photos/photo/exports",
        serde_json::json!({}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
