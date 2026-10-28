use super::browse_selection::decide_selection;
use super::*;
use crate::wire::BrowseFilterConditions;

/// Opens one source with combined filter conditions and reads the whole view
/// through bounded windows, exactly as the browser must.
async fn browse_filtered_ids(
    application: &Application,
    source: BrowseSourceRequest,
    selection: BrowseSelectionFilter,
    conditions: &BrowseFilterConditions,
) -> (BrowseOpenResponse, Vec<PhotoSummary>) {
    let order = default_order(&source);
    let opened = application
        .browse_open_filtered(source, order, selection, conditions.clone(), None)
        .await
        .expect("filtered browse open succeeds");
    let mut photos = Vec::new();
    let start = 0;
    let mut cursor = start;
    loop {
        let window = application
            .browse_window(&opened.token, cursor, 60)
            .await
            .expect("filtered browse window succeeds");
        assert_eq!(window.total, opened.total);
        let count = window.photos.len();
        photos.extend(window.photos);
        cursor += count;
        if count == 0 || cursor >= opened.total {
            break;
        }
    }
    assert_eq!(photos.len(), opened.total, "filtered traversal incomplete");
    application.browse_close(&opened.token);
    (opened, photos)
}

/// One seeded Library: five JPEGs with distinct Capture Times, one RAW that
/// shares a basename with a JPEG, and one Original removed from disk so the
/// Photo is remembered but unavailable.
fn seed_filter_fixture() -> (PathBuf, Config) {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    for (name, capture_time) in [
        ("a.jpg", "2026:01:01 09:00:00"),
        ("b.jpg", "2026:01:02 09:00:00"),
        ("c.jpg", "2026:01:03 09:00:00"),
        ("d.jpg", "2026:01:04 09:00:00"),
        ("e.jpg", "2026:01:05 09:00:00"),
    ] {
        jpeg_fixture_with_capture_time(&root.join(name), 8, 4, [32, 64, 192], capture_time);
    }
    // A RAW Photo that shares its basename with a JPEG Photo.
    approved_raw_fixture(&root.join("pair.ARW"));
    jpeg_fixture_with_capture_time(
        &root.join("pair.JPG"),
        8,
        4,
        [192, 64, 32],
        "2026:01:06 09:00:00",
    );
    // One Original with no Capture Time at all.
    jpeg_fixture(&root.join("untimed.jpg"), 8, 4, [12, 24, 36]);
    (base, config)
}

/// Commits one Rating through the HTTP mutation route so the fixture holds a
/// real persisted decision.
async fn decide_rating(router: &Router, photo_id: &str, value: u8) -> StatusCode {
    post_json(
        router,
        &format!("https://camera.local/api/photos/{photo_id}/state"),
        serde_json::json!({"field": "rating", "value": value}),
        Some("https://camera.local"),
    )
    .await
    .status()
}

