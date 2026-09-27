use super::*;

/// Bounded traversal for one explicit view order. Tests observe order only
/// through the bounded protocol, never a complete-Photo route.
async fn browse_ids_in_order(
    application: &Application,
    source: BrowseSourceRequest,
    order: BrowseViewOrder,
) -> Vec<String> {
    browse_ids_in_pages(application, source, order, 60).await
}

async fn browse_ids_in_pages(
    application: &Application,
    source: BrowseSourceRequest,
    order: BrowseViewOrder,
    limit: usize,
) -> Vec<String> {
    let opened = application
        .browse_open(source, order, BrowseSelectionFilter::All, None)
        .await
        .expect("browse open succeeds");
    let mut ids = Vec::new();
    let mut start = 0;
    loop {
        let window = application
            .browse_window(&opened.token, start, limit)
            .await
            .expect("browse window succeeds");
        let count = window.photos.len();
        ids.extend(window.photos.into_iter().map(|photo| photo.id));
        start += count;
        if count == 0 || start >= opened.total {
            break;
        }
    }
    application.browse_close(&opened.token);
    ids
}

/// Maps Photo IDs to their ordering Location names from one Library snapshot
/// read, so order assertions compare real persisted facts rather than test
/// guesses about generated identities.
async fn ordering_locations(application: &Application, ids: &[String]) -> Vec<String> {
    let snapshot = application.library.snapshot().await.unwrap();
    let locations: HashMap<&str, &str> = snapshot
        .photos
        .iter()
        .map(|photo| (photo.id.as_str(), photo.sort_path.as_str()))
        .collect();
    ids.iter()
        .map(|id| locations[id.as_str()].to_owned())
        .collect()
}

/// Reads the Photo an Album Snapshot resumes at, exactly like a browser:
/// open the Snapshot, then read its reported position.
async fn album_resume(
    application: &Application,
    album_id: &str,
    order: BrowseViewOrder,
) -> (usize, String) {
    let opened = application
        .browse_open(
            BrowseSourceRequest::Album(album_id.to_owned()),
            order,
            BrowseSelectionFilter::All,
            None,
        )
        .await
        .expect("album browse open succeeds");
    let window = application
        .browse_window(&opened.token, opened.position, 1)
        .await
        .expect("album resume window succeeds");
    let photo_id = window.photos[0].id.clone();
    application.browse_close(&opened.token);
    (opened.position, photo_id)
}

