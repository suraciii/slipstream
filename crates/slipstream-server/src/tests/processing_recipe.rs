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

#[tokio::test]
async fn automatic_recipe_failure_leaves_the_saved_recipe_unchanged() {
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
    let (_, read) = get_json(&router, &uri).await;
    let source = read["sourceRevision"].as_str().unwrap().to_owned();
    let body = serde_json::json!({
        "requestId": "automatic-failure",
        "expectedRecipeRevision": null,
        "expectedSourceRevision": source,
        "currentStepId": "develop",
        "steps": [original_step(&photo_id, &source, serde_json::json!({"stack": []}))],
        "automaticAdjustment": {
            "stepId": "develop",
            "operation": "exposure",
            "multiPriority": 0,
            "instruction": {
                "deflicker_percentile": 50.0,
                "deflicker_target_level": -4.0
            }
        }
    });
    let response = post_json(&router, &uri, body, Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        response_json(response).await["error"]["code"],
        "automatic_adjustment_failed"
    );
    let (_, after) = get_json(&router, &uri).await;
    assert!(after["recipe"].is_null());
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

fn original_step(photo_id: &str, source: &str, tree: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "stepId": "develop", "module": "darktable",
        "input": {"kind": "original", "photoId": photo_id, "sourceRevision": source},
        "parameters": {"schemaVersion": "darktable-params-1", "tree": tree}
    })
}