#[tokio::test]
async fn browse_fact_filters_combine_with_and_and_are_applied_before_pagination() {
    let (base, config) = seed_filter_fixture();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let by_location = photo_ids_by_location(&application, &ids).await;
    for (name, value) in [
        ("a.jpg", 5u8),
        ("b.jpg", 4),
        ("c.jpg", 3),
        ("d.jpg", 2),
        ("e.jpg", 1),
    ] {
        assert_eq!(
            decide_rating(&router, &by_location[name], value).await,
            StatusCode::OK,
            "{name}"
        );
    }

    // A Rating minimum alone selects from the source order.
    let (opened, photos) = browse_filtered_ids(
        &application,
        BrowseSourceRequest::Library,
        BrowseSelectionFilter::All,
        &BrowseFilterConditions {
            rating_minimum: Some(4),
            ..BrowseFilterConditions::default()
        },
    )
    .await;
    assert_eq!(opened.total, 2);
    assert_eq!(photos.len(), 2, "AND-composed view is complete");
    // The count describes the whole unfiltered source, so a filtered view
    // still reads its source progress.
    assert_eq!(
        opened.selection_counts,
        SelectionCountsWire {
            picked: 0,
            rejected: 0,
            unflagged: 8
        }
    );

    // Rating minimum and maximum compose with AND across the same order.
    let (bounded, _) = browse_filtered_ids(
        &application,
        BrowseSourceRequest::Library,
        BrowseSelectionFilter::All,
        &BrowseFilterConditions {
            rating_minimum: Some(2),
            rating_maximum: Some(4),
            ..BrowseFilterConditions::default()
        },
    )
    .await;
    assert_eq!(bounded.total, 3);

    // A Selection State filter and a fact filter combine with AND.
    assert_eq!(
        decide_selection(&router, &by_location["b.jpg"], "picked").await,
        StatusCode::OK
    );
    let (combined, photos) = browse_filtered_ids(
        &application,
        BrowseSourceRequest::Library,
        BrowseSelectionFilter::Picked,
        &BrowseFilterConditions {
            rating_minimum: Some(4),
            ..BrowseFilterConditions::default()
        },
    )
    .await;
    assert_eq!(combined.total, 1);
    assert_eq!(photos[0].id, by_location["b.jpg"]);
    // The per-state counts still describe the complete source.
    assert_eq!(
        combined.selection_counts,
        SelectionCountsWire {
            picked: 1,
            rejected: 0,
            unflagged: 7
        }
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn rating_zero_is_a_real_bound_meaning_no_rating() {
    let (base, config) = seed_filter_fixture();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let by_location = photo_ids_by_location(&application, &ids).await;
    assert_eq!(
        decide_rating(&router, &by_location["a.jpg"], 3).await,
        StatusCode::OK
    );
    assert_eq!(
        decide_rating(&router, &by_location["b.jpg"], 0).await,
        StatusCode::OK
    );

    // No Rating is Rating 0, not a missing value: the bound keeps exactly the
    // Photos whose decision is zero stars.
    let (no_rating, _) = browse_filtered_ids(
        &application,
        BrowseSourceRequest::Library,
        BrowseSelectionFilter::All,
        &BrowseFilterConditions {
            rating_minimum: Some(0),
            ..BrowseFilterConditions::default()
        },
    )
    .await;
    assert_eq!(no_rating.total, 8, "every Photo has at least zero stars");
    let (any_rating, _) = browse_filtered_ids(
        &application,
        BrowseSourceRequest::Library,
        BrowseSelectionFilter::All,
        &BrowseFilterConditions {
            rating_maximum: Some(0),
            ..BrowseFilterConditions::default()
        },
    )
    .await;
    assert_eq!(any_rating.total, 7);
    let (rated, photos) = browse_filtered_ids(
        &application,
        BrowseSourceRequest::Library,
        BrowseSelectionFilter::All,
        &BrowseFilterConditions {
            rating_minimum: Some(1),
            ..BrowseFilterConditions::default()
        },
    )
    .await;
    assert_eq!(rated.total, 1);
    assert_eq!(photos[0].id, by_location["a.jpg"]);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn kind_keeps_a_raw_photo_and_a_same_basename_jpeg_photo_independent() {
    let (base, config) = seed_filter_fixture();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let by_location = photo_ids_by_location(&application, &ids).await;
    assert!(
        by_location.contains_key("pair.ARW"),
        "the RAW and the JPEG are separate Photos"
    );
    assert!(by_location.contains_key("pair.JPG"));

    let (raw, photos) = browse_filtered_ids(
        &application,
        BrowseSourceRequest::Library,
        BrowseSelectionFilter::All,
        &BrowseFilterConditions {
            kind: Some(slipstream_core::OriginalKind::Raw),
            ..BrowseFilterConditions::default()
        },
    )
    .await;
    assert_eq!(raw.total, 1);
    assert_eq!(photos[0].id, by_location["pair.ARW"]);
    assert_eq!(photos[0].original_filename.as_deref(), Some("pair.ARW"));

    // The JPEG of the same basename is not merged into the RAW view, and the
    // RAW is not merged into the JPEG view.
    let (jpeg, photos) = browse_filtered_ids(
        &application,
        BrowseSourceRequest::Library,
        BrowseSelectionFilter::All,
        &BrowseFilterConditions {
            kind: Some(slipstream_core::OriginalKind::Jpeg),
            ..BrowseFilterConditions::default()
        },
    )
    .await;
    assert_eq!(jpeg.total, 7);
    assert!(
        photos
            .iter()
            .all(|photo| photo.id != by_location["pair.ARW"])
    );
    assert!(
        photos
            .iter()
            .any(|photo| photo.id == by_location["pair.JPG"])
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn availability_describes_the_original_file_not_preview_readiness() {
    let (base, config) = seed_filter_fixture();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let by_location = photo_ids_by_location(&application, &ids).await;
    // The RAW's Preview is still pending inspection while its Original is
    // available: an availability filter must not read Preview readiness.
    let (available, photos) = browse_filtered_ids(
        &application,
        BrowseSourceRequest::Library,
        BrowseSelectionFilter::All,
        &BrowseFilterConditions {
            available: Some(true),
            ..BrowseFilterConditions::default()
        },
    )
    .await;
    assert_eq!(available.total, 8);
    assert!(
        photos
            .iter()
            .any(|photo| photo.id == by_location["pair.ARW"])
    );
    assert_eq!(
        photos
            .iter()
            .find(|photo| photo.id == by_location["pair.ARW"])
            .map(|photo| photo.preview.state),
        Some("inspection-pending"),
        "Preview readiness is a separate fact from Original availability"
    );

    // Removing one Original from disk makes that Photo unavailable while it
    // keeps every other fact.
    fs::remove_file(config.library_root.join("a.jpg")).unwrap();
    application.rescan().await.unwrap();
    wait_for_scan_settled(&application).await;
    let (unavailable, photos) = browse_filtered_ids(
        &application,
        BrowseSourceRequest::Library,
        BrowseSelectionFilter::All,
        &BrowseFilterConditions {
            available: Some(false),
            ..BrowseFilterConditions::default()
        },
    )
    .await;
    assert_eq!(unavailable.total, 1);
    assert_eq!(photos[0].id, by_location["a.jpg"]);
    assert!(!photos[0].original.available);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn capture_time_bounds_are_inclusive_lower_and_exclusive_upper() {
    let (base, config) = seed_filter_fixture();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let by_location = photo_ids_by_location(&application, &ids).await;

    let bound = |from: &str, before: &str| BrowseFilterConditions {
        captured_from: Some(slipstream_core::CaptureTimeBound::parse(from).unwrap()),
        captured_before: Some(slipstream_core::CaptureTimeBound::parse(before).unwrap()),
        ..BrowseFilterConditions::default()
    };
    // The lower bound includes its own second; the upper bound excludes it.
    let (opened, photos) = browse_filtered_ids(
        &application,
        BrowseSourceRequest::Library,
        BrowseSelectionFilter::All,
        &bound("2026-01-02T09:00:00", "2026-01-04T09:00:00"),
    )
    .await;
    assert_eq!(opened.total, 2);
    let locations = photos
        .iter()
        .map(|photo| photo.id.clone())
        .collect::<Vec<_>>();
    assert!(locations.contains(&by_location["b.jpg"]));
    assert!(locations.contains(&by_location["c.jpg"]));
    assert!(!locations.contains(&by_location["d.jpg"]));

    // A Photo without a Capture Time never matches a supplied bound.
    assert!(by_location.contains_key("untimed.jpg"));
    let (opened, _) = browse_filtered_ids(
        &application,
        BrowseSourceRequest::Library,
        BrowseSelectionFilter::All,
        &bound("2026-01-01T00:00:00", "2027-01-01T00:00:00"),
    )
    .await;
    assert_eq!(opened.total, 7, "the untimed Photo is excluded");

    // Without a bound the same Photo stays eligible.
    let (all, _) = browse_filtered_ids(
        &application,
        BrowseSourceRequest::Library,
        BrowseSelectionFilter::All,
        &BrowseFilterConditions::default(),
    )
    .await;
    assert_eq!(all.total, 8);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn malformed_and_reversed_filters_are_rejected_before_any_snapshot_exists() {
    let (base, config) = seed_filter_fixture();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());

    for body in [
        serde_json::json!({"source": "library", "ratingMinimum": 6}),
        serde_json::json!({"source": "library", "ratingMaximum": 6}),
        serde_json::json!({"source": "library", "ratingMinimum": 4, "ratingMaximum": 2}),
        serde_json::json!({"source": "library", "kind": "tiff"}),
        serde_json::json!({"source": "library", "capturedFrom": "2026-01-01"}),
        serde_json::json!({"source": "library", "capturedBefore": "2026-13-01T00:00:00"}),
        serde_json::json!({
            "source": "library",
            "capturedFrom": "2026-02-01T00:00:00",
            "capturedBefore": "2026-01-01T00:00:00",
        }),
        // Equal bounds are reversed: the upper bound is exclusive.
        serde_json::json!({
            "source": "library",
            "capturedFrom": "2026:01:01T00:00:00",
            "capturedBefore": "2026:01:01T00:00:00",
        }),
    ] {
        let response = post_json(&router, "/api/browse", body, Some("https://camera.local")).await;
        assert_eq!(
            response.status(),
            StatusCode::BAD_REQUEST,
            "invalid filter must be refused before a Snapshot exists"
        );
        assert_eq!(
            response_json(response).await,
            serde_json::json!({"error": "Invalid browse filter"})
        );
    }

    // A valid filter is admitted, and an expired Snapshot stays a distinct
    // failure from a new empty result.
    let valid = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({
                "source": "library",
                "ratingMaximum": 5,
                "kind": "raw",
                "available": true,
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(valid["total"], 1);
    let token = valid["token"].as_str().unwrap().to_owned();
    application.browse_close(&token);
    let window = send(
        &router,
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/browse/{token}?start=0&limit=10"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        window.status(),
        StatusCode::NOT_FOUND,
        "an expired Snapshot is a distinct failure"
    );
    // A new empty result is admitted as a real zero-match view instead.
    let empty = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source": "library", "ratingMinimum": 5, "kind": "jpeg"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(empty["total"], 0);
    assert_eq!(empty["position"], 0);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn a_filtered_snapshot_freezes_its_sequence_and_keeps_positions_inside_it() {
    let (base, config) = seed_filter_fixture();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let by_location = photo_ids_by_location(&application, &ids).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    assert_eq!(
        decide_rating(&router, &by_location["c.jpg"], 5).await,
        StatusCode::OK
    );

    let filter = BrowseFilterConditions {
        rating_minimum: Some(5),
        ..BrowseFilterConditions::default()
    };
    let opened = application
        .browse_open_filtered(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::All,
            filter.clone(),
            Some(&by_location["c.jpg"]),
        )
        .await
        .unwrap();
    assert_eq!(opened.total, 1);
    assert_eq!(opened.position, 0);
    let position = application
        .browse_position(&opened.token, &by_location["c.jpg"])
        .unwrap();
    assert_eq!(position.position, Some(0));
    // A Photo the filter excluded is not in the frozen sequence.
    let excluded = application
        .browse_position(&opened.token, &by_location["a.jpg"])
        .unwrap();
    assert_eq!(excluded.position, None);

    // A decision that changes a fact does not move the open sequence.
    assert_eq!(
        decide_rating(&router, &by_location["a.jpg"], 5).await,
        StatusCode::OK
    );
    let window = application
        .browse_window(&opened.token, 0, 60)
        .await
        .unwrap();
    assert_eq!(window.photos.len(), 1);
    assert_eq!(window.photos[0].id, by_location["c.jpg"]);
    application.browse_close(&opened.token);

    // Reopening applies the same filter to the new facts.
    let reopened = application
        .browse_open_filtered(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::All,
            filter,
            None,
        )
        .await
        .unwrap();
    assert_eq!(reopened.total, 2);
    application.browse_close(&reopened.token);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