#[tokio::test]
async fn browse_view_order_reverses_only_capture_time_and_keeps_missing_last() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    capture_metadata_fixture(&root.join("a.jpg"), "2026:01:01 09:00:00");
    capture_metadata_fixture(&root.join("c.jpg"), "2026:01:01 09:00:00");
    capture_metadata_fixture(&root.join("b.jpg"), "2026:01:01 10:00:00");
    jpeg_fixture(&root.join("d.jpg"), 8, 4, [1, 2, 3]);
    jpeg_fixture(&root.join("z.jpg"), 8, 4, [4, 5, 6]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;

    let ascending = browse_ids_in_order(
        &application,
        BrowseSourceRequest::Library,
        BrowseViewOrder::CaptureTimeAscending,
    )
    .await;
    assert_eq!(
        ordering_locations(&application, &ascending).await,
        vec!["a.jpg", "c.jpg", "b.jpg", "d.jpg", "z.jpg"]
    );
    let descending = browse_ids_in_order(
        &application,
        BrowseSourceRequest::Library,
        BrowseViewOrder::CaptureTimeDescending,
    )
    .await;
    // Equal Capture Times keep the Location tie-breaker ascending and the
    // missing-time partition stays last instead of leading the view.
    assert_eq!(
        ordering_locations(&application, &descending).await,
        vec!["b.jpg", "a.jpg", "c.jpg", "d.jpg", "z.jpg"]
    );
    assert_eq!(ascending.len(), 5);
    // The Published Library keeps its natural ascending order: a view order
    // is a projection, not a rewrite.
    let snapshot = application.library.snapshot().await.unwrap();
    assert_eq!(
        snapshot
            .photos
            .iter()
            .map(|photo| photo.sort_path.as_str())
            .collect::<Vec<_>>(),
        vec!["a.jpg", "c.jpg", "b.jpg", "d.jpg", "z.jpg"]
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn folder_view_order_reverses_only_capture_time_within_the_subtree() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    fs::create_dir_all(root.join("sub")).unwrap();
    capture_metadata_fixture(&root.join("sub/a.jpg"), "2026:01:01 09:00:00");
    capture_metadata_fixture(&root.join("sub/b.jpg"), "2026:01:01 10:00:00");
    jpeg_fixture(&root.join("sub/c.jpg"), 8, 4, [1, 2, 3]);
    capture_metadata_fixture(&root.join("other.jpg"), "2026:01:01 08:00:00");
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let publication = application
        .file_locations(None, "", 0, 60)
        .await
        .unwrap()
        .publication;
    let folder = || BrowseSourceRequest::Folder {
        location: "sub".to_owned(),
        publication: publication.clone(),
    };
    let ascending = browse_ids_in_order(
        &application,
        folder(),
        BrowseViewOrder::CaptureTimeAscending,
    )
    .await;
    assert_eq!(
        ordering_locations(&application, &ascending).await,
        vec!["sub/a.jpg", "sub/b.jpg", "sub/c.jpg"]
    );
    let descending = browse_ids_in_order(
        &application,
        folder(),
        BrowseViewOrder::CaptureTimeDescending,
    )
    .await;
    assert_eq!(
        ordering_locations(&application, &descending).await,
        vec!["sub/b.jpg", "sub/a.jpg", "sub/c.jpg"]
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn album_time_views_order_members_without_rewriting_membership_positions() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    capture_metadata_fixture(&root.join("a.jpg"), "2026:01:01 09:00:00");
    capture_metadata_fixture(&root.join("b.jpg"), "2026:01:01 10:00:00");
    jpeg_fixture(&root.join("c.jpg"), 8, 4, [1, 2, 3]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let library_ids = browse_ids_in_order(
        &application,
        BrowseSourceRequest::Library,
        BrowseViewOrder::CaptureTimeAscending,
    )
    .await;
    let locations = ordering_locations(&application, &library_ids).await;
    let by_location: HashMap<&str, &String> = locations
        .iter()
        .map(|location| location.as_str())
        .zip(library_ids.iter())
        .collect();
    let a = by_location["a.jpg"].clone();
    let b = by_location["b.jpg"].clone();
    let c = by_location["c.jpg"].clone();

    application
        .mutate_album(slipstream_core::AlbumMutation::Create {
            name: "Picks".to_owned(),
        })
        .await
        .unwrap();
    let album_id = application
        .albums()
        .await
        .unwrap()
        .albums
        .into_iter()
        .find(|album| album.name == "Picks")
        .unwrap()
        .id;
    // Membership order is deliberately not Capture Time order.
    application
        .mutate_album(slipstream_core::AlbumMutation::AddMembers {
            album_id: album_id.clone(),
            photo_ids: vec![c.clone(), b.clone(), a.clone()],
        })
        .await
        .unwrap();

    let album_order = browse_ids_in_order(
        &application,
        BrowseSourceRequest::Album(album_id.clone()),
        BrowseViewOrder::AlbumOrder,
    )
    .await;
    assert_eq!(album_order, vec![c.clone(), b.clone(), a.clone()]);
    let time_ascending = browse_ids_in_order(
        &application,
        BrowseSourceRequest::Album(album_id.clone()),
        BrowseViewOrder::CaptureTimeAscending,
    )
    .await;
    assert_eq!(time_ascending, vec![a.clone(), b.clone(), c.clone()]);
    let time_descending = browse_ids_in_order(
        &application,
        BrowseSourceRequest::Album(album_id.clone()),
        BrowseViewOrder::CaptureTimeDescending,
    )
    .await;
    assert_eq!(time_descending, vec![b.clone(), a.clone(), c.clone()]);

    // A preferred Photo resolves by identity inside the requested view.
    let preferred = application
        .browse_open(
            BrowseSourceRequest::Album(album_id.clone()),
            BrowseViewOrder::CaptureTimeDescending,
            BrowseSelectionFilter::All,
            Some(&a),
        )
        .await
        .unwrap();
    assert_eq!(preferred.position, 1);
    application.browse_close(&preferred.token);

    // Persisted membership positions keep the Album's own order.
    let album = application
        .library
        .list_albums()
        .await
        .unwrap()
        .into_iter()
        .find(|album| album.id == album_id)
        .unwrap();
    assert_eq!(
        album
            .members
            .iter()
            .map(|member| member.photo_id.as_str())
            .collect::<Vec<_>>(),
        vec![c.as_str(), b.as_str(), a.as_str()]
    );
    assert_eq!(
        album
            .members
            .iter()
            .map(|member| member.position)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn browse_open_rejects_unknown_and_source_invalid_order() {
    let (base, config) = prepare_fixture();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let unknown = post_json(
        &router,
        "/api/browse",
        serde_json::json!({"source": "library", "order": "newest-first"}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(unknown).await,
        serde_json::json!({"error": "Invalid browse order"})
    );
    let album_order_on_library = post_json(
        &router,
        "/api/browse",
        serde_json::json!({"source": "library", "order": "album-order"}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(album_order_on_library.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(album_order_on_library).await,
        serde_json::json!({"error": "Invalid browse order"})
    );
    let folder_album_order = post_json(
        &router,
        "/api/browse",
        serde_json::json!({
            "source": "folder",
            "folderPath": "",
            "publication": "0123456789abcdef",
            "order": "album-order"
        }),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(folder_album_order.status(), StatusCode::BAD_REQUEST);
    let accepted = post_json(
        &router,
        "/api/browse",
        serde_json::json!({"source": "library", "order": "capture-time-desc"}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(accepted.status(), StatusCode::OK);
    // The source/order compatibility rule lives in the application boundary,
    // so a direct caller cannot silently reinterpret `album-order`.
    assert!(matches!(
        application
            .browse_open(
                BrowseSourceRequest::Library,
                BrowseViewOrder::AlbumOrder,
                BrowseSelectionFilter::All,
                None,
            )
            .await,
        Err(ServerError::BrowseOrder)
    ));
    assert!(matches!(
        application
            .browse_open(
                BrowseSourceRequest::Folder {
                    location: "".to_owned(),
                    publication: "0123456789abcdef".to_owned(),
                },
                BrowseViewOrder::AlbumOrder,
                BrowseSelectionFilter::All,
                None,
            )
            .await,
        Err(ServerError::BrowseOrder)
    ));
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn photo_albums_route_reports_true_membership_from_the_owner() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    jpeg_fixture(&root.join("a.jpg"), 8, 4, [1, 2, 3]);
    jpeg_fixture(&root.join("b.jpg"), 8, 4, [4, 5, 6]);
    jpeg_fixture(&root.join("c.jpg"), 8, 4, [7, 8, 9]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_ids_in_order(
        &application,
        BrowseSourceRequest::Library,
        BrowseViewOrder::CaptureTimeAscending,
    )
    .await;
    assert_eq!(ids.len(), 3);
    for name in ["Picks", "Later"] {
        application
            .mutate_album(slipstream_core::AlbumMutation::Create {
                name: name.to_owned(),
            })
            .await
            .unwrap();
    }
    let albums = application.albums().await.unwrap().albums;
    let picks = albums
        .iter()
        .find(|album| album.name == "Picks")
        .unwrap()
        .id
        .clone();
    let later = albums
        .iter()
        .find(|album| album.name == "Later")
        .unwrap()
        .id
        .clone();
    application
        .mutate_album(slipstream_core::AlbumMutation::AddMembers {
            album_id: picks.clone(),
            photo_ids: vec![ids[0].clone()],
        })
        .await
        .unwrap();
    application
        .mutate_album(slipstream_core::AlbumMutation::AddMembers {
            album_id: later.clone(),
            photo_ids: vec![ids[0].clone(), ids[1].clone()],
        })
        .await
        .unwrap();
    // Re-adding an existing member must not duplicate membership.
    application
        .mutate_album(slipstream_core::AlbumMutation::AddMembers {
            album_id: picks.clone(),
            photo_ids: vec![ids[0].clone()],
        })
        .await
        .unwrap();

    let (status, body) = get_json(&router, &format!("/api/photos/{}/albums", ids[0])).await;
    assert_eq!(status, StatusCode::OK);
    // Both routes order Albums by creation time and ID. Equal timestamps are
    // resolved by the generated IDs, not by the order of create calls.
    let expected = albums
        .iter()
        .map(|album| serde_json::json!({"id": album.id, "name": album.name}))
        .collect::<Vec<_>>();
    assert_eq!(body, serde_json::json!({"albums": expected}));
    let (status, body) = get_json(&router, &format!("/api/photos/{}/albums", ids[1])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        serde_json::json!({"albums": [{"id": later, "name": "Later"}]})
    );
    let (status, body) = get_json(&router, &format!("/api/photos/{}/albums", ids[2])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({"albums": []}));
    let (status, body) = get_json(
        &router,
        "/api/photos/00000000-0000-4000-8000-000000000000/albums",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, serde_json::json!({"error": "Photo not found"}));
    let (status, body) = get_json(&router, "/api/photos/NOT-A-ID/albums").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, serde_json::json!({"error": "Invalid Photo"}));
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn album_saved_position_falls_back_by_membership_position_in_time_views() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    // `c` deliberately has no Capture Time so a time view puts it last.
    capture_metadata_fixture(&root.join("a.jpg"), "2026:01:01 09:00:00");
    capture_metadata_fixture(&root.join("b.jpg"), "2026:01:01 10:00:00");
    jpeg_fixture(&root.join("c.jpg"), 8, 4, [1, 2, 3]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let ids = browse_ids_in_order(
        &application,
        BrowseSourceRequest::Library,
        BrowseViewOrder::CaptureTimeAscending,
    )
    .await;
    let by_name = photo_ids_by_location(&application, &ids).await;
    let (a, b, c) = (
        by_name["a.jpg"].clone(),
        by_name["b.jpg"].clone(),
        by_name["c.jpg"].clone(),
    );

    // Membership order starts with the Photo that will become unavailable.
    let album = application
        .mutate_album(slipstream_core::AlbumMutation::Create {
            name: "Picks".to_owned(),
        })
        .await
        .unwrap()
        .albums
        .into_iter()
        .find(|album| album.name == "Picks")
        .unwrap()
        .id;
    application
        .mutate_album(slipstream_core::AlbumMutation::AddMembers {
            album_id: album.clone(),
            photo_ids: vec![c.clone(), a.clone(), b.clone()],
        })
        .await
        .unwrap();
    application
        .mutate_album(slipstream_core::AlbumMutation::SetProgress {
            album_id: album.clone(),
            photo_id: c.clone(),
        })
        .await
        .unwrap();
    fs::remove_file(root.join("c.jpg")).unwrap();
    application.rescan().await.unwrap();

    // Saved `c` is unavailable, so every order resumes at the next available
    // member by membership position: `a`.
    assert_eq!(
        album_resume(&application, &album, BrowseViewOrder::AlbumOrder).await,
        (1, a.clone())
    );
    let ascending = browse_ids_in_order(
        &application,
        BrowseSourceRequest::Album(album.clone()),
        BrowseViewOrder::CaptureTimeAscending,
    )
    .await;
    assert_eq!(
        ordering_locations(&application, &ascending).await,
        vec!["a.jpg", "b.jpg", "c.jpg"]
    );
    assert_eq!(
        album_resume(&application, &album, BrowseViewOrder::CaptureTimeAscending).await,
        (0, a.clone())
    );
    let descending = browse_ids_in_order(
        &application,
        BrowseSourceRequest::Album(album.clone()),
        BrowseViewOrder::CaptureTimeDescending,
    )
    .await;
    assert_eq!(
        ordering_locations(&application, &descending).await,
        vec!["b.jpg", "a.jpg", "c.jpg"]
    );
    // Membership position picks `a` even though its view position differs.
    assert_eq!(
        album_resume(&application, &album, BrowseViewOrder::CaptureTimeDescending).await,
        (1, a.clone())
    );
    // The explicit preferred Photo still outranks the saved position, and the
    // persisted membership positions never move.
    let opened = application
        .browse_open(
            BrowseSourceRequest::Album(album.clone()),
            BrowseViewOrder::CaptureTimeDescending,
            BrowseSelectionFilter::All,
            Some(&b),
        )
        .await
        .unwrap();
    assert_eq!(opened.position, 0);
    application.browse_close(&opened.token);
    assert_eq!(
        browse_ids_in_order(
            &application,
            BrowseSourceRequest::Album(album),
            BrowseViewOrder::AlbumOrder,
        )
        .await,
        vec![c, a, b]
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn descending_paged_windows_stay_globally_ordered_without_duplicates() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    for index in 0..65u32 {
        let name = format!("n{index:02}.jpg");
        capture_metadata_fixture(
            &root.join(&name),
            &format!("2026:01:01 09:{:02}:{:02}", index / 60, index % 60),
        );
    }
    // Two Photos without a valid Capture Time stay last in both directions.
    jpeg_fixture(&root.join("d.jpg"), 8, 4, [1, 2, 3]);
    jpeg_fixture(&root.join("z.jpg"), 8, 4, [4, 5, 6]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;

    let descending = browse_ids_in_pages(
        &application,
        BrowseSourceRequest::Library,
        BrowseViewOrder::CaptureTimeDescending,
        7,
    )
    .await;
    let mut expected_timed = (0..65u32)
        .map(|index| format!("n{index:02}.jpg"))
        .collect::<Vec<_>>();
    expected_timed.reverse();
    let mut expected = expected_timed;
    expected.extend(["d.jpg".to_owned(), "z.jpg".to_owned()]);
    assert_eq!(descending.len(), 67);
    assert_eq!(
        ordering_locations(&application, &descending).await,
        expected
    );
    let unique = descending.iter().collect::<std::collections::BTreeSet<_>>();
    assert_eq!(unique.len(), descending.len(), "windows repeated a Photo");

    // The same paged traversal in ascending order is the exact inverse of the
    // time partition, proving both directions page one global order.
    let ascending = browse_ids_in_pages(
        &application,
        BrowseSourceRequest::Library,
        BrowseViewOrder::CaptureTimeAscending,
        7,
    )
    .await;
    let ascending_timed = &ascending[..65];
    let mut reversed_timed = ascending_timed.to_vec();
    reversed_timed.reverse();
    assert_eq!(reversed_timed, descending[..65].to_vec());
    assert_eq!(
        ordering_locations(&application, &ascending[65..]).await,
        vec!["d.jpg", "z.jpg"]
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn overview_and_browse_windows_remain_bounded_for_four_thousand_photos() {
    // The assertions below are about bounded responses, not about this
    // magnitude: the Overview body bound and the Browse window size hold at
    // any Library size, and no scan or Browse path changes batch size between
    // four thousand and forty thousand Photos.
    let (base, config) = prepare_fixture();
    for directory in ["a", "b"] {
        fs::create_dir(base.join("originals").join(directory)).unwrap();
        for index in 0..2_000 {
            fs::write(
                base.join("originals")
                    .join(directory)
                    .join(format!("{index:05}.jpg")),
                b"not-a-decodable-jpeg",
            )
            .unwrap();
        }
    }
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    application.rescan().await.unwrap();
    let router = authorized_router(Arc::clone(&application), config.web_root());

    let overview_response = send(
        &router,
        authenticated_request()
            .uri("https://camera.local/api/overview")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(overview_response.status(), StatusCode::OK);
    let overview_bytes = axum::body::to_bytes(overview_response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let overview: serde_json::Value = serde_json::from_slice(&overview_bytes).unwrap();
    assert_eq!(overview["photoCount"], 4_000);
    assert!(overview_bytes.len() < 20_000);

    let opened = post_json(
        &router,
        "/api/browse",
        serde_json::json!({"source":"library"}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(opened.status(), StatusCode::OK);
    let opened: serde_json::Value = response_json(opened).await;
    let token = opened["token"].as_str().unwrap();
    let window = send(
        &router,
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/browse/{}?start=3940&limit=60",
                token
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(window.status(), StatusCode::OK);
    let window: serde_json::Value = response_json(window).await;
    assert_eq!(window["start"], 3_940);
    assert_eq!(window["total"], 4_000);
    assert_eq!(window["photos"].as_array().unwrap().len(), 60);

    let oversized = send(
        &router,
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/browse/{}?start=0&limit=61",
                token
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(oversized.status(), StatusCode::BAD_REQUEST);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn album_browse_open_resolves_saved_position_without_members_response() {
    let (base, config) = prepare_fixture();
    for name in ["a.jpg", "b.jpg", "c.jpg"] {
        jpeg_fixture(&config.library_root.join(name), 8, 4, [32, 64, 192]);
    }
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    application.rescan().await.unwrap();
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    application
        .mutate_album(slipstream_core::AlbumMutation::Create {
            name: "Picks".to_owned(),
        })
        .await
        .unwrap();
    let album_id = application
        .albums()
        .await
        .unwrap()
        .albums
        .into_iter()
        .find(|album| album.name == "Picks")
        .unwrap()
        .id;
    application
        .mutate_album(slipstream_core::AlbumMutation::AddMembers {
            album_id: album_id.clone(),
            photo_ids: ids.clone(),
        })
        .await
        .unwrap();
    application
        .mutate_album(slipstream_core::AlbumMutation::SetProgress {
            album_id: album_id.clone(),
            photo_id: ids[1].clone(),
        })
        .await
        .unwrap();
    let opened = application
        .browse_open(
            BrowseSourceRequest::Album(album_id),
            BrowseViewOrder::AlbumOrder,
            BrowseSelectionFilter::All,
            None,
        )
        .await
        .unwrap();
    assert_eq!(opened.total, 3);
    assert_eq!(opened.position, 1);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn file_location_windows_derive_bounded_folders_from_one_publication() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    jpeg_fixture(&root.join("b.JPG"), 8, 4, [32, 64, 192]);
    fs::write(root.join("a.ARW"), b"raw-bytes-a").unwrap();
    jpeg_fixture(&root.join("a.JPG"), 8, 4, [64, 32, 192]);
    fs::create_dir_all(root.join("shoot/sub")).unwrap();
    jpeg_fixture(&root.join("shoot/c.JPG"), 8, 4, [1, 2, 3]);
    fs::write(root.join("shoot/d.ARW"), b"raw-bytes-d").unwrap();
    jpeg_fixture(&root.join("shoot/d.JPG"), 8, 4, [4, 5, 6]);
    jpeg_fixture(&root.join("shoot/sub/e.JPG"), 8, 4, [7, 8, 9]);
    fs::create_dir_all(root.join("shoot/sub/deep")).unwrap();
    jpeg_fixture(&root.join("shoot/sub/deep/deep.JPG"), 8, 4, [2, 4, 6]);
    fs::create_dir_all(root.join("a")).unwrap();
    jpeg_fixture(&root.join("a/f.JPG"), 8, 4, [9, 8, 7]);
    fs::create_dir_all(root.join("ab")).unwrap();
    jpeg_fixture(&root.join("ab/g.JPG"), 8, 4, [6, 5, 4]);
    fs::create_dir_all(root.join("\u{76f8}\u{518c}")).unwrap();
    jpeg_fixture(&root.join("\u{76f8}\u{518c}/h.JPG"), 8, 4, [3, 2, 1]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    application.rescan().await.unwrap();
    wait_for_scan_settled(&application).await;

    // The first window binds to the current publication without supplying one.
    let first = application.file_locations(None, "", 0, 60).await.unwrap();
    assert_eq!(first.parent, "");
    assert_eq!(first.total, 4);
    assert_eq!(first.children.len(), 4);
    let names: Vec<&str> = first
        .children
        .iter()
        .map(|child| child.name.as_str())
        .collect();
    assert_eq!(names, vec!["a", "ab", "shoot", "\u{76f8}\u{518c}"]);
    let by_location = |location: &str| {
        first
            .children
            .iter()
            .find(|child| child.location == location)
            .unwrap()
    };
    // Folder counts are recursive and count independent Photos, through every
    // ancestor level: `shoot` aggregates `shoot/sub` and `shoot/sub/deep`.
    assert_eq!(by_location("a").photo_count, 1);
    assert!(!by_location("a").has_descendant_folders);
    assert_eq!(by_location("ab").photo_count, 1);
    assert_eq!(by_location("shoot").photo_count, 5);
    assert!(by_location("shoot").has_descendant_folders);
    assert_eq!(by_location("\u{76f8}\u{518c}").photo_count, 1);
    let publication = first.publication.clone();
    assert!(!publication.is_empty());

    // A retained window with the same publication stays coherent.
    let shoot = application
        .file_locations(Some(&publication), "shoot", 0, 60)
        .await
        .unwrap();
    assert_eq!(shoot.total, 1);
    assert_eq!(shoot.children[0].location, "shoot/sub");
    // The intermediate chain aggregates upward: `shoot/sub` counts its own
    // Photo and the deeper Folder's.
    assert_eq!(shoot.children[0].photo_count, 2);
    // Files are not Folders at any level: `shoot/sub` holds one Photo and one
    // Folder, and the deepest Folder counts only its own Photo.
    let sub = application
        .file_locations(Some(&publication), "shoot/sub", 0, 60)
        .await
        .unwrap();
    assert_eq!(sub.children.len(), 1);
    assert_eq!(sub.children[0].location, "shoot/sub/deep");
    assert_eq!(sub.children[0].photo_count, 1);

    // Window bounds are enforced.
    assert!(matches!(
        application
            .file_locations(Some(&publication), "", 0, 0)
            .await,
        Err(ServerError::FileLocationWindow)
    ));
    assert!(matches!(
        application
            .file_locations(Some(&publication), "", 0, MAXIMUM_FILE_LOCATION_WINDOW + 1)
            .await,
        Err(ServerError::FileLocationWindow)
    ));
    // Malformed and unknown parents are rejected without fallback.
    for malformed in ["/abs", "a/../b", "a//b", "a/.", "\0"] {
        assert!(matches!(
            application
                .file_locations(Some(&publication), malformed, 0, 60)
                .await,
            Err(ServerError::FolderInvalid)
        ));
    }
    assert!(matches!(
        application
            .file_locations(Some(&publication), "missing", 0, 60)
            .await,
        Err(ServerError::FolderNotFound)
    ));

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn file_location_queries_decode_spaces_and_report_exact_expiry() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    fs::create_dir_all(root.join("My Photos")).unwrap();
    jpeg_fixture(&root.join("My Photos/one.JPG"), 8, 4, [1, 2, 3]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    application.rescan().await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let publication = application
        .file_locations(None, "", 0, 60)
        .await
        .unwrap()
        .publication;

    // `+` decodes as space in query values, so the spaced Folder opens.
    let response = tower::ServiceExt::oneshot(
        router.clone(),
        authenticated_request()
            .method("GET")
            .uri(format!(
                "https://camera.local/api/file-locations?publication={publication}&parent=My+Photos&start=0&limit=60"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["total"], 0);

    // A superseded publication reports the exact expiry contract.
    let response = tower::ServiceExt::oneshot(
        router.clone(),
        authenticated_request()
            .method("GET")
            .uri("https://camera.local/api/file-locations?publication=0000000000000000&start=0&limit=60")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["error"], "File Locations expired");
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn folder_sources_filter_ancestry_and_expire_with_publication() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    jpeg_fixture(&root.join("b.JPG"), 8, 4, [32, 64, 192]);
    fs::write(root.join("a.ARW"), b"raw-bytes-a").unwrap();
    jpeg_fixture(&root.join("a.JPG"), 8, 4, [64, 32, 192]);
    fs::create_dir_all(root.join("a")).unwrap();
    jpeg_fixture(&root.join("a/f.JPG"), 8, 4, [9, 8, 7]);
    fs::create_dir_all(root.join("ab")).unwrap();
    jpeg_fixture(&root.join("ab/g.JPG"), 8, 4, [6, 5, 4]);
    fs::create_dir_all(root.join("shoot/sub")).unwrap();
    jpeg_fixture(&root.join("shoot/c.JPG"), 8, 4, [1, 2, 3]);
    jpeg_fixture(&root.join("shoot/sub/e.JPG"), 8, 4, [7, 8, 9]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    application.rescan().await.unwrap();
    wait_for_scan_settled(&application).await;
    let publication = application
        .file_locations(None, "", 0, 60)
        .await
        .unwrap()
        .publication;

    // Persisted Photo IDs are opaque allocations; read the Published snapshot
    // through the in-crate boundary to build path expectations.
    let ids_by_path = |application: &Application| {
        let guard = application.shared.snapshot.read().unwrap();
        let published = guard.as_ref().unwrap();
        let mut map = std::collections::HashMap::new();
        for photo in &published.snapshot.photos {
            let position = published.originals_by_id[&photo.original_id];
            map.insert(
                published.snapshot.originals[position]
                    .relative_path
                    .as_str()
                    .to_owned(),
                photo.id.clone(),
            );
        }
        map
    };
    let ids = ids_by_path(&application);
    let photo_a = ids["a.ARW"].clone();
    let photo_f = ids["a/f.JPG"].clone();
    let photo_c = ids["shoot/c.JPG"].clone();
    let photo_e = ids["shoot/sub/e.JPG"].clone();

    // Component-aware ancestry: folder "a" never includes sibling "ab".
    let folder_a = browse_photo_ids(
        &application,
        BrowseSourceRequest::Folder {
            location: "a".to_owned(),
            publication: publication.clone(),
        },
    )
    .await;
    assert_eq!(folder_a, vec![photo_f.clone()]);
    // Recursive subtree membership in Capture Time order.
    let shoot = browse_photo_ids(
        &application,
        BrowseSourceRequest::Folder {
            location: "shoot".to_owned(),
            publication: publication.clone(),
        },
    )
    .await;
    assert_eq!(shoot.len(), 2);
    assert!(shoot.contains(&photo_c));
    assert!(shoot.contains(&photo_e));
    // The root Folder location covers the whole Published Library.
    let root_source = browse_photo_ids(
        &application,
        BrowseSourceRequest::Folder {
            location: String::new(),
            publication: publication.clone(),
        },
    )
    .await;
    assert_eq!(root_source.len(), 7);
    assert!(root_source.contains(&photo_a));
    assert!(root_source.contains(&ids["b.JPG"]));

    // A rescan that removes one Original supersedes the publication: every
    // retained File Location value and Folder-source open fails as expired.
    fs::remove_file(root.join("shoot/sub/e.JPG")).unwrap();
    application.rescan().await.unwrap();
    wait_for_scan_settled(&application).await;
    assert!(matches!(
        application
            .file_locations(Some(&publication), "", 0, 60)
            .await,
        Err(ServerError::FileLocationsExpired)
    ));
    assert!(matches!(
        application
            .browse_open(
                BrowseSourceRequest::Folder {
                    location: "a".to_owned(),
                    publication: publication.clone(),
                },
                BrowseViewOrder::CaptureTimeAscending,
                BrowseSelectionFilter::All,
                None,
            )
            .await,
        Err(ServerError::FileLocationsExpired)
    ));
    // A fresh window binds to the new publication and still projects the
    // remembered unavailable Photo at its last known Location.
    let fresh = application.file_locations(None, "", 0, 60).await.unwrap();
    assert_ne!(fresh.publication, publication);
    let shoot = application
        .file_locations(Some(&fresh.publication), "shoot", 0, 60)
        .await
        .unwrap();
    // The child window keeps projecting the remembered unavailable Photo.
    assert_eq!(shoot.children[0].photo_count, 1);
    let root_counts = application
        .file_locations(Some(&fresh.publication), "", 0, 60)
        .await
        .unwrap();
    let shoot_child = root_counts
        .children
        .iter()
        .find(|child| child.location == "shoot")
        .unwrap();
    assert_eq!(shoot_child.photo_count, 2);
    let reopened = browse_photo_ids(
        &application,
        BrowseSourceRequest::Folder {
            location: "shoot".to_owned(),
            publication: fresh.publication.clone(),
        },
    )
    .await;
    assert_eq!(
        reopened.len(),
        2,
        "remembered unavailable Photo is retained"
    );
    assert!(reopened.contains(&photo_c));
    assert!(reopened.contains(&photo_e));

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn folder_album_add_uses_recursive_publication_and_is_idempotent() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    fs::create_dir_all(root.join("shoot/nested")).unwrap();
    jpeg_fixture(&root.join("shoot/first.JPG"), 8, 4, [1, 2, 3]);
    jpeg_fixture(&root.join("shoot/nested/second.JPG"), 8, 4, [4, 5, 6]);
    jpeg_fixture(&root.join("outside.JPG"), 8, 4, [7, 8, 9]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    application.rescan().await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let publication = application
        .file_locations(None, "", 0, 60)
        .await
        .unwrap()
        .publication;
    let folder_ids = browse_photo_ids(
        &application,
        BrowseSourceRequest::Folder {
            location: "shoot".to_owned(),
            publication: publication.clone(),
        },
    )
    .await;
    assert_eq!(folder_ids.len(), 2);

    let created: serde_json::Value = response_json(
        post_json(
            &router,
            "/api/albums",
            serde_json::json!({"name": "Folder Picks"}),
            None,
        )
        .await,
    )
    .await;
    let album_id = created["albums"][0]["id"].as_str().unwrap().to_owned();
    let first: serde_json::Value = response_json(
        post_json(
            &router,
            &format!("/api/albums/{album_id}/folder-members"),
            serde_json::json!({
                "folderPath": "shoot",
                "publication": publication.clone(),
            }),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(first["albumId"], album_id);
    assert_eq!(first["folderPath"], "shoot");
    assert_eq!(first["matchedCount"], 2);
    assert_eq!(first["addedCount"], 2);
    assert_eq!(first["alreadyMemberCount"], 0);
    assert_eq!(
        browse_photo_ids(&application, BrowseSourceRequest::Album(album_id.clone()),).await,
        folder_ids
    );

    let repeated: serde_json::Value = response_json(
        post_json(
            &router,
            &format!("/api/albums/{album_id}/folder-members"),
            serde_json::json!({
                "folderPath": "shoot",
                "publication": publication.clone(),
            }),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(repeated["matchedCount"], 2);
    assert_eq!(repeated["addedCount"], 0);
    assert_eq!(repeated["alreadyMemberCount"], 2);
    let stale = post_json(
        &router,
        &format!("/api/albums/{album_id}/folder-members"),
        serde_json::json!({
            "folderPath": "shoot",
            "publication": "0000000000000000",
        }),
        None,
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(
        browse_photo_ids(&application, BrowseSourceRequest::Album(album_id)).await,
        folder_ids
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn empty_album_opens_lists_and_accepts_first_member() {
    let (base, config) = prepare_fixture();
    jpeg_fixture(&config.library_root.join("a.jpg"), 8, 4, [32, 64, 192]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    application
        .mutate_album(slipstream_core::AlbumMutation::Create {
            name: "Empty".to_owned(),
        })
        .await
        .unwrap();
    let album_id = application
        .albums()
        .await
        .unwrap()
        .albums
        .into_iter()
        .find(|album| album.name == "Empty")
        .unwrap()
        .id;
    let summaries = application.albums().await.unwrap().albums;
    let summary = summaries.iter().find(|album| album.id == album_id).unwrap();
    assert_eq!(summary.photo_count, 0);
    assert!(!summary.has_saved_position);
    let opened = application
        .browse_open(
            BrowseSourceRequest::Album(album_id.clone()),
            BrowseViewOrder::AlbumOrder,
            BrowseSelectionFilter::All,
            None,
        )
        .await
        .unwrap();
    assert_eq!(opened.total, 0);
    assert_eq!(opened.position, 0);
    application
        .mutate_album(slipstream_core::AlbumMutation::AddMembers {
            album_id,
            photo_ids: ids.clone(),
        })
        .await
        .unwrap();
    let summaries = application.albums().await.unwrap().albums;
    let summary = summaries
        .iter()
        .find(|album| album.name == "Empty")
        .unwrap();
    assert_eq!(summary.photo_count, ids.len());
    let opened = application
        .browse_open(
            BrowseSourceRequest::Album(summary.id.clone()),
            BrowseViewOrder::AlbumOrder,
            BrowseSelectionFilter::All,
            None,
        )
        .await
        .unwrap();
    assert_eq!(opened.total, ids.len());
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn browse_tokens_are_process_unique_and_expiry_is_enforced() {
    let (base_a, config_a) = prepare_fixture();
    let (base_b, config_b) = prepare_fixture();
    let application_a = Application::open(&config_a).await.unwrap();
    let application_b = Application::open(&config_b).await.unwrap();
    wait_for_scan_settled(&application_a).await;
    wait_for_scan_settled(&application_b).await;
    let opened_a = application_a
        .browse_open(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::All,
            None,
        )
        .await
        .unwrap();
    let opened_b = application_b
        .browse_open(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::All,
            None,
        )
        .await
        .unwrap();
    assert_ne!(opened_a.token, opened_b.token);
    assert_eq!(opened_a.token.len(), 49);
    assert!(
        opened_a
            .token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );
    assert!(matches!(
        application_b.browse_window(&opened_a.token, 0, 10).await,
        Err(ServerError::BrowseNotFound)
    ));
    {
        let mut snapshots = application_a
            .retained_queries
            .lock()
            .expect("retained queries poisoned");
        let snapshot = snapshots.entries.get_mut(&opened_a.token).unwrap();
        snapshot.last_used -= BROWSE_SNAPSHOT_IDLE + Duration::from_secs(1);
    }
    assert!(matches!(
        application_a.browse_window(&opened_a.token, 0, 10).await,
        Err(ServerError::BrowseNotFound)
    ));
    assert!(
        !application_a
            .retained_queries
            .lock()
            .unwrap()
            .entries
            .contains_key(&opened_a.token)
    );
    application_a.shutdown().await.unwrap();
    application_b.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base_a);
    let _ = fs::remove_dir_all(base_b);
}

#[tokio::test]
async fn browse_delete_releases_the_snapshot() {
    let (base, config) = prepare_fixture();
    jpeg_fixture(&config.library_root.join("a.jpg"), 8, 4, [32, 64, 192]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let opened = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source":"library"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let token = opened["token"].as_str().unwrap().to_owned();
    async fn window(router: &Router, token: &str) -> Response<Body> {
        send(
            router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/browse/{token}?start=0&limit=10"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
    }
    assert_eq!(window(&router, &token).await.status(), StatusCode::OK);
    let foreign_origin = send(
        &router,
        authenticated_request()
            .method("DELETE")
            .uri(format!("https://camera.local/api/browse/{token}"))
            .header(header::ORIGIN, "http://elsewhere.example")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(foreign_origin.status(), StatusCode::FORBIDDEN);
    assert_eq!(window(&router, &token).await.status(), StatusCode::OK);
    let deleted = send(
        &router,
        Request::builder()
            .method("DELETE")
            .uri(format!("/api/browse/{token}"))
            .header(
                "Authorization",
                format!("Bearer {}", crate::access::TEST_TOKEN),
            )
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        window(&router, &token).await.status(),
        StatusCode::NOT_FOUND
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn browse_open_honors_preferred_photo_and_rejects_invalid_ids() {
    let (base, config) = prepare_fixture();
    for name in ["a.jpg", "b.jpg", "c.jpg"] {
        jpeg_fixture(&config.library_root.join(name), 8, 4, [32, 64, 192]);
    }
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    application.rescan().await.unwrap();
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let library = application
        .browse_open(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::All,
            Some(&ids[2]),
        )
        .await
        .unwrap();
    assert_eq!(library.position, 2);
    let fallback = application
        .browse_open(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::All,
            None,
        )
        .await
        .unwrap();
    assert_eq!(fallback.position, 0);
    application
        .mutate_album(slipstream_core::AlbumMutation::Create {
            name: "Picks".to_owned(),
        })
        .await
        .unwrap();
    let album_id = application
        .albums()
        .await
        .unwrap()
        .albums
        .into_iter()
        .find(|album| album.name == "Picks")
        .unwrap()
        .id;
    application
        .mutate_album(slipstream_core::AlbumMutation::AddMembers {
            album_id: album_id.clone(),
            photo_ids: ids.clone(),
        })
        .await
        .unwrap();
    application
        .mutate_album(slipstream_core::AlbumMutation::SetProgress {
            album_id: album_id.clone(),
            photo_id: ids[0].clone(),
        })
        .await
        .unwrap();
    let preferred = application
        .browse_open(
            BrowseSourceRequest::Album(album_id),
            BrowseViewOrder::AlbumOrder,
            BrowseSelectionFilter::All,
            Some(&ids[2]),
        )
        .await
        .unwrap();
    assert_eq!(preferred.position, 2);
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let invalid = post_json(
        &router,
        "/api/browse",
        serde_json::json!({"source":"library","photoId":"NOT-A-ID"}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn browse_position_resolves_identity_within_one_snapshot() {
    let (base, config) = prepare_fixture();
    for name in ["a.jpg", "b.jpg", "c.jpg"] {
        jpeg_fixture(&config.library_root.join(name), 8, 4, [32, 64, 192]);
    }
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let opened = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source":"library"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let token = opened["token"].as_str().unwrap().to_owned();
    let get_position = |photo_id: String| {
        let uri = format!("https://camera.local/api/browse/{token}/position?photoId={photo_id}");
        let router = &router;
        async move {
            response_json(
                send(
                    router,
                    authenticated_request()
                        .uri(uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await,
            )
            .await
        }
    };
    assert_eq!(get_position(ids[1].clone()).await["position"], 1);
    assert!(get_position("f".repeat(64)).await["position"].is_null());

    let invalid = send(
        &router,
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/browse/{token}/position?photoId=bad"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

    application.browse_close(&token);
    let expired = send(
        &router,
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/browse/{token}/position?photoId={}",
                ids[0]
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(expired.status(), StatusCode::NOT_FOUND);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// One bounded batch Selection State write reports exactly one outcome per
/// requested Photo, presents the confirmed states in the open Browse Snapshot,
/// and moves the source's counts when the source is reopened.
#[tokio::test]
async fn batch_photo_state_applies_to_every_requested_photo() {
    let (base, config) = prepare_fixture();
    for name in ["a.jpg", "b.jpg", "c.jpg"] {
        jpeg_fixture(&config.library_root.join(name), 8, 4, [32, 64, 192]);
    }
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    assert_eq!(ids.len(), 3);
    let opened = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source":"library"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let token = opened["token"].as_str().unwrap().to_owned();
    assert_eq!(opened["selectionCounts"]["undecided"], 3);

    let applied = response_json(
        post_json(
            &router,
            "/api/photos/state",
            serde_json::json!({
                "photos": ids
                    .iter()
                    .map(|photo_id| serde_json::json!({"photoId": photo_id, "expectedCurrent": "undecided"}))
                    .collect::<Vec<_>>(),
                "selectionState": "rejected"
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(applied["changedElsewhere"].as_array().unwrap().len(), 0);
    assert_eq!(applied["missing"].as_array().unwrap().len(), 0);
    let entries = applied["applied"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    for (entry, id) in entries.iter().zip(&ids) {
        assert_eq!(entry["photoId"], *id);
        assert_eq!(entry["priorValue"], "undecided");
    }

    // The open Snapshot presents the confirmed states without a reload.
    let window = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/browse/{token}?start=0&limit=10"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert!(
        window["photos"]
            .as_array()
            .unwrap()
            .iter()
            .all(|photo| photo["selectionState"] == "rejected")
    );

    // A later writer changes one Photo after the browser's confirmed state.
    // The next batch reports that identity and leaves its newer value intact.
    let external = response_json(
        post_json(
            &router,
            &format!("/api/photos/{}/state", ids[0]),
            serde_json::json!({
                "field": "selectionState",
                "value": "selected",
                "expectedCurrent": "rejected"
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(external["kind"], "applied");
    // A single-Photo write updates only that Photo in the open Snapshot: its
    // siblings and the window cardinality stay untouched.
    let window = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/browse/{token}?start=0&limit=10"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    let photos = window["photos"].as_array().unwrap();
    assert_eq!(photos.len(), ids.len());
    for photo in photos {
        let expected = if photo["id"] == ids[0] {
            "selected"
        } else {
            "rejected"
        };
        assert_eq!(photo["selectionState"], expected, "{}", photo["id"]);
    }
    let changed = response_json(
        post_json(
            &router,
            "/api/photos/state",
            serde_json::json!({
                "photos": ids
                    .iter()
                    .map(|photo_id| serde_json::json!({"photoId": photo_id, "expectedCurrent": "rejected"}))
                    .collect::<Vec<_>>(),
                "selectionState": "selected"
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(
        changed["changedElsewhere"],
        serde_json::json!([{"photoId": ids[0], "currentValue": "selected"}])
    );
    assert_eq!(changed["applied"].as_array().unwrap().len(), 2);
    assert!(changed["missing"].as_array().unwrap().is_empty());
    let changed_window = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/browse/{token}?start=0&limit=10"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert!(
        changed_window["photos"]
            .as_array()
            .unwrap()
            .iter()
            .all(|photo| photo["selectionState"] == "selected")
    );

    // Repeating the now-confirmed state is idempotent: every Photo reports the
    // state it already holds as its prior value.
    let repeated = response_json(
        post_json(
            &router,
            "/api/photos/state",
            serde_json::json!({
                "photos": ids
                    .iter()
                    .map(|photo_id| serde_json::json!({"photoId": photo_id, "expectedCurrent": "selected"}))
                    .collect::<Vec<_>>(),
                "selectionState": "selected"
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert!(
        repeated["applied"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["priorValue"] == "selected")
    );
    let reopened = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source":"library"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(reopened["selectionCounts"]["selected"], 3);
    assert_eq!(reopened["selectionCounts"]["rejected"], 0);
    assert_eq!(reopened["selectionCounts"]["undecided"], 0);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A Photo that is no longer in the current Library is reported as missing
/// and never rolls back the confirmed Photos of the same batch.
#[tokio::test]
async fn batch_photo_state_reports_a_missing_photo_without_blocking_the_rest() {
    let (base, config) = prepare_fixture();
    for name in ["a.jpg", "b.jpg"] {
        jpeg_fixture(&config.library_root.join(name), 8, 4, [32, 64, 192]);
    }
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let missing = "00000000-0000-4000-8000-000000000000";

    let result = response_json(
        post_json(
            &router,
            "/api/photos/state",
            serde_json::json!({
                "photos": [
                    {"photoId": ids[0], "expectedCurrent": "undecided"},
                    {"photoId": missing, "expectedCurrent": "undecided"},
                    {"photoId": ids[1], "expectedCurrent": "undecided"}
                ],
                "selectionState": "selected"
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let applied = result["applied"].as_array().unwrap();
    assert_eq!(applied.len(), 2);
    assert_eq!(applied[0]["photoId"], ids[0]);
    assert_eq!(applied[1]["photoId"], ids[1]);
    let missing_outcomes = result["missing"].as_array().unwrap();
    assert_eq!(missing_outcomes.len(), 1);
    assert_eq!(missing_outcomes[0]["photoId"], missing);
    assert_eq!(
        missing_outcomes[0],
        serde_json::json!({ "photoId": missing })
    );
    assert!(result["changedElsewhere"].as_array().unwrap().is_empty());
    // The Library no longer holds a state for that Photo, so the missing
    // outcome names it and nothing else.

    // The confirmed Photos persisted; the unknown Photo changed nothing.
    let reopened = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source":"library"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(reopened["selectionCounts"]["selected"], 2);
    assert_eq!(reopened["selectionCounts"]["undecided"], 0);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// The batch bound, identifier shape, uniqueness, and value vocabulary are
/// rejected before any write.
#[tokio::test]
async fn batch_photo_state_rejects_over_limit_duplicate_and_unknown_requests() {
    let (base, config) = prepare_fixture();
    jpeg_fixture(&config.library_root.join("a.jpg"), 8, 4, [32, 64, 192]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;

    let over_limit: Vec<String> = (0..=slipstream_core::PHOTO_STATE_BATCH_MAX)
        .map(|index| format!("00000000-0000-4000-8000-{index:012}"))
        .collect();
    let valid_items = || {
        ids.iter()
            .map(
                |photo_id| serde_json::json!({"photoId": photo_id, "expectedCurrent": "undecided"}),
            )
            .collect::<Vec<_>>()
    };
    for body in [
        serde_json::json!({
            "photos": over_limit
                .into_iter()
                .map(|photo_id| serde_json::json!({"photoId": photo_id, "expectedCurrent": "undecided"}))
                .collect::<Vec<_>>(),
            "selectionState": "selected"
        }),
        serde_json::json!({
            "photos": [
                {"photoId": ids[0], "expectedCurrent": "undecided"},
                {"photoId": ids[0], "expectedCurrent": "undecided"}
            ],
            "selectionState": "selected"
        }),
        serde_json::json!({"photos": [], "selectionState": "selected"}),
        serde_json::json!({"photos": valid_items(), "selectionState": "undecided"}),
        serde_json::json!({"photos": valid_items(), "selectionState": "maybe"}),
        serde_json::json!({"photos": valid_items(), "rating": 3}),
        serde_json::json!({
            "photos": [{"photoId": ids[0]}],
            "selectionState": "selected"
        }),
        serde_json::json!({
            "photos": [{
                "photoId": ids[0],
                "expectedCurrent": "undecided",
                "unexpected": true
            }],
            "selectionState": "selected"
        }),
        serde_json::json!({
            "photos": [{"photoId": "NOT-A-PHOTO-ID", "expectedCurrent": "undecided"}],
            "selectionState": "selected"
        }),
    ] {
        assert_eq!(
            post_json(
                &router,
                "/api/photos/state",
                body,
                Some("https://camera.local")
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }

    // No rejected request wrote anything.
    let reopened = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source":"library"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(reopened["selectionCounts"]["undecided"], 1);
    assert_eq!(reopened["selectionCounts"]["selected"], 0);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// Commits one Selection State through the HTTP mutation route so a fixture
/// holds real persisted decisions rather than fabricated facts.
async fn decide_selection(router: &Router, photo_id: &str, value: &str) -> StatusCode {
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
        ("a.jpg", "selected"),
        ("c.jpg", "selected"),
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
        (BrowseSelectionFilter::Selected, vec!["a", "c"]),
        (BrowseSelectionFilter::Rejected, vec!["b"]),
        (BrowseSelectionFilter::Undecided, vec!["d", "e"]),
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
                selected: 2,
                rejected: 1,
                undecided: 2,
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
            BrowseSelectionFilter::Selected,
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
            BrowseSelectionFilter::Selected,
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
        serde_json::json!({"selected": 2, "rejected": 1, "undecided": 2})
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
        decide_selection(&router, &by_location["shoot/p1.jpg"], "selected").await,
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
        BrowseSelectionFilter::Selected,
    )
    .await;
    assert_eq!(opened.total, 1);
    assert_eq!(photos[0].id, by_location["shoot/p1.jpg"]);
    assert_eq!(
        opened.selection_counts,
        SelectionCountsWire {
            selected: 1,
            rejected: 1,
            undecided: 1,
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
        decide_selection(&router, &by_location["outside.jpg"], "selected").await,
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
        BrowseSelectionFilter::Selected,
    )
    .await;
    assert_eq!(folder_opened.total, 1);
    assert_eq!(folder_photos[0].id, by_location["shoot/p1.jpg"]);
    // The Folder's counts stay scoped to the Folder: `outside.jpg` is selected
    // in the Library but is not part of this source.
    assert_eq!(folder_opened.selection_counts.selected, 1);
    assert_eq!(folder_opened.selection_counts.rejected, 1);
    assert_eq!(folder_opened.selection_counts.undecided, 1);

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
        decide_selection(&router, &b, "selected").await,
        StatusCode::OK
    );
    assert_eq!(
        decide_selection(&router, &c, "selected").await,
        StatusCode::OK
    );

    // A view change reopens with the browser's current Photo as the anchor.
    // The anchor no longer matches the filter, so the view starts at its first
    // Photo (`b`) instead of resuming at the saved Photo (`c`).
    let changed = application
        .browse_open(
            BrowseSourceRequest::Album(album.clone()),
            BrowseViewOrder::AlbumOrder,
            BrowseSelectionFilter::Selected,
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
            BrowseSelectionFilter::Selected,
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
            BrowseSelectionFilter::Selected,
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
        decide_selection(&router, &by_location["a.jpg"], "selected").await,
        StatusCode::OK
    );
    let opened = application
        .browse_open(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::Selected,
            None,
        )
        .await
        .unwrap();
    assert_eq!(opened.total, 1);
    assert_eq!(opened.selection_counts.selected, 1);

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
            BrowseSelectionFilter::Selected,
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
            selected: 0,
            rejected: 1,
            undecided: 1,
        }
    );
    application.browse_close(&opened.token);
    application.browse_close(&reopened.token);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn browse_windows_report_the_ordering_original_filename() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    fs::create_dir_all(root.join("shoot")).unwrap();
    fs::write(root.join("shoot/IMG_4521.ARW"), b"raw-bytes").unwrap();
    jpeg_fixture(&root.join("shoot/IMG_4521.JPG"), 8, 4, [64, 32, 192]);
    jpeg_fixture(&root.join("shoot/IMG_4522.JPG"), 8, 4, [32, 64, 192]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());

    let opened = post_json(
        &router,
        "/api/browse",
        serde_json::json!({"source":"library"}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(opened.status(), StatusCode::OK);
    let opened: serde_json::Value = response_json(opened).await;
    let token = opened["token"].as_str().unwrap();
    let window = send(
        &router,
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/browse/{}?start=0&limit=60",
                token
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(window.status(), StatusCode::OK);
    let window: serde_json::Value = response_json(window).await;
    let photos = window["photos"].as_array().unwrap();
    assert_eq!(photos.len(), 3);

    // Each Photo carries its own Original's filename. Both are basenames, so
    // the relative Location never crosses the boundary.
    let by_name = |name: &str| {
        photos
            .iter()
            .find(|photo| photo["originalFilename"] == name)
            .unwrap_or_else(|| panic!("window is missing {name}"))
    };
    assert_eq!(by_name("IMG_4521.ARW")["original"]["kind"], "raw");
    assert_eq!(by_name("IMG_4522.JPG")["original"]["kind"], "jpeg");
    assert!(!window.to_string().contains("shoot/"));

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn browse_windows_hydrate_only_current_thumbnail_manifests() {
    let (base, config) = prepare_fixture();
    let original = config.library_root.join("photo.jpg");
    jpeg_fixture(&original, 90, 45, [192, 64, 32]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .into_iter()
        .next()
        .unwrap();

    let before_generation = published_photo_summary(&application, &photo_id).await;
    assert_eq!(before_generation.preview.thumbnail_url, None);

    let thumbnail = application.thumbnail(&photo_id).await.unwrap();
    let thumbnail_url = thumbnail.url.unwrap();
    assert!(thumbnail_url.starts_with("/api/private/derivatives/"));
    assert!(thumbnail_url.contains("/thumbnail/"));
    let hydrated = published_photo_summary(&application, &photo_id).await;
    assert_eq!(
        hydrated.preview.thumbnail_url.as_deref(),
        Some(thumbnail_url.as_str())
    );

    // A source revision invalidates the old manifest for Browse Window
    // hydration even though the old derivative remains on disk.
    jpeg_fixture(&original, 91, 46, [32, 192, 64]);
    application.rescan().await.unwrap();
    let after_revision = published_photo_summary(&application, &photo_id).await;
    assert_eq!(after_revision.preview.thumbnail_url, None);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn thumbnail_requests_keep_review_preview_facts_exact() {
    let (base, config) = prepare_fixture();
    let original = config.library_root.join("photo.jpg");
    jpeg_fixture(&original, 90, 45, [192, 64, 32]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .into_iter()
        .next()
        .unwrap();
    let preview = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{photo_id}/preview"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(preview["state"], "ready");
    assert_eq!(preview["source"], "jpeg-original");
    let review_url = preview["url"].as_str().unwrap().to_owned();
    let review_key = review_url
        .rsplit('/')
        .next()
        .unwrap()
        .trim_end_matches(".jpg");
    async fn facts(
        application: &Application,
        photo_id: &str,
    ) -> (&'static str, Option<&'static str>, Option<u32>, Option<u32>) {
        let photo = published_photo_summary(application, photo_id).await;
        (
            photo.preview.state,
            photo.preview.source,
            photo.preview.width,
            photo.preview.height,
        )
    }
    let established = facts(&application, &photo_id).await;
    assert_eq!(
        established,
        ("ready", Some("jpeg-original"), Some(90), Some(45))
    );

    let thumbnail = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{photo_id}/thumbnail"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(thumbnail["state"], "ready");
    let thumbnail_url = thumbnail["url"].as_str().unwrap();
    assert!(thumbnail_url.contains("/thumbnail/"));
    let thumbnail_key = thumbnail_url
        .rsplit('/')
        .next()
        .unwrap()
        .trim_end_matches(".jpg");
    assert_ne!(thumbnail_key, review_key);
    assert_eq!(facts(&application, &photo_id).await, established);

    // The persisted facts and both derivative identities survive reopen.
    application.shutdown().await.unwrap();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    assert_eq!(facts(&application, &photo_id).await, established);
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let reopened = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{photo_id}/thumbnail"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(reopened["url"].as_str().unwrap(), thumbnail_url);
    let review = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{photo_id}/preview"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(review["url"].as_str().unwrap(), review_url);
    assert_eq!(facts(&application, &photo_id).await, established);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
