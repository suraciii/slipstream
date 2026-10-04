use super::*;

static ORIGIN_COUNTER: AtomicU64 = AtomicU64::new(0);

struct OriginFixture {
    base: PathBuf,
    config: Config,
}

impl OriginFixture {
    fn new() -> Self {
        let base = env::temp_dir().join(format!(
            "slipstream-origin-access-{}-{}",
            std::process::id(),
            ORIGIN_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&base).unwrap();
        fs::create_dir(base.join("originals")).unwrap();
        fs::create_dir(base.join("web")).unwrap();
        fs::write(base.join("web/index.html"), "public shell").unwrap();
        Self {
            config: Config {
                library_root: base.join("originals"),
                state_directory: base.join("state"),
                cache_directory: base.join("cache"),
                database_basename: "library.sqlite".into(),
                host: "localhost".into(),
                port: 3010,
                public_origin: "https://photos.example.test".into(),
                access_origins: vec![
                    "http://localhost:3010".into(),
                    "http://127.0.0.1:3010".into(),
                ],
                web_root: Some(base.join("web")),
                processing: None,
                export_retained_output_bytes: None,
                metadata_supervisor: None,
            },
            base,
        }
    }
}

impl Drop for OriginFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

fn req(method: &str, uri: &str) -> ::http::request::Builder {
    Request::builder().method(method).uri(uri)
}

async fn body_json(response: Response<Body>) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), 10_000).await.unwrap()).unwrap()
}

fn cookie(response: &Response<Body>) -> String {
    response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned()
}

async fn login(access: &Access, origin: &str) -> Response<Body> {
    access
        .endpoint(
            req("POST", SESSION_PATH)
                .header(header::ORIGIN, origin)
                .header(
                    header::HOST,
                    origin
                        .trim_start_matches("http://")
                        .trim_start_matches("https://"),
                )
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"token": TEST_TOKEN}).to_string(),
                ))
                .unwrap(),
        )
        .await
}

#[test]
fn configured_origins_are_bounded_canonical_unique_and_explicit() {
    let public = "https://photos.example.test";
    let valid = vec![
        "http://localhost:3010".to_owned(),
        "http://127.0.0.1:3010".to_owned(),
    ];
    assert_eq!(validate_origins(valid, public).unwrap().len(), 3);
    assert!(
        validate_origins(
            vec![
                "http://localhost:3010".into(),
                "http://localhost:3010/".into()
            ],
            public
        )
        .is_err()
    );
    assert!(validate_origins(vec!["http://localhost:3010".into(), public.into()], public).is_ok());
    assert!(validate_origins(vec!["http://localhost:0".into()], public).is_err());
    assert!(validate_origins(vec!["http://*.example.test:3010".into()], public).is_err());
    assert!(validate_origins(vec!["http://localhost:3010/path".into()], public).is_err());
    assert!(
        validate_origins((0..65).map(|i| format!("http://host{i}.test:3010")), public).is_err()
    );
    assert!(
        configured_origins(
            "[\"http://localhost:3010\",\"http://localhost:3010\"]",
            public
        )
        .is_err()
    );
    assert!(
        configured_origins(
            "[\"http://localhost:3010\",\"https://photos.example.test\"]",
            public
        )
        .is_ok()
    );
    assert!(configured_origins("{\"origin\":\"http://localhost:3010\"}", public).is_err());
}

