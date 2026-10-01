use super::*;

#[tokio::test]
async fn cli_direct_photo_metadata_stays_with_its_published_revision() {
    let (base, config) = prepare_fixture();
    let path = config.library_root.join("a.jpg");
    capture_metadata_fixture(&path, "2026:01:01 10:00:00");
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .remove(0);
    let old_revision = application
        .shared
        .snapshot
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .snapshot
        .originals[0]
        .capture
        .source_revision
        .clone()
        .unwrap();
    let captured_before_replacement = application
        .published_photo_detail(&photo_id)
        .await
        .unwrap()
        .unwrap();

    use std::os::unix::fs::MetadataExt;
    let original_metadata = fs::metadata(&path).unwrap();
    let original_mtime = original_metadata.modified().unwrap();
    let replacement = config.library_root.join("replacement.tmp");
    capture_metadata_fixture(&replacement, "2026:01:01 11:00:00");
    let replacement_file = fs::OpenOptions::new()
        .write(true)
        .open(&replacement)
        .unwrap();
    replacement_file
        .set_times(std::fs::FileTimes::new().set_modified(original_mtime))
        .unwrap();
    drop(replacement_file);
    let replacement_metadata = fs::metadata(&replacement).unwrap();
    assert_eq!(replacement_metadata.len(), original_metadata.len());
    assert_eq!(replacement_metadata.modified().unwrap(), original_mtime);
    assert_ne!(replacement_metadata.ino(), original_metadata.ino());
    fs::rename(&replacement, &path).unwrap();
    let replaced_metadata = fs::metadata(&path).unwrap();
    assert_eq!(replaced_metadata.len(), original_metadata.len());
    assert_eq!(replaced_metadata.modified().unwrap(), original_mtime);
    assert_ne!(replaced_metadata.ino(), original_metadata.ino());

    let router = authorized_router(Arc::clone(&application), config.web_root());
    let unpublished_metadata = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{photo_id}/metadata"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(unpublished_metadata, serde_json::json!({}));
    let unpublished_direct = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!("https://camera.local/api/photos/{photo_id}"))
                .header("Slipstream-CLI-Contract", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(
        unpublished_direct["captureTime"],
        "2026-01-01T10:00:00.000000000"
    );
    assert_eq!(
        unpublished_direct["metadata"]["captureTime"],
        "2026-01-01T10:00:00.000000000"
    );

    let (publish_sender, publish_receiver) = tokio::sync::oneshot::channel();
    let scan = application
        .admit_scan_cycle(None, Some(publish_receiver))
        .unwrap();
    for _ in 0..400 {
        let persisted = application.library.snapshot().await.unwrap();
        if persisted.originals[0].capture.order_key.as_deref()
            == Some("2026-01-01T11:00:00.000000000")
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        application.library.snapshot().await.unwrap().originals[0]
            .capture
            .order_key
            .as_deref(),
        Some("2026-01-01T11:00:00.000000000"),
        "scan did not reach the publication gate"
    );

    // The current file has the next generation's bytes, but both consumers
    // still present the prior publication. Metadata inspection must reject the
    // replacement's inode-bound revision until the scan publishes it.
    let gated_metadata = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{photo_id}/metadata"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(gated_metadata, serde_json::json!({}));
    let gated = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!("https://camera.local/api/photos/{photo_id}"))
                .header("Slipstream-CLI-Contract", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(gated["captureTime"], "2026-01-01T10:00:00.000000000");
    assert_eq!(
        gated["metadata"]["captureTime"],
        "2026-01-01T10:00:00.000000000"
    );

    // Replace the publication between detail capture and metadata inspection.
    // The captured detail remains internally coherent and does not adopt facts
    // from the new generation.
    drop(publish_sender);
    scan.await.unwrap().unwrap();
    let (prior_photo, prior_metadata) = application
        .inspect_published_photo_detail(captured_before_replacement)
        .await;
    assert_eq!(
        prior_photo.capture.order_key.as_deref(),
        Some("2026-01-01T10:00:00.000000000")
    );
    assert_eq!(
        prior_metadata,
        slipstream_core::CaptureReviewMetadata::default()
    );

    let new_revision = application
        .shared
        .snapshot
        .read()
        .unwrap()
        .as_ref()
        .unwrap()
        .snapshot
        .originals[0]
        .capture
        .source_revision
        .clone()
        .unwrap();
    assert_ne!(new_revision, old_revision);

    let fresh_metadata = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{photo_id}/metadata"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(
        fresh_metadata["captureTime"],
        "2026-01-01T11:00:00.000000000"
    );
    // A metadata response omits a field the Original does not carry.
    assert!(fresh_metadata["aperture"].is_null());
    let fresh = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!("https://camera.local/api/photos/{photo_id}"))
                .header("Slipstream-CLI-Contract", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(fresh["captureTime"], "2026-01-01T11:00:00.000000000");
    assert_eq!(
        fresh["metadata"]["captureTime"],
        "2026-01-01T11:00:00.000000000"
    );

    // A vanished Original still answers fail-soft: the published revision no
    // longer resolves, and the route reports an empty document rather than a
    // storage failure.
    fs::remove_file(&path).unwrap();
    let vanished = send(
        &router,
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/photos/{photo_id}/metadata"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(vanished.status(), StatusCode::OK);
    assert_eq!(response_json(vanished).await, serde_json::json!({}));

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn cli_photo_reads_keep_prior_membership_until_scan_publication() {
    let (base, config) = prepare_fixture();
    fs::create_dir_all(config.library_root.join("old")).unwrap();
    jpeg_fixture_with_capture_time(
        &config.library_root.join("old/a.jpg"),
        8,
        4,
        [32, 64, 192],
        "2026:01:01 10:00:00",
    );
    {
        let application = Application::open(&config).await.unwrap();
        wait_for_scan_settled(&application).await;
        application.shutdown().await.unwrap();
    }
    fs::create_dir_all(config.library_root.join("New")).unwrap();
    jpeg_fixture_with_capture_time(
        &config.library_root.join("old/a.jpg"),
        8,
        4,
        [48, 80, 176],
        "2026:01:01 11:00:00",
    );
    jpeg_fixture(&config.library_root.join("New/b.jpg"), 8, 4, [64, 96, 160]);
    let (publish_sender, publish_receiver) = tokio::sync::oneshot::channel();
    let application =
        Application::open_with_gate(&config, ScanLimits::default(), None, Some(publish_receiver))
            .await
            .unwrap();
    for _ in 0..400 {
        if application.library.snapshot().await.unwrap().photos.len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let persisted = application.library.snapshot().await.unwrap();
    assert_eq!(
        persisted.photos.len(),
        2,
        "scan did not reach the publish gate"
    );
    let old_id = persisted
        .photos
        .iter()
        .find(|photo| photo.sort_path == "old/a.jpg")
        .unwrap()
        .id
        .clone();
    let new_id = persisted
        .photos
        .iter()
        .find(|photo| photo.sort_path == "New/b.jpg")
        .unwrap()
        .id
        .clone();
    application
        .mutate_photo_state(slipstream_core::PhotoStateMutation {
            photo_id: old_id.clone(),
            field: slipstream_core::PhotoStateField::SelectionState,
            value: slipstream_core::PhotoStateValue::Selection(SelectionState::Selected),
            expected_current: None,
            album_id: None,
        })
        .await
        .unwrap();
    let router = authorized_router(Arc::clone(&application), config.web_root());

    let prior = response_json(
        send(
            &router,
            authenticated_request()
                .method("POST")
                .uri("https://camera.local/api/photo-queries")
                .header("Slipstream-CLI-Contract", "1")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"selection":"selected","limit":60}"#))
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(prior["total"], 1);
    assert_eq!(prior["items"][0]["id"], old_id);
    assert_eq!(
        prior["items"][0]["captureTime"],
        "2026-01-01T10:00:00.000000000"
    );

    let unpublished = send(
        &router,
        authenticated_request()
            .uri(format!("https://camera.local/api/photos/{new_id}"))
            .header("Slipstream-CLI-Contract", "1")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(unpublished.status(), StatusCode::NOT_FOUND);
    let unpublished_folder = send(
        &router,
        authenticated_request()
            .method("POST")
            .uri("https://camera.local/api/photo-queries")
            .header("Slipstream-CLI-Contract", "1")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"source":{"kind":"folder","location":"New"}}"#,
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(unpublished_folder.status(), StatusCode::NOT_FOUND);

    drop(publish_sender);
    wait_for_scan_settled(&application).await;
    let published = response_json(
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
    assert_eq!(published["total"], 2);
    assert_eq!(
        published["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == old_id)
            .unwrap()["captureTime"],
        "2026-01-01T11:00:00.000000000"
    );
    assert!(matches!(
        application
            .create_photo_query(
                slipstream_core::PhotoQuery {
                    source: slipstream_core::PhotoQuerySource::AllPhotos,
                    selection_state: None,
                    rating_minimum: None,
                    rating_maximum: None,
                    original_kind: None,
                    original_available: None,
                    captured_from: None,
                    captured_before: None,
                    order: slipstream_core::PhotoQueryOrder::CaptureTimeAscending,
                },
                1,
            )
            .await,
        Err(LibraryError::Query(
            slipstream_core::PhotoQueryError::ResultLimitExceeded { limit: 1 }
        ))
    ));
    assert!(
        application
            .retained_queries
            .lock()
            .unwrap()
            .entries
            .values()
            .all(|query| query.kind != crate::queries::RetainedKind::Photo)
    );
    let published_new = send(
        &router,
        authenticated_request()
            .uri(format!("https://camera.local/api/photos/{new_id}"))
            .header("Slipstream-CLI-Contract", "1")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(published_new.status(), StatusCode::OK);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn cli_contract_header_rejects_reused_writes_before_domain_admission() {
    let (base, config) = prepare_fixture();
    jpeg_fixture(&config.library_root.join("one.jpg"), 8, 4, [32, 64, 192]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .remove(0);
    let album_id = application
        .mutate_album(slipstream_core::AlbumMutation::Create {
            name: "Guarded".to_owned(),
        })
        .await
        .unwrap()
        .albums
        .into_iter()
        .find(|album| album.name == "Guarded")
        .unwrap()
        .id;
    let before_photo = application.library.photo(&photo_id).await.unwrap().unwrap();
    let before_album = application.library.album(&album_id).await.unwrap().unwrap();
    let router = authorized_router(Arc::clone(&application), config.web_root());

    let unsupported = send(
        &router,
        authenticated_request()
            .method("POST")
            .uri(format!("https://camera.local/api/photos/{photo_id}/state"))
            .header("Slipstream-CLI-Contract", "2")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"field":"rating","value":4}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(unsupported.status(), StatusCode::UPGRADE_REQUIRED);

    let malformed = send(
        &router,
        authenticated_request()
            .method("POST")
            .uri(format!("https://camera.local/api/albums/{album_id}/rename"))
            .header(
                "Slipstream-CLI-Contract",
                header::HeaderValue::from_bytes(&[0xff]).unwrap(),
            )
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"name":"Changed"}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(malformed.status(), StatusCode::UPGRADE_REQUIRED);

    let duplicate = send(
        &router,
        authenticated_request()
            .method("POST")
            .uri(format!("https://camera.local/api/photos/{photo_id}/state"))
            .header("Slipstream-CLI-Contract", "1")
            .header("Slipstream-CLI-Contract", "1")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"field":"selectionState","value":"selected"}"#,
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::UPGRADE_REQUIRED);

    let after_photo = application.library.photo(&photo_id).await.unwrap().unwrap();
    let after_album = application.library.album(&album_id).await.unwrap().unwrap();
    assert_eq!(after_photo.rating, before_photo.rating);
    assert_eq!(after_photo.selection_state, before_photo.selection_state);
    assert_eq!(after_photo.decision_version, before_photo.decision_version);
    assert_eq!(after_album.name, before_album.name);
    assert_eq!(after_album.album_version, before_album.album_version);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn cli_album_routes_map_checked_atomic_results_and_keep_web_shapes() {
    let (base, config) = prepare_fixture();
    for name in ["a.jpg", "b.jpg", "c.jpg"] {
        jpeg_fixture(&config.library_root.join(name), 8, 4, [32, 64, 192]);
    }
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let photo_ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());

    let legacy = post_json(
        &router,
        "/api/albums",
        serde_json::json!({"name": "Web Album"}),
        None,
    )
    .await;
    assert_eq!(legacy.status(), StatusCode::OK);
    let legacy = response_json(legacy).await;
    assert!(legacy.get("albums").is_some());
    assert!(legacy.get("album").is_none());
    assert!(legacy["albums"][0].get("albumVersion").is_none());

    let created = post_cli_json(
        &router,
        "/api/albums",
        serde_json::json!({"name": "CLI Picks"}),
    )
    .await;
    assert_eq!(created.status(), StatusCode::OK);
    let created = response_json(created).await;
    let album_id = created["album"]["id"].as_str().unwrap().to_owned();
    let initial_version = created["album"]["albumVersion"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(created["album"]["name"], "CLI Picks");
    assert_eq!(created["album"]["photoCount"], 0);
    assert_eq!(created["album"]["hasSavedPosition"], false);
    assert_eq!(
        created["album"]["webPath"],
        format!("/?source=album&albumId={album_id}")
    );

    let name_conflict = post_cli_json(
        &router,
        "/api/albums",
        serde_json::json!({"name": "cli picks"}),
    )
    .await;
    assert_eq!(name_conflict.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json(name_conflict).await["error"],
        serde_json::json!({
            "code": "name_conflict",
            "message": "Inspect the existing Album before choosing a different name.",
            "effect": "none",
            "details": {"name": "cli picks", "albumId": album_id}
        })
    );

    let added = post_cli_json(
        &router,
        &format!("/api/albums/{album_id}/changes"),
        serde_json::json!({
            "operation": "add",
            "photoIds": [photo_ids[1], photo_ids[0]],
            "ifVersion": initial_version
        }),
    )
    .await;
    assert_eq!(added.status(), StatusCode::OK);
    let added = response_json(added).await;
    assert_eq!(
        added["addedPhotoIds"],
        serde_json::json!([photo_ids[1], photo_ids[0]])
    );
    assert_eq!(added["alreadyMemberPhotoIds"], serde_json::json!([]));
    assert_eq!(added["album"]["photoCount"], 2);
    let added_version = added["album"]["albumVersion"].as_str().unwrap().to_owned();

    let no_op = post_cli_json(
        &router,
        &format!("/api/albums/{album_id}/changes"),
        serde_json::json!({
            "operation": "add",
            "photoIds": [photo_ids[0]],
            "ifVersion": added_version
        }),
    )
    .await;
    assert_eq!(no_op.status(), StatusCode::OK);
    let no_op = response_json(no_op).await;
    assert_eq!(no_op["addedPhotoIds"], serde_json::json!([]));
    assert_eq!(
        no_op["alreadyMemberPhotoIds"],
        serde_json::json!([photo_ids[0]])
    );
    assert_eq!(no_op["album"]["albumVersion"], added_version);

    let stale = post_cli_json(
        &router,
        &format!("/api/albums/{album_id}/changes"),
        serde_json::json!({
            "operation": "rename",
            "name": "Stale",
            "ifVersion": initial_version
        }),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(response_json(stale).await["error"]["code"], "conflict");

    let missing_id = "00000000-0000-4000-8000-00000000dead";
    let missing = post_cli_json(
        &router,
        &format!("/api/albums/{album_id}/changes"),
        serde_json::json!({
            "operation": "add",
            "photoIds": [photo_ids[2], missing_id],
            "ifVersion": added_version
        }),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response_json(missing).await["error"]["details"],
        serde_json::json!({"resource": "photo", "reference": missing_id})
    );
    let after_missing = application.library.album(&album_id).await.unwrap().unwrap();
    assert_eq!(after_missing.photo_count, 2);
    assert_eq!(after_missing.album_version, added_version);

    let incomplete = post_cli_json(
        &router,
        &format!("/api/albums/{album_id}/changes"),
        serde_json::json!({
            "operation": "reorder",
            "photoIds": [photo_ids[0]],
            "ifVersion": added_version
        }),
    )
    .await;
    assert_eq!(incomplete.status(), StatusCode::CONFLICT);
    let incomplete = response_json(incomplete).await;
    assert_eq!(incomplete["error"]["code"], "conflict");
    assert_eq!(
        incomplete["error"]["details"]["currentVersion"],
        added_version
    );

    let reordered = response_json(
        post_cli_json(
            &router,
            &format!("/api/albums/{album_id}/changes"),
            serde_json::json!({
                "operation": "reorder",
                "photoIds": [photo_ids[0], photo_ids[1]],
                "ifVersion": added_version
            }),
        )
        .await,
    )
    .await;
    assert_eq!(
        reordered["orderedPhotoIds"],
        serde_json::json!([photo_ids[0], photo_ids[1]])
    );
    assert_eq!(reordered["reordered"], true);
    let reordered_version = reordered["album"]["albumVersion"]
        .as_str()
        .unwrap()
        .to_owned();

    let progress = post_json(
        &router,
        &format!("/api/albums/{album_id}/progress"),
        serde_json::json!({"photoId": photo_ids[1]}),
        None,
    )
    .await;
    assert_eq!(progress.status(), StatusCode::OK);
    assert_eq!(
        application
            .library
            .album(&album_id)
            .await
            .unwrap()
            .unwrap()
            .album_version,
        reordered_version
    );

    let removed = response_json(
        post_cli_json(
            &router,
            &format!("/api/albums/{album_id}/changes"),
            serde_json::json!({
                "operation": "remove",
                "photoIds": [photo_ids[0], photo_ids[2]],
                "ifVersion": reordered_version
            }),
        )
        .await,
    )
    .await;
    assert_eq!(
        removed["removedPhotoIds"],
        serde_json::json!([photo_ids[0]])
    );
    assert_eq!(
        removed["alreadyAbsentPhotoIds"],
        serde_json::json!([photo_ids[2]])
    );
    assert_eq!(removed["savedPhotoId"], photo_ids[1]);
    let removed_version = removed["album"]["albumVersion"]
        .as_str()
        .unwrap()
        .to_owned();

    let other = response_json(
        post_cli_json(&router, "/api/albums", serde_json::json!({"name": "Other"})).await,
    )
    .await;
    let other_id = other["album"]["id"].as_str().unwrap();
    let rename_conflict = post_cli_json(
        &router,
        &format!("/api/albums/{album_id}/changes"),
        serde_json::json!({
            "operation": "rename",
            "name": "OTHER",
            "ifVersion": removed_version
        }),
    )
    .await;
    assert_eq!(rename_conflict.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json(rename_conflict).await["error"]["details"],
        serde_json::json!({"name": "OTHER", "albumId": other_id})
    );

    let deleted = post_cli_json(
        &router,
        &format!("/api/albums/{album_id}/changes"),
        serde_json::json!({
            "operation": "delete",
            "ifVersion": removed_version
        }),
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::OK);
    assert_eq!(
        response_json(deleted).await,
        serde_json::json!({
            "albumId": album_id,
            "deleted": true,
            "originalFilesChanged": false
        })
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn cli_album_routes_reject_unnegotiated_unbounded_and_open_object_input() {
    let (base, config) = prepare_fixture();
    jpeg_fixture(&config.library_root.join("a.jpg"), 8, 4, [32, 64, 192]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .remove(0);
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let created = response_json(
        post_cli_json(
            &router,
            "/api/albums",
            serde_json::json!({"name": "Bounded"}),
        )
        .await,
    )
    .await;
    let album_id = created["album"]["id"].as_str().unwrap().to_owned();
    let version = created["album"]["albumVersion"]
        .as_str()
        .unwrap()
        .to_owned();

    let unnegotiated = post_json(
        &router,
        &format!("/api/albums/{album_id}/changes"),
        serde_json::json!({
            "operation": "add",
            "photoIds": [photo_id],
            "ifVersion": version
        }),
        None,
    )
    .await;
    assert_eq!(unnegotiated.status(), StatusCode::UPGRADE_REQUIRED);

    for body in [
        r#"{"name":"Unknown","extra":true}"#,
        r#"{"name":"First","name":"Second"}"#,
    ] {
        let rejected = send(
            &router,
            authenticated_request()
                .method("POST")
                .uri("https://camera.local/api/albums")
                .header("Slipstream-CLI-Contract", "1")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response_json(rejected).await["error"]["code"],
            "invalid_input"
        );
    }

    let unknown_key = post_cli_json(
        &router,
        &format!("/api/albums/{album_id}/changes"),
        serde_json::json!({
            "operation": "add",
            "photoIds": [photo_id],
            "ifVersion": version,
            "force": true
        }),
    )
    .await;
    assert_eq!(unknown_key.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(unknown_key).await["error"]["code"],
        "invalid_input"
    );

    let duplicate_key = send(
        &router,
        authenticated_request()
            .method("POST")
            .uri(format!(
                "https://camera.local/api/albums/{album_id}/changes"
            ))
            .header("Slipstream-CLI-Contract", "1")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(format!(
                r#"{{"operation":"add","photoIds":["{photo_id}"],"photoIds":["{photo_id}"],"ifVersion":"{version}"}}"#
            )))
            .unwrap(),
    )
    .await;
    assert_eq!(duplicate_key.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(duplicate_key).await["error"]["code"],
        "invalid_input"
    );

    let duplicate_ids = post_cli_json(
        &router,
        &format!("/api/albums/{album_id}/changes"),
        serde_json::json!({
            "operation": "add",
            "photoIds": [photo_id, photo_id],
            "ifVersion": version
        }),
    )
    .await;
    assert_eq!(duplicate_ids.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(duplicate_ids).await["error"]["details"]["argument"],
        "photoIds"
    );

    let too_many_ids = (0..=slipstream_core::ALBUM_MEMBERSHIP_BATCH_MAX)
        .map(|index| format!("00000000-0000-4000-8000-{index:012x}"))
        .collect::<Vec<_>>();
    let over_limit = post_cli_json(
        &router,
        &format!("/api/albums/{album_id}/changes"),
        serde_json::json!({
            "operation": "reorder",
            "photoIds": too_many_ids,
            "ifVersion": version
        }),
    )
    .await;
    assert_eq!(over_limit.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        response_json(over_limit).await["error"],
        serde_json::json!({
            "code": "limit_exceeded",
            "message": "Reduce the Photo ID list and try again.",
            "effect": "none",
            "details": {
                "limitName": "albumReorderMembersMaximum",
                "limit": 100,
                "actual": 101
            }
        })
    );

    let unchanged = application.library.album(&album_id).await.unwrap().unwrap();
    assert_eq!(unchanged.photo_count, 0);
    assert_eq!(unchanged.album_version, version);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn cli_photo_decisions_route_maps_checked_outcomes_and_partitions_batches() {
    let (base, config) = prepare_fixture();
    for name in ["a.jpg", "b.jpg", "c.jpg"] {
        jpeg_fixture(&config.library_root.join(name), 8, 4, [32, 64, 192]);
    }
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let photo_ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let (a, b, c) = (&photo_ids[0], &photo_ids[1], &photo_ids[2]);
    let router = create_router(Arc::clone(&application), config.web_root());

    // A single-field change reports the exact prior and current decision
    // objects and becomes visible to Web browsing.
    let initial = cli_photo_read(&router, a).await;
    let initial_version = initial["decisionVersion"].as_str().unwrap().to_owned();
    assert_eq!(initial["selectionState"], "undecided");
    assert_eq!(initial["rating"], 0);
    let changed = post_cli_json(
        &router,
        "/api/photo-decisions",
        serde_json::json!({
            "field": "selectionState",
            "value": "selected",
            "photos": [{"photoId": a, "ifVersion": initial_version}]
        }),
    )
    .await;
    assert_eq!(changed.status(), StatusCode::OK);
    let changed = response_json(changed).await;
    let selected_version = changed["results"][0]["current"]["decisionVersion"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(selected_version, initial_version);
    assert_eq!(changed["results"][0]["photoId"], *a);
    assert_eq!(changed["results"][0]["outcome"], "changed");
    assert_eq!(
        changed["results"][0]["prior"],
        serde_json::json!({"selectionState": "undecided", "rating": 0})
    );
    assert_eq!(
        changed["results"][0]["current"],
        serde_json::json!({
            "selectionState": "selected",
            "rating": 0,
            "decisionVersion": selected_version
        })
    );
    assert_eq!(
        changed["results"][0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["photoId", "outcome", "prior", "current"])
    );
    assert_eq!(
        changed["counts"],
        serde_json::json!({"changed": 1, "unchanged": 0, "conflict": 0, "missing": 0})
    );
    let web_summary = published_photo_summary(&application, a).await;
    assert_eq!(web_summary.selection_state, "selected");
    assert_eq!(web_summary.rating, 0);

    // A Rating change leaves Selection State untouched, and a repeated value
    // with the current version is unchanged without advancing the version.
    let rated = response_json(
        post_cli_json(
            &router,
            "/api/photo-decisions",
            serde_json::json!({
                "field": "rating",
                "value": 3,
                "photos": [{"photoId": a, "ifVersion": selected_version}]
            }),
        )
        .await,
    )
    .await;
    let rated_version = rated["results"][0]["current"]["decisionVersion"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(rated["results"][0]["outcome"], "changed");
    assert_eq!(
        rated["results"][0]["prior"],
        serde_json::json!({"selectionState": "selected", "rating": 0})
    );
    assert_eq!(rated["results"][0]["current"]["selectionState"], "selected");
    assert_eq!(rated["results"][0]["current"]["rating"], 3);
    let no_op = response_json(
        post_cli_json(
            &router,
            "/api/photo-decisions",
            serde_json::json!({
                "field": "rating",
                "value": 3,
                "photos": [{"photoId": a, "ifVersion": rated_version}]
            }),
        )
        .await,
    )
    .await;
    assert_eq!(no_op["results"][0]["outcome"], "unchanged");
    assert_eq!(no_op["results"][0]["current"]["rating"], 3);
    assert_eq!(
        no_op["results"][0]["current"]["decisionVersion"],
        rated_version
    );
    assert_eq!(no_op["counts"]["unchanged"], 1);
    assert_eq!(
        no_op["results"][0]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["photoId", "outcome", "current"])
    );

    // Web edits between a CLI read and write conflict, including edits that
    // change the guarded value away and back.
    let stale = cli_photo_read(&router, b).await;
    let stale_version = stale["decisionVersion"].as_str().unwrap().to_owned();
    for value in [4, 0] {
        let web = post_json(
            &router,
            &format!("/api/photos/{b}/state"),
            serde_json::json!({"field": "rating", "value": value}),
            None,
        )
        .await;
        assert_eq!(web.status(), StatusCode::OK);
    }
    let away_and_back = post_cli_json(
        &router,
        "/api/photo-decisions",
        serde_json::json!({
            "field": "rating",
            "value": 0,
            "photos": [{"photoId": b, "ifVersion": stale_version}]
        }),
    )
    .await;
    assert_eq!(away_and_back.status(), StatusCode::OK);
    let away_and_back = response_json(away_and_back).await;
    assert_eq!(away_and_back["results"][0]["outcome"], "conflict");
    assert_eq!(away_and_back["results"][0]["current"]["rating"], 0);
    let b_version = away_and_back["results"][0]["current"]["decisionVersion"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(b_version, stale_version);
    let after_conflict = cli_photo_read(&router, b).await;
    assert_eq!(after_conflict["rating"], 0);
    assert_eq!(after_conflict["decisionVersion"], b_version);

    // A mixed batch partitions every requested Photo in request order and
    // leaves Album facts and the saved browsing position untouched.
    let album = response_json(
        post_cli_json(
            &router,
            "/api/albums",
            serde_json::json!({"name": "Resume"}),
        )
        .await,
    )
    .await;
    let album_id = album["album"]["id"].as_str().unwrap().to_owned();
    let add_version = album["album"]["albumVersion"].as_str().unwrap().to_owned();
    let added = post_cli_json(
        &router,
        &format!("/api/albums/{album_id}/changes"),
        serde_json::json!({
            "operation": "add",
            "photoIds": [a, c],
            "ifVersion": add_version
        }),
    )
    .await;
    assert_eq!(added.status(), StatusCode::OK);
    let added_version = response_json(added).await["album"]["albumVersion"]
        .as_str()
        .unwrap()
        .to_owned();
    let progress = post_json(
        &router,
        &format!("/api/albums/{album_id}/progress"),
        serde_json::json!({"photoId": a}),
        None,
    )
    .await;
    assert_eq!(progress.status(), StatusCode::OK);

    let c_initial = cli_photo_read(&router, c).await;
    let c_initial_version = c_initial["decisionVersion"].as_str().unwrap().to_owned();
    let rejected = response_json(
        post_cli_json(
            &router,
            "/api/photo-decisions",
            serde_json::json!({
                "field": "selectionState",
                "value": "rejected",
                "photos": [{"photoId": c, "ifVersion": c_initial_version}]
            }),
        )
        .await,
    )
    .await;
    assert_eq!(rejected["results"][0]["outcome"], "changed");
    let c_version = rejected["results"][0]["current"]["decisionVersion"]
        .as_str()
        .unwrap()
        .to_owned();

    let missing_id = "00000000-0000-4000-8000-00000000dead";
    let mixed = response_json(
        post_cli_json(
            &router,
            "/api/photo-decisions",
            serde_json::json!({
                "field": "selectionState",
                "value": "rejected",
                "photos": [
                    {"photoId": a, "ifVersion": rated_version},
                    {"photoId": b, "ifVersion": stale_version},
                    {"photoId": missing_id, "ifVersion": stale_version},
                    {"photoId": c, "ifVersion": c_version}
                ]
            }),
        )
        .await,
    )
    .await;
    let results = mixed["results"].as_array().unwrap();
    assert_eq!(results.len(), 4);
    assert_eq!(results[0]["photoId"], *a);
    assert_eq!(results[0]["outcome"], "changed");
    assert_eq!(results[1]["photoId"], *b);
    assert_eq!(results[1]["outcome"], "conflict");
    assert_eq!(results[2]["photoId"], missing_id);
    assert_eq!(results[2]["outcome"], "missing");
    assert_eq!(
        results[2]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["photoId", "outcome"])
    );
    assert_eq!(results[3]["photoId"], *c);
    assert_eq!(results[3]["outcome"], "unchanged");
    assert_eq!(
        mixed["counts"],
        serde_json::json!({"changed": 1, "unchanged": 1, "conflict": 1, "missing": 1})
    );
    let album_after = application.library.album(&album_id).await.unwrap().unwrap();
    assert_eq!(album_after.album_version, added_version);
    assert!(album_after.has_saved_position);

    // A closed Library reports the confirmed storage failure shape.
    application.shutdown().await.unwrap();
    let closed = post_cli_json(
        &router,
        "/api/photo-decisions",
        serde_json::json!({
            "field": "selectionState",
            "value": "selected",
            "photos": [{"photoId": a, "ifVersion": rated_version}]
        }),
    )
    .await;
    assert_eq!(closed.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response_json(closed).await["error"],
        serde_json::json!({
            "code": "storage_failed",
            "message": "Inspect server health and the current Photo decisions before trying again.",
            "effect": "none",
            "details": {"operation": "photos-set"}
        })
    );
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn cli_photo_decisions_route_rejects_unnegotiated_open_and_over_limit_input() {
    let (base, config) = prepare_fixture();
    jpeg_fixture(&config.library_root.join("one.jpg"), 8, 4, [32, 64, 192]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .remove(0);
    let before = application.library.photo(&photo_id).await.unwrap().unwrap();
    let router = create_router(Arc::clone(&application), config.web_root());

    let unnegotiated = post_json(
        &router,
        "/api/photo-decisions",
        serde_json::json!({
            "field": "rating",
            "value": 4,
            "photos": [{"photoId": photo_id, "ifVersion": before.decision_version}]
        }),
        None,
    )
    .await;
    assert_eq!(unnegotiated.status(), StatusCode::UPGRADE_REQUIRED);

    let version = before.decision_version.as_str();
    for body in [
        r#"not json"#,
        r#"{"field":"rating","value":4}"#,
        r#"{"field":"rating","photos":[{"photoId":"00000000-0000-4000-8000-000000000001","ifVersion":"v"}]}"#,
        r#"{"field":"rating","value":4,"photos":[],"unexpected":1}"#,
        r#"{"field":"rating","value":4,"value":3,"photos":[]}"#,
        r#"{"field":"rating","value":4,"photos":[{"photoId":"00000000-0000-4000-8000-000000000001","ifVersion":"v","force":true}]}"#,
        r#"{"field":"rating","value":4,"photos":[{"photoId":"00000000-0000-4000-8000-000000000001","photoId":"00000000-0000-4000-8000-000000000001","ifVersion":"v"}]}"#,
        r#"{"field":"rating","value":4,"photos":[{"ifVersion":"v"}]}"#,
        r#"{"field":"rating","value":4,"photos":"one.jpg"}"#,
    ] {
        let rejected = send(
            &router,
            authenticated_request()
                .method("POST")
                .uri("http://camera.local/api/photo-decisions")
                .header("Slipstream-CLI-Contract", "1")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await;
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response_json(rejected).await["error"]["code"],
            "invalid_input"
        );
    }

    for (body, argument) in [
        (
            serde_json::json!({
                "field": "favorite",
                "value": true,
                "photos": [{"photoId": photo_id, "ifVersion": version}]
            }),
            "field",
        ),
        (
            serde_json::json!({
                "field": "rating",
                "value": "4",
                "photos": [{"photoId": photo_id, "ifVersion": version}]
            }),
            "value",
        ),
        (
            serde_json::json!({
                "field": "rating",
                "value": 6,
                "photos": [{"photoId": photo_id, "ifVersion": version}]
            }),
            "value",
        ),
        (
            serde_json::json!({
                "field": "rating",
                "value": 4.5,
                "photos": [{"photoId": photo_id, "ifVersion": version}]
            }),
            "value",
        ),
        (
            serde_json::json!({
                "field": "selectionState",
                "value": 1,
                "photos": [{"photoId": photo_id, "ifVersion": version}]
            }),
            "value",
        ),
        (
            serde_json::json!({
                "field": "selectionState",
                "value": "picked",
                "photos": [{"photoId": photo_id, "ifVersion": version}]
            }),
            "value",
        ),
        (
            serde_json::json!({"field": "rating", "value": 4, "photos": []}),
            "photos",
        ),
        (
            serde_json::json!({
                "field": "rating",
                "value": 4,
                "photos": [
                    {"photoId": photo_id, "ifVersion": version},
                    {"photoId": photo_id, "ifVersion": version}
                ]
            }),
            "photos",
        ),
        (
            serde_json::json!({
                "field": "rating",
                "value": 4,
                "photos": [{"photoId": "not-an-id", "ifVersion": version}]
            }),
            "photos",
        ),
        (
            serde_json::json!({
                "field": "rating",
                "value": 4,
                "photos": [{"photoId": photo_id, "ifVersion": ""}]
            }),
            "photos",
        ),
    ] {
        let rejected = post_cli_json(&router, "/api/photo-decisions", body).await;
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
        let error = response_json(rejected).await["error"].clone();
        assert_eq!(error["code"], "invalid_input");
        assert_eq!(error["details"]["argument"], argument);
    }

    let too_many = (0..=slipstream_core::PHOTO_STATE_BATCH_MAX)
        .map(|index| {
            serde_json::json!({
                "photoId": format!("00000000-0000-4000-8000-{index:012x}"),
                "ifVersion": "v"
            })
        })
        .collect::<Vec<_>>();
    let over_limit = post_cli_json(
        &router,
        "/api/photo-decisions",
        serde_json::json!({"field": "rating", "value": 4, "photos": too_many}),
    )
    .await;
    assert_eq!(over_limit.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        response_json(over_limit).await["error"],
        serde_json::json!({
            "code": "limit_exceeded",
            "message": "Reduce the Photo ID list and try again.",
            "effect": "none",
            "details": {"limitName": "photoIds", "limit": 100, "actual": 101}
        })
    );

    let oversized = send(
        &router,
        authenticated_request()
            .method("POST")
            .uri("http://camera.local/api/photo-decisions")
            .header("Slipstream-CLI-Contract", "1")
            .header(header::CONTENT_TYPE, "application/json")
            .header(
                header::CONTENT_LENGTH,
                (MAXIMUM_MUTATION_BODY_BYTES + 1).to_string(),
            )
            .body(Body::from(vec![b'x'; 16]))
            .unwrap(),
    )
    .await;
    assert_eq!(oversized.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        response_json(oversized).await["error"],
        serde_json::json!({
            "code": "limit_exceeded",
            "message": "Reduce the request body and try again.",
            "effect": "none",
            "details": {
                "limitName": "requestBodyBytesMaximum",
                "limit": MAXIMUM_MUTATION_BODY_BYTES,
                "actual": MAXIMUM_MUTATION_BODY_BYTES + 1
            }
        })
    );

    let after = application.library.photo(&photo_id).await.unwrap().unwrap();
    assert_eq!(after.rating, before.rating);
    assert_eq!(after.selection_state, before.selection_state);
    assert_eq!(after.decision_version, before.decision_version);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn cli_read_routes_execute_exact_query_and_continuation_shapes() {
    let (base, config) = prepare_fixture();
    for index in 0..5 {
        let folder = if index < 3 { "first" } else { "second" };
        fs::create_dir_all(config.library_root.join(folder)).unwrap();
        jpeg_fixture_with_capture_time(
            &config
                .library_root
                .join(folder)
                .join(format!("{index}.JPG")),
            32,
            24,
            [index as u8 * 20, 64, 128],
            &format!("2026:01:01 10:00:0{index}"),
        );
    }
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    assert_eq!(ids.len(), 5);
    let first_album = application
        .mutate_album(slipstream_core::AlbumMutation::Create {
            name: "First".to_owned(),
        })
        .await
        .unwrap()
        .albums[0]
        .id
        .clone();
    let second_album = application
        .mutate_album(slipstream_core::AlbumMutation::Create {
            name: "Second".to_owned(),
        })
        .await
        .unwrap()
        .albums
        .into_iter()
        .find(|album| album.name == "Second")
        .unwrap()
        .id;
    application
        .add_album_members(&first_album, vec![ids[0].clone(), ids[1].clone()])
        .await
        .unwrap();
    let router = authorized_router(Arc::clone(&application), config.web_root());

    let year_zero = send(
        &router,
        authenticated_request()
            .method("POST")
            .uri("https://camera.local/api/photo-queries")
            .header("Slipstream-CLI-Contract", "1")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"capturedFrom":"0000-01-01T00:00:00"}"#))
            .unwrap(),
    )
    .await;
    assert_eq!(year_zero.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(year_zero).await["error"]["code"],
        "invalid_input"
    );

    let incompatible = send(
        &router,
        authenticated_request()
            .uri("https://camera.local/api/capabilities")
            .header("Slipstream-CLI-Contract", "2")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(incompatible.status(), StatusCode::UPGRADE_REQUIRED);
    assert_eq!(
        response_json(incompatible).await,
        serde_json::json!({
            "error": {
                "code": "incompatible_server",
                "message": "The server does not support the requested CLI contract; use a compatible client or server.",
                "effect": "none",
                "details": {
                    "requestedContractVersion": 2,
                    "supportedContractVersions": [1]
                }
            }
        })
    );

    let capabilities = response_json(
        send(
            &router,
            authenticated_request()
                .uri("https://camera.local/api/capabilities")
                .header("Slipstream-CLI-Contract", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(
        capabilities,
        serde_json::json!({
            "serverVersion": "0.0.0",
            "supportedCliContractVersions": [1],
            "limits": {
                "listPageMaximum": 60,
                "mutationPhotoIdsMaximum": 100,
                "albumReorderMembersMaximum": 100,
                "removalPhotoIdsMaximum": 100,
                "retainedQueryIdsMaximum": 1_000_000,
                "retainedQueryIdleSeconds": 900,
                "recoveryPageMaximum": 60,
                "recoveryMappingsMaximum": 10_000,
                "recoveryApplyMaximum": 100,
                "recoveryReviewIdleSeconds": 900
            }
        })
    );
    let processing = response_json(
        send(
            &router,
            authenticated_request()
                .uri("https://camera.local/api/processing/capability")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(
        processing,
        serde_json::json!({
            "state": "disabled",
            "bundleId": null,
            "incarnation": null,
            "exposure": {"minimumEv": 0.0, "maximumEv": 1.0, "stepEv": 0.001},
            "profiles": [
                {
                    "profileId": "sony-ilce-7rm5-arw",
                    "whiteBalanceModes": ["as-shot"],
                    "whiteBalanceRanges": null
                },
                {
                    "profileId": "sony-ilce-7cm2-arw",
                    "whiteBalanceModes": ["as-shot"],
                    "whiteBalanceRanges": null
                }
            ],
            "stages": {"develop": "unavailable", "film": "unavailable"}
        })
    );
    let configured_router = crate::http::create_router_with_processing(
        Arc::clone(&application),
        open_web_root(config.web_root()),
        Some(unresolved_processing_config()),
    );
    let opted_in = response_json(
        send(
            &configured_router,
            authenticated_request()
                .uri("https://camera.local/api/processing/capability")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(
        opted_in,
        serde_json::json!({
            "bundleId": null,
            "state": "bundle-unavailable",
            "incarnation": null,
            "exposure": {"minimumEv": 0.0, "maximumEv": 1.0, "stepEv": 0.001},
            "profiles": [
                {
                    "profileId": "sony-ilce-7rm5-arw",
                    "whiteBalanceModes": ["as-shot"],
                    "whiteBalanceRanges": null
                },
                {
                    "profileId": "sony-ilce-7cm2-arw",
                    "whiteBalanceModes": ["as-shot"],
                    "whiteBalanceRanges": null
                }
            ],
            "stages": {"develop": "unavailable", "film": "unavailable"}
        })
    );
    let health = send(
        &configured_router,
        authenticated_request()
            .uri("https://camera.local/healthz")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(health.status(), StatusCode::OK);
    let status = response_json(
        send(
            &router,
            authenticated_request()
                .uri("https://camera.local/api/status")
                .header("Slipstream-CLI-Contract", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(status["serverVersion"], "0.0.0");
    assert_eq!(status["cliContractVersion"], 1);
    assert_eq!(status["published"], true);
    assert_eq!(status["photoCount"], 5);
    assert_eq!(status["scan"]["state"], "idle");
    assert!(status["scan"].get("lastRecovery").is_some());
    assert!(status["scan"].get("fingerprints").is_some());

    let album_page = response_json(
        send(
            &router,
            authenticated_request()
                .uri("https://camera.local/api/album-summaries?limit=1")
                .header("Slipstream-CLI-Contract", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(album_page["total"], 2);
    assert_eq!(album_page["items"].as_array().unwrap().len(), 1);
    assert!(album_page["nextCursor"].is_string());
    assert!(album_page["evaluatedAt"].as_str().unwrap().ends_with('Z'));
    assert!(album_page["expiresAt"].as_str().unwrap().ends_with('Z'));
    let first_summary = &album_page["items"][0];
    assert_eq!(
        first_summary
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "albumVersion".to_owned(),
            "hasSavedPosition".to_owned(),
            "id".to_owned(),
            "name".to_owned(),
            "photoCount".to_owned(),
            "webPath".to_owned(),
        ])
    );
    let retained_album = first_summary["id"].as_str().unwrap().to_owned();
    let deleted_album = if retained_album == first_album {
        second_album.clone()
    } else {
        first_album.clone()
    };
    let album_cursor = album_page["nextCursor"].as_str().unwrap();
    application
        .mutate_album(slipstream_core::AlbumMutation::Delete {
            album_id: deleted_album.clone(),
        })
        .await
        .unwrap();
    let album_page_two = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/album-summaries?cursor={album_cursor}"
                ))
                .header("Slipstream-CLI-Contract", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(album_page_two["total"], 2);
    assert_eq!(album_page_two["nextCursor"], Value::Null);
    assert_eq!(album_page_two["expiresAt"], Value::Null);
    assert_eq!(
        album_page_two["items"][0],
        serde_json::json!({"id": deleted_album, "state": "missing"})
    );
    let returned_album_ids = [
        album_page["items"][0]["id"].as_str().unwrap(),
        album_page_two["items"][0]["id"].as_str().unwrap(),
    ]
    .into_iter()
    .collect::<BTreeSet<_>>();
    assert_eq!(
        returned_album_ids,
        BTreeSet::from([first_album.as_str(), second_album.as_str()])
    );

    let photo_page = response_json(
        send(
            &router,
            authenticated_request()
                .method("POST")
                .uri("https://camera.local/api/photo-queries")
                .header("Slipstream-CLI-Contract", "1")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "source": {"kind": "all"},
                        "selection": "all",
                        "ratingMinimum": 0,
                        "ratingMaximum": 5,
                        "kind": "jpeg",
                        "available": true,
                        "capturedFrom": "2026-01-01T10:00:00",
                        "capturedBefore": "2026-01-01T10:01:00",
                        "order": "capture-time-asc",
                        "limit": 2
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(photo_page["total"], 5);
    assert_eq!(photo_page["items"].as_array().unwrap().len(), 2);
    assert!(photo_page["nextCursor"].is_string());
    let item = &photo_page["items"][0];
    assert_eq!(
        item.as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "captureTime".to_owned(),
            "decisionVersion".to_owned(),
            "filename".to_owned(),
            "hasSavedEdits".to_owned(),
            "id".to_owned(),
            "location".to_owned(),
            "originalAvailable".to_owned(),
            "originalId".to_owned(),
            "originalKind".to_owned(),
            "preview".to_owned(),
            "rating".to_owned(),
            "removedAtMs".to_owned(),
            "selectionState".to_owned(),
            "webPath".to_owned(),
        ])
    );
    assert_eq!(
        item["preview"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "detailLimited".to_owned(),
            "height".to_owned(),
            "source".to_owned(),
            "sourceRevision".to_owned(),
            "state".to_owned(),
            "width".to_owned(),
        ])
    );

    application
        .mutate_photo_state(slipstream_core::PhotoStateMutation {
            photo_id: ids[2].clone(),
            field: slipstream_core::PhotoStateField::Rating,
            value: slipstream_core::PhotoStateValue::Rating(4),
            expected_current: None,
            album_id: None,
        })
        .await
        .unwrap();
    let photo_cursor = photo_page["nextCursor"].as_str().unwrap();
    let photo_page_two = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photo-queries/{photo_cursor}"
                ))
                .header("Slipstream-CLI-Contract", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(photo_page_two["items"][0]["id"], ids[2]);
    assert_eq!(photo_page_two["items"][0]["rating"], 4);
    let photo_cursor_two = photo_page_two["nextCursor"].as_str().unwrap();
    application.rescan().await.unwrap();
    let photo_page_three = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photo-queries/{photo_cursor_two}"
                ))
                .header("Slipstream-CLI-Contract", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    let traversed = photo_page["items"]
        .as_array()
        .unwrap()
        .iter()
        .chain(photo_page_two["items"].as_array().unwrap())
        .chain(photo_page_three["items"].as_array().unwrap())
        .map(|item| item["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        traversed,
        ids.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert_eq!(traversed.iter().copied().collect::<BTreeSet<_>>().len(), 5);
    assert_eq!(photo_page_three["nextCursor"], Value::Null);
    assert_eq!(photo_page_three["expiresAt"], Value::Null);

    let direct = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!("https://camera.local/api/photos/{}", ids[2]))
                .header("Slipstream-CLI-Contract", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(direct["id"], ids[2]);
    assert_eq!(direct["rating"], 4);
    assert_eq!(direct["metadata"]["state"], "known");
    assert_eq!(
        direct["metadata"]["captureTime"],
        "2026-01-01T10:00:02.000000000"
    );

    let direct_album = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!("https://camera.local/api/albums/{retained_album}"))
                .header("Slipstream-CLI-Contract", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(direct_album["id"], retained_album);
    assert!(direct_album["photoCount"].as_u64().is_some());
    assert!(direct_album["albumVersion"].as_str().unwrap().len() > 20);

    let missing_id = "00000000-0000-4000-8000-000000000099";
    let missing_token = "test-missing-photo";
    let evaluated_at = SystemTime::now();
    application
        .retained_queries
        .lock()
        .unwrap()
        .insert(
            missing_token.to_owned(),
            crate::queries::RetainedKind::Photo,
            None,
            vec![missing_id.to_owned()],
            Instant::now(),
            evaluated_at,
        )
        .unwrap();
    let missing_cursor = application.cursor_signer.query_cursor(
        application.browse_namespace,
        crate::queries::RetainedKind::Photo,
        missing_token,
        0,
        1,
    );
    let missing_page = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photo-queries/{missing_cursor}"
                ))
                .header("Slipstream-CLI-Contract", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(missing_page["total"], 1);
    assert_eq!(
        missing_page["items"],
        serde_json::json!([{"id": missing_id, "state": "missing"}])
    );
    assert_eq!(missing_page["nextCursor"], Value::Null);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn cli_folder_cursor_maps_publication_replacement_and_query_expiry() {
    let (base, config) = prepare_fixture();
    for folder in ["a", "b"] {
        fs::create_dir_all(config.library_root.join(folder)).unwrap();
        jpeg_fixture(
            &config.library_root.join(folder).join("photo.JPG"),
            32,
            24,
            [32, 64, 128],
        );
    }
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let folders = response_json(
        send(
            &router,
            authenticated_request()
                .uri("https://camera.local/api/file-locations?limit=1")
                .header("Slipstream-CLI-Contract", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(folders["total"], 2);
    assert_eq!(folders["items"].as_array().unwrap().len(), 1);
    assert_eq!(folders["parent"], "");
    assert_eq!(folders["expiresAt"], Value::Null);
    let folder_cursor = folders["nextCursor"].as_str().unwrap().to_owned();
    application.rescan().await.unwrap();
    let expired_folder = send(
        &router,
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/file-locations?cursor={folder_cursor}"
            ))
            .header("Slipstream-CLI-Contract", "1")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(expired_folder.status(), StatusCode::GONE);
    assert_eq!(
        response_json(expired_folder).await["error"]["details"],
        serde_json::json!({"cursorKind": "folder", "reason": "publication_replaced"})
    );

    let query = response_json(
        send(
            &router,
            authenticated_request()
                .method("POST")
                .uri("https://camera.local/api/photo-queries")
                .header("Slipstream-CLI-Contract", "1")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(r#"{"limit":1}"#))
                .unwrap(),
        )
        .await,
    )
    .await;
    let cursor = query["nextCursor"].as_str().unwrap().to_owned();
    {
        let mut retained = application.retained_queries.lock().unwrap();
        let photo = retained
            .entries
            .values_mut()
            .find(|query| query.kind == crate::queries::RetainedKind::Photo)
            .unwrap();
        photo.last_used -= crate::queries::QUERY_IDLE + Duration::from_secs(1);
    }
    let expired_query = send(
        &router,
        authenticated_request()
            .uri(format!("https://camera.local/api/photo-queries/{cursor}"))
            .header("Slipstream-CLI-Contract", "1")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(expired_query.status(), StatusCode::GONE);
    assert_eq!(
        response_json(expired_query).await["error"]["details"],
        serde_json::json!({"cursorKind": "photo", "reason": "idle_or_evicted"})
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// Decodes one hexadecimal header value so a test can compare the repeated
/// `sourceRevision` with the revision the Preview metadata was admitted with.
fn decode_repeated_revision(value: &str) -> String {
    let mut bytes = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().chunks_exact(2) {
        let digit = |byte: u8| (byte as char).to_digit(16).expect("hexadecimal byte") as u8;
        bytes.push(digit(pair[0]) * 16 + digit(pair[1]));
    }
    String::from_utf8(bytes).expect("repeated revision is valid UTF-8")
}

/// The published Original Location facts for one Photo, which own the
/// `sourceRevision` every current Preview request is admitted against.
async fn published_original_facts_for(
    application: &Application,
    relative_path: &str,
) -> (String, u64, f64) {
    let snapshot = application.library.snapshot().await.unwrap();
    let original = snapshot
        .originals
        .iter()
        .find(|original| original.relative_path.as_str() == relative_path)
        .expect("Original is published");
    (
        original.relative_path.as_str().to_owned(),
        original.facts.size,
        original.facts.mtime_ms,
    )
}

/// Ends-to-end over the CLI seam: admitted metadata, a repeat of the same
/// typed facts beside the JPEG bytes, and a refusal instead of bytes the
/// caller can no longer identify as current.
#[tokio::test]
async fn cli_preview_download_repeats_identity_and_refuses_stale_bytes() {
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
    let (location, size, mtime_ms) = published_original_facts_for(&application, "photo.jpg").await;
    let revision = source_revision(&location, size, mtime_ms).unwrap();

    let admitted = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{photo_id}/preview"
                ))
                .header("Slipstream-CLI-Contract", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(
        admitted,
        serde_json::json!({
            "photoId": photo_id,
            "state": "ready",
            "source": "jpeg-original",
            "sourceRevision": revision,
            "width": 90,
            "height": 45,
            "detailLimited": true,
            "url": format!(
                "/api/private/derivatives/{photo_id}/review/{}.jpg",
                admitted["url"]
                    .as_str()
                    .unwrap()
                    .rsplit('/')
                    .next()
                    .unwrap()
                    .trim_end_matches(".jpg")
            ),
            "webPath": format!("/?photoId={photo_id}"),
        })
    );
    let url = admitted["url"].as_str().unwrap().to_owned();

    // A contract version this server does not support is an incompatible CLI
    // on this route, not a silent fall through to the Web answer.
    let incompatible = send(
        &router,
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/photos/{photo_id}/preview"
            ))
            .header("Slipstream-CLI-Contract", "2")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(incompatible.status(), StatusCode::UPGRADE_REQUIRED);
    assert_eq!(
        response_json(incompatible).await["error"]["code"],
        "incompatible_server"
    );

    // The download repeats the admitted Photo, Source, revision, and dimensions
    // as typed metadata beside the JPEG bytes.
    let derivative = send(
        &router,
        authenticated_request()
            .uri(format!("https://camera.local{url}"))
            .header("Slipstream-CLI-Contract", "1")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(derivative.status(), StatusCode::OK);
    assert_eq!(derivative.headers()[header::CONTENT_TYPE], "image/jpeg");
    assert_eq!(
        derivative.headers()[crate::wire::PREVIEW_PHOTO_HEADER],
        photo_id.as_str()
    );
    assert_eq!(
        derivative.headers()[crate::wire::PREVIEW_SOURCE_HEADER],
        "jpeg-original"
    );
    assert_eq!(
        decode_repeated_revision(
            derivative.headers()[crate::wire::PREVIEW_REVISION_HEADER]
                .to_str()
                .unwrap()
        ),
        revision
    );
    assert_eq!(
        derivative.headers()[crate::wire::PREVIEW_WIDTH_HEADER],
        "90"
    );
    assert_eq!(
        derivative.headers()[crate::wire::PREVIEW_HEIGHT_HEADER],
        "45"
    );
    let bytes = axum::body::to_bytes(derivative.into_body(), 64 * 1024 * 1024)
        .await
        .unwrap();
    assert!(bytes.starts_with(&[0xff, 0xd8]));

    // The thumbnail target repeats the same identity beside its own bytes.
    let thumbnail = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{photo_id}/thumbnail"
                ))
                .header("Slipstream-CLI-Contract", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(thumbnail["state"], "ready");
    assert_eq!(thumbnail["source"], "jpeg-original");
    assert_eq!(thumbnail["sourceRevision"], revision);
    let thumbnail_url = thumbnail["url"].as_str().unwrap().to_owned();
    let thumbnail_delivery = send(
        &router,
        authenticated_request()
            .uri(format!("https://camera.local{thumbnail_url}"))
            .header("Slipstream-CLI-Contract", "1")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(thumbnail_delivery.status(), StatusCode::OK);
    assert_eq!(
        thumbnail_delivery.headers()[crate::wire::PREVIEW_PHOTO_HEADER],
        photo_id.as_str()
    );
    assert_eq!(
        decode_repeated_revision(
            thumbnail_delivery.headers()[crate::wire::PREVIEW_REVISION_HEADER]
                .to_str()
                .unwrap()
        ),
        revision
    );
    assert_eq!(
        thumbnail_delivery.headers()[crate::wire::PREVIEW_HEIGHT_HEADER],
        thumbnail["height"].to_string()
    );
    let thumbnail_bytes = axum::body::to_bytes(thumbnail_delivery.into_body(), 64 * 1024 * 1024)
        .await
        .unwrap();
    assert!(thumbnail_bytes.starts_with(&[0xff, 0xd8]));

    // A changed Source makes the Previously delivered derivative stale
    // evidence. The CLI is refused rather than served bytes it can no longer
    // identify as current, and it reports the not-ready state the Published
    // Library publishes for a changed source revision.
    std::thread::sleep(std::time::Duration::from_millis(10));
    fs::write(&original, b"malformed replacement").unwrap();
    send(
        &router,
        authenticated_request()
            .method("POST")
            .uri("https://camera.local/api/scan")
            .header(header::ORIGIN, "https://camera.local")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let refused = send(
        &router,
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/photos/{photo_id}/preview"
            ))
            .header("Slipstream-CLI-Contract", "1")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response_json(refused).await,
        serde_json::json!({
            "error": {
                "code": "preview_unavailable",
                "message": "Request the current Preview again for this Photo.",
                "effect": "none",
                "details": {"photoId": photo_id, "state": "inspection-pending"}
            }
        })
    );
    assert_eq!(
        send(
            &router,
            authenticated_request()
                .uri(format!("https://camera.local{url}"))
                .header("Slipstream-CLI-Contract", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(
            &router,
            authenticated_request()
                .uri(format!("https://camera.local{thumbnail_url}"))
                .header("Slipstream-CLI-Contract", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    // The Web answer for the same Photo keeps reporting its stale Preview
    // truth, and the Web derivative route still repeats no CLI metadata.
    let web_stale = response_json(
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
    assert_eq!(web_stale["state"], "ready");
    assert_eq!(web_stale["stale"], true);
    assert_eq!(web_stale["url"], url);
    let web_derivative = send(
        &router,
        authenticated_request()
            .uri(format!("https://camera.local{url}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(web_derivative.status(), StatusCode::OK);
    assert!(
        web_derivative
            .headers()
            .get(crate::wire::PREVIEW_PHOTO_HEADER)
            .is_none()
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A Photo whose Original is no longer present is reported as not ready with
/// its state, and an unknown Photo ID is a distinct missing failure.
#[tokio::test]
async fn cli_preview_download_reports_unavailable_and_missing_truthfully() {
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

    std::thread::sleep(std::time::Duration::from_millis(10));
    fs::remove_file(&original).unwrap();
    send(
        &router,
        authenticated_request()
            .method("POST")
            .uri("https://camera.local/api/scan")
            .header(header::ORIGIN, "https://camera.local")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let unavailable = send(
        &router,
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/photos/{photo_id}/preview"
            ))
            .header("Slipstream-CLI-Contract", "1")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response_json(unavailable).await,
        serde_json::json!({
            "error": {
                "code": "preview_unavailable",
                "message": "No allowed source can produce a current Preview for this Photo.",
                "effect": "none",
                "details": {"photoId": photo_id, "state": "unavailable"}
            }
        })
    );
    let unavailable_thumbnail = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{photo_id}/thumbnail"
                ))
                .header("Slipstream-CLI-Contract", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(
        unavailable_thumbnail["error"]["details"]["state"],
        "unavailable"
    );

    let missing_id = "0".repeat(36);
    let missing = send(
        &router,
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/photos/{missing_id}/preview"
            ))
            .header("Slipstream-CLI-Contract", "1")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response_json(missing).await,
        serde_json::json!({
            "error": {
                "code": "not_found",
                "message": "Query Photos and use a current Photo ID.",
                "effect": "none",
                "details": {"resource": "photo", "reference": missing_id}
            }
        })
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn unpublished_cli_preview_reports_library_status_before_photo_lookup() {
    let (base, config) = prepare_fixture();
    jpeg_fixture(
        &config.library_root.join("photo.jpg"),
        90,
        45,
        [192, 64, 32],
    );
    let (gate_sender, gate_receiver) = tokio::sync::oneshot::channel();
    let application =
        Application::open_with_gate(&config, ScanLimits::default(), Some(gate_receiver), None)
            .await
            .unwrap();
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let missing_id = "0".repeat(36);
    let key = "a".repeat(64);
    for path in [
        format!("/api/photos/{missing_id}/preview"),
        format!("/api/photos/{missing_id}/thumbnail"),
        format!("/api/private/derivatives/{missing_id}/review/{key}.jpg"),
    ] {
        let response = send(
            &router,
            authenticated_request()
                .uri(format!("https://camera.local{path}"))
                .header("Slipstream-CLI-Contract", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = response_json(response).await;
        assert_eq!(body["error"]["code"], "library_unavailable");
        assert_eq!(body["error"]["details"]["scan"]["state"], "initializing");
    }

    gate_sender.send(()).unwrap();
    wait_for_scan_runs(&application, 1).await;
    for target in ["preview", "thumbnail"] {
        let malformed = send(
            &router,
            authenticated_request()
                .uri(format!("https://camera.local/api/photos/bad/{target}"))
                .header("Slipstream-CLI-Contract", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response_json(malformed).await["error"]["code"],
            "invalid_input"
        );
        let missing = send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{missing_id}/{target}"
                ))
                .header("Slipstream-CLI-Contract", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(missing.status(), StatusCode::NOT_FOUND);
        assert_eq!(response_json(missing).await["error"]["code"], "not_found");
    }
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A dropped CLI check response cannot cancel an application-owned scan cycle,
/// and a later status query reports the service state, not the caller's fate.
#[tokio::test]
async fn cli_scan_check_reports_service_state_after_an_interrupted_request() {
    let (base, config) = prepare_fixture();
    jpeg_fixture(
        &config.library_root.join("photo.jpg"),
        90,
        45,
        [192, 64, 32],
    );
    let (gate_sender, gate_receiver) = tokio::sync::oneshot::channel();
    let application =
        Application::open_with_gate(&config, ScanLimits::default(), Some(gate_receiver), None)
            .await
            .unwrap();
    let router = authorized_router(Arc::clone(&application), config.web_root());
    assert_eq!(
        application.shared.runs_started.load(Ordering::Relaxed),
        0,
        "the startup scan is admitted before it runs"
    );
    assert_eq!(application.scan_status().state, "initializing");

    // The CLI check joins the parked application-owned cycle, then the caller
    // disconnects before any answer exists. The cycle only completes when the
    // application releases it, so the caller's departure is what interrupts it.
    let interrupted = tokio::time::timeout(
        std::time::Duration::from_millis(50),
        send(
            &router,
            authenticated_request()
                .method("POST")
                .uri("https://camera.local/api/scan")
                .header("Slipstream-CLI-Contract", "1")
                .header(header::ORIGIN, "https://camera.local")
                .body(Body::empty())
                .unwrap(),
        ),
    )
    .await;
    assert!(
        interrupted.is_err(),
        "no scan answer may reach an interrupted caller"
    );

    // The cycle is application-owned, so releasing the gate completes it.
    gate_sender.send(()).unwrap();
    wait_for_scan_runs(&application, 1).await;
    assert_eq!(
        application.shared.runs_started.load(Ordering::Relaxed),
        1,
        "the interrupted check joined the one application-owned cycle"
    );
    let status = response_json(
        send(
            &router,
            authenticated_request()
                .uri("https://camera.local/api/status")
                .header("Slipstream-CLI-Contract", "1")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(status["published"], true);
    assert_eq!(status["scan"]["state"], "idle");
    assert!(
        status["scan"]["updatedMs"]
            .as_u64()
            .is_some_and(|value| value > 0)
    );
    assert_eq!(status["photoCount"], 1);
    // A later check reports a terminal state of its own.
    let settled = response_json(
        send(
            &router,
            authenticated_request()
                .method("POST")
                .uri("https://camera.local/api/scan")
                .header("Slipstream-CLI-Contract", "1")
                .header(header::ORIGIN, "https://camera.local")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(settled["state"], "idle");
    assert_eq!(settled["completed"], 1);
    assert_eq!(settled["total"], 1);
    assert!(settled["updatedMs"].as_u64().is_some_and(|value| value > 0));
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
