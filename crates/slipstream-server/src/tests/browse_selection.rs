use super::*;
/// Commits one Selection State through the HTTP mutation route so a fixture
/// holds real persisted decisions rather than fabricated facts.
pub(super) async fn decide_selection(router: &Router, photo_id: &str, value: &str) -> StatusCode {
    post_json(
        router,
        &format!("https://camera.local/api/photos/{photo_id}/state"),
        serde_json::json!({"field": "selectionState", "value": value}),
        Some("https://camera.local"),
    )
    .await
    .status()
}

/// Opens one source with one Selection State filter and reads the whole view
/// through bounded windows, exactly as the browser must.
async fn browse_filtered_summaries(
    application: &Application,
    source: BrowseSourceRequest,
    selection: BrowseSelectionFilter,
) -> (BrowseOpenResponse, Vec<PhotoSummary>) {
    let order = default_order(&source);
    let opened = application
        .browse_open(source, order, selection, None)
        .await
        .expect("filtered browse open succeeds");
    let mut photos = Vec::new();
    let mut start = 0;
    loop {
        let window = application
            .browse_window(&opened.token, start, 60)
            .await
            .expect("filtered browse window succeeds");
        assert_eq!(window.total, opened.total);
        let count = window.photos.len();
        photos.extend(window.photos);
        start += count;
        if count == 0 || start >= opened.total {
            break;
        }
    }
    assert_eq!(photos.len(), opened.total, "filtered traversal incomplete");
    application.browse_close(&opened.token);
    (opened, photos)
}

