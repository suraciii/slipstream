use super::*;
use tower::ServiceExt;
static COUNTER: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    base: PathBuf,
    config: Config,
}
impl Fixture {
    fn new() -> Self {
        let base = env::temp_dir().join(format!(
            "slipstream-access-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&base).unwrap();
        fs::create_dir(base.join("originals")).unwrap();
        fs::create_dir(base.join("web")).unwrap();
        fs::write(base.join("web/index.html"), "public shell").unwrap();
        let config = Config {
            library_root: base.join("originals"),
            state_directory: base.join("state"),
            cache_directory: base.join("cache"),
            database_basename: "library.sqlite".into(),
            host: "127.0.0.1".into(),
            port: 0,
            public_origin: "https://camera.local".into(),
            web_root: Some(base.join("web")),
            processing: None,
            export_retained_output_bytes: None,
            metadata_supervisor: None,
        };
        Self { base, config }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}
fn request(method: &str, path: &str) -> ::http::request::Builder {
    Request::builder().method(method).uri(path)
}
async fn exchange(access: &Access) -> Response<Body> {
    access
        .endpoint(
            request("POST", SESSION_PATH)
                .header("origin", &access.origin)
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"token":TEST_TOKEN}).to_string(),
                ))
                .unwrap(),
        )
        .await
}
fn cookie_value(response: &Response<Body>) -> String {
    response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}
async fn json(response: Response<Body>) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap()
}

