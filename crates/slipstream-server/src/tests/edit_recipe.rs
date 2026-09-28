use super::*;

/// The supported source support requires the approved camera identity and an
/// ARW container; it is a fact about the class, so it survives a deployment
/// without the processing capability while processing becomes unavailable.
#[tokio::test]
async fn edit_recipe_read_reports_recipe_absence_support_and_controls() {
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

    let (status, read) = get_edit_recipe(&router, &photo_id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read["photoId"], photo_id);
    assert!(read["recipe"].is_null());
    assert!(!read["sourceRevision"].as_str().unwrap().is_empty());
    assert_eq!(read["sourceSupport"], "supported");
    assert!(read["supportReason"].is_null());
    // An Original edit source omits the proxy identity entirely: the Web
    // parser refuses the key on a non-proxy source, so it must not even
    // serialize as null.
    assert_eq!(read["editSource"], "original");
    assert!(read.get("editSourceProxyId").is_none());
    // The deployment is configured but this test harness has no launcher
    // socket, so the reconciled condition is launcher-unavailable and
    // processing is not available for the Photo.
    assert_eq!(read["processingAvailable"], false);
    assert_eq!(
        read["controls"],
        serde_json::json!({
            "exposure": {"minimumEv": 0.0, "maximumEv": 1.0, "stepEv": 0.001},
            "whiteBalanceModes": ["as-shot"]
        })
    );

    // Without the configured deployment the class stays supported; only the
    // processing availability of the Photo changes.
    let plain = authorized_router(Arc::clone(&application), config.web_root());
    let (_, disabled) = get_edit_recipe(&plain, &photo_id).await;
    assert_eq!(disabled["sourceSupport"], "supported");
    assert!(disabled["supportReason"].is_null());
    assert_eq!(disabled["processingAvailable"], false);

    // An invalid Photo reference fails with the closed unknown_photo code.
    let response = get_cli_json(
        &router,
        "/api/photos/00000000-0000-4000-8000-000000000000/edit-recipe",
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let missing = response_json(response).await;
    assert_eq!(error_code(&missing), "unknown_photo");

    // The metadata response carries the observed camera identity.
    let detail = cli_photo_read(&router, &photo_id).await;
    assert_eq!(detail["metadata"]["make"], "SONY");
    assert_eq!(detail["metadata"]["model"], "ILCE-7RM5");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// One guarded save flow over the real routes: first save, idempotent
/// replay, request-identity conflict, stale recipe revision, source change,
/// and the explicit rebind that adopts the new source revision.
#[tokio::test]
async fn edit_recipe_save_replay_conflicts_and_rebind_follow_the_contract() {
    let (base, config) = prepare_fixture();
    let raw_path = config.library_root.join("approved.ARW");
    let raw_bytes = approved_raw_fixture(&raw_path);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = configured_router(&application, config.web_root());
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .into_iter()
        .next()
        .unwrap();

    let (_, read) = get_edit_recipe(&router, &photo_id).await;
    let source_revision = read["sourceRevision"].as_str().unwrap().to_owned();

    // Rebinding without a recipe finds none.
    let (status, missing) = rebind_recipe(
        &router,
        &photo_id,
        serde_json::json!({
            "requestId": "probe-rebind",
            "expectedRecipeVersion": "00000000-0000-4000-8000-000000000000",
            "newSourceRevision": source_revision,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error_code(&missing), "missing_recipe");

    // Photo facts carry the saved-edit fact before any save.
    let detail = cli_photo_read(&router, &photo_id).await;
    assert_eq!(detail["hasSavedEdits"], false);

    // The first save creates a revision. A success body carries exactly the
    // outcome, the committed recipe version, and the bound source revision.
    let (status, saved) = save_recipe(
        &router,
        &photo_id,
        save_body("save-1", None, &source_revision, 0.25),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["outcome"], "saved");
    let revision = saved["recipeVersion"].as_str().unwrap().to_owned();
    assert_eq!(saved["sourceRevision"], source_revision);

    // Photo facts now report the saved edit through detail, Browse, and the
    // bounded Photo query.
    let detail = cli_photo_read(&router, &photo_id).await;
    assert_eq!(detail["hasSavedEdits"], true);
    // Browse windows serve the frozen published snapshot, so the summary
    // still reports the published fact until the next scan publishes.
    let opened = application
        .browse_open(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::All,
            None,
        )
        .await
        .unwrap();
    let window = application
        .browse_window(&opened.token, 0, 60)
        .await
        .unwrap();
    application.browse_close(&opened.token);
    assert_eq!(window.photos[0].id, photo_id);
    assert!(!window.photos[0].has_saved_edits);
    let query = response_json(
        send(
            &router,
            authenticated_request()
                .method("POST")
                .uri("https://camera.local/api/photo-queries")
                .header("Slipstream-CLI-Contract", "1")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"limit":60}"#))
                .unwrap(),
        )
        .await,
    )
    .await;
    let queried = query["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == photo_id)
        .unwrap();
    assert_eq!(queried["hasSavedEdits"], true);

    // The same identity and payload replay to the committed outcome.
    let (status, replay) = save_recipe(
        &router,
        &photo_id,
        save_body("save-1", None, &source_revision, 0.25),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["outcome"], "unchanged");
    assert_eq!(replay["recipeVersion"], revision);

    // The same identity with a different payload is refused.
    let (status, request_conflict) = save_recipe(
        &router,
        &photo_id,
        save_body("save-1", None, &source_revision, 0.5),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_code(&request_conflict), "request_conflict");

    // A replayed identity after a later write reports the stored receipt as
    // unchanged: no write occurs for the replay.
    let (status, second) = save_recipe(
        &router,
        &photo_id,
        save_body("save-2", Some(&revision), &source_revision, 0.75),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(second["outcome"], "saved");
    let second_revision = second["recipeVersion"].as_str().unwrap().to_owned();
    assert_ne!(second_revision, revision);
    let (status, stale_replay) = save_recipe(
        &router,
        &photo_id,
        save_body("save-1", None, &source_revision, 0.25),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(stale_replay["outcome"], "unchanged");
    assert_eq!(stale_replay["recipeVersion"], revision);

    // A stale recipe revision conflicts and carries the current facts under
    // the contract's field names.
    let (status, recipe_conflict) = save_recipe(
        &router,
        &photo_id,
        save_body(
            "stale-revision",
            Some("00000000-0000-4000-8000-000000000000"),
            &source_revision,
            0.5,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_code(&recipe_conflict), "recipe_conflict");
    assert_eq!(
        recipe_conflict["error"]["details"]["currentRecipeVersion"],
        second_revision
    );
    assert_eq!(
        recipe_conflict["error"]["details"]["currentSourceRevision"],
        source_revision
    );

    // A changed source fails a stale save and preserves the saved intent.
    let mut changed = raw_bytes.clone();
    changed.push(0);
    fs::write(&raw_path, &changed).unwrap();
    let scanned = post_json(&router, "/api/scan", serde_json::json!({}), None).await;
    assert_eq!(scanned.status(), StatusCode::OK);
    let (_, changed_read) = get_edit_recipe(&router, &photo_id).await;
    let new_source = changed_read["sourceRevision"].as_str().unwrap().to_owned();
    assert_ne!(new_source, source_revision);
    // The new publication carries the saved-edit fact into Browse summaries.
    let opened = application
        .browse_open(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::All,
            None,
        )
        .await
        .unwrap();
    let window = application
        .browse_window(&opened.token, 0, 60)
        .await
        .unwrap();
    application.browse_close(&opened.token);
    assert_eq!(window.photos[0].id, photo_id);
    assert!(window.photos[0].has_saved_edits);

    let (status, source_changed) = save_recipe(
        &router,
        &photo_id,
        save_body("changed-source", Some(&revision), &source_revision, 0.25),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_code(&source_changed), "source_changed");
    // The carried facts name the currently committed recipe version and the
    // newly published source revision.
    assert_eq!(
        source_changed["error"]["details"]["currentRecipeVersion"],
        second_revision
    );
    assert_eq!(
        source_changed["error"]["details"]["currentSourceRevision"],
        new_source
    );

    // The explicit rebind adopts the newly observed source revision with a
    // new recipe version, and a read renders the committed intent.
    let (status, rebound) = rebind_recipe(
        &router,
        &photo_id,
        rebind_body("rebind-1", &second_revision, &new_source),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rebound["outcome"], "saved");
    let rebound_revision = rebound["recipeVersion"].as_str().unwrap().to_owned();
    assert_ne!(rebound_revision, second_revision);
    assert_eq!(rebound["sourceRevision"], new_source);

    // The same rebind identity and payload replay to the committed outcome:
    // no second write, and the replay carries the receipt's recipe version.
    let (status, rebind_replay) = rebind_recipe(
        &router,
        &photo_id,
        rebind_body("rebind-1", &second_revision, &new_source),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rebind_replay["outcome"], "unchanged");
    assert_eq!(rebind_replay["recipeVersion"], rebound_revision);
    assert_eq!(rebind_replay["sourceRevision"], new_source);

    // The same rebind identity with a different payload is refused.
    let (status, rebind_conflict) = rebind_recipe(
        &router,
        &photo_id,
        rebind_body("rebind-1", &rebound_revision, &new_source),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error_code(&rebind_conflict), "request_conflict");

    // The rebound recipe keeps its committed settings and renders the stored
    // white-balance mode in the shared field shape.
    let (_, rebound_read) = get_edit_recipe(&router, &photo_id).await;
    assert_eq!(rebound_read["recipe"]["recipeVersion"], rebound_revision);
    assert_eq!(rebound_read["recipe"]["exposureEv"], 0.75);
    assert_eq!(
        rebound_read["recipe"]["whiteBalance"],
        serde_json::json!({"mode": "as-shot"})
    );
    assert_eq!(rebound_read["sourceRevision"], new_source);
    assert_eq!(rebound_read["sourceSupport"], "supported");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A Photo whose source class has no approved profile is refused before any
/// guarded write and reads as unsupported, with a present source revision.
#[tokio::test]
async fn edit_recipe_refuses_unsupported_source_classes() {
    let (base, config) = prepare_fixture();
    unapproved_raw_fixture(&config.library_root.join("other.ARW"));
    jpeg_fixture(&config.library_root.join("plain.jpg"), 8, 4, [1, 2, 3]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = configured_router(&application, config.web_root());
    let by_location = photo_ids_by_location(
        &application,
        &browse_photo_ids(&application, BrowseSourceRequest::Library).await,
    )
    .await;
    let unapproved_id = by_location["other.ARW"].clone();
    let jpeg_id = by_location["plain.jpg"].clone();

    // The observed identity without a matching profile is unsupported.
    let (status, read) = get_edit_recipe(&router, &unapproved_id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read["sourceSupport"], "unsupported");
    assert!(read["supportReason"].is_null());
    assert!(!read["sourceRevision"].as_str().unwrap().is_empty());
    assert_eq!(read["processingAvailable"], false);

    // A JPEG is a known class without an approved profile, so it reads as
    // unsupported too.
    let (status, jpeg_read) = get_edit_recipe(&router, &jpeg_id).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(jpeg_read["sourceSupport"], "unsupported");
    assert!(jpeg_read["supportReason"].is_null());

    for photo_id in [unapproved_id, jpeg_id] {
        let (status, refused) = save_recipe(
            &router,
            &photo_id,
            save_body("refused", None, "00000000-0000-4000-8000-000000000000", 0.1),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(error_code(&refused), "unsupported_photo");
        let (status, rebound) = rebind_recipe(
            &router,
            &photo_id,
            serde_json::json!({
                "requestId": "refused-rebind",
                "expectedRecipeVersion": "00000000-0000-4000-8000-000000000000",
                "newSourceRevision": "00000000-0000-4000-8000-000000000000",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(error_code(&rebound), "unsupported_photo");
    }

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A RAW Photo whose camera identity cannot be observed, or that has
/// disappeared from its remembered Location, reads as unavailable with the
/// matching closed reason, reports a null source revision, and refuses every
/// guarded write with the closed resource refusal.
#[tokio::test]
async fn edit_recipe_reports_unavailable_originals_with_closed_reasons() {
    let (base, config) = prepare_fixture();
    generated_non_tiff_raw_fixture(&config.library_root.join("opaque.ARW"));
    let vanishing = config.library_root.join("vanishing.ARW");
    approved_raw_fixture(&vanishing);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = configured_router(&application, config.web_root());
    let by_location = photo_ids_by_location(
        &application,
        &browse_photo_ids(&application, BrowseSourceRequest::Library).await,
    )
    .await;
    let photo_id = by_location["opaque.ARW"].clone();
    let vanishing_id = by_location["vanishing.ARW"].clone();

    let (_, read) = get_edit_recipe(&router, &photo_id).await;
    assert_eq!(read["sourceSupport"], "unavailable");
    assert_eq!(read["supportReason"], "original-unreadable");
    // The null source revision marks exactly the unavailable state.
    assert!(read["sourceRevision"].is_null());
    assert_eq!(read["processingAvailable"], false);

    // The identity is unobservable, so no guarded write is possible.
    let (status, refused) = save_recipe(
        &router,
        &photo_id,
        save_body(
            "opaque-save",
            None,
            "00000000-0000-4000-8000-000000000000",
            0.4,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(&refused), "resource_unavailable");
    let (status, rebind_refused) = rebind_recipe(
        &router,
        &photo_id,
        serde_json::json!({
            "requestId": "opaque-rebind",
            "expectedRecipeVersion": "00000000-0000-4000-8000-000000000000",
            "newSourceRevision": "00000000-0000-4000-8000-000000000000",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(&rebind_refused), "resource_unavailable");

    // The metadata response reports the absent camera identity as absent
    // values, not as a failure.
    let detail = cli_photo_read(&router, &photo_id).await;
    assert!(detail["metadata"]["make"].is_null());
    assert!(detail["metadata"]["model"].is_null());

    // A readable Original reads as supported until it disappears; the next
    // scan reports the closed missing-Original reason.
    let (_, before) = get_edit_recipe(&router, &vanishing_id).await;
    assert_eq!(before["sourceSupport"], "supported");
    fs::remove_file(&vanishing).unwrap();
    let scanned = post_json(&router, "/api/scan", serde_json::json!({}), None).await;
    assert_eq!(scanned.status(), StatusCode::OK);
    wait_for_scan_settled(&application).await;
    let (_, read) = get_edit_recipe(&router, &vanishing_id).await;
    assert_eq!(read["sourceSupport"], "unavailable");
    assert_eq!(read["supportReason"], "original-missing");
    assert!(read["sourceRevision"].is_null());
    assert_eq!(read["processingAvailable"], false);
    let (status, refused) = save_recipe(
        &router,
        &vanishing_id,
        save_body(
            "missing-save",
            None,
            "00000000-0000-4000-8000-000000000000",
            0.2,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(&refused), "resource_unavailable");
    let (status, missing_rebind) = rebind_recipe(
        &router,
        &vanishing_id,
        serde_json::json!({
            "requestId": "missing-rebind",
            "expectedRecipeVersion": "00000000-0000-4000-8000-000000000000",
            "newSourceRevision": "00000000-0000-4000-8000-000000000000",
        }),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error_code(&missing_rebind), "resource_unavailable");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// Settings outside the approved range, off the milli-EV grid, with an
/// unapproved white-balance mode, or with unknown fields return the closed
/// invalid_settings code; Web requests without the CLI header share routes.
#[tokio::test]
async fn edit_recipe_validates_settings_before_the_write() {
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
    let (_, read) = get_edit_recipe(&router, &photo_id).await;
    let source_revision = read["sourceRevision"].as_str().unwrap().to_owned();

    for body in [
        save_body("bad-range", None, &source_revision, 1.5),
        save_body("off-grid", None, &source_revision, 0.0005),
        save_body("", None, &source_revision, 0.1),
        // The retired request field name is an unknown field: a client that
        // sends the pre-contract name is refused before any state change.
        serde_json::json!({
            "requestId": "retired-name",
            "expectedRecipeRevision": "00000000-0000-4000-8000-000000000000",
            "expectedSourceRevision": source_revision,
            "settings": {"exposureEv": 0.1, "whiteBalance": {"mode": "as-shot"}}
        }),
        // The request identity admits only letters, digits, `.`, `_`, `-`.
        save_body("space id", None, &source_revision, 0.1),
        save_body("slash/id", None, &source_revision, 0.1),
        serde_json::json!({
            "requestId": "custom-wb",
            "expectedSourceRevision": source_revision,
            "settings": {"exposureEv": 0.1, "whiteBalance": {"mode": "custom"}}
        }),
        serde_json::json!({
            "requestId": "out-of-bounds",
            "expectedSourceRevision": source_revision,
            "settings": {"exposureEv": 0.1, "whiteBalance": {"mode": "temperature-tint", "temperatureKelvin": 500, "tintMilli": 0}}
        }),
        serde_json::json!({
            "requestId": "unknown-field",
            "expectedSourceRevision": source_revision,
            "settings": {"exposureEv": 0.1, "whiteBalance": {"mode": "as-shot", "tint": 3}},
        }),
    ] {
        let (status, refused) = save_recipe(&router, &photo_id, body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(error_code(&refused), "invalid_settings");
    }

    // A temperature-tint intent inside the published payload bounds is
    // wire-valid editing intent: it commits like any save, reads back with
    // its values, and reports processing as unavailable because the
    // capability does not admit the mode.
    let (status, saved) = save_recipe(
        &router,
        &photo_id,
        serde_json::json!({
            "requestId": "tint-intent",
            "expectedSourceRevision": source_revision,
            "settings": {"exposureEv": 0.3, "whiteBalance": {"mode": "temperature-tint", "temperatureKelvin": 6500, "tintMilli": -12}}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["outcome"], "saved");
    let tint_revision = saved["recipeVersion"].as_str().unwrap().to_owned();
    // The identical intent replays as unchanged: different values under the
    // same request identity would conflict.
    let (status, replay) = save_recipe(
        &router,
        &photo_id,
        serde_json::json!({
            "requestId": "tint-intent",
            "expectedSourceRevision": source_revision,
            "settings": {"exposureEv": 0.3, "whiteBalance": {"mode": "temperature-tint", "temperatureKelvin": 6500, "tintMilli": -12}}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(replay["outcome"], "unchanged");
    assert_eq!(replay["recipeVersion"], tint_revision);
    let (_, tint_read) = get_edit_recipe(&router, &photo_id).await;
    assert_eq!(
        tint_read["recipe"]["whiteBalance"],
        serde_json::json!({"mode": "temperature-tint", "temperatureKelvin": 6500, "tintMilli": -12})
    );
    assert_eq!(tint_read["recipe"]["exposureEv"], 0.3);
    assert_eq!(tint_read["processingAvailable"], false);

    // An empty rebind revision and an unknown rebind field are invalid settings.
    let (status, refused) = rebind_recipe(
        &router,
        &photo_id,
        serde_json::json!({"requestId": "bad-rebind", "expectedRecipeVersion": "", "newSourceRevision": source_revision}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(error_code(&refused), "invalid_settings");
    let (status, refused) = rebind_recipe(
        &router,
        &photo_id,
        serde_json::json!({
            "requestId": "probe-rebind",
            "expectedRecipeVersion": "00000000-0000-4000-8000-000000000000",
            "newSourceRevision": source_revision,
            "force": true,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(error_code(&refused), "invalid_settings");

    // The same Web route works without the CLI contract header.
    let web_read = send(
        &router,
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/photos/{photo_id}/edit-recipe"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(web_read.status(), StatusCode::OK);
    let web_saved = post_json(
        &router,
        &format!("/api/photos/{photo_id}/edit-recipe"),
        save_body("web-save", Some(&tint_revision), &source_revision, 0.1),
        None,
    )
    .await;
    assert_eq!(web_saved.status(), StatusCode::OK);
    let saved = response_json(web_saved).await;
    assert_eq!(saved["outcome"], "saved");

    // A wrong CLI contract version fails before the domain.
    let wrong = send(
        &router,
        authenticated_request()
            .method("POST")
            .uri(edit_recipe_uri(&photo_id))
            .header("Slipstream-CLI-Contract", "2")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                save_body("v2", None, &source_revision, 0.1).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(wrong.status(), StatusCode::UPGRADE_REQUIRED);

    // A Web-shaped malformed body answers with the closed invalid_settings
    // code instead of the legacy error shape.
    let web_malformed = send(
        &router,
        authenticated_request()
            .method("POST")
            .uri(edit_recipe_uri(&photo_id))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from("{not json"))
            .unwrap(),
    )
    .await;
    assert_eq!(web_malformed.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let refused = response_json(web_malformed).await;
    assert_eq!(error_code(&refused), "invalid_settings");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
