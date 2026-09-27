use super::*;

#[tokio::test]
async fn rejected_result_removal_hides_photos_and_undo_restores_them_exactly() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    jpeg_fixture(&root.join("a.jpg"), 8, 4, [1, 2, 3]);
    jpeg_fixture(&root.join("b.jpg"), 8, 4, [4, 5, 6]);
    jpeg_fixture(&root.join("c.jpg"), 8, 4, [7, 8, 9]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    assert_eq!(ids.len(), 3);
    let by_location = photo_ids_by_location(&application, &ids).await;
    let rejected_one = by_location["a.jpg"].clone();
    let rejected_two = by_location["b.jpg"].clone();
    let kept = by_location["c.jpg"].clone();
    for photo_id in [&rejected_one, &rejected_two] {
        let response = post_json(
            &router,
            &format!("/api/photos/{photo_id}/state"),
            serde_json::json!({"field": "selectionState", "value": "rejected"}),
            Some("https://camera.local"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    let bytes_before = fs::read(root.join("a.jpg")).unwrap();

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
    assert_eq!(opened["total"], 2);
    let token = opened["token"].as_str().unwrap().to_owned();

    let operation_id = "00000000-0000-4000-8000-000000000001";
    let removed = response_json(
        post_json(
            &router,
            "/api/photos/remove",
            serde_json::json!({"token": token, "operationId": operation_id}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(removed["operationId"], operation_id);
    assert_eq!(removed["counts"]["removed"], 2);
    assert_eq!(removed["counts"]["changedElsewhere"], 0);
    assert_eq!(removed["counts"]["missing"], 0);
    assert_eq!(removed["counts"]["alreadyRemoved"], 0);
    assert_eq!(removed["changedElsewhere"], serde_json::json!([]));
    assert_eq!(removed["missing"], serde_json::json!([]));
    assert_eq!(removed["alreadyRemoved"], serde_json::json!([]));

    // Every normal Library source drops the removed Photos.
    assert_eq!(
        browse_photo_ids(&application, BrowseSourceRequest::Library).await,
        vec![kept.clone()]
    );
    let (_, overview) = get_json(&router, "/api/overview").await;
    assert_eq!(overview["photoCount"], 1);
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
    assert_eq!(reopened["total"], 0);

    // Removal never touches the Original File.
    assert_eq!(fs::read(root.join("a.jpg")).unwrap(), bytes_before);

    // The Removed Photos listing is the recoverable surface.
    let (status, listing) = get_json(&router, "/api/photos/removed?start=0&limit=60").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(listing["total"], 2);
    assert_eq!(listing["start"], 0);
    assert_eq!(listing["limit"], 60);
    assert_eq!(
        listing["operation"],
        serde_json::json!({"operationId": operation_id, "removed": 2})
    );
    let listed = listing["photos"].as_array().unwrap();
    assert_eq!(listed.len(), 2);
    let listed_ids = listed
        .iter()
        .map(|item| item["photo"]["id"].as_str().unwrap().to_owned())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        listed_ids,
        BTreeSet::from([rejected_one.clone(), rejected_two.clone()])
    );
    assert!(
        listed
            .iter()
            .all(|item| item["removedAtMs"].as_u64().is_some_and(|at| at > 0))
    );
    assert!(
        listed
            .iter()
            .all(|item| item["photo"]["selectionState"] == "rejected")
    );

    // Undo restores the whole operation, and the restored Photos keep the
    // decisions they had before the removal.
    let restored = response_json(
        post_json(
            &router,
            "/api/photos/restore",
            serde_json::json!({"operation": operation_id}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(restored["counts"]["restored"], 2);
    assert_eq!(restored["changedElsewhere"], serde_json::json!([]));
    assert_eq!(restored["missing"], serde_json::json!([]));
    let (_, overview) = get_json(&router, "/api/overview").await;
    assert_eq!(overview["photoCount"], 3);
    let (_, listing) = get_json(&router, "/api/photos/removed?start=0&limit=60").await;
    assert_eq!(listing["total"], 0);
    assert_eq!(listing["operation"], serde_json::Value::Null);
    assert_eq!(listing["photos"], serde_json::json!([]));
    let retried = response_json(
        post_json(
            &router,
            "/api/photos/remove",
            serde_json::json!({"token": token, "operationId": operation_id}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(retried["counts"]["removed"], 2);
    let (_, listing) = get_json(&router, "/api/photos/removed?start=0&limit=60").await;
    assert_eq!(listing["total"], 0);
    assert_eq!(listing["photos"], serde_json::json!([]));
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
    assert_eq!(reopened["total"], 2);
    assert_eq!(reopened["selectionCounts"]["rejected"], 2);

    // Undo is a compare-and-set: a second Undo of the same operation restores
    // nothing and overwrites nothing.
    let repeated = response_json(
        post_json(
            &router,
            "/api/photos/restore",
            serde_json::json!({"operation": operation_id}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(repeated["counts"]["restored"], 0);
    assert_eq!(repeated["changedElsewhere"], serde_json::json!([]));

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn a_removed_photo_leaves_a_window_of_a_snapshot_opened_before_its_removal() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    jpeg_fixture(&root.join("a.jpg"), 8, 4, [1, 2, 3]);
    jpeg_fixture(&root.join("b.jpg"), 8, 4, [4, 5, 6]);
    jpeg_fixture(&root.join("c.jpg"), 8, 4, [7, 8, 9]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let by_location = photo_ids_by_location(&application, &ids).await;
    for name in ["a.jpg", "b.jpg"] {
        let response = post_json(
            &router,
            &format!("/api/photos/{}/state", by_location[name]),
            serde_json::json!({"field": "selectionState", "value": "rejected"}),
            Some("https://camera.local"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    // A Snapshot opened before the removal retains the reviewed result, and
    // every window of it is a read of a normal Library source.
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
    assert_eq!(opened["total"], 2);
    let token = opened["token"].as_str().unwrap().to_owned();

    let removed = response_json(
        post_json(
            &router,
            "/api/photos/remove",
            serde_json::json!({
                "token": token,
                "operationId": "00000000-0000-4000-8000-000000000009",
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(removed["counts"]["removed"], 2);

    // No window of the retained Snapshot presents a Photo the Library no
    // longer holds, and no window is answered with a page shorter than the
    // position it names: the Snapshot is expired instead, so the browser
    // reopens the source and reads the Library as it now is.
    let (status, window) =
        get_json(&router, &format!("/api/browse/{token}?start=0&limit=10")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(window["error"], "Browse source expired or not found");

    // A source opened after the removal excludes them too.
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
    assert_eq!(reopened["total"], 0);
    let reopened_token = reopened["token"].as_str().unwrap().to_owned();
    let (_, reopened_window) = get_json(
        &router,
        &format!("/api/browse/{reopened_token}?start=0&limit=10"),
    )
    .await;
    assert_eq!(reopened_window["total"], 0);
    assert_eq!(reopened_window["photos"], serde_json::json!([]));

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn removal_requires_a_rejected_snapshot_and_reports_concurrent_changes() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    jpeg_fixture(&root.join("a.jpg"), 8, 4, [1, 2, 3]);
    jpeg_fixture(&root.join("b.jpg"), 8, 4, [4, 5, 6]);
    jpeg_fixture(&root.join("c.jpg"), 8, 4, [7, 8, 9]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let by_location = photo_ids_by_location(&application, &ids).await;
    let changed = by_location["a.jpg"].clone();
    let removed_id = by_location["b.jpg"].clone();
    for photo_id in [&changed, &removed_id] {
        let response = post_json(
            &router,
            &format!("/api/photos/{photo_id}/state"),
            serde_json::json!({"field": "selectionState", "value": "rejected"}),
            Some("https://camera.local"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    let operation_id = "00000000-0000-4000-8000-000000000002";

    // An unfiltered Snapshot is refused before any state changes.
    let unfiltered = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source": "library"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let refused = post_json(
        &router,
        "/api/photos/remove",
        serde_json::json!({"token": unfiltered["token"], "operationId": operation_id}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(refused).await["error"],
        "Removal requires a Browse Snapshot filtered to Rejected"
    );
    assert_eq!(
        browse_photo_ids(&application, BrowseSourceRequest::Library)
            .await
            .len(),
        3
    );

    // An unknown or expired Snapshot is refused as not found.
    let unknown = post_json(
        &router,
        "/api/photos/remove",
        serde_json::json!({
            "token": "00000000-0000-4000-8000-0000000000ff",
            "operationId": operation_id,
        }),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

    // A malformed removal body is refused without a Snapshot lookup.
    let malformed = post_json(
        &router,
        "/api/photos/remove",
        serde_json::json!({"token": unfiltered["token"]}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);

    // The reviewed result is frozen: a decision changed after the review is
    // reported instead of being removed.
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
    assert_eq!(opened["total"], 2);
    let token = opened["token"].as_str().unwrap().to_owned();
    let decision = post_json(
        &router,
        &format!("/api/photos/{changed}/state"),
        serde_json::json!({"field": "selectionState", "value": "selected"}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(decision.status(), StatusCode::OK);

    let removed = response_json(
        post_json(
            &router,
            "/api/photos/remove",
            serde_json::json!({"token": token, "operationId": operation_id}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(removed["counts"]["removed"], 1);
    assert_eq!(removed["counts"]["changedElsewhere"], 1);
    assert_eq!(removed["counts"]["missing"], 0);
    assert_eq!(removed["counts"]["alreadyRemoved"], 0);
    assert_eq!(removed["changedElsewhere"], serde_json::json!([changed]));

    // A retried request adopts what this operation already removed instead of
    // inventing a second outcome set.
    let retried = response_json(
        post_json(
            &router,
            "/api/photos/remove",
            serde_json::json!({"token": token, "operationId": operation_id}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(retried["counts"]["removed"], 1);
    assert_eq!(retried["counts"]["alreadyRemoved"], 0);
    assert_eq!(retried["counts"]["changedElsewhere"], 1);
    assert_eq!(retried["alreadyRemoved"], serde_json::json!([]));

    // Only the reviewed rejected Photo left the Library.
    let remaining = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    assert_eq!(remaining.len(), 2);
    assert!(remaining.contains(&changed));
    let (_, overview) = get_json(&router, "/api/overview").await;
    assert_eq!(overview["photoCount"], 2);

    // A restore body that names two different sets, none, or a marker the
    // caller could not have read is refused before any state changes.
    for body in [
        serde_json::json!({"operation": operation_id, "photos": [{"id": removed_id, "removedAtMs": 1}]}),
        serde_json::json!({"photos": []}),
        serde_json::json!({"unknown": operation_id}),
        serde_json::json!({"photos": [{"id": removed_id}]}),
        serde_json::json!({"photos": [{"id": removed_id, "removedAtMs": -1}]}),
        serde_json::json!({"photos": [{"id": removed_id, "removedAtMs": 1, "extra": true}]}),
        serde_json::json!({"photos": [{"id": removed_id, "removedAtMs": 1}, {"id": removed_id, "removedAtMs": 2}]}),
    ] {
        let response = post_json(
            &router,
            "/api/photos/restore",
            body,
            Some("https://camera.local"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    // Undo restores exactly this operation's Photo.
    let restored = response_json(
        post_json(
            &router,
            "/api/photos/restore",
            serde_json::json!({"operation": operation_id}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(restored["counts"]["restored"], 1);
    assert_eq!(
        restored["operations"],
        serde_json::json!([{"operationId": operation_id, "removed": 0}])
    );
    let (_, overview) = get_json(&router, "/api/overview").await;
    assert_eq!(overview["photoCount"], 3);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn explicit_cli_removal_restore_reconciles_and_web_reads_back_the_same_state() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    jpeg_fixture(&root.join("a.jpg"), 8, 4, [1, 2, 3]);
    jpeg_fixture(&root.join("b.jpg"), 8, 4, [4, 5, 6]);
    jpeg_fixture(&root.join("c.jpg"), 8, 4, [7, 8, 9]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let by_location = photo_ids_by_location(&application, &ids).await;
    let first = by_location["a.jpg"].clone();
    let second = by_location["b.jpg"].clone();
    let kept = by_location["c.jpg"].clone();
    for photo_id in [&first, &second] {
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
    let first_read = cli_photo_read(&router, &first).await;
    let second_read = cli_photo_read(&router, &second).await;
    assert_eq!(first_read["removedAtMs"], serde_json::Value::Null);
    assert_eq!(second_read["removedAtMs"], serde_json::Value::Null);

    assert_eq!(
        post_json(
            &router,
            &format!("/api/photos/{first}/state"),
            serde_json::json!({"field": "selectionState", "value": "selected"}),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let removed = post_cli_json(
        &router,
        "/api/photos/remove-explicit",
        serde_json::json!({
            "operationId": "00000000-0000-4000-8000-000000000101",
            "photos": [
                {
                    "photoId": first,
                    "selectionState": "rejected",
                    "decisionVersion": first_read["decisionVersion"],
                    "removedAtMs": null
                },
                {
                    "photoId": second,
                    "selectionState": "rejected",
                    "decisionVersion": second_read["decisionVersion"],
                    "removedAtMs": null
                }
            ]
        }),
    )
    .await;
    assert_eq!(removed.status(), StatusCode::OK);
    let removed = response_json(removed).await;
    assert_eq!(removed["counts"]["removed"], 1);
    assert_eq!(removed["counts"]["changedElsewhere"], 1);
    assert_eq!(removed["counts"]["missing"], 0);
    assert_eq!(removed["counts"]["alreadyRemoved"], 0);
    assert_eq!(removed["results"].as_array().unwrap().len(), 2);
    assert_eq!(
        removed["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["photoId"] == second)
            .unwrap()["outcome"],
        "removed"
    );
    let removed_marker = removed["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["photoId"] == second)
        .unwrap()["removedAtMs"]
        .as_i64()
        .unwrap();

    let inspected = get_cli_json(
        &router,
        "/api/photos/removal-operations/00000000-0000-4000-8000-000000000101",
    )
    .await;
    assert_eq!(inspected.status(), StatusCode::OK);
    assert_eq!(response_json(inspected).await, removed);
    let unknown = get_cli_json(
        &router,
        "/api/photos/removal-operations/00000000-0000-4000-8000-000000000199",
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json(unknown).await["error"]["code"],
        "outcome_unknown"
    );

    assert_eq!(
        browse_photo_ids(&application, BrowseSourceRequest::Library).await,
        vec![first.clone(), kept.clone()]
    );
    let (_, trash) = get_json(&router, "/api/photos/removed?start=0&limit=60").await;
    assert_eq!(trash["total"], 1);
    assert_eq!(trash["photos"][0]["photo"]["id"], second);
    assert_eq!(trash["photos"][0]["removedAtMs"], removed_marker);

    let restored = post_cli_json(
        &router,
        "/api/photos/restore-explicit",
        serde_json::json!({
            "operationId": "00000000-0000-4000-8000-000000000102",
            "photos": [{"photoId": second, "removedAtMs": removed_marker}]
        }),
    )
    .await;
    assert_eq!(restored.status(), StatusCode::OK);
    let restored = response_json(restored).await;
    assert_eq!(restored["counts"]["restored"], 1);
    assert_eq!(restored["counts"]["alreadyActive"], 0);
    assert_eq!(restored["counts"]["changedElsewhere"], 0);
    assert_eq!(restored["results"][0]["outcome"], "restored");
    let restore_inspected = get_cli_json(
        &router,
        "/api/photos/restore-operations/00000000-0000-4000-8000-000000000102",
    )
    .await;
    assert_eq!(restore_inspected.status(), StatusCode::OK);
    assert_eq!(response_json(restore_inspected).await, restored);
    assert_eq!(
        browse_photo_ids(&application, BrowseSourceRequest::Library).await,
        vec![first.clone(), second.clone(), kept.clone()]
    );
    assert_eq!(
        cli_photo_read(&router, &second).await["removedAtMs"],
        serde_json::Value::Null
    );

    let duplicate = post_cli_json(
        &router,
        "/api/photos/remove-explicit",
        serde_json::json!({
            "operationId": "00000000-0000-4000-8000-000000000103",
            "photos": [
                {
                    "photoId": first,
                    "selectionState": "rejected",
                    "decisionVersion": first_read["decisionVersion"],
                    "removedAtMs": null
                },
                {
                    "photoId": first,
                    "selectionState": "rejected",
                    "decisionVersion": first_read["decisionVersion"],
                    "removedAtMs": null
                }
            ]
        }),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::BAD_REQUEST);

    let over_limit = (0..=slipstream_core::PHOTO_REMOVAL_MAX)
        .map(|index| {
            serde_json::json!({
                "photoId": format!("00000000-0000-4000-8000-{index:012}"),
                "selectionState": "rejected",
                "decisionVersion": "version",
                "removedAtMs": null
            })
        })
        .collect::<Vec<_>>();
    let over_limit = post_cli_json(
        &router,
        "/api/photos/remove-explicit",
        serde_json::json!({
            "operationId": "00000000-0000-4000-8000-000000000104",
            "photos": over_limit
        }),
    )
    .await;
    assert_eq!(over_limit.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        browse_photo_ids(&application, BrowseSourceRequest::Library).await,
        vec![first, second, kept]
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
#[tokio::test]
async fn removed_photos_leave_folder_counts_and_cli_queries_but_stay_recoverable() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    fs::create_dir_all(root.join("shoot")).unwrap();
    fs::create_dir_all(root.join("other")).unwrap();
    jpeg_fixture(&root.join("shoot/a.jpg"), 8, 4, [1, 2, 3]);
    jpeg_fixture(&root.join("shoot/b.jpg"), 8, 4, [4, 5, 6]);
    jpeg_fixture(&root.join("other/c.jpg"), 8, 4, [7, 8, 9]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    assert_eq!(ids.len(), 3);
    let by_location = photo_ids_by_location(&application, &ids).await;
    for name in ["shoot/a.jpg", "shoot/b.jpg"] {
        let response = post_json(
            &router,
            &format!("/api/photos/{}/state", by_location[name]),
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
    assert_eq!(opened["total"], 2);
    let removed = response_json(
        post_json(
            &router,
            "/api/photos/remove",
            serde_json::json!({
                "token": opened["token"],
                "operationId": "00000000-0000-4000-8000-000000000003",
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(removed["counts"]["removed"], 2);

    // The Original Folder the removed Photos projected through now counts and
    // opens without them.
    let (_, overview) = get_json(&router, "/api/overview").await;
    let publication = overview["publication"].as_str().unwrap().to_owned();
    let (status, folders) = get_json(
        &router,
        &format!("/api/file-locations?publication={publication}&parent=shoot&start=0&limit=60"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(folders["total"], 0);
    let (_, root_folders) = get_json(
        &router,
        &format!("/api/file-locations?publication={publication}&start=0&limit=60"),
    )
    .await;
    let counts = root_folders["children"]
        .as_array()
        .unwrap()
        .iter()
        .map(|child| {
            (
                child["name"].as_str().unwrap().to_owned(),
                child["photoCount"].as_u64().unwrap(),
            )
        })
        .collect::<HashMap<_, _>>();
    assert_eq!(counts["shoot"], 0);
    assert_eq!(counts["other"], 1);
    let folder_snapshot = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({
                "source": "folder",
                "folderPath": "shoot",
                "publication": publication,
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(folder_snapshot["total"], 0);

    // The CLI Photo query is a normal source too.
    let query = response_json(
        post_cli_json(
            &router,
            "/api/photo-queries",
            serde_json::json!({"limit": 10}),
        )
        .await,
    )
    .await;
    let items = query["items"].as_array().unwrap();
    assert_eq!(query["total"], 1);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], by_location["other/c.jpg"]);

    // The Removed Photos listing is bounded and ordered newest removal first.
    let (_, bounded) = get_json(&router, "/api/photos/removed?start=0&limit=0").await;
    assert_eq!(bounded["error"], "Removed Photos window is invalid");
    let (_, bounded) = get_json(&router, "/api/photos/removed?start=0&limit=61").await;
    assert_eq!(bounded["error"], "Removed Photos window is invalid");
    let (_, first_page) = get_json(&router, "/api/photos/removed?start=0&limit=1").await;
    assert_eq!(first_page["total"], 2);
    assert_eq!(first_page["photos"].as_array().unwrap().len(), 1);
    let (_, second_page) = get_json(&router, "/api/photos/removed?start=1&limit=1").await;
    assert_eq!(second_page["total"], 2);
    assert_eq!(second_page["photos"].as_array().unwrap().len(), 1);
    let first = first_page["photos"][0]["photo"]["id"].as_str().unwrap();
    let second = second_page["photos"][0]["photo"]["id"].as_str().unwrap();
    assert_ne!(first, second);
    let at_first = first_page["photos"][0]["removedAtMs"].as_u64().unwrap();
    let at_second = second_page["photos"][0]["removedAtMs"].as_u64().unwrap();
    assert!(at_first >= at_second);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn removal_keeps_the_cli_projection_in_step_without_a_rescan() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    jpeg_fixture(&root.join("a.jpg"), 8, 4, [1, 2, 3]);
    jpeg_fixture(&root.join("b.jpg"), 8, 4, [4, 5, 6]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    assert_eq!(ids.len(), 2);
    let by_location = photo_ids_by_location(&application, &ids).await;
    for name in ["a.jpg", "b.jpg"] {
        let response = post_json(
            &router,
            &format!("/api/photos/{}/state", by_location[name]),
            serde_json::json!({"field": "selectionState", "value": "rejected"}),
            Some("https://camera.local"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }
    let removed_id = by_location["a.jpg"].clone();

    // A machine client reads its query before the removal, so the retained
    // sequence holds the identities the removal is about to hide. The second
    // page is read back after the removal.
    let query = response_json(
        post_cli_json(
            &router,
            "/api/photo-queries",
            serde_json::json!({"limit": 1}),
        )
        .await,
    )
    .await;
    assert_eq!(query["total"], 2);
    assert_eq!(query["items"].as_array().unwrap().len(), 1);
    let cursor = query["nextCursor"].as_str().unwrap().to_owned();
    let before =
        response_json(get_cli_json(&router, &format!("/api/photos/{removed_id}")).await).await;
    assert_eq!(before["id"], removed_id);

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
                "operationId": "00000000-0000-4000-8000-000000000004",
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(removed["counts"]["removed"], 2);

    // The committed removal moves the derived CLI projection with it, so the
    // retained page reports the removed Photo as missing instead of present,
    // and reading that Photo is not found.
    let page =
        response_json(get_cli_json(&router, &format!("/api/photo-queries/{cursor}")).await).await;
    assert_eq!(page["total"], 2);
    let items = page["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["state"], "missing");
    assert!(items[0].get("preview").is_none());
    let removed_id = items[0]["id"].as_str().unwrap().to_owned();
    let after = get_cli_json(&router, &format!("/api/photos/{removed_id}")).await;
    assert_eq!(after.status(), StatusCode::NOT_FOUND);
    assert_eq!(response_json(after).await["error"]["code"], "not_found");

    // Undo re-admits the Photo into the projection in the same step, so a
    // query created afterwards already lists it again.
    let restored = response_json(
        post_json(
            &router,
            "/api/photos/restore",
            serde_json::json!({"operation": "00000000-0000-4000-8000-000000000004"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(restored["counts"]["restored"], 2);
    let query = response_json(
        post_cli_json(
            &router,
            "/api/photo-queries",
            serde_json::json!({"limit": 10}),
        )
        .await,
    )
    .await;
    assert_eq!(query["total"], 2);
    assert!(
        query["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["state"].is_null())
    );
    let after = get_cli_json(&router, &format!("/api/photos/{removed_id}")).await;
    assert_eq!(after.status(), StatusCode::OK);

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