#[test]
fn configured_origin_is_exact_http_or_https_origin() {
    for input in [
        "ftp://camera.local",
        "https://camera.local/extra",
        "https://a:b@camera.local",
        "https://camera.local?x",
        "https://camera.local#x",
        "https://",
    ] {
        assert!(canonical_origin(input).is_none(), "{input}");
    }
    assert_eq!(
        canonical_origin("https://CAMERA.local:443/"),
        Some("https://camera.local".into())
    );
    assert_eq!(
        canonical_origin("http://CAMERA.local:80/"),
        Some("http://camera.local".into())
    );
}
#[tokio::test]
async fn http_sessions_preserve_origin_csrf_expiry_and_logout() {
    let mut fixture = Fixture::new();
    fixture.config.public_origin = "http://camera.local:3000".into();
    let access = Access::open(&fixture.config).unwrap();
    assert_eq!(exchange(&access).await.status(), 503);
    assert_eq!(
        access
            .admit(&request("GET", "/api/status").body(Body::empty()).unwrap())
            .unwrap_err()
            .status(),
        401
    );
    access.seed_test_token();
    let response = exchange(&access).await;
    assert_eq!(response.status(), 204);
    let set = response.headers()[header::SET_COOKIE].to_str().unwrap();
    assert!(set.starts_with("slipstream="));
    assert!(!set.contains("Secure") && !set.contains("Domain="));
    assert!(set.contains("HttpOnly; SameSite=Lax; Path=/; Max-Age=604800"));
    let cookie = cookie_value(&response);
    let status_request = || {
        request("GET", SESSION_PATH)
            .header("cookie", &cookie)
            .body(Body::empty())
            .unwrap()
    };
    let first = json(access.endpoint(status_request()).await).await;
    assert_eq!(first["authenticated"], true);
    let second = json(access.endpoint(status_request()).await).await;
    assert_eq!(first["expiresAt"], second["expiresAt"]);
    let csrf = first["csrfToken"].as_str().unwrap();
    for (origin, token, expected) in [
        ("http://camera.local:3000", csrf, 204),
        ("https://camera.local:3000", csrf, 403),
        ("http://camera.local:3001", csrf, 403),
        ("http://camera.local:3000", "invalid", 403),
    ] {
        let result = access.admit(
            &request("POST", "/api/albums")
                .header("cookie", &cookie)
                .header("origin", origin)
                .header("x-csrf-token", token)
                .body(Body::empty())
                .unwrap(),
        );
        assert_eq!(
            result.map_or_else(|error| error.status().as_u16(), |_| 204),
            expected
        );
    }
    let wrong_name = cookie.replacen("slipstream=", "__Host-slipstream=", 1);
    assert_eq!(
        access
            .admit(
                &request("GET", "/api/status")
                    .header("cookie", wrong_name)
                    .body(Body::empty())
                    .unwrap()
            )
            .unwrap_err()
            .status(),
        401
    );
    let logout = access
        .endpoint(
            request("DELETE", SESSION_PATH)
                .header("cookie", &cookie)
                .header("origin", &access.origin)
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(logout.status(), 204);
    assert_eq!(
        logout.headers()[header::SET_COOKIE],
        "slipstream=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0"
    );
    let expired = access.endpoint(status_request()).await;
    assert_eq!(
        expired.headers()[header::SET_COOKIE],
        "slipstream=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0"
    );
    assert_eq!(json(expired).await["authenticated"], false);
}

#[test]
fn rate_windows_and_peer_storage_are_bounded() {
    let mut rate = Rate::default();
    let start = Instant::now();
    let peer = IpAddr::from([1, 2, 3, 4]);
    for _ in 0..5 {
        assert!(rate.admit(peer, start).is_ok());
    }
    assert_eq!(rate.admit(peer, start), Err(60));
    assert!(rate.admit(peer, start + Duration::from_secs(60)).is_ok());
    for index in 1..=19 {
        assert!(
            rate.admit(
                IpAddr::from([1, 2, 4, index]),
                start + Duration::from_secs(60)
            )
            .is_ok()
        );
    }
    assert!(
        rate.admit(IpAddr::from([1, 2, 4, 40]), start + Duration::from_secs(60))
            .is_err()
    );
    let mut rate = Rate::default();
    for index in 0..1024 {
        rate.peers.insert(
            IpAddr::V4(std::net::Ipv4Addr::from(index)),
            VecDeque::from([start]),
        );
    }
    assert_eq!(rate.admit(peer, start), Err(60));
    assert!(rate.admit(peer, start + Duration::from_secs(60)).is_ok());
}
#[tokio::test]
async fn session_persists_and_logout_revokes_only_presented_session() {
    let fixture = Fixture::new();
    let access = Access::open(&fixture.config).unwrap();
    access.seed_test_token();
    let first = exchange(&access).await;
    assert_eq!(first.status(), 204);
    let cookie = cookie_value(&first);
    let set = first.headers()[header::SET_COOKIE].to_str().unwrap();
    for attribute in [
        "Secure",
        "HttpOnly",
        "SameSite=Lax",
        "Path=/",
        "Max-Age=604800",
    ] {
        assert!(set.contains(attribute));
    }
    let second = cookie_value(&exchange(&access).await);
    drop(access);
    let access = Access::open(&fixture.config).unwrap();
    let status = json(
        access
            .endpoint(
                request("GET", SESSION_PATH)
                    .header("cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await,
    )
    .await;
    assert_eq!(status["authenticated"], true);
    assert!(
        !fs::read(fixture.config.state_directory.join("access.sqlite"))
            .unwrap()
            .windows(TEST_TOKEN.len())
            .any(|v| v == TEST_TOKEN.as_bytes())
    );
    let csrf = status["csrfToken"].as_str().unwrap();
    let mutation = || {
        request("POST", "/api/albums")
            .header("cookie", &cookie)
            .header("origin", "https://camera.local")
    };
    assert_eq!(
        access
            .admit(&mutation().body(Body::empty()).unwrap())
            .unwrap_err()
            .status(),
        403
    );
    assert!(
        access
            .admit(
                &mutation()
                    .header("x-csrf-token", csrf)
                    .body(Body::empty())
                    .unwrap()
            )
            .is_ok()
    );
    let logout = access
        .endpoint(
            request("DELETE", SESSION_PATH)
                .header("cookie", &cookie)
                .header("origin", "https://camera.local")
                .header("x-csrf-token", csrf)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(logout.status(), 204);
    assert_eq!(
        access
            .admit(
                &request("GET", "/api/status")
                    .header("cookie", &cookie)
                    .body(Body::empty())
                    .unwrap()
            )
            .unwrap_err()
            .status(),
        401
    );
    assert!(
        access
            .admit(
                &request("GET", "/api/status")
                    .header("cookie", second)
                    .body(Body::empty())
                    .unwrap()
            )
            .is_ok()
    );
    assert_eq!(
        access
            .endpoint(
                request("DELETE", SESSION_PATH)
                    .header("cookie", cookie)
                    .header("origin", "https://camera.local")
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .status(),
        204
    );
}
#[tokio::test]
async fn expiry_capacity_and_revocation_deny_without_replaying() {
    let fixture = Fixture::new();
    let access = Access::open(&fixture.config).unwrap();
    access.seed_test_token();
    let first = exchange(&access).await;
    let cookie = cookie_value(&first);
    {
        let mut store = access.store.lock().unwrap();
        let mut records = store.read().unwrap();
        records.sessions[0].created = 1;
        records.sessions[0].expires = 1 + LIFETIME;
        store.write(&records).unwrap();
    }
    assert_eq!(
        access
            .admit(
                &request("GET", "/api/status")
                    .header("cookie", &cookie)
                    .body(Body::empty())
                    .unwrap()
            )
            .unwrap_err()
            .status(),
        401
    );
    assert!(
        access
            .store
            .lock()
            .unwrap()
            .read()
            .unwrap()
            .sessions
            .is_empty()
    );
    exchange(&access).await;
    {
        let mut store = access.store.lock().unwrap();
        let mut records = store.read().unwrap();
        let session = records.sessions[0].clone();
        records.sessions.resize(32, session);
        store.write(&records).unwrap();
    }
    let response = exchange(&access).await;
    assert_eq!(response.status(), 429);
    assert_eq!(json(response).await["error"], "session_capacity");
    {
        let mut store = access.store.lock().unwrap();
        store.write(&Records::default()).unwrap();
    }
    assert_eq!(exchange(&access).await.status(), 503);
    assert_eq!(
        access
            .admit(
                &request("GET", "/api/status")
                    .header("authorization", format!("Bearer {TEST_TOKEN}"))
                    .body(Body::empty())
                    .unwrap()
            )
            .unwrap_err()
            .status(),
        401
    );
}
#[tokio::test]
async fn exchange_rejects_malformed_origin_credentials_and_ambiguous_input() {
    let fixture = Fixture::new();
    let access = Access::open(&fixture.config).unwrap();
    access.seed_test_token();
    for body in [
        format!("{{\"token\":\"{TEST_TOKEN}\",\"token\":\"{TEST_TOKEN}\"}}"),
        format!("{{\"token\":\"{TEST_TOKEN}\",\"extra\":1}}"),
        format!("{{\"token\":\"{TEST_TOKEN}\"}} trailing"),
        "x".repeat(257),
    ] {
        let response = access
            .endpoint(
                request("POST", SESSION_PATH)
                    .header("origin", "https://camera.local")
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await;
        assert_eq!(response.status(), 400);
    }
    assert_eq!(
        access
            .endpoint(request("POST", SESSION_PATH).body(Body::empty()).unwrap())
            .await
            .status(),
        403
    );
    assert_eq!(
        access
            .endpoint(
                request("POST", SESSION_PATH)
                    .header("origin", "https://camera.local.evil")
                    .body(Body::empty())
                    .unwrap()
            )
            .await
            .status(),
        403
    );
    let response = exchange(&access).await;
    assert_eq!(response.status(), 204);
    let cookie = cookie_value(&response);
    let mixed = request("GET", "/api/status")
        .header("cookie", &cookie)
        .header("authorization", format!("Bearer {TEST_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(access.admit(&mixed).unwrap_err().status(), 400);
    let bearer = request("POST", "/api/scan")
        .header("authorization", format!("Bearer {TEST_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    assert!(access.admit(&bearer).is_ok());
    let bad_origin = request("POST", "/api/scan")
        .header("authorization", format!("Bearer {TEST_TOKEN}"))
        .header("origin", "https://evil.local")
        .body(Body::empty())
        .unwrap();
    assert_eq!(access.admit(&bad_origin).unwrap_err().status(), 403);
    assert_eq!(exchange(&access).await.status(), 429);
}
#[tokio::test]
async fn routing_protects_all_api_methods_before_contract_or_resource_processing() {
    let fixture = Fixture::new();
    let application = Application::open(&fixture.config).await.unwrap();
    application.access.seed_test_token();
    let router = create_router(Arc::clone(&application), fixture.config.web_root());
    let paths = [
        "/api/status",
        "/api/capabilities",
        "/api/processing/capability",
        "/api/processing/modules",
        "/api/overview",
        "/api/albums",
        "/api/albums/missing",
        "/api/album-summaries",
        "/api/photo-queries",
        "/api/photos/missing",
        "/api/photos/missing/processing-recipe",
        "/api/photos/missing/preview",
        "/api/photos/missing/thumbnail",
        "/api/photos/missing/metadata",
        "/api/photos/missing/albums",
        "/api/photos/state",
        "/api/photos/remove",
        "/api/photos/remove-explicit",
        "/api/photos/removal-operations/missing",
        "/api/photos/restore",
        "/api/photos/restore-explicit",
        "/api/photos/restore-operations/missing",
        "/api/trash",
        "/api/trash/review",
        "/api/trash/delete",
        "/api/trash/operations/missing",
        "/api/browse",
        "/api/browse/missing",
        "/api/scan",
        "/api/recovery/unavailable",
        "/api/recovery/propose",
        "/api/recovery/apply",
        "/api/derivatives/missing/review/image.jpg",
        "/api/private/derivatives/missing/review/image.jpg",
        "/api/future-route",
        "/api/exports/missing",
    ];
    for path in paths {
        for method in ["GET", "HEAD", "POST", "DELETE", "PUT"] {
            let response = router
                .clone()
                .oneshot(
                    request(method, path)
                        .header("range", "bytes=0-1")
                        .header("if-none-match", "\"cached\"")
                        .header("slipstream-cli-contract", "999")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), 401, "{method} {path}");
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert!(response.headers().contains_key(header::WWW_AUTHENTICATE));
        }
    }
    for path in ["/healthz", "/", "/api/access/session"] {
        let response = router
            .clone()
            .oneshot(request("GET", path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{path}");
    }
    let response = router
        .oneshot(
            request("GET", "/api/derivatives/missing/review/image.jpg")
                .header("authorization", format!("Bearer {TEST_TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    application.shutdown().await.unwrap();
}
#[test]
fn corrupt_or_unsafe_authentication_storage_fails_closed() {
    let fixture = Fixture::new();
    drop(Access::open(&fixture.config).unwrap());
    fs::write(
        fixture.config.state_directory.join("access.sqlite"),
        "corrupt",
    )
    .unwrap();
    assert!(Access::open(&fixture.config).is_err());
    fs::remove_file(fixture.config.state_directory.join("access.sqlite")).unwrap();
    std::os::unix::fs::symlink(
        fixture.base.join("outside"),
        fixture.config.state_directory.join("access.sqlite"),
    )
    .unwrap();
    assert!(Access::open(&fixture.config).is_err());
    assert!(!fixture.base.join("outside").exists());
}
