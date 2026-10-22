use super::*;

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
