use super::*;
/// The CLI Photo query resolves one Selection State filter through the single
/// server parser: every canonical name filters, and any other name is refused
/// with the unchanged invalid-input reason.
#[tokio::test]
async fn cli_photo_query_selection_filter_uses_the_shared_parser() {
    let (base, config) = prepare_fixture();
    jpeg_fixture(&config.library_root.join("picked.jpg"), 8, 4, [64, 32, 192]);
    jpeg_fixture(
        &config.library_root.join("rejected.jpg"),
        8,
        4,
        [192, 32, 64],
    );
    jpeg_fixture(&config.library_root.join("plain.jpg"), 8, 4, [32, 192, 64]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    assert_eq!(ids.len(), 3);
    let photos = application.library.snapshot().await.unwrap().photos.clone();
    for (sort_path, state) in [
        ("picked.jpg", SelectionState::Picked),
        ("rejected.jpg", SelectionState::Rejected),
    ] {
        let target = photos
            .iter()
            .find(|photo| photo.sort_path.ends_with(sort_path))
            .unwrap_or_else(|| panic!("{sort_path} is missing from the published Library"))
            .id
            .clone();
        application
            .mutate_photo_state(slipstream_core::PhotoStateMutation {
                photo_id: target,
                field: slipstream_core::PhotoStateField::SelectionState,
                value: slipstream_core::PhotoStateValue::Selection(state),
                expected_current: None,
                album_id: None,
            })
            .await
            .unwrap();
    }
    let router = authorized_router(Arc::clone(&application), config.web_root());

    for name in ["unflagged", "picked", "rejected"] {
        let query = post_cli_json(
            &router,
            "/api/photo-queries",
            serde_json::json!({"selection": name, "limit": 60}),
        )
        .await;
        assert_eq!(query.status(), StatusCode::OK, "{name}");
        let body = response_json(query).await;
        assert_eq!(body["total"], 1, "{name}");
    }

    let unfiltered = post_cli_json(
        &router,
        "/api/photo-queries",
        serde_json::json!({"selection": "all", "limit": 60}),
    )
    .await;
    assert_eq!(unfiltered.status(), StatusCode::OK);
    assert_eq!(response_json(unfiltered).await["total"], 3);

    let invalid = post_cli_json(
        &router,
        "/api/photo-queries",
        serde_json::json!({"selection": "selected", "limit": 60}),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(invalid).await["error"]["details"],
        serde_json::json!({
            "argument": "selection",
            "reason": "The Selection State filter is invalid."
        })
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
