use super::*;

#[tokio::test]
async fn trash_restore_all_excludes_and_replays_fixed_selection() {
    let (base, config) = prepare_fixture();
    for (name, color) in [
        ("a.jpg", [1, 2, 3]),
        ("b.jpg", [4, 5, 6]),
        ("c.jpg", [7, 8, 9]),
    ] {
        jpeg_fixture(&config.library_root.join(name), 8, 4, color);
    }
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let by_location = photo_ids_by_location(&application, &ids).await;
    for photo_id in &ids {
        assert_eq!(
            post_json(
                &router,
                &format!("/api/photos/{photo_id}/state"),
                serde_json::json!({"field":"selectionState","value":"rejected"}),
                Some("https://camera.local")
            )
            .await
            .status(),
            StatusCode::OK
        );
    }
    let opened = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source":"library","selection":"rejected"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let removed = response_json(post_json(&router, "/api/photos/remove",
        serde_json::json!({"token":opened["token"],"operationId":"00000000-0000-4000-8000-000000000100"}),
        Some("https://camera.local")).await).await;
    assert_eq!(removed["counts"]["removed"], 3);
    let too_many: Vec<String> = (0..=slipstream_core::PHOTO_REMOVAL_MAX)
        .map(|index| format!("photo-{index}"))
        .collect();
    for (operation_id, photo_ids, expected_code) in [
        ("invalid id", vec![ids[0].clone()], "invalid_input"),
        (
            "00000000-0000-4000-8000-000000000104",
            too_many,
            "limit_exceeded",
        ),
    ] {
        let response = post_json(
            &router,
            "/api/trash/restore",
            serde_json::json!({"operationId":operation_id,"all":false,
                "photoIds":photo_ids,"excludePhotoIds":[]}),
            Some("https://camera.local"),
        )
        .await;
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], expected_code);
        assert_eq!(body["error"]["effect"], "none");
        assert_eq!(body["error"]["details"]["operationId"], operation_id);
        let (_, trash) = get_json(&router, "/api/trash?start=0&limit=60").await;
        assert_eq!(trash["total"], 3);
    }
    for (all, photo_ids, exclusions) in [
        (true, vec![ids[0].clone()], Vec::<String>::new()),
        (false, Vec::new(), Vec::new()),
        (false, vec![ids[0].clone()], vec![ids[1].clone()]),
    ] {
        let invalid = post_json(
            &router,
            "/api/trash/restore",
            serde_json::json!({
                "operationId":"00000000-0000-4000-8000-000000000102",
                "all":all, "photoIds":photo_ids, "excludePhotoIds":exclusions,
            }),
            Some("https://camera.local"),
        )
        .await;
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    }
    let selection = serde_json::json!({"operationId":"00000000-0000-4000-8000-000000000101",
        "all":true,"photoIds":[],"excludePhotoIds":[by_location["b.jpg"]]});
    let restored = response_json(
        post_json(
            &router,
            "/api/trash/restore",
            selection.clone(),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(restored["counts"]["restored"], 2);
    assert_eq!(restored["results"].as_array().unwrap().len(), 2);
    assert!(
        restored["results"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["photoId"] != by_location["b.jpg"])
    );
    let (_, trash) = get_json(&router, "/api/trash?start=0&limit=60").await;
    assert_eq!(trash["total"], 1);
    assert_eq!(trash["photos"][0]["photo"]["id"], by_location["b.jpg"]);
    let replay = response_json(
        post_json(
            &router,
            "/api/trash/restore",
            selection,
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(replay, restored);
    let conflict = post_json(
        &router,
        "/api/trash/restore",
        serde_json::json!({
        "operationId":"00000000-0000-4000-8000-000000000101", "all":true,
        "photoIds":[], "excludePhotoIds":[]}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    let explicit = response_json(
        post_json(
            &router,
            "/api/trash/restore",
            serde_json::json!({
                "operationId":"00000000-0000-4000-8000-000000000103", "all":false,
                "photoIds":[by_location["a.jpg"], by_location["b.jpg"]], "excludePhotoIds":[],
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(explicit["counts"]["restored"], 1);
    assert_eq!(explicit["counts"]["alreadyActive"], 1);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn trash_capture_cleanup_and_retired_routes() {
    let (base, config) = prepare_fixture();
    for name in ["a.jpg", "b.jpg", "c.jpg"] {
        jpeg_fixture(&config.library_root.join(name), 8, 4, [1, 2, 3]);
    }
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let by_location = photo_ids_by_location(&application, &ids).await;
    let workspace = post_json(
        &router,
        &format!("/api/photos/{}/workspace", by_location["c.jpg"]),
        serde_json::json!({}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(workspace.status(), StatusCode::OK);
    for id in &ids {
        assert_eq!(
            post_json(
                &router,
                &format!("/api/photos/{id}/state"),
                serde_json::json!({"field":"selectionState","value":"rejected"}),
                Some("https://camera.local")
            )
            .await
            .status(),
            StatusCode::OK
        );
    }
    let opened = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source":"library","selection":"rejected"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let removed = response_json(post_json(&router, "/api/photos/remove",
        serde_json::json!({"token":opened["token"],"operationId":"00000000-0000-4000-8000-000000000201"}),
        Some("https://camera.local")).await).await;
    assert_eq!(removed["counts"]["removed"], 3);
    let (_, first) = get_json(&router, "/api/trash?limit=1").await;
    assert_eq!(first["total"], 3);
    assert_eq!(first["namespace"], "service");
    assert_eq!(
        first["libraryRoot"],
        config.library_root.to_string_lossy().as_ref()
    );
    let cursor = first["nextCursor"].as_str().unwrap();
    let (_, second) = get_json(&router, &format!("/api/trash?cursor={cursor}")).await;
    let next = second["nextCursor"].as_str().unwrap();
    let (_, third) = get_json(&router, &format!("/api/trash?cursor={next}")).await;
    assert!(third["nextCursor"].is_null());
    let items: Vec<_> = [&first, &second, &third]
        .into_iter()
        .map(|page| page["photos"][0].clone())
        .collect();
    assert_eq!(items.len(), 3);
    fs::remove_file(config.library_root.join("a.jpg")).unwrap();
    fs::remove_file(config.library_root.join("c.jpg")).unwrap();
    let intent: Vec<_> = items
        .iter()
        .map(|item| {
            serde_json::json!({
                "photoId":item["photoId"], "removedAtMs":item["removedAtMs"],
                "originalId":item["originalId"], "originalLocation":item["originalLocation"]
            })
        })
        .collect();
    let operation = "00000000-0000-4000-8000-000000000202";
    let preview = response_json(
        post_json(
            &router,
            "/api/trash/cleanup-preview",
            serde_json::json!({"items":intent.clone()}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(preview["items"].as_array().unwrap().len(), 3);
    assert!(
        preview["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["absence"] == "absent" && item["state"] == "captured")
    );
    assert!(
        preview["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["absence"] == "present")
    );
    assert!(preview["items"].as_array().unwrap().iter().any(|item| {
        item["blockers"].as_array().is_some_and(|blockers| {
            blockers
                .iter()
                .any(|blocker| blocker["kind"] == "workspace")
        })
    }));
    let (_, still_three) = get_json(&router, "/api/trash").await;
    assert_eq!(still_three["total"], 3);
    let request = serde_json::json!({"operationId":operation,"items":intent});
    let response = post_json(
        &router,
        "/api/trash/cleanup",
        request.clone(),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let outcome = response_json(response).await;
    assert_eq!(outcome["counts"]["cleaned"], 1);
    assert_eq!(outcome["counts"]["present"], 1);
    assert_eq!(outcome["counts"]["blocked"], 1);
    assert!(config.library_root.join("b.jpg").exists());
    assert!(
        outcome["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["state"] == "blocked" && item["blockers"][0]["kind"] == "workspace")
    );
    let replay = response_json(
        post_json(
            &router,
            "/api/trash/cleanup",
            request.clone(),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(replay, outcome);
    let mut different = request;
    different["items"][0]["originalLocation"] = serde_json::json!("different.jpg");
    let conflict = post_json(
        &router,
        "/api/trash/cleanup",
        different,
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    let (status, stored) = get_json(
        &router,
        &format!("/api/trash/cleanup-operations/{operation}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stored, outcome);
    for path in ["/api/trash/review", "/api/trash/delete"] {
        assert_eq!(
            post_json(
                &router,
                path,
                serde_json::json!({}),
                Some("https://camera.local")
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
    }
    let retired = send(
        &router,
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/trash/operations/{operation}"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(retired.status(), StatusCode::NOT_FOUND);
    let missing = "00000000-0000-4000-8000-000000000299";
    let (status, unknown) =
        get_json(&router, &format!("/api/trash/cleanup-operations/{missing}")).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(unknown["error"]["code"], "outcome_unknown");
    assert_eq!(unknown["error"]["effect"], "unknown");
    assert_eq!(unknown["error"]["details"]["operationId"], missing);
    let (_, remaining) = get_json(&router, "/api/trash").await;
    assert_eq!(remaining["total"], 2);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn trash_cursor_retains_markers_when_new_removals_arrive() {
    let (base, config) = prepare_fixture();
    for name in ["a.jpg", "b.jpg", "c.jpg"] {
        jpeg_fixture(&config.library_root.join(name), 8, 4, [1, 2, 3]);
    }
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let by_location = photo_ids_by_location(&application, &ids).await;
    for name in ["a.jpg", "b.jpg"] {
        assert_eq!(
            post_json(
                &router,
                &format!("/api/photos/{}/state", by_location[name]),
                serde_json::json!({"field":"selectionState","value":"rejected"}),
                Some("https://camera.local")
            )
            .await
            .status(),
            StatusCode::OK
        );
    }
    let browse = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source":"library","selection":"rejected"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    response_json(post_json(&router, "/api/photos/remove", serde_json::json!({"token":browse["token"],"operationId":"00000000-0000-4000-8000-000000000301"}), Some("https://camera.local")).await).await;
    let (_, first) = get_json(&router, "/api/trash?limit=1").await;
    let captured_operation = first["operation"].clone();
    let cursor = first["nextCursor"].as_str().unwrap();
    let (_, original_page) = get_json(&router, &format!("/api/trash?cursor={cursor}")).await;
    let captured = original_page["photos"][0].clone();
    let restored = post_json(&router, "/api/photos/restore", serde_json::json!({"photos":[{"id":captured["photoId"],"removedAtMs":captured["removedAtMs"]}]}), Some("https://camera.local")).await;
    assert_eq!(restored.status(), StatusCode::OK);
    assert_eq!(
        post_json(
            &router,
            &format!("/api/photos/{}/state", by_location["c.jpg"]),
            serde_json::json!({"field":"selectionState","value":"rejected"}),
            Some("https://camera.local")
        )
        .await
        .status(),
        StatusCode::OK
    );
    let browse = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source":"library","selection":"rejected"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    response_json(post_json(&router, "/api/photos/remove", serde_json::json!({"token":browse["token"],"operationId":"00000000-0000-4000-8000-000000000302"}), Some("https://camera.local")).await).await;
    let (_, continued) = get_json(&router, &format!("/api/trash?cursor={cursor}")).await;
    assert_eq!(continued["operation"], captured_operation);
    assert_eq!(continued["total"], 2);
    assert_eq!(continued["photos"][0]["photoId"], captured["photoId"]);
    assert_eq!(
        continued["photos"][0]["removedAtMs"],
        captured["removedAtMs"]
    );
    assert_eq!(continued["photos"][0]["state"], "changed");
    assert!(continued["nextCursor"].is_null());
    let intent = serde_json::json!({"operationId":"00000000-0000-4000-8000-000000000303","items":[{
        "photoId":captured["photoId"],"removedAtMs":captured["removedAtMs"],"originalId":captured["originalId"],"originalLocation":captured["originalLocation"]}]});
    let result = response_json(
        post_json(
            &router,
            "/api/trash/cleanup",
            intent,
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(result["counts"]["changed"], 1);
    assert!(
        config
            .library_root
            .join(captured["originalLocation"].as_str().unwrap())
            .exists()
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
