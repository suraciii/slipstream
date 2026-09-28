use super::*;

#[tokio::test]
async fn healthz_is_exact_json_and_head_api_has_no_body() {
    let (base, config) = prepare_fixture();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let response = tower::ServiceExt::oneshot(
        router.clone(),
        authenticated_request()
            .uri("https://camera.local/healthz")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    assert_eq!(
        axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap()
            .as_ref(),
        br#"{"status":"ok"}"#
    );
    let missing_web = base.join("missing-web");
    let missing_router = Router::new()
        .route(HEALTH_PATH, get(healthz))
        .with_state(HttpState {
            application: Arc::clone(&application),
            web_root: Arc::new(open_web_root(missing_web)),
            processing: None,
            edit_preview: Arc::new(crate::edit_preview::EditPreviewOwner::production(None)),
        });
    let response = tower::ServiceExt::oneshot(
        missing_router,
        authenticated_request()
            .uri("https://camera.local/healthz")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let response = tower::ServiceExt::oneshot(
        router,
        authenticated_request()
            .method("HEAD")
            .uri("https://camera.local/api/overview")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap()
            .len(),
        0
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn header_limit_rejects_only_values_over_sixteen_kib() {
    let (base, config) = prepare_fixture();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let exact = "x".repeat(
        MAXIMUM_HEADER_BYTES
            - "x-test".len()
            - "authorization".len()
            - 7
            - crate::access::TEST_TOKEN.len(),
    );
    let response = tower::ServiceExt::oneshot(
        router.clone(),
        authenticated_request()
            .uri("https://camera.local/api/overview")
            .header("x-test", exact)
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let over = "x".repeat(
        MAXIMUM_HEADER_BYTES
            - "x-test".len()
            - "authorization".len()
            - 7
            - crate::access::TEST_TOKEN.len()
            + 1,
    );
    let response = tower::ServiceExt::oneshot(
        router,
        authenticated_request()
            .uri("https://camera.local/api/overview")
            .header("x-test", over)
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn real_port_zero_server_is_ready_and_close_is_idempotent() {
    let (base, mut config) = prepare_fixture();
    config.port = 0;
    let server = start_server(config.clone()).await.unwrap();
    let address = server.url.strip_prefix("http://").unwrap().to_owned();
    let response = tokio::task::spawn_blocking(move || {
        let mut stream = std::net::TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        stream
            .write_all(b"GET /healthz HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut response = Vec::new();
        let mut chunk = [0_u8; 1024];
        loop {
            let read = stream.read(&mut chunk).unwrap();
            if read == 0 {
                break;
            }
            response.extend_from_slice(&chunk[..read]);
            let Some(headers_end) = response.windows(4).position(|window| window == b"\r\n\r\n")
            else {
                continue;
            };
            let headers_end = headers_end + 4;
            let content_length = response[..headers_end]
                .split(|byte| *byte == b'\n')
                .find_map(|line| {
                    line.strip_prefix(b"content-length:")
                        .or_else(|| line.strip_prefix(b"Content-Length:"))
                        .and_then(|value| std::str::from_utf8(value).ok())
                        .and_then(|value| value.trim().parse::<usize>().ok())
                })
                .expect("health response content length");
            if response.len() >= headers_end + content_length {
                break;
            }
        }
        String::from_utf8(response).unwrap()
    })
    .await
    .unwrap();
    assert!(response.starts_with("HTTP/1.1 200 OK"));
    assert!(response.ends_with("{\"status\":\"ok\"}"));
    server.close().await.unwrap();
    server.close().await.unwrap();
    let address = server.url.strip_prefix("http://").unwrap();
    let listener = std::net::TcpListener::bind(address).unwrap();
    drop(listener);
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn occupied_port_startup_cleans_up_and_can_retry() {
    let (base, mut config) = prepare_fixture();
    let blocker = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    config.port = blocker.local_addr().unwrap().port();
    assert!(start_server(config.clone()).await.is_err());
    drop(blocker);
    let server = start_server(config).await.unwrap();
    server.close().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn unbounded_library_routes_are_retired() {
    let (base, config) = prepare_fixture();
    jpeg_fixture(&config.library_root.join("a.jpg"), 8, 4, [32, 64, 192]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    for uri in [
        "https://camera.local/api/photos",
        "https://camera.local/api/albums",
    ] {
        let response = send(
            &router,
            authenticated_request()
                .uri(uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{uri}");
        assert_eq!(
            response_json(response).await,
            serde_json::json!({"error": "Not found"}),
            "{uri}"
        );
    }
    let deleted = send(
        &router,
        authenticated_request()
            .method("DELETE")
            .uri("https://camera.local/api/photos")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    // The request policy admits DELETE only for /api/browse/{token} and the
    // Development Proxy removal, so the retired list endpoint is rejected 405
    // before routing instead of reaching the API 404 fallback.
    assert_eq!(deleted.status(), StatusCode::METHOD_NOT_ALLOWED);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn bearer_mutations_without_origin_work_but_foreign_origins_are_rejected() {
    let (base, config) = prepare_fixture();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    for (name, origin) in [
        ("No Origin", None),
        ("Foreign Origin", Some("https://foreign.example")),
        ("Malformed Origin", Some("not an origin")),
        ("Null Origin", Some("null")),
    ] {
        assert_eq!(
            post_json(
                &router,
                "/api/albums",
                serde_json::json!({"name": name}),
                origin,
            )
            .await
            .status(),
            if origin.is_none() {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            },
            "{name}"
        );
    }
    assert_eq!(
        post_json(&router, "/api/scan", serde_json::json!(null), None)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        response_json(
            post_json(
                &router,
                "/api/albums",
                serde_json::json!({"name": ""}),
                None
            )
            .await,
        )
        .await,
        serde_json::json!({"error": "Invalid Album name"})
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn bearer_access_ignores_forwarded_host_but_enforces_browser_origin() {
    let (base, mut config) = prepare_fixture();
    config.host = "0.0.0.0".to_owned();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());

    let overview = send(
        &router,
        authenticated_request()
            .uri("https://attacker.example/api/overview")
            .header(header::HOST, "camera.local")
            .header("forwarded", "host=photos.example;proto=https")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(overview.status(), StatusCode::OK);

    let mutation = send(
        &router,
        authenticated_request()
            .method("POST")
            .uri("/api/albums")
            .header(header::HOST, "attacker.example")
            .header(header::ORIGIN, "https://attacker.example")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::json!({"name": "Trusted Network"}).to_string(),
            ))
            .unwrap(),
    )
    .await;
    assert_eq!(mutation.status(), StatusCode::FORBIDDEN);

    let health = send(
        &router,
        authenticated_request()
            .uri("/healthz")
            .header(header::HOST, "attacker.example")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(health.status(), StatusCode::OK);
    let health_mutation = send(
        &router,
        authenticated_request()
            .method("POST")
            .uri("/healthz")
            .header(header::HOST, "attacker.example")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(health_mutation.status(), StatusCode::METHOD_NOT_ALLOWED);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn mutation_body_limits_and_json_errors_are_rejected_before_writes() {
    let (base, config) = prepare_fixture();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let request = |body: Body, length: Option<&str>| {
        let mut builder = authenticated_request()
            .method("POST")
            .uri("https://camera.local/api/albums")
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(length) = length {
            builder = builder.header(header::CONTENT_LENGTH, length);
        }
        send(&router, builder.body(body).unwrap())
    };
    assert_eq!(
        request(Body::from(r#"{"name":"Never"}"#), Some("65537"))
            .await
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        request(Body::from(r#"{"name":"Never"}"#), Some("-1"))
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            Body::from(vec![b'x'; MAXIMUM_MUTATION_BODY_BYTES + 1]),
            None
        )
        .await
        .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert_eq!(
        request(Body::from(b"[]".as_slice()), None).await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(Body::from(b"{bad json".as_slice()), None)
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    let scan = response_json(
        send(
            &router,
            authenticated_request()
                .method("POST")
                .uri("https://camera.local/api/scan")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    // The scan response is Loading Status and carries no Photo facts.
    assert!(scan["publication"].as_str().is_some());
    assert_eq!(scan["state"], "idle");
    assert_eq!(scan["completed"], 0);
    assert_eq!(scan["total"], 0);
    assert_eq!(application.albums().await.unwrap().albums.len(), 0);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