#[tokio::test]
async fn browse_selection_filter_selects_from_the_source_order_with_source_counts() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    for (name, capture_time) in [
        ("a.jpg", "2026:01:01 09:00:00"),
        ("b.jpg", "2026:01:01 10:00:00"),
        ("c.jpg", "2026:01:01 11:00:00"),
        ("d.jpg", "2026:01:01 12:00:00"),
        ("e.jpg", "2026:01:01 13:00:00"),
    ] {
        capture_metadata_fixture(&root.join(name), capture_time);
    }
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let by_location = photo_ids_by_location(&application, &ids).await;
    for (name, value) in [
        ("a.jpg", "picked"),
        ("c.jpg", "picked"),
        ("b.jpg", "rejected"),
    ] {
        assert_eq!(
            decide_selection(&router, &by_location[name], value).await,
            StatusCode::OK,
            "{name}"
        );
    }

    // Every view is a projection of the same source order: the filtered
    // sequence keeps Capture Time order and the counts stay source-wide.
    for (selection, expected_locations) in [
        (BrowseSelectionFilter::All, vec!["a", "b", "c", "d", "e"]),
        (BrowseSelectionFilter::Picked, vec!["a", "c"]),
        (BrowseSelectionFilter::Rejected, vec!["b"]),
        (BrowseSelectionFilter::Unflagged, vec!["d", "e"]),
    ] {
        let (opened, photos) =
            browse_filtered_summaries(&application, BrowseSourceRequest::Library, selection).await;
        assert_eq!(opened.total, expected_locations.len(), "{selection:?}");
        assert_eq!(photos.len(), expected_locations.len(), "{selection:?}");
        assert_eq!(
            photos
                .iter()
                .map(|photo| photo
                    .original_filename
                    .clone()
                    .unwrap()
                    .split('.')
                    .next()
                    .unwrap()
                    .to_owned())
                .collect::<Vec<_>>(),
            expected_locations
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>(),
            "{selection:?}"
        );
        assert_eq!(
            opened.selection_counts,
            SelectionCountsWire {
                picked: 2,
                rejected: 1,
                unflagged: 2,
            },
            "counts describe the source for {selection:?}"
        );
    }

    // Positions resolve inside the filtered sequence, so an anchor that the
    // filter excluded falls back to the first filtered Photo.
    let selected = application
        .browse_open(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::Picked,
            Some(&by_location["c.jpg"]),
        )
        .await
        .unwrap();
    assert_eq!(selected.position, 1);
    let window = application
        .browse_window(&selected.token, 0, 1)
        .await
        .unwrap();
    assert_eq!(window.photos[0].id, by_location["a.jpg"]);
    let filtered_out = application
        .browse_open(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::Picked,
            Some(&by_location["b.jpg"]),
        )
        .await
        .unwrap();
    assert_eq!(filtered_out.position, 0);
    application.browse_close(&selected.token);
    application.browse_close(&filtered_out.token);

    // The route rejects an unknown value before any Snapshot exists, and a
    // valid value reaches the same filtered projection as the direct call.
    let unknown = post_json(
        &router,
        "/api/browse",
        serde_json::json!({"source": "library", "selection": "maybe"}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(unknown).await,
        serde_json::json!({"error": "Invalid browse selection"})
    );
    let routed = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source": "library", "selection": "rejected"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(routed["total"], 1);
    assert_eq!(
        routed["selectionCounts"],
        serde_json::json!({"picked": 2, "rejected": 1, "unflagged": 2})
    );
    let routed_token = routed["token"].as_str().unwrap().to_owned();
    let routed_window = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/browse/{routed_token}?start=0&limit=60"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(routed_window["photos"][0]["id"], by_location["b.jpg"]);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn browse_selection_filter_projects_album_and_folder_sources_without_writes() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    fs::create_dir_all(root.join("shoot")).unwrap();
    let shoot = root.join("shoot");
    for name in ["p1.jpg", "p2.jpg", "p3.jpg"] {
        jpeg_fixture(&shoot.join(name), 8, 4, [32, 64, 192]);
    }
    jpeg_fixture(&root.join("outside.jpg"), 8, 4, [12, 24, 36]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let by_location = photo_ids_by_location(&application, &ids).await;
    application
        .mutate_album(slipstream_core::AlbumMutation::Create {
            name: "Review".to_owned(),
        })
        .await
        .unwrap();
    let album_id = application
        .albums()
        .await
        .unwrap()
        .albums
        .into_iter()
        .find(|album| album.name == "Review")
        .unwrap()
        .id;
    application
        .mutate_album(slipstream_core::AlbumMutation::AddMembers {
            album_id: album_id.clone(),
            photo_ids: vec![
                by_location["shoot/p3.jpg"].clone(),
                by_location["shoot/p1.jpg"].clone(),
                by_location["shoot/p2.jpg"].clone(),
            ],
        })
        .await
        .unwrap();
    assert_eq!(
        decide_selection(&router, &by_location["shoot/p1.jpg"], "picked").await,
        StatusCode::OK
    );
    assert_eq!(
        decide_selection(&router, &by_location["shoot/p2.jpg"], "rejected").await,
        StatusCode::OK
    );

    // Album pages stay in membership position, so the filtered view keeps
    // the persisted member order instead of the Library order.
    let (opened, photos) = browse_filtered_summaries(
        &application,
        BrowseSourceRequest::Album(album_id.clone()),
        BrowseSelectionFilter::Picked,
    )
    .await;
    assert_eq!(opened.total, 1);
    assert_eq!(photos[0].id, by_location["shoot/p1.jpg"]);
    assert_eq!(
        opened.selection_counts,
        SelectionCountsWire {
            picked: 1,
            rejected: 1,
            unflagged: 1,
        }
    );
    let (all_members, _) = browse_filtered_summaries(
        &application,
        BrowseSourceRequest::Album(album_id.clone()),
        BrowseSelectionFilter::All,
    )
    .await;
    assert_eq!(all_members.total, 3);
    // No filter value rewrites persisted membership position.
    let target = application
        .library
        .album_browse_target(&album_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        target
            .members
            .iter()
            .map(|member| member.photo_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            by_location["shoot/p3.jpg"].as_str(),
            by_location["shoot/p1.jpg"].as_str(),
            by_location["shoot/p2.jpg"].as_str()
        ]
    );

    // A Folder source filters the same recursive projection, and Photos
    // outside the Folder never appear in it: a matching Photo elsewhere in
    // the Library must not leak into the Folder view or its counts.
    assert_eq!(
        decide_selection(&router, &by_location["outside.jpg"], "picked").await,
        StatusCode::OK
    );
    let publication = {
        let guard = application.shared.snapshot.read().unwrap();
        guard.as_ref().unwrap().publication_value()
    };
    let (folder_opened, folder_photos) = browse_filtered_summaries(
        &application,
        BrowseSourceRequest::Folder {
            location: "shoot".to_owned(),
            publication,
        },
        BrowseSelectionFilter::Picked,
    )
    .await;
    assert_eq!(folder_opened.total, 1);
    assert_eq!(folder_photos[0].id, by_location["shoot/p1.jpg"]);
    // The Folder's counts stay scoped to the Folder: `outside.jpg` is selected
    // in the Library but is not part of this source.
    assert_eq!(folder_opened.selection_counts.picked, 1);
    assert_eq!(folder_opened.selection_counts.rejected, 1);
    assert_eq!(folder_opened.selection_counts.unflagged, 1);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn album_view_change_anchors_the_current_photo_instead_of_the_saved_position() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    capture_metadata_fixture(&root.join("a.jpg"), "2026:01:01 09:00:00");
    capture_metadata_fixture(&root.join("b.jpg"), "2026:01:01 10:00:00");
    capture_metadata_fixture(&root.join("c.jpg"), "2026:01:01 11:00:00");
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let by_location = photo_ids_by_location(&application, &ids).await;
    let (a, b, c) = (
        by_location["a.jpg"].clone(),
        by_location["b.jpg"].clone(),
        by_location["c.jpg"].clone(),
    );
    let album = application
        .mutate_album(slipstream_core::AlbumMutation::Create {
            name: "Review".to_owned(),
        })
        .await
        .unwrap()
        .albums
        .into_iter()
        .find(|album| album.name == "Review")
        .unwrap()
        .id;
    application
        .mutate_album(slipstream_core::AlbumMutation::AddMembers {
            album_id: album.clone(),
            photo_ids: vec![a.clone(), b.clone(), c.clone()],
        })
        .await
        .unwrap();
    // `c` is the durable saved position and `a` is the browser's current
    // Photo, so the saved member matches the filter while the anchor does not.
    application
        .mutate_album(slipstream_core::AlbumMutation::SetProgress {
            album_id: album.clone(),
            photo_id: c.clone(),
        })
        .await
        .unwrap();
    assert_eq!(
        decide_selection(&router, &b, "picked").await,
        StatusCode::OK
    );
    assert_eq!(
        decide_selection(&router, &c, "picked").await,
        StatusCode::OK
    );

    // A view change reopens with the browser's current Photo as the anchor.
    // The anchor no longer matches the filter, so the view starts at its first
    // Photo (`b`) instead of resuming at the saved Photo (`c`).
    let changed = application
        .browse_open(
            BrowseSourceRequest::Album(album.clone()),
            BrowseViewOrder::AlbumOrder,
            BrowseSelectionFilter::Picked,
            Some(&a),
        )
        .await
        .unwrap();
    assert_eq!(changed.total, 2);
    assert_eq!(changed.position, 0);
    let window = application
        .browse_window(&changed.token, changed.position, 1)
        .await
        .unwrap();
    assert_eq!(window.photos[0].id, b);
    application.browse_close(&changed.token);

    // An explicit anchor that matches the filter still outranks the saved
    // position, and an open without one keeps resuming at it.
    let anchored = application
        .browse_open(
            BrowseSourceRequest::Album(album.clone()),
            BrowseViewOrder::AlbumOrder,
            BrowseSelectionFilter::Picked,
            Some(&c),
        )
        .await
        .unwrap();
    assert_eq!(anchored.position, 1);
    application.browse_close(&anchored.token);
    let plain = application
        .browse_open(
            BrowseSourceRequest::Album(album),
            BrowseViewOrder::AlbumOrder,
            BrowseSelectionFilter::Picked,
            None,
        )
        .await
        .unwrap();
    assert_eq!(plain.position, 1);
    application.browse_close(&plain.token);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn browse_selection_filter_membership_is_frozen_until_the_source_reopens() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    for name in ["a.jpg", "b.jpg"] {
        jpeg_fixture(&root.join(name), 8, 4, [32, 64, 192]);
    }
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let by_location = photo_ids_by_location(&application, &ids).await;
    assert_eq!(
        decide_selection(&router, &by_location["a.jpg"], "picked").await,
        StatusCode::OK
    );
    let opened = application
        .browse_open(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::Picked,
            None,
        )
        .await
        .unwrap();
    assert_eq!(opened.total, 1);
    assert_eq!(opened.selection_counts.picked, 1);

    // A decision cannot change an open Snapshot's membership: the frozen
    // view still lists the Photo, and only reopening applies the filter to
    // the latest facts.
    assert_eq!(
        decide_selection(&router, &by_location["a.jpg"], "rejected").await,
        StatusCode::OK
    );
    let window = application
        .browse_window(&opened.token, 0, 60)
        .await
        .unwrap();
    assert_eq!(window.total, 1);
    assert_eq!(window.photos[0].id, by_location["a.jpg"]);
    assert_eq!(window.photos[0].selection_state, "rejected");

    let reopened = application
        .browse_open(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::Picked,
            Some(&by_location["a.jpg"]),
        )
        .await
        .unwrap();
    assert_eq!(reopened.total, 0);
    // The anchor no longer matches, so the reopened view reports the same
    // empty position as any other empty source.
    assert_eq!(reopened.position, 0);
    assert_eq!(
        reopened.selection_counts,
        SelectionCountsWire {
            picked: 0,
            rejected: 1,
            unflagged: 1,
        }
    );
    application.browse_close(&opened.token);
    application.browse_close(&reopened.token);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
