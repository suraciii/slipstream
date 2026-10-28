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
async fn trash_permanent_deletion_reviews_current_items_and_reconciles_stale_files() {
    use std::os::unix::fs::PermissionsExt;

    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    fs::create_dir(root.join("locked")).unwrap();
    jpeg_fixture(&root.join("a.jpg"), 8, 4, [1, 2, 3]);
    jpeg_fixture(&root.join("b.jpg"), 8, 4, [4, 5, 6]);
    jpeg_fixture(&root.join("locked/c.jpg"), 8, 4, [7, 8, 9]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let by_location = photo_ids_by_location(&application, &ids).await;
    let a_id = by_location["a.jpg"].clone();
    let b_id = by_location["b.jpg"].clone();
    let c_id = by_location["locked/c.jpg"].clone();

    for photo_id in [&a_id, &b_id, &c_id] {
        let response = post_json(
            &router,
            &format!("/api/photos/{photo_id}/state"),
            serde_json::json!({"field": "selectionState", "value": "rejected"}),
            Some("https://camera.local"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    let opened = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source": "library", "selection": "rejected"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let removed = response_json(
        post_json(
            &router,
            "/api/photos/remove",
            serde_json::json!({
                "token": opened["token"],
                "operationId": "00000000-0000-4000-8000-000000000010",
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(removed["counts"]["removed"], 3);

    let (status, listing) = get_json(&router, "/api/trash?start=0&limit=60").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listing["total"], 3);
    assert!(listing["photos"].as_array().unwrap().iter().all(|item| {
        item["originalLocation"].is_string()
            && item["originalKind"] == "jpeg"
            && item["originalSize"].as_u64().is_some()
    }));

    let operation_id = "00000000-0000-4000-8000-000000000011";
    let review = response_json(
        post_json(
            &router,
            "/api/trash/review",
            serde_json::json!({
                "operationId": operation_id,
                "all": true,
                "photoIds": [],
                "excludePhotoIds": [],
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(review["operationId"], operation_id);
    assert_eq!(review["items"].as_array().unwrap().len(), 3);
    assert_eq!(review["rejected"], serde_json::json!([]));

    let b_size = fs::metadata(root.join("b.jpg")).unwrap().len();
    let c_size = fs::metadata(root.join("locked/c.jpg")).unwrap().len();
    fs::write(root.join("a.jpg"), b"changed after review").unwrap();
    fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o500)).unwrap();
    let deleted = response_json(
        post_json(
            &router,
            "/api/trash/delete",
            serde_json::json!({"operationId": operation_id}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let outcomes = deleted["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["photoId"].as_str().unwrap(),
                item["state"].as_str().unwrap(),
            )
        })
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(outcomes[a_id.as_str()], "changed");
    assert_eq!(outcomes[b_id.as_str()], "deleted");
    assert_eq!(outcomes[c_id.as_str()], "failed");
    assert!(!root.join("b.jpg").exists());
    assert!(root.join("a.jpg").exists());
    assert!(root.join("locked/c.jpg").exists());
    assert_eq!(deleted["logicalBytesDeleted"].as_u64().unwrap(), b_size);

    let (status, operation) =
        get_json(&router, &format!("/api/trash/operations/{operation_id}")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(operation, deleted);
    fs::set_permissions(root.join("locked"), fs::Permissions::from_mode(0o700)).unwrap();
    let retried = response_json(
        post_json(
            &router,
            "/api/trash/delete",
            serde_json::json!({"operationId": operation_id}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let retry_outcomes = retried["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["photoId"].as_str().unwrap(),
                item["state"].as_str().unwrap(),
            )
        })
        .collect::<std::collections::HashMap<_, _>>();
    assert_eq!(retry_outcomes[a_id.as_str()], "changed");
    assert_eq!(retry_outcomes[b_id.as_str()], "deleted");
    assert_eq!(retry_outcomes[c_id.as_str()], "deleted");
    assert!(!root.join("locked/c.jpg").exists());
    assert_eq!(
        retried["logicalBytesDeleted"].as_u64().unwrap(),
        b_size + c_size
    );
    let (_, operation) = get_json(&router, &format!("/api/trash/operations/{operation_id}")).await;
    assert_eq!(operation, retried);
    let (_, remaining) = get_json(&router, "/api/trash?start=0&limit=60").await;
    assert_eq!(remaining["total"], 1);

    let final_operation = "00000000-0000-4000-8000-000000000012";
    assert_eq!(
        post_json(&router, "/api/scan", serde_json::json!({}), None)
            .await
            .status(),
        StatusCode::OK
    );
    wait_for_scan_settled(&application).await;
    let final_review = response_json(
        post_json(
            &router,
            "/api/trash/review",
            serde_json::json!({
                "operationId": final_operation,
                "all": false,
                "photoIds": [a_id],
                "excludePhotoIds": [],
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(final_review["items"].as_array().unwrap().len(), 1);
    let final_deleted = response_json(
        post_json(
            &router,
            "/api/trash/delete",
            serde_json::json!({"operationId": final_operation}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(final_deleted["items"][0]["state"], "deleted");
    assert!(!root.join("a.jpg").exists());

    application.shutdown().await.unwrap();
    let restarted = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&restarted).await;
    let restarted_router = authorized_router(Arc::clone(&restarted), config.web_root());
    let (_, restarted_listing) = get_json(&restarted_router, "/api/trash?start=0&limit=60").await;
    assert_eq!(restarted_listing["total"], 0);
    let (status, restarted_operation) = get_json(
        &restarted_router,
        &format!("/api/trash/operations/{final_operation}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(restarted_operation, final_deleted);
    restarted.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn trash_permanent_deletion_keeps_identity_and_reports_pending_verification() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    jpeg_fixture(&root.join("a.jpg"), 8, 4, [1, 2, 3]);
    jpeg_fixture(&root.join("b.jpg"), 8, 4, [4, 5, 6]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let by_location = photo_ids_by_location(&application, &ids).await;
    let a_id = by_location["a.jpg"].clone();
    let b_id = by_location["b.jpg"].clone();

    for photo_id in [&a_id, &b_id] {
        assert_eq!(
            post_json(
                &router,
                &format!("/api/photos/{photo_id}/state"),
                serde_json::json!({"field": "selectionState", "value": "rejected"}),
                Some("https://camera.local"),
            )
            .await
            .status(),
            StatusCode::OK
        );
    }
    let removal_operation = "00000000-0000-4000-8000-000000000020";
    let opened = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source": "library", "selection": "rejected"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let removed = response_json(
        post_json(
            &router,
            "/api/photos/remove",
            serde_json::json!({"token": opened["token"], "operationId": removal_operation}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(removed["counts"]["removed"], 2);

    // The Trash listing publishes the bound one review may capture, and no
    // Photo carries an unresolved deletion yet.
    let (status, listing) = get_json(&router, "/api/trash?start=0&limit=60").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listing["total"], 2);
    assert_eq!(listing["reviewMaximum"], 5000);
    assert!(
        listing["photos"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["pendingVerificationOperationId"].is_null())
    );

    let deletion_operation = "00000000-0000-4000-8000-000000000021";
    let review = response_json(
        post_json(
            &router,
            "/api/trash/review",
            serde_json::json!({
                "operationId": deletion_operation,
                "all": false,
                "photoIds": [a_id],
                "excludePhotoIds": [],
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(review["items"].as_array().unwrap().len(), 1);
    assert_eq!(review["rejected"], serde_json::json!([]));
    let deleted = response_json(
        post_json(
            &router,
            "/api/trash/delete",
            serde_json::json!({"operationId": deletion_operation}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(deleted["items"][0]["state"], "deleted", "{}", deleted);
    assert_eq!(deleted["items"][0]["photoId"], a_id);
    assert_eq!(deleted["items"][0]["originalLocation"], "a.jpg");
    assert_eq!(deleted["items"][0]["originalKind"], "jpeg");
    assert!(!root.join("a.jpg").exists());

    // A new file at the deleted Photo's Location becomes a new Photo: the
    // deleted identity stays out of the Library and Trash forever.
    jpeg_fixture(&root.join("a.jpg"), 8, 4, [9, 8, 7]);
    assert_eq!(
        post_json(&router, "/api/scan", serde_json::json!({}), None)
            .await
            .status(),
        StatusCode::OK
    );
    wait_for_scan_settled(&application).await;
    let rescanned = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let rescanned_by_location = photo_ids_by_location(&application, &rescanned).await;
    let replacement_id = rescanned_by_location["a.jpg"].clone();
    assert_ne!(replacement_id, a_id);
    assert!(!rescanned.contains(&a_id));
    assert!(!rescanned.contains(&b_id));
    assert_eq!(rescanned.len(), 1);
    let (_, after) = get_json(&router, "/api/trash?start=0&limit=60").await;
    assert_eq!(after["total"], 1);
    assert!(
        after["photos"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["photo"]["id"] != serde_json::json!(a_id))
    );
    let (status, operation) = get_json(
        &router,
        &format!("/api/trash/operations/{deletion_operation}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(operation, deleted);

    // The same scan settles again without inventing a third Photo.
    assert_eq!(
        post_json(&router, "/api/scan", serde_json::json!({}), None)
            .await
            .status(),
        StatusCode::OK
    );
    wait_for_scan_settled(&application).await;
    let settled = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    assert_eq!(settled.len(), rescanned.len());
    assert!(settled.contains(&replacement_id));

    // Restore of the removal reports the deleted Photo instead of reviving it.
    let restored = response_json(
        post_json(
            &router,
            "/api/photos/restore",
            serde_json::json!({"operation": removal_operation}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(restored["counts"]["restored"], 1);
    assert_eq!(restored["changedElsewhere"], serde_json::json!([a_id]));
    assert!(root.join("a.jpg").exists());
    assert!(root.join("b.jpg").exists());

    // Remove the restored Photo again so the next deletion owns it.
    let second_removal = "00000000-0000-4000-8000-000000000024";
    let reopened = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source": "library", "selection": "rejected"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let removed_again = response_json(
        post_json(
            &router,
            "/api/photos/remove",
            serde_json::json!({"token": reopened["token"], "operationId": second_removal}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(removed_again["counts"]["removed"], 1);

    // An interrupted deletion leaves its operation on record: the listing
    // reports it, Restore and another review refuse the Photo, and resuming
    // the operation settles it.
    let interrupted_operation = "00000000-0000-4000-8000-000000000022";
    let interrupted_review = response_json(
        post_json(
            &router,
            "/api/trash/review",
            serde_json::json!({
                "operationId": interrupted_operation,
                "all": false,
                "photoIds": [b_id],
                "excludePhotoIds": [],
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(interrupted_review["items"].as_array().unwrap().len(), 1);
    let database =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    let item_key = format!("permanent_deletion_item:{interrupted_operation}:{b_id}");
    let item: String = database
        .query_row(
            "SELECT value FROM library_metadata WHERE key = ?",
            rusqlite::params![item_key],
            |row| row.get(0),
        )
        .unwrap();
    assert!(item.contains("\"state\":\"Pending\""));
    database
        .execute(
            "UPDATE library_metadata SET value = ? WHERE key = ?",
            rusqlite::params![
                item.replace("\"state\":\"Pending\"", "\"state\":\"Deleting\""),
                item_key
            ],
        )
        .unwrap();
    database
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES(?,?)",
            rusqlite::params![
                format!("permanent_deletion_unsettled:{b_id}"),
                interrupted_operation
            ],
        )
        .unwrap();
    drop(database);

    let (_, pending) = get_json(&router, "/api/trash?start=0&limit=60").await;
    assert_eq!(pending["total"], 1);
    assert_eq!(pending["photos"][0]["photo"]["id"], b_id);
    assert_eq!(
        pending["photos"][0]["pendingVerificationOperationId"],
        interrupted_operation
    );
    let selected_restore = response_json(
        post_json(
            &router,
            "/api/trash/restore",
            serde_json::json!({
                "operationId": "00000000-0000-4000-8000-000000000025",
                "all": true, "photoIds": [], "excludePhotoIds": [],
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(selected_restore["counts"]["restored"], 0);
    assert_eq!(selected_restore["counts"]["changedElsewhere"], 1);
    assert_eq!(selected_restore["results"][0]["photoId"], b_id);
    assert_eq!(
        selected_restore["results"][0]["outcome"],
        "changed-elsewhere"
    );
    let refused = response_json(
        post_json(
            &router,
            "/api/photos/restore",
            serde_json::json!({"operation": second_removal}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(refused["counts"]["restored"], 0);
    assert_eq!(refused["changedElsewhere"], serde_json::json!([b_id]));
    assert!(root.join("b.jpg").exists());
    let blocked_review = response_json(
        post_json(
            &router,
            "/api/trash/review",
            serde_json::json!({
                "operationId": "00000000-0000-4000-8000-000000000023",
                "all": true,
                "photoIds": [],
                "excludePhotoIds": [],
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(blocked_review["items"], serde_json::json!([]));
    assert_eq!(blocked_review["rejected"][0]["photoId"], b_id);
    assert_eq!(
        blocked_review["rejected"][0]["reason"],
        "pending-verification"
    );
    let blocked_selection = response_json(
        post_json(
            &router,
            "/api/trash/review",
            serde_json::json!({
                "operationId": "00000000-0000-4000-8000-000000000025",
                "all": false,
                "photoIds": [b_id],
                "excludePhotoIds": [],
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(blocked_selection["items"], serde_json::json!([]));
    assert_eq!(blocked_selection["rejected"][0]["photoId"], b_id);
    assert_eq!(
        blocked_selection["rejected"][0]["reason"],
        "pending-verification"
    );

    // Resuming the recorded operation settles the Photo it owned.
    let resumed = response_json(
        post_json(
            &router,
            "/api/trash/delete",
            serde_json::json!({"operationId": interrupted_operation}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(resumed["items"][0]["state"], "deleted");
    assert_eq!(resumed["items"][0]["originalLocation"], "b.jpg");
    assert!(!root.join("b.jpg").exists());
    let (_, cleared) = get_json(&router, "/api/trash?start=0&limit=60").await;
    assert_eq!(cleared["total"], 0);
    let (_, recorded) = get_json(
        &router,
        &format!("/api/trash/operations/{interrupted_operation}"),
    )
    .await;
    assert_eq!(recorded, resumed);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