#[tokio::test]
async fn saved_unsupported_controls_survive_and_malformed_later_entries_refuse() {
    let (base, config) = prepare_fixture();
    approved_raw_fixture(&config.library_root.join("approved.ARW"));
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = configured_router(&application, config.web_root());
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .remove(0);
    let uri = format!("/api/photos/{photo_id}/processing-recipe");
    let (_, read) = get_json(&router, &uri).await;
    let source = read["sourceRevision"].as_str().unwrap();
    let tree = serde_json::json!({"stack": [{
        "operation": "temperature", "multiPriority": 0, "enabled": true,
        "params": {"temperatureKelvin": 6500, "tintMilli": -12}
    }]});
    let body = serde_json::json!({
        "requestId": "retained-white-balance", "expectedRecipeRevision": null,
        "expectedSourceRevision": source, "currentStepId": "develop",
        "steps": [original_step(&photo_id, source, tree.clone())]
    });
    let response = post_json(&router, &uri, body, Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let saved = response_json(response).await;
    let (_, read) = get_json(&router, &uri).await;
    assert_eq!(read["recipe"]["steps"][0]["parameters"]["tree"], tree);
    assert_eq!(read["recipe"]["executionRefusals"][0]["stepId"], "develop");
    assert_eq!(
        read["recipe"]["executionRefusals"][0]["code"],
        "unsupported-control"
    );
    // Retained editing intent never grants execution admission.
    let parameters = slipstream_processing::modules::Parameters {
        module: "darktable".to_owned(),
        version: "darktable-params-1".to_owned(),
        tree: tree.clone(),
    };
    let registry = slipstream_processing::modules::ModuleRegistry::new(
        slipstream_processing::modules::ModuleAvailability::ready(),
        slipstream_processing::modules::ModuleAvailability::ready(),
    );
    assert!(registry.validate_parameters(&parameters).is_err());
    let mut malformed = tree.clone();
    malformed["stack"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "operation": "exposure", "multiPriority": 0, "enabled": "yes", "params": {}
        }));
    let body = serde_json::json!({
        "requestId": "malformed-after-retained", "expectedRecipeRevision": saved["recipe"]["revision"],
        "expectedSourceRevision": source, "currentStepId": "develop",
        "steps": [original_step(&photo_id, source, malformed)]
    });
    let response = post_json(&router, &uri, body, Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let (_, current) = get_json(&router, &uri).await;
    assert_eq!(current["recipe"], saved["recipe"]);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn composable_recipe_requires_explicit_rebind_and_replays_it() {
    let (base, config) = prepare_fixture();
    let original = config.library_root.join("approved.ARW");
    approved_raw_fixture(&original);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = configured_router(&application, config.web_root());
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .remove(0);
    let uri = format!("/api/photos/{photo_id}/processing-recipe");
    let (_, read) = get_json(&router, &uri).await;
    let old_source = read["sourceRevision"].as_str().unwrap().to_owned();
    let body = serde_json::json!({
        "requestId": "before-rebind", "expectedRecipeRevision": null,
        "expectedSourceRevision": old_source, "currentStepId": "develop",
        "steps": [original_step(&photo_id, &old_source, serde_json::json!({"stack": []}))]
    });
    let response = post_json(&router, &uri, body, Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let saved = response_json(response).await;
    fs::OpenOptions::new()
        .append(true)
        .open(&original)
        .unwrap()
        .write_all(&[0])
        .unwrap();
    application.rescan().await.unwrap();
    wait_for_scan_settled(&application).await;
    let (_, changed) = get_json(&router, &uri).await;
    let new_source = changed["sourceRevision"].as_str().unwrap().to_owned();
    assert_ne!(old_source, new_source);
    assert_eq!(changed["recipe"], saved["recipe"]);
    let draft = serde_json::json!({
        "requestId": "implicit-rebind", "expectedRecipeRevision": saved["recipe"]["revision"],
        "expectedSourceRevision": new_source, "currentStepId": "develop",
        "steps": [original_step(&photo_id, &new_source, serde_json::json!({"stack": []}))]
    });
    let response = post_json(&router, &uri, draft, Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json(response).await["error"]["code"],
        "source_changed"
    );
    let rebind_uri = format!("{uri}/rebind");
    let rebind = serde_json::json!({
        "requestId": "explicit-rebind", "expectedRecipeRevision": saved["recipe"]["revision"],
        "newSourceRevision": new_source
    });
    let response = post_json(
        &router,
        &rebind_uri,
        rebind.clone(),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let rebound = response_json(response).await;
    assert_eq!(rebound["recipe"]["sourceRevision"], new_source);
    assert_eq!(
        rebound["recipe"]["steps"][0]["input"]["sourceRevision"],
        new_source
    );
    let response = post_json(
        &router,
        &rebind_uri,
        rebind.clone(),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_json(response).await["outcome"], "replayed");
    let mut conflict = rebind;
    conflict["newSourceRevision"] = serde_json::json!(old_source);
    let response = post_json(&router, &rebind_uri, conflict, Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json(response).await["error"]["code"],
        "request_conflict"
    );
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

#[tokio::test]
async fn artifact_only_recipe_remains_readable_and_saveable_without_original() {
    use slipstream_core::{
        ProcessingArtifact, ProcessingArtifactId, ProcessingArtifactPublication,
        ProcessingGeometry, ProcessingImageContract, ProcessingInput, ProcessingInputEvidence,
        ProcessingModuleId, ProcessingParameterSnapshot, ProcessingStepId,
    };
    let (base, config) = prepare_fixture();
    let original = config.library_root.join("approved.ARW");
    approved_raw_fixture(&original);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = configured_router(&application, config.web_root());
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .remove(0);
    let uri = format!("/api/photos/{photo_id}/processing-recipe");
    let (_, read) = get_json(&router, &uri).await;
    let source = read["sourceRevision"].as_str().unwrap().to_owned();
    let artifact = ProcessingArtifact {
        artifact_id: ProcessingArtifactId::new("offline-artifact").unwrap(),
        photo_id: photo_id.clone(),
        step_id: ProcessingStepId::new("upstream").unwrap(),
        module: ProcessingModuleId::new("darktable").unwrap(),
        adapter_schema_version: "darktable-adapter-1".to_owned(),
        parameters: ProcessingParameterSnapshot::new(
            "darktable-params-1",
            serde_json::json!({"stack": []}),
        )
        .unwrap(),
        input: ProcessingInputEvidence::new(
            ProcessingInput::Original {
                photo_id: photo_id.clone(),
                source_revision: source.clone(),
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
        bundle_id: "b".repeat(64),
        sha256: "c".repeat(64),
        byte_length: 12288,
    };
    assert_eq!(
        application
            .library
            .publish_processing_artifact(artifact)
            .await
            .unwrap(),
        ProcessingArtifactPublication::Published
    );
    let mut body = serde_json::json!({
        "requestId": "artifact-before-offline", "expectedRecipeRevision": null,
        "expectedSourceRevision": source, "currentStepId": "film",
        "steps": [{
            "stepId": "film", "module": "spektrafilm",
            "input": {"kind": "artifact", "artifactId": "offline-artifact", "contract": {
                "format": "image/tiff", "precision": "float32", "colorSpace": "prophoto-rgb",
                "transfer": "linear", "geometry": {"width": 9504, "height": 6336}, "encoding": "deflate"
            }},
            "parameters": {"schemaVersion": "spektrafilm-params-1", "tree": slipstream_processing::modules::spektrafilm_default_tree()}
        }]
    });
    let response = post_json(&router, &uri, body.clone(), Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let saved = response_json(response).await;
    fs::remove_file(&original).unwrap();
    application.rescan().await.unwrap();
    wait_for_scan_settled(&application).await;
    let (status, offline) = get_json(&router, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(offline["sourceAvailable"], false);
    assert_eq!(offline["sourceRevision"], source);
    assert_eq!(offline["recipe"], saved["recipe"]);
    body["requestId"] = serde_json::json!("artifact-offline-save");
    body["expectedRecipeRevision"] = saved["recipe"]["revision"].clone();
    body["steps"][0]["stepId"] = serde_json::json!("film-offline");
    body["currentStepId"] = serde_json::json!("film-offline");
    let response = post_json(&router, &uri, body, Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(
        response_json(response).await["recipe"]["currentStepId"],
        serde_json::json!("film-offline")
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn old_save_receipt_replays_after_recipe_advances_and_original_disappears() {
    let (base, config) = prepare_fixture();
    let original = config.library_root.join("approved.ARW");
    approved_raw_fixture(&original);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = configured_router(&application, config.web_root());
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .remove(0);
    let uri = format!("/api/photos/{photo_id}/processing-recipe");
    let (_, current) = get_json(&router, &uri).await;
    let source = current["sourceRevision"].as_str().unwrap();
    let first = serde_json::json!({
        "requestId": "retained-save", "expectedRecipeRevision": null,
        "expectedSourceRevision": source, "currentStepId": "develop",
        "steps": [original_step(&photo_id, source, serde_json::json!({"stack": []}))]
    });
    let response = post_json(&router, &uri, first.clone(), Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let saved = response_json(response).await;
    let later = serde_json::json!({
        "requestId": "later-save", "expectedRecipeRevision": saved["recipe"]["revision"],
        "expectedSourceRevision": source, "currentStepId": null, "steps": []
    });
    let response = post_json(&router, &uri, later, Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let later = response_json(response).await;
    fs::remove_file(original).unwrap();
    application.rescan().await.unwrap();
    wait_for_scan_settled(&application).await;
    let response = post_json(&router, &uri, first.clone(), Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let replay = response_json(response).await;
    assert_eq!(replay["outcome"], "replayed");
    assert_eq!(replay["recipe"], saved["recipe"]);
    let (_, current) = get_json(&router, &uri).await;
    assert_eq!(current["recipe"], later["recipe"]);
    let mut conflict = first;
    conflict["steps"][0]["stepId"] = serde_json::json!("develop-conflict");
    conflict["currentStepId"] = serde_json::json!("develop-conflict");
    let response = post_json(&router, &uri, conflict, Some("https://camera.local")).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json(response).await["error"]["code"],
        "request_conflict"
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn recipe_read_publication_gap_keeps_saved_binding_separate_from_current_facts() {
    let (base, config) = prepare_fixture();
    approved_raw_fixture(&config.library_root.join("approved.ARW"));
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = configured_router(&application, config.web_root());
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .remove(0);
    let uri = format!("/api/photos/{photo_id}/processing-recipe");
    let (_, current) = get_json(&router, &uri).await;
    let source = current["sourceRevision"].as_str().unwrap();
    let response = post_json(
        &router,
        &uri,
        serde_json::json!({
            "requestId":"before-publication-gap", "expectedRecipeRevision":null,
            "expectedSourceRevision":source, "currentStepId":"develop",
            "steps":[original_step(&photo_id, source, serde_json::json!({"stack":[]}))]
        }),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let saved = response_json(response).await;
    let connection =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    connection
        .execute("UPDATE original_files SET capture_metadata_state='pending',capture_order_key=NULL,capture_time_field=NULL,capture_offset_minutes=NULL,capture_source_revision=NULL,camera_identity_state='pending',camera_make=NULL,camera_model=NULL", [])
        .unwrap();
    let (status, gap) = get_json(&router, &uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(gap["sourceRevision"], "");
    assert!(gap["currentSourceRevision"].is_null());
    assert_eq!(gap["sourceAvailable"], true);
    assert_eq!(gap["recipe"], saved["recipe"]);
    assert_eq!(gap["recipe"]["sourceRevision"], source);
    connection
        .execute("UPDATE original_files SET available=0", [])
        .unwrap();
    let (_, unavailable) = get_json(&router, &uri).await;
    assert_eq!(unavailable["sourceRevision"], "");
    assert!(unavailable["currentSourceRevision"].is_null());
    assert_eq!(unavailable["sourceAvailable"], false);
    assert_eq!(unavailable["recipe"], saved["recipe"]);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