#[tokio::test]
async fn each_explicit_origin_supports_login_status_private_write_csrf_and_logout() {
    let fixture = OriginFixture::new();
    let access = Access::open(&fixture.config).unwrap();
    access.seed_test_token();
    for origin in [
        "http://localhost:3010",
        "http://127.0.0.1:3010",
        "https://photos.example.test",
    ] {
        let response = login(&access, origin).await;
        assert_eq!(response.status(), 204, "{origin}");
        let set_cookie = response.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(set_cookie.contains("HttpOnly; SameSite=Lax; Path=/; Max-Age=604800"));
        assert_eq!(set_cookie.contains("Secure"), origin.starts_with("https:"));
        assert!(!set_cookie.contains("Domain="));
        let session_cookie = cookie(&response);
        let status = access
            .endpoint(
                req("GET", SESSION_PATH)
                    .header(header::ORIGIN, origin)
                    .header(header::HOST, origin.split_once("://").unwrap().1)
                    .header(header::COOKIE, &session_cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        let status_json = body_json(status).await;
        assert_eq!(status_json["authenticated"], true);
        let csrf = status_json["csrfToken"].as_str().unwrap();
        let write = req("POST", "/api/albums")
            .header(header::ORIGIN, origin)
            .header(header::HOST, origin.split_once("://").unwrap().1)
            .header(header::COOKIE, &session_cookie)
            .header("x-csrf-token", csrf)
            .body(Body::empty())
            .unwrap();
        assert!(access.admit(&write).is_ok(), "{origin}");
        let no_csrf = write_without_csrf(origin, &session_cookie);
        assert_eq!(access.admit(&no_csrf).unwrap_err().status(), 403);
        let logout = access
            .endpoint(
                req("DELETE", SESSION_PATH)
                    .header(header::ORIGIN, origin)
                    .header(header::HOST, origin.split_once("://").unwrap().1)
                    .header(header::COOKIE, &session_cookie)
                    .header("x-csrf-token", csrf)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(logout.status(), 204);
        assert_eq!(
            cookie(&logout),
            format!("{}=", session_cookie.split('=').next().unwrap())
        );
        assert_eq!(
            access
                .admit(&status_request(origin, &session_cookie))
                .unwrap_err()
                .status(),
            401
        );
    }
}

fn write_without_csrf(origin: &str, session_cookie: &str) -> Request<Body> {
    req("POST", "/api/albums")
        .header(header::ORIGIN, origin)
        .header(header::HOST, origin.split_once("://").unwrap().1)
        .header(header::COOKIE, session_cookie)
        .body(Body::empty())
        .unwrap()
}

fn status_request(origin: &str, session_cookie: &str) -> Request<Body> {
    req("GET", "/api/status")
        .header(header::ORIGIN, origin)
        .header(header::HOST, origin.split_once("://").unwrap().1)
        .header(header::COOKIE, session_cookie)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn wrong_host_origin_duplicates_and_bearer_rules_are_enforced() {
    let fixture = OriginFixture::new();
    let access = Access::open(&fixture.config).unwrap();
    access.seed_test_token();
    let bad_origin = access
        .endpoint(
            req("POST", SESSION_PATH)
                .header(header::ORIGIN, "https://evil.example.test")
                .header(header::HOST, "evil.example.test")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({"token": TEST_TOKEN}).to_string(),
                ))
                .unwrap(),
        )
        .await;
    assert_eq!(bad_origin.status(), 403);
    for (request, expected) in [
        (
            req("GET", "/api/status").header(header::HOST, "photos.example.test"),
            401,
        ),
        (
            req("GET", "/api/status")
                .header(header::HOST, "localhost:3010")
                .header(header::HOST, "127.0.0.1:3010"),
            403,
        ),
        (
            req("GET", "/api/status")
                .header(header::ORIGIN, "http://localhost:3010")
                .header(header::ORIGIN, "http://127.0.0.1:3010"),
            403,
        ),
    ] {
        assert_eq!(
            access
                .admit(&request.body(Body::empty()).unwrap())
                .unwrap_err()
                .status(),
            expected
        );
    }
    let bearer = req("GET", "/api/status")
        .header(header::HOST, "localhost:3010")
        .header(header::AUTHORIZATION, format!("Bearer {TEST_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    assert!(access.admit(&bearer).is_ok());
    let bearer_cross_host = req("GET", "/api/status")
        .header(header::HOST, "photos.example.test")
        .header(header::AUTHORIZATION, format!("Bearer {TEST_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    assert!(access.admit(&bearer_cross_host).is_ok());
    let bearer_origin = req("GET", "/api/status")
        .header(header::HOST, "localhost:3010")
        .header(header::ORIGIN, "https://photos.example.test")
        .header(header::AUTHORIZATION, format!("Bearer {TEST_TOKEN}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(access.admit(&bearer_origin).unwrap_err().status(), 403);
}

#[test]
fn expired_sessions_are_not_admitted() {
    let fixture = OriginFixture::new();
    let access = Access::open(&fixture.config).unwrap();
    access.seed_test_token();
    let value = secret().unwrap();
    let digest_value = digest(&value);
    let session = Session {
        digest: digest_value,
        generation: access
            .store
            .lock()
            .unwrap()
            .read()
            .unwrap()
            .credential
            .unwrap()
            .generation,
        created: 1,
        expires: 604801,
        csrf: secret().unwrap(),
    };
    let mut store = access.store.lock().unwrap();
    let mut records = store.read().unwrap();
    records.sessions.push(session);
    store.write(&records).unwrap();
    drop(store);
    let request = req("GET", "/api/status")
        .header(header::HOST, "localhost:3010")
        .header(header::COOKIE, format!("slipstream={value}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(access.admit(&request).unwrap_err().status(), 401);
}
