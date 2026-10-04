use super::*;

fn marker_complete_corrupt_jpeg(width: u16, height: u16) -> Vec<u8> {
    let mut bytes = vec![0xff, 0xd8, 0xff, 0xc0, 0x00, 0x11, 0x08];
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&[0; 11]);
    bytes.extend_from_slice(&[0xff, 0xda, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00]);
    bytes.extend_from_slice(&[0xff, 0xd9]);
    bytes
}

#[tokio::test]
async fn scan_warms_new_review_preview_without_foreground_request() {
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
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let facts = {
            let guard = application
                .shared
                .snapshot
                .read()
                .expect("published Library poisoned");
            let published = guard.as_ref().expect("published Library");
            let photo_position = published.photos_by_id[&photo_id];
            let photo = &published.snapshot.photos[photo_position];
            let original_position = published.originals_by_id[&photo.original_id];
            PreviewFacts::from_records(
                photo.clone(),
                vec![published.snapshot.originals[original_position].clone()],
            )
        };
        let cached = application
            .preview
            .lookup_current(&facts, DerivativeTarget::Review2560)
            .await
            .unwrap();
        let summary = published_photo_summary(&application, &photo_id).await;
        if cached.is_some() && summary.preview.state == "ready" {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "scan did not warm the review Preview"
        );
        tokio::task::yield_now().await;
        std::thread::sleep(Duration::from_millis(5));
    }
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn offline_proxy_develop_returns_and_reuses_current_jpeg() {
    use sha2::{Digest, Sha256};

    let (base, mut config) = prepare_fixture();
    let original = config.library_root.join("approved.ARW");
    let original_bytes = approved_raw_fixture(&original);
    config.processing = Some(ProcessingConfig {
        instance: "f".repeat(32),
        policy_sha256: "b".repeat(64),
        bundle_sha256: "c".repeat(64),
        socket_override: None,
    });
    config.export_retained_output_bytes = Some(1024 * 1024 * 1024);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .into_iter()
        .next()
        .unwrap();
    let revision = application
        .library
        .edit_recipe(&photo_id)
        .await
        .unwrap()
        .unwrap()
        .current_source_revision
        .expect("settled scan publishes the source revision");
    let frame = slipstream_core::derivative::development_tiff_fixture(&[0.25; 8 * 4 * 3], 8, 4);
    let record = slipstream_core::DevelopmentProxyRecord {
        photo_id: photo_id.clone(),
        source_revision: revision,
        source_relative_path: "approved.ARW".to_owned(),
        source_sha256: format!("{:x}", Sha256::digest(&original_bytes)),
        source_size: original_bytes.len() as u64,
        profile_id: "sony-ilce-7rm5-arw".to_owned(),
        pipeline_version: slipstream_core::DEVELOPMENT_PROXY_PIPELINE_VERSION.to_owned(),
        bundle_sha256: "c".repeat(64),
        long_edge: slipstream_core::DEVELOPMENT_PROXY_LONG_EDGE,
        width: 8,
        height: 4,
        artifact_sha256: format!("{:x}", Sha256::digest(&frame)),
        artifact_bytes: frame.len() as u64,
        created_at: 1_700_000_000,
    };
    let artifact_root = config.state_directory.join("development-proxies");
    fs::create_dir_all(&artifact_root).unwrap();
    fs::write(
        artifact_root.join(format!("{}.tiff", record.identity_digest())),
        frame,
    )
    .unwrap();
    assert!(
        application
            .library
            .record_development_proxy(record)
            .await
            .unwrap()
    );
    fs::remove_file(&original).unwrap();
    application.library.scan().await.unwrap();
    assert!(
        !application
            .library
            .edit_recipe(&photo_id)
            .await
            .unwrap()
            .unwrap()
            .source_available
    );
    assert!(
        application
            .proxies
            .as_ref()
            .unwrap()
            .current_record(&photo_id)
            .await
            .is_some()
    );
    let router = configured_router(&application, config.web_root());
    let uri = format!("https://camera.local/api/photos/{photo_id}/edit-preview/develop");
    let mut first_digest = None;
    for _ in 0..2 {
        let response = send(
            &router,
            authenticated_request()
                .uri(&uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        if response.status() != StatusCode::OK {
            panic!("proxy preview refused: {}", response_json(response).await);
        }
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/jpeg");
        assert_eq!(
            response.headers()["slipstream-edit-preview-source"],
            "development-proxy"
        );
        let bytes = axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        let digest = Sha256::digest(&bytes);
        assert!(bytes.starts_with(&[0xff, 0xd8]));
        if let Some(first) = first_digest {
            assert_eq!(first, digest);
        } else {
            first_digest = Some(digest);
        }
    }
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn proxy_removal_is_admitted_while_other_api_deletes_stay_refused() {
    let (base, mut config) = prepare_fixture();
    jpeg_fixture(
        &config.library_root.join("photo.jpg"),
        90,
        45,
        [192, 64, 32],
    );
    config.processing = Some(unresolved_processing_config());
    config.export_retained_output_bytes = Some(1024 * 1024 * 1024);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = configured_router(&application, config.web_root());
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .into_iter()
        .next()
        .unwrap();
    // The request policy admits DELETE on exactly the routes that declare it,
    // so the removal must reach its handler instead of being refused 405.
    let removed = send(
        &router,
        authenticated_request()
            .method("DELETE")
            .uri(format!(
                "https://camera.local/api/photos/{photo_id}/development-proxy"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(removed.status(), StatusCode::OK);
    assert_eq!(
        response_json(removed).await,
        serde_json::json!({"photoId": photo_id, "state": "absent", "removed": false})
    );
    for uri in [
        format!("https://camera.local/api/photos/{photo_id}/processing-recipe"),
        format!("https://camera.local/api/photos/{photo_id}"),
        "https://camera.local/api/photos".to_owned(),
    ] {
        let refused = send(
            &router,
            authenticated_request()
                .method("DELETE")
                .uri(&uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(refused.status(), StatusCode::METHOD_NOT_ALLOWED, "{uri}");
    }
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn preview_derivative_protocol_revalidates_source_and_reports_stale_truth() {
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
    assert_eq!(preview["stale"], false);
    let url = preview["url"].as_str().unwrap().to_owned();
    let key = url.rsplit('/').next().unwrap().trim_end_matches(".jpg");
    let summary = published_photo_summary(&application, &photo_id).await;
    assert_eq!(summary.preview.state, "ready");
    assert_eq!(summary.preview.url.as_deref(), Some(url.as_str()));
    let derivative = send(
        &router,
        authenticated_request()
            .uri(format!("https://camera.local{url}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(derivative.status(), StatusCode::OK);
    assert_eq!(derivative.headers()[header::CONTENT_TYPE], "image/jpeg");
    assert_eq!(
        derivative.headers()[header::CACHE_CONTROL],
        "private, max-age=3600, must-revalidate"
    );
    assert_eq!(derivative.headers()[header::VARY], "Cookie, Authorization");
    assert_eq!(derivative.headers()["x-content-type-options"], "nosniff");
    let etag = derivative.headers()[header::ETAG]
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(etag, format!("\"{key}\""));
    let body = axum::body::to_bytes(derivative.into_body(), 64 * 1024 * 1024)
        .await
        .unwrap();
    assert!(!body.is_empty());

    // A marker-complete but truncated derivative must be rejected and rebuilt
    // before the derivative route serves its bytes.
    let cache_path = application
        .preview
        .scheduler()
        .cache()
        .root()
        .join("rust-vips-v2")
        .join(format!("{key}.jpg"));
    fs::write(&cache_path, marker_complete_corrupt_jpeg(90, 45)).unwrap();
    let repaired = send(
        &router,
        authenticated_request()
            .uri(format!("https://camera.local{url}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(repaired.status(), StatusCode::OK);
    let repaired_body = axum::body::to_bytes(repaired.into_body(), 64 * 1024 * 1024)
        .await
        .unwrap();
    assert!(repaired_body.len() > marker_complete_corrupt_jpeg(90, 45).len());

    // The published cache hit does not need the Original to remain present.
    fs::remove_file(&original).unwrap();
    let cached = response_json(
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
    assert_eq!(cached["state"], "ready");
    assert_eq!(cached["url"], url);
    assert_eq!(
        send(
            &router,
            authenticated_request()
                .uri(format!("https://camera.local{url}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .status(),
        StatusCode::OK
    );
    jpeg_fixture(&original, 90, 45, [192, 64, 32]);
    let head = send(
        &router,
        authenticated_request()
            .method("HEAD")
            .uri(format!("https://camera.local{url}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(
        axum::body::to_bytes(head.into_body(), 1024)
            .await
            .unwrap()
            .len(),
        0
    );
    let not_modified = send(
        &router,
        authenticated_request()
            .uri(format!("https://camera.local{url}"))
            .header(header::IF_NONE_MATCH, etag)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        not_modified.headers()[header::CACHE_CONTROL],
        "private, max-age=3600, must-revalidate"
    );
    assert_eq!(
        not_modified.headers()[header::VARY],
        "Cookie, Authorization"
    );
    assert_eq!(
        axum::body::to_bytes(not_modified.into_body(), 1024)
            .await
            .unwrap()
            .len(),
        0
    );

    std::thread::sleep(std::time::Duration::from_millis(10));
    jpeg_fixture(&original, 120, 60, [32, 192, 64]);
    assert_eq!(
        send(
            &router,
            authenticated_request()
                .method("POST")
                .uri("https://camera.local/api/scan")
                .header(header::ORIGIN, "https://camera.local")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let changed = response_json(
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
    assert_eq!(changed["state"], "ready");
    let changed_url = changed["url"].as_str().unwrap();
    assert_ne!(changed_url, url);
    assert_eq!(
        send(
            &router,
            authenticated_request()
                .uri(format!("https://camera.local{url}"))
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
                .uri(format!("https://camera.local{changed_url}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .status(),
        StatusCode::OK
    );

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
    let stale = response_json(
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
    assert_eq!(stale["state"], "ready");
    assert_eq!(stale["stale"], true);
    assert_eq!(stale["source"], "jpeg-original");
    assert_eq!(stale["url"], changed_url);
    assert!(stale["message"].as_str().unwrap().contains("stale"));

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
    let unavailable = response_json(
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
    assert_eq!(unavailable["state"], "unavailable");
    assert!(
        unavailable["message"]
            .as_str()
            .unwrap()
            .contains("Original")
    );
    assert!(!unavailable.to_string().contains(base.to_str().unwrap()));
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn no_usable_source_seed_is_short_circuited_from_published_facts() {
    let (base, config) = prepare_fixture();
    let original = config.library_root.join("photo.jpg");
    fs::write(&original, b"not jpeg").unwrap();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .into_iter()
        .next()
        .unwrap();

    let first = response_json(
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
    assert_eq!(first["state"], "unavailable");
    assert_eq!(first["message"], "No usable camera-produced Preview");

    {
        let published = application
            .shared
            .snapshot
            .read()
            .expect("published Library poisoned");
        let published = published.as_ref().expect("Library is published");
        let position = published
            .photos_by_id
            .get(&photo_id)
            .copied()
            .expect("published Photo exists");
        let photo = published
            .snapshot
            .photos
            .get(position)
            .expect("published Photo position exists");
        assert_eq!(photo.preview_state, PreviewState::Unavailable);
        assert!(photo.preview_source_revision.is_some());
    }

    // The second request must use the durable seed without reopening the
    // Original. Removing it makes any accidental slow-path inspection visible
    // as a different Original-unavailable response.
    fs::remove_file(&original).unwrap();
    let second = response_json(
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
    assert_eq!(second["state"], "unavailable");
    assert_eq!(second["message"], first["message"]);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
