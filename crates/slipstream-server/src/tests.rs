use super::*;
use std::{
    collections::HashMap,
    fs,
    io::{ErrorKind, Read, Write},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_base() -> PathBuf {
    loop {
        let suffix = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "slipstream-server-test-{}-{suffix}",
            std::process::id()
        ));
        match fs::create_dir(&path) {
            Ok(()) => return path,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("create test directory: {error}"),
        }
    }
}

fn test_config(base: &Path, web_root: PathBuf, port: u16) -> Config {
    Config {
        library_root: base.join("originals"),
        state_directory: base.join("state"),
        cache_directory: base.join("cache"),
        database_basename: "library.sqlite".to_owned(),
        host: "127.0.0.1".to_owned(),
        port,
        web_root: Some(web_root),
    }
}

/// Waits until the background scan opened by `Application::open` has
/// completed, so tests observe the same published state an operator sees
/// once startup work settles. Deterministic even when the scan finishes
/// between status polls.
async fn wait_for_scan_settled(application: &Application) {
    let started = application.shared.runs_started.load(Ordering::Relaxed);
    let target = started.max(1);
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        if application.shared.runs_completed.load(Ordering::Relaxed) >= target {
            return;
        }
        tokio::task::yield_now().await;
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("Library scan did not settle before the test deadline");
}

/// Bounded traversal of one Browse source. Tests must observe Library
/// state through the bounded protocol, never a complete-Photo route.
async fn browse_summaries(
    application: &Application,
    source: BrowseSourceRequest,
) -> Vec<PhotoSummary> {
    let opened = application
        .browse_open(source, None)
        .await
        .expect("browse open succeeds");
    let mut photos = Vec::new();
    let mut start = 0;
    loop {
        let window = application
            .browse_window(&opened.token, start, 60)
            .await
            .expect("browse window succeeds");
        let total = window.total;
        let count = window.photos.len();
        photos.extend(window.photos);
        start += count;
        if count == 0 || start >= total {
            break;
        }
    }
    assert_eq!(photos.len(), opened.total, "browse traversal incomplete");
    application.browse_close(&opened.token);
    photos
}

async fn browse_photo_ids(application: &Application, source: BrowseSourceRequest) -> Vec<String> {
    browse_summaries(application, source)
        .await
        .into_iter()
        .map(|photo| photo.id)
        .collect()
}

async fn published_photo_summary(application: &Application, photo_id: &str) -> PhotoSummary {
    browse_summaries(application, BrowseSourceRequest::Library)
        .await
        .into_iter()
        .find(|photo| photo.id == photo_id)
        .unwrap_or_else(|| panic!("photo {photo_id} is missing from the published Library"))
}

fn prepare_fixture() -> (PathBuf, Config) {
    let base = unique_base();
    let web_root = base.join("web");
    fs::create_dir(base.join("originals")).unwrap();
    fs::create_dir(&web_root).unwrap();
    fs::write(
        web_root.join("index.html"),
        b"<main>compatibility web</main>",
    )
    .unwrap();
    (base.clone(), test_config(&base, web_root, 3000))
}

fn environment(values: &[(&str, &str)]) -> HashMap<String, String> {
    values
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

#[test]
fn startup_vectors_keep_defaults_and_explicit_binding_typed() {
    let defaults = Config::from_env(environment(&[
        ("SLIPSTREAM_LIBRARY_ROOT", "/photos"),
        ("SLIPSTREAM_STATE_DIRECTORY", "/state"),
        ("SLIPSTREAM_CACHE_DIRECTORY", "/cache"),
    ]))
    .unwrap();
    assert_eq!(defaults.library_root, PathBuf::from("/photos"));
    assert_eq!(defaults.database_basename, "library.sqlite");
    assert_eq!((defaults.host.as_str(), defaults.port), ("127.0.0.1", 3000));
    let explicit = Config::from_env(environment(&[
        ("SLIPSTREAM_LIBRARY_ROOT", "/photos"),
        ("SLIPSTREAM_STATE_DIRECTORY", "/state"),
        ("SLIPSTREAM_CACHE_DIRECTORY", "/cache"),
        ("SLIPSTREAM_DATABASE_BASENAME", "review.sqlite"),
        ("SLIPSTREAM_HOST", "0.0.0.0"),
        ("SLIPSTREAM_PORT", "8080"),
    ]))
    .unwrap();
    assert_eq!(explicit.database_basename, "review.sqlite");
    assert_eq!((explicit.host.as_str(), explicit.port), ("0.0.0.0", 8080));
}

#[test]
fn checked_in_startup_vectors_parse_through_the_typed_config() {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../compatibility/startup/vectors.json");
    let vectors: Vec<serde_json::Value> = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    for vector in vectors {
        let environment = vector["environment"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(key, value)| (key.clone(), value.as_str().unwrap().to_owned()));
        let config = Config::from_env(environment).unwrap();
        assert_eq!(
            config.library_root,
            PathBuf::from(vector["expected"]["libraryRoot"].as_str().unwrap())
        );
        assert_eq!(
            config.state_directory,
            PathBuf::from(vector["expected"]["stateDirectory"].as_str().unwrap())
        );
        assert_eq!(
            config.cache_directory,
            PathBuf::from(vector["expected"]["cacheDirectory"].as_str().unwrap())
        );
        assert_eq!(
            config.database_basename,
            vector["expected"]["databaseBasename"]
        );
        assert_eq!(config.host, vector["expected"]["host"]);
        assert_eq!(
            config.port,
            vector["expected"]["port"].as_u64().unwrap() as u16
        );
    }
}

#[tokio::test]
async fn application_rejects_overlapping_paths_before_opening_state_or_cache() {
    let base = unique_base();
    let originals = base.join("originals");
    let state = originals.join("state");
    let cache = base.join("cache");
    fs::create_dir(&originals).unwrap();
    let config = test_config(&base, base.join("web"), 3000);
    let config = Config {
        library_root: originals,
        state_directory: state.clone(),
        cache_directory: cache,
        ..config
    };
    assert!(matches!(
        Application::open(&config).await,
        Err(ServerError::StorageLayout)
    ));
    assert!(!state.exists());
    assert!(!config.cache_directory.exists());
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn start_server_rejects_missing_web_root_before_opening_application() {
    let base = unique_base();
    let originals = base.join("originals");
    fs::create_dir(&originals).unwrap();
    let config = test_config(&base, base.join("missing-web"), 0);
    assert!(matches!(
        start_server(config).await,
        Err(ServerError::WebUnavailable)
    ));
    assert!(!base.join("state").exists());
    assert!(!base.join("cache").exists());
    let _ = fs::remove_dir_all(base);
}

#[test]
fn storage_layout_rejects_symlink_aliases() {
    let base = unique_base();
    let originals = base.join("originals");
    let state_parent = base.join("state-parent");
    fs::create_dir(&originals).unwrap();
    fs::create_dir(&state_parent).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&originals, state_parent.join("alias")).unwrap();
    let config = test_config(&base, base.join("web"), 3000);
    let config = Config {
        library_root: originals,
        state_directory: state_parent.join("alias"),
        cache_directory: base.join("cache"),
        ..config
    };
    assert!(matches!(
        validate_storage_layout(&config),
        Err(ServerError::StorageLayout)
    ));
    let _ = fs::remove_dir_all(base);
}

#[test]
fn startup_vectors_reject_relative_paths_and_invalid_ports() {
    let missing = Config::from_env(HashMap::new());
    assert_eq!(
        missing,
        Err(ConfigError::Missing("SLIPSTREAM_LIBRARY_ROOT"))
    );
    let relative = Config::from_env(environment(&[
        ("SLIPSTREAM_LIBRARY_ROOT", "photos"),
        ("SLIPSTREAM_STATE_DIRECTORY", "/state"),
        ("SLIPSTREAM_CACHE_DIRECTORY", "/cache"),
    ]));
    assert_eq!(
        relative,
        Err(ConfigError::NotAbsolute("SLIPSTREAM_LIBRARY_ROOT"))
    );
    let invalid = Config::from_env(environment(&[
        ("SLIPSTREAM_LIBRARY_ROOT", "/photos"),
        ("SLIPSTREAM_STATE_DIRECTORY", "/state"),
        ("SLIPSTREAM_CACHE_DIRECTORY", "/cache"),
        ("SLIPSTREAM_PORT", "65536"),
    ]));
    assert_eq!(invalid, Err(ConfigError::Invalid("SLIPSTREAM_PORT")));
}

#[tokio::test]
async fn post_to_static_path_is_rejected_without_reading_or_mutating() {
    let (base, config) = prepare_fixture();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = create_router(Arc::clone(&application), config.web_root());
    let response = tower::ServiceExt::oneshot(
        router,
        Request::builder()
            .method("POST")
            .uri("/")
            .body(Body::from(b"not-json".as_slice()))
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn static_file_symlink_is_not_followed() {
    let (base, config) = prepare_fixture();
    let outside = base.join("outside.txt");
    fs::write(&outside, b"outside").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, base.join("web").join("escape.txt")).unwrap();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = create_router(Arc::clone(&application), config.web_root());
    let response = tower::ServiceExt::oneshot(
        router,
        Request::builder()
            .uri("/escape.txt")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .as_ref(),
        b"<main>compatibility web</main>"
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn static_files_have_revalidation_and_head_without_a_body() {
    let base = unique_base();
    let root = base.join("web");
    fs::create_dir_all(root.join("assets")).unwrap();
    fs::write(root.join("index.html"), b"<main>compatibility web</main>").unwrap();
    fs::write(root.join("assets/app.js"), b"console.log(1)").unwrap();
    fs::create_dir_all(base.join("originals")).unwrap();
    let application = Application::open(&test_config(&base, root.clone(), 3000))
        .await
        .unwrap();
    let app = Router::new().fallback(static_web).with_state(HttpState {
        application: Arc::clone(&application),
        web_root: Arc::new(open_web_root(root.clone())),
    });
    let response = tower::ServiceExt::oneshot(
        app.clone(),
        Request::builder().uri("/").body(Body::empty()).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
    let response = tower::ServiceExt::oneshot(
        app,
        Request::builder()
            .method("HEAD")
            .uri("/assets/app.js")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );
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

fn substitute_captured_set_id(value: &serde_json::Value, set_id: &str) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) => {
            serde_json::Value::String(text.replace("$setId", set_id))
        }
        serde_json::Value::Array(values) => serde_json::Value::Array(
            values
                .iter()
                .map(|item| substitute_captured_set_id(item, set_id))
                .collect(),
        ),
        serde_json::Value::Object(entries) => serde_json::Value::Object(
            entries
                .iter()
                .map(|(name, item)| (name.clone(), substitute_captured_set_id(item, set_id)))
                .collect(),
        ),
        other => other.clone(),
    }
}

#[tokio::test]
async fn shared_protocol_vectors_execute_all_requests_with_exact_results() {
    let (base, config) = prepare_fixture();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = create_router(Arc::clone(&application), config.web_root());
    let vectors: Vec<serde_json::Value> = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../compatibility/protocol/vectors.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let mut captured_set_id = String::new();
    for vector in vectors {
        let request_definition = &vector["request"];
        let method = request_definition["method"].as_str().unwrap();
        let path = request_definition["path"].as_str().unwrap();
        let mut builder = Request::builder().method(method).uri(path);
        if let Some(headers) = request_definition["headers"].as_object() {
            for (name, value) in headers {
                builder = builder.header(name, value.as_str().unwrap());
            }
        }
        let body = request_definition
            .get("body")
            .map(|body| Body::from(serde_json::to_vec(body).unwrap()))
            .unwrap_or_else(Body::empty);
        let request = builder.body(body).unwrap();
        let response = tower::ServiceExt::oneshot(router.clone(), request)
            .await
            .unwrap();
        assert_eq!(
            response.status().as_u16(),
            vector["expected"]["status"].as_u64().unwrap() as u16,
            "{}",
            vector["name"]
        );
        if let Some(expected_headers) = vector["expected"]["headers"].as_object() {
            for (name, expected) in expected_headers {
                assert_eq!(
                    response
                        .headers()
                        .get(name)
                        .and_then(|value| value.to_str().ok()),
                    expected.as_str(),
                    "{} header {name}",
                    vector["name"]
                );
            }
        }
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        if let Some(expected) = vector["expected"]["body"].as_object() {
            let actual: serde_json::Value = serde_json::from_slice(&body).unwrap();
            if captured_set_id.is_empty()
                && let Some(id) = actual["albums"][0]["id"].as_str()
            {
                captured_set_id = id.to_owned();
            }
            let expected = serde_json::to_value(expected).unwrap();
            let expected = if captured_set_id.is_empty() {
                expected
            } else {
                substitute_captured_set_id(&expected, &captured_set_id)
            };
            assert_eq!(
                actual.as_object().unwrap(),
                expected.as_object().unwrap(),
                "{}",
                vector["name"]
            );
        }
        if let Some(expected) = vector["expected"]["bodyText"].as_str() {
            assert_eq!(
                std::str::from_utf8(&body).unwrap(),
                expected,
                "{}",
                vector["name"]
            );
        }
    }
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn browse_protocol_fixtures_execute_with_captured_token() {
    let (base, config) = prepare_fixture();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = create_router(Arc::clone(&application), config.web_root());
    let vectors: Vec<serde_json::Value> = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../compatibility/protocol/browse-vectors.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert!(vectors.len() >= 12);
    fn substitute(value: &serde_json::Value, token: &str) -> serde_json::Value {
        match value {
            serde_json::Value::String(text) => {
                serde_json::Value::String(text.replace("$token", token))
            }
            serde_json::Value::Array(values) => serde_json::Value::Array(
                values.iter().map(|item| substitute(item, token)).collect(),
            ),
            serde_json::Value::Object(entries) => serde_json::Value::Object(
                entries
                    .iter()
                    .map(|(name, item)| (name.clone(), substitute(item, token)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    let mut token = String::new();
    for vector in vectors {
        let name = vector["name"].as_str().unwrap().to_owned();
        let request_definition = &vector["request"];
        let method = request_definition["method"].as_str().unwrap();
        let path = request_definition["path"]
            .as_str()
            .unwrap()
            .replace("$token", &token);
        let mut builder = Request::builder().method(method).uri(&path);
        if let Some(headers) = request_definition["headers"].as_object() {
            for (header_name, value) in headers {
                builder = builder.header(header_name, value.as_str().unwrap());
            }
        }
        let body = request_definition
            .get("body")
            .map(|body| Body::from(serde_json::to_vec(body).unwrap()))
            .unwrap_or_else(Body::empty);
        let request = builder.body(body).unwrap();
        let response = tower::ServiceExt::oneshot(router.clone(), request)
            .await
            .unwrap();
        assert_eq!(
            response.status().as_u16(),
            vector["expected"]["status"].as_u64().unwrap() as u16,
            "{name}"
        );
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let actual: Option<serde_json::Value> = if vector["expected"]["body"].is_object() {
            Some(serde_json::from_slice(&body).unwrap())
        } else {
            None
        };
        if let Some(actual_value) = &actual
            && method == "POST"
            && path == "/api/browse"
            && let Some(new_token) = actual_value.get("token").and_then(|value| value.as_str())
        {
            token = new_token.to_owned();
            assert!(token.len() >= 36, "{name} token is not opaque");
        }
        if let (Some(actual_value), Some(expected)) =
            (actual.as_ref(), vector["expected"]["body"].as_object())
        {
            assert_eq!(
                actual_value.as_object().unwrap(),
                substitute(&serde_json::Value::Object(expected.clone()), &token)
                    .as_object()
                    .unwrap(),
                "{name}"
            );
        }
    }
    assert!(!token.is_empty(), "fixtures must exercise a captured token");
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn photo_json_omits_optional_values_and_preserves_original_order() {
    let contract: serde_json::Value = serde_json::from_str(include_str!(
        "../../../compatibility/protocol/capture-order-omission.json"
    ))
    .unwrap();
    let ordered_paths = contract["orderedPaths"].as_array().unwrap();
    let (base, config) = prepare_fixture();
    capture_metadata_fixture(&config.library_root.join("z.JPG"), "2026:01:01 09:00:00");
    capture_metadata_fixture(&config.library_root.join("a.jpg"), "2026:01:01 10:00:00");
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let opened = application
        .browse_open(BrowseSourceRequest::Library, None)
        .await
        .unwrap();
    let window = application
        .browse_window(&opened.token, 0, 60)
        .await
        .unwrap();
    application.browse_close(&opened.token);
    let photos = serde_json::to_value(window).unwrap();
    let list = photos["photos"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    let snapshot = application.library.snapshot().await.unwrap();
    assert_eq!(
        snapshot
            .photos
            .iter()
            .map(|photo| photo.sort_path.as_str())
            .collect::<Vec<_>>(),
        ordered_paths
            .iter()
            .map(|path| path.as_str().unwrap())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        list.iter()
            .map(|photo| photo["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        snapshot
            .photos
            .iter()
            .map(|photo| photo.id.as_str())
            .collect::<Vec<_>>()
    );
    for photo in list {
        assert_eq!(photo["originals"][0]["kind"], "jpeg");
        assert_eq!(
            photo["preview"],
            serde_json::json!({"state": "inspection-pending"})
        );
        assert!(!photo.to_string().contains(":null"));
        for hidden in contract["hiddenFields"].as_array().unwrap() {
            let hidden = hidden.as_str().unwrap();
            assert!(photo.get(hidden).is_none(), "{hidden} leaked into protocol");
        }
        assert!(
            !photo
                .to_string()
                .contains(config.library_root.to_str().unwrap())
        );
    }
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn healthz_is_exact_json_and_head_api_has_no_body() {
    let (base, config) = prepare_fixture();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = create_router(Arc::clone(&application), config.web_root());
    let response = tower::ServiceExt::oneshot(
        router.clone(),
        Request::builder()
            .uri(HEALTH_PATH)
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
        });
    let response = tower::ServiceExt::oneshot(
        missing_router,
        Request::builder()
            .uri(HEALTH_PATH)
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let response = tower::ServiceExt::oneshot(
        router,
        Request::builder()
            .method("HEAD")
            .uri("/api/overview")
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
    let router = create_router(Arc::clone(&application), config.web_root());
    let exact = "x".repeat(MAXIMUM_HEADER_BYTES - "x-test".len());
    let response = tower::ServiceExt::oneshot(
        router.clone(),
        Request::builder()
            .uri("/api/overview")
            .header("x-test", exact)
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let over = "x".repeat(MAXIMUM_HEADER_BYTES - "x-test".len() + 1);
    let response = tower::ServiceExt::oneshot(
        router,
        Request::builder()
            .uri("/api/overview")
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
async fn overview_and_browse_windows_remain_bounded_for_forty_thousand_photos() {
    let (base, config) = prepare_fixture();
    for directory in ["a", "b"] {
        fs::create_dir(base.join("originals").join(directory)).unwrap();
        for index in 0..20_000 {
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
    let router = create_router(Arc::clone(&application), config.web_root());

    let overview_response = send(
        &router,
        Request::builder()
            .uri("http://camera.local/api/overview")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(overview_response.status(), StatusCode::OK);
    let overview_bytes = axum::body::to_bytes(overview_response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    let overview: serde_json::Value = serde_json::from_slice(&overview_bytes).unwrap();
    assert_eq!(overview["photoCount"], 40_000);
    assert!(overview_bytes.len() < 20_000);

    let opened = post_json(
        &router,
        "/api/browse",
        serde_json::json!({"source":"library"}),
        None,
    )
    .await;
    assert_eq!(opened.status(), StatusCode::OK);
    let opened: serde_json::Value = response_json(opened).await;
    let token = opened["token"].as_str().unwrap();
    let window = send(
        &router,
        Request::builder()
            .uri(format!(
                "http://camera.local/api/browse/{}?start=39940&limit=60",
                token
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(window.status(), StatusCode::OK);
    let window: serde_json::Value = response_json(window).await;
    assert_eq!(window["start"], 39_940);
    assert_eq!(window["total"], 40_000);
    assert_eq!(window["photos"].as_array().unwrap().len(), 60);

    let oversized = send(
        &router,
        Request::builder()
            .uri(format!(
                "http://camera.local/api/browse/{}?start=0&limit=61",
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
    let set_id = application
        .albums()
        .await
        .unwrap()
        .albums
        .into_iter()
        .find(|set| set.name == "Picks")
        .unwrap()
        .id;
    application
        .mutate_album(slipstream_core::AlbumMutation::AddMembers {
            album_id: set_id.clone(),
            photo_ids: ids.clone(),
        })
        .await
        .unwrap();
    application
        .mutate_album(slipstream_core::AlbumMutation::SetProgress {
            album_id: set_id.clone(),
            photo_id: ids[1].clone(),
        })
        .await
        .unwrap();
    let opened = application
        .browse_open(BrowseSourceRequest::Album(set_id), None)
        .await
        .unwrap();
    assert_eq!(opened.total, 3);
    assert_eq!(opened.position, 1);
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
        .browse_open(BrowseSourceRequest::Album(album_id.clone()), None)
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
        .browse_open(BrowseSourceRequest::Album(summary.id.clone()), None)
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
        .browse_open(BrowseSourceRequest::Library, None)
        .await
        .unwrap();
    let opened_b = application_b
        .browse_open(BrowseSourceRequest::Library, None)
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
            .browse_snapshots
            .lock()
            .expect("browse snapshots poisoned");
        let snapshot = snapshots.get_mut(&opened_a.token).unwrap();
        snapshot.last_used -= BROWSE_SNAPSHOT_IDLE + Duration::from_secs(1);
    }
    assert!(matches!(
        application_a.browse_window(&opened_a.token, 0, 10).await,
        Err(ServerError::BrowseNotFound)
    ));
    assert!(
        !application_a
            .browse_snapshots
            .lock()
            .unwrap()
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
    let router = create_router(Arc::clone(&application), config.web_root());
    let opened = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source":"library"}),
            Some("http://camera.local"),
        )
        .await,
    )
    .await;
    let token = opened["token"].as_str().unwrap().to_owned();
    async fn window(router: &Router, token: &str) -> Response<Body> {
        send(
            router,
            Request::builder()
                .uri(format!(
                    "http://camera.local/api/browse/{token}?start=0&limit=10"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
    }
    assert_eq!(window(&router, &token).await.status(), StatusCode::OK);
    let cross_origin = send(
        &router,
        Request::builder()
            .method("DELETE")
            .uri(format!("http://camera.local/api/browse/{token}"))
            .header(header::ORIGIN, "http://elsewhere.example")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(cross_origin.status(), StatusCode::FORBIDDEN);
    let removed = send(
        &router,
        Request::builder()
            .method("DELETE")
            .uri(format!("http://camera.local/api/browse/{token}"))
            .header(header::ORIGIN, "http://camera.local")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(removed.status(), StatusCode::NO_CONTENT);
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
        .browse_open(BrowseSourceRequest::Library, Some(&ids[2]))
        .await
        .unwrap();
    assert_eq!(library.position, 2);
    let fallback = application
        .browse_open(BrowseSourceRequest::Library, None)
        .await
        .unwrap();
    assert_eq!(fallback.position, 0);
    application
        .mutate_album(slipstream_core::AlbumMutation::Create {
            name: "Picks".to_owned(),
        })
        .await
        .unwrap();
    let set_id = application
        .albums()
        .await
        .unwrap()
        .albums
        .into_iter()
        .find(|set| set.name == "Picks")
        .unwrap()
        .id;
    application
        .mutate_album(slipstream_core::AlbumMutation::AddMembers {
            album_id: set_id.clone(),
            photo_ids: ids.clone(),
        })
        .await
        .unwrap();
    application
        .mutate_album(slipstream_core::AlbumMutation::SetProgress {
            album_id: set_id.clone(),
            photo_id: ids[0].clone(),
        })
        .await
        .unwrap();
    let preferred = application
        .browse_open(BrowseSourceRequest::Album(set_id), Some(&ids[2]))
        .await
        .unwrap();
    assert_eq!(preferred.position, 2);
    let router = create_router(Arc::clone(&application), config.web_root());
    let invalid = post_json(
        &router,
        "/api/browse",
        serde_json::json!({"source":"library","photoId":"NOT-A-ID"}),
        Some("http://camera.local"),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn photo_state_mutation_updates_the_browse_snapshot_without_reload() {
    let (base, config) = prepare_fixture();
    for name in ["a.jpg", "b.jpg"] {
        jpeg_fixture(&config.library_root.join(name), 8, 4, [32, 64, 192]);
    }
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = create_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let opened = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source":"library"}),
            Some("http://camera.local"),
        )
        .await,
    )
    .await;
    let token = opened["token"].as_str().unwrap().to_owned();
    let window = || async {
        response_json(
            send(
                &router,
                Request::builder()
                    .uri(format!(
                        "http://camera.local/api/browse/{token}?start=0&limit=10"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await,
        )
        .await
    };
    let before = window().await;
    assert_eq!(before["photos"][0]["selectionState"], "undecided");
    assert_eq!(
        post_json(
            &router,
            &format!("http://camera.local/api/photos/{}/state", ids[0]),
            serde_json::json!({"field": "selectionState", "value": "selected"}),
            Some("http://camera.local"),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let after = window().await;
    assert_eq!(after["photos"][0]["selectionState"], "selected");
    assert_eq!(after["photos"][1]["selectionState"], "undecided");
    assert_eq!(
        after["photos"].as_array().unwrap().len(),
        before["photos"].as_array().unwrap().len()
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn publication_preserves_facts_committed_between_scan_and_publication() {
    let (base, config) = prepare_fixture();
    for name in ["a.jpg", "b.jpg"] {
        jpeg_fixture(&config.library_root.join(name), 8, 4, [32, 64, 192]);
    }
    // First boot persists the initial scan so the second boot publishes
    // from stored state and the background rescan is the cycle under test.
    {
        let application = Application::open(&config).await.unwrap();
        wait_for_scan_settled(&application).await;
        application.shutdown().await.unwrap();
    }
    // Park the background rescan after its apply and before publication.
    let (publish_sender, publish_receiver) = tokio::sync::oneshot::channel();
    let application =
        Application::open_with_gate(&config, ScanLimits::default(), None, Some(publish_receiver))
            .await
            .unwrap();
    let router = create_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;

    // Commit a Selection State and a Review Preview seed while the
    // completed scan is parked before publication.
    assert_eq!(
        post_json(
            &router,
            &format!("http://camera.local/api/photos/{}/state", ids[0]),
            serde_json::json!({"field": "selectionState", "value": "selected"}),
            Some("http://camera.local"),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let preview = application.preview(&ids[0]).await.unwrap();
    assert_eq!(preview.state, "ready");

    // Release publication. The fresh persisted read must retain both
    // committed facts instead of reverting to the scan's apply snapshot.
    drop(publish_sender);
    wait_for_scan_settled(&application).await;
    let opened = application
        .browse_open(BrowseSourceRequest::Library, None)
        .await
        .unwrap();
    let window = application
        .browse_window(&opened.token, 0, 10)
        .await
        .unwrap();
    let first = window
        .photos
        .iter()
        .find(|photo| photo.id == ids[0])
        .unwrap();
    assert_eq!(first.selection_state, "selected");
    assert_eq!(first.preview.state, "ready");
    assert_eq!(first.preview.width, Some(8));
    assert_eq!(first.preview.height, Some(4));
    application.browse_close(&opened.token);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn publication_keeps_scan_owned_invalidation_availability_and_user_state() {
    let (base, config) = prepare_fixture();
    for name in ["a.jpg", "b.jpg"] {
        jpeg_fixture(&config.library_root.join(name), 8, 4, [32, 64, 192]);
    }
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    assert_eq!(application.preview(&ids[0]).await.unwrap().state, "ready");
    assert_eq!(
        application
            .mutate_photo_state(slipstream_core::PhotoStateMutation {
                photo_id: ids[0].clone(),
                field: slipstream_core::PhotoStateField::SelectionState,
                value: slipstream_core::PhotoStateValue::Selection(SelectionState::Selected),
                expected_current: None,
                album_id: None,
            })
            .await
            .unwrap()
            .photo_id,
        ids[0]
    );

    // A changed source revision and a removed Original are scan-owned
    // facts. The publication must keep the invalidation and availability
    // while the committed user decision survives the fresh read.
    jpeg_fixture(&config.library_root.join("a.jpg"), 9, 5, [10, 20, 30]);
    fs::remove_file(config.library_root.join("b.jpg")).unwrap();
    application.rescan().await.unwrap();

    let opened = application
        .browse_open(BrowseSourceRequest::Library, None)
        .await
        .unwrap();
    let window = application
        .browse_window(&opened.token, 0, 10)
        .await
        .unwrap();
    assert_eq!(window.total, 2);
    let first = window
        .photos
        .iter()
        .find(|photo| photo.id == ids[0])
        .unwrap();
    assert_eq!(first.selection_state, "selected");
    assert_eq!(first.preview.state, "inspection-pending");
    assert_eq!(first.preview.source, None);
    assert_eq!(first.preview.width, None);
    let second = window
        .photos
        .iter()
        .find(|photo| photo.id == ids[1])
        .unwrap();
    assert!(!second.available);
    application.browse_close(&opened.token);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn fresh_library_reports_initializing_and_rejects_browse_until_first_publication() {
    let (base, config) = prepare_fixture();
    let (gate_sender, gate_receiver) = tokio::sync::oneshot::channel();
    let application =
        Application::open_with_gate(&config, ScanLimits::default(), Some(gate_receiver), None)
            .await
            .unwrap();
    let router = create_router(Arc::clone(&application), config.web_root());

    let overview: serde_json::Value = response_json(
        send(
            &router,
            Request::builder()
                .uri("http://camera.local/api/overview")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(overview["published"], false);
    assert_eq!(overview["photoCount"], 0);
    assert_eq!(overview["scan"]["state"], "initializing");

    let status: serde_json::Value = response_json(
        send(
            &router,
            Request::builder()
                .uri("http://camera.local/api/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(status["state"], "initializing");

    let overview: serde_json::Value = response_json(
        send(
            &router,
            Request::builder()
                .uri("http://camera.local/api/overview")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(overview["published"], false);
    assert_eq!(overview["photoCount"], 0);

    let rejected = post_json(
        &router,
        "/api/browse",
        serde_json::json!({"source":"library"}),
        None,
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::SERVICE_UNAVAILABLE);

    drop(gate_sender);
    wait_for_scan_settled(&application).await;
    let overview: serde_json::Value = response_json(
        send(
            &router,
            Request::builder()
                .uri("http://camera.local/api/overview")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(overview["published"], true);
    assert_eq!(overview["scan"]["state"], "idle");
    let opened = post_json(
        &router,
        "/api/browse",
        serde_json::json!({"source":"library"}),
        None,
    )
    .await;
    assert_eq!(opened.status(), StatusCode::OK);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn persisted_library_serves_immediately_while_background_rescan_runs() {
    let (base, config) = prepare_fixture();
    capture_metadata_fixture(&config.library_root.join("a.jpg"), "2026:01:01 09:00:00");
    capture_metadata_fixture(&config.library_root.join("z.jpg"), "2026:01:01 10:00:00");
    {
        let application = Application::open(&config).await.unwrap();
        wait_for_scan_settled(&application).await;
        assert_eq!(
            browse_photo_ids(&application, BrowseSourceRequest::Library)
                .await
                .len(),
            2
        );
        application.shutdown().await.unwrap();
    }

    let (gate_sender, gate_receiver) = tokio::sync::oneshot::channel();
    let application =
        Application::open_with_gate(&config, ScanLimits::default(), Some(gate_receiver), None)
            .await
            .unwrap();
    let router = create_router(Arc::clone(&application), config.web_root());

    // The published Library must be served before the background rescan
    // has run at all.
    let overview: serde_json::Value = response_json(
        send(
            &router,
            Request::builder()
                .uri("http://camera.local/api/overview")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(overview["published"], true);
    assert_eq!(overview["photoCount"], 2);
    assert_eq!(overview["scan"]["state"], "idle");

    let opened = post_json(
        &router,
        "/api/browse",
        serde_json::json!({"source":"library"}),
        None,
    )
    .await;
    assert_eq!(opened.status(), StatusCode::OK);
    let opened: serde_json::Value = response_json(opened).await;
    assert_eq!(opened["total"], 2);

    drop(gate_sender);
    wait_for_scan_settled(&application).await;
    let overview: serde_json::Value = response_json(
        send(
            &router,
            Request::builder()
                .uri("http://camera.local/api/overview")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(overview["photoCount"], 2);
    assert_eq!(overview["scan"]["state"], "idle");
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn background_scan_failure_keeps_prior_published_library_and_reports_failed() {
    let (base, config) = prepare_fixture();
    capture_metadata_fixture(&config.library_root.join("a.jpg"), "2026:01:01 09:00:00");
    {
        let application = Application::open(&config).await.unwrap();
        wait_for_scan_settled(&application).await;
        application.shutdown().await.unwrap();
    }
    fs::write(config.library_root.join("b.jpg"), b"jpeg").unwrap();
    let application = Application::open_with_gate(
        &config,
        ScanLimits::new(100, 1, 25_000).unwrap(),
        None,
        None,
    )
    .await
    .unwrap();
    wait_for_scan_settled(&application).await;
    let router = create_router(Arc::clone(&application), config.web_root());

    assert_eq!(application.scan_status().state, "failed");
    let overview: serde_json::Value = response_json(
        send(
            &router,
            Request::builder()
                .uri("http://camera.local/api/overview")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(overview["published"], true);
    assert_eq!(overview["photoCount"], 1);
    assert_eq!(overview["scan"]["state"], "failed");
    assert_eq!(
        browse_photo_ids(&application, BrowseSourceRequest::Library)
            .await
            .len(),
        1
    );

    // An explicit rescan under the same failing limit reports the failure
    // and keeps the prior published Library browsable.
    let rescanned = send(
        &router,
        Request::builder()
            .method("POST")
            .uri("http://camera.local/api/scan")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(rescanned.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(application.scan_status().state, "failed");
    assert_eq!(
        browse_photo_ids(&application, BrowseSourceRequest::Library)
            .await
            .len(),
        1
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn shutdown_drains_background_scan_before_closing() {
    let (base, config) = prepare_fixture();
    capture_metadata_fixture(&config.library_root.join("a.jpg"), "2026:01:01 09:00:00");
    let (gate_sender, gate_receiver) = tokio::sync::oneshot::channel();
    let application =
        Application::open_with_gate(&config, ScanLimits::default(), Some(gate_receiver), None)
            .await
            .unwrap();
    drop(gate_sender);
    application.shutdown().await.unwrap();
    for suffix in ["-journal", "-wal", "-shm"] {
        assert!(
            !config
                .state_directory
                .join("library.sqlite".to_owned() + suffix)
                .exists()
        );
    }
    let reopened = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&reopened).await;
    assert_eq!(reopened.published_photo_count(), 1);
    assert_eq!(reopened.scan_status().state, "idle");
    reopened.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn persisted_forty_thousand_photo_library_serves_bounded_overview_before_rescan_completes() {
    let (base, config) = prepare_fixture();
    fs::create_dir(config.state_directory.clone()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            config.state_directory.clone(),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
    }
    let database =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    database
        .execute_batch(include_str!("../../../compatibility/sqlite/schema-v4.sql"))
        .unwrap();
    database
        .execute(
            "INSERT INTO library_metadata VALUES('canonical_root',?)",
            [config.library_root.to_str().unwrap()],
        )
        .unwrap();
    database.execute("BEGIN", []).unwrap();
    for index in 0..40_000_u32 {
        let padded = format!("{index:06}");
        let original_id = format!("{:08x}", index).repeat(8);
        let photo_id = format!("{:08x}", 1_000_000 + index).repeat(8);
        let path = format!("{padded}.jpg");
        database
                .execute(
                    "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available) VALUES(?1,?2,'jpeg',1,1.0,1)",
                    rusqlite::params![original_id, path],
                )
                .unwrap();
        database
                .execute(
                    "INSERT INTO photos(id,jpeg_original_id,ambiguous,available,preview_state,sort_path) VALUES(?1,?2,0,1,'inspection-pending',?3)",
                    rusqlite::params![photo_id, original_id, path],
                )
                .unwrap();
    }
    database.execute("COMMIT", []).unwrap();
    drop(database);

    let (gate_sender, gate_receiver) = tokio::sync::oneshot::channel();
    let application =
        Application::open_with_gate(&config, ScanLimits::default(), Some(gate_receiver), None)
            .await
            .unwrap();
    let router = create_router(Arc::clone(&application), config.web_root());

    // Served from the persisted Library before the background rescan runs.
    let overview_response = send(
        &router,
        Request::builder()
            .uri("http://camera.local/api/overview")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(overview_response.status(), StatusCode::OK);
    let overview_bytes = axum::body::to_bytes(overview_response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert!(overview_bytes.len() < 20_000);
    let overview: serde_json::Value = serde_json::from_slice(&overview_bytes).unwrap();
    assert_eq!(overview["published"], true);
    assert_eq!(overview["photoCount"], 40_000);

    let opened = post_json(
        &router,
        "/api/browse",
        serde_json::json!({"source":"library"}),
        None,
    )
    .await;
    assert_eq!(opened.status(), StatusCode::OK);
    let opened: serde_json::Value = response_json(opened).await;
    let token = opened["token"].as_str().unwrap();
    let window: serde_json::Value = response_json(
        send(
            &router,
            Request::builder()
                .uri(format!(
                    "http://camera.local/api/browse/{token}?start=39940&limit=60"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(window["start"], 39_940);
    assert_eq!(window["total"], 40_000);
    assert_eq!(window["photos"].as_array().unwrap().len(), 60);

    drop(gate_sender);
    wait_for_scan_settled(&application).await;
    let overview: serde_json::Value = response_json(
        send(
            &router,
            Request::builder()
                .uri("http://camera.local/api/overview")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(overview["photoCount"], 40_000);
    assert_eq!(overview["scan"]["state"], "idle");
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

async fn send(router: &Router, request: Request<Body>) -> Response<Body> {
    tower::ServiceExt::oneshot(router.clone(), request)
        .await
        .unwrap()
}

async fn response_json(response: Response<Body>) -> serde_json::Value {
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap()
}

async fn post_json(
    router: &Router,
    uri: &str,
    body: serde_json::Value,
    origin: Option<&str>,
) -> Response<Body> {
    let uri = if uri.starts_with('/') {
        format!("http://camera.local{uri}")
    } else {
        uri.to_owned()
    };
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(origin) = origin {
        builder = builder.header(header::ORIGIN, origin);
    }
    send(router, builder.body(Body::from(body.to_string())).unwrap()).await
}

fn jpeg_fixture(path: &Path, width: u32, height: u32, color: [u8; 3]) {
    image::RgbImage::from_pixel(width, height, image::Rgb(color))
        .save_with_format(path, image::ImageFormat::Jpeg)
        .unwrap();
}

fn marker_complete_corrupt_jpeg(width: u16, height: u16) -> Vec<u8> {
    let mut bytes = vec![0xff, 0xd8, 0xff, 0xc0, 0x00, 0x11, 0x08];
    bytes.extend_from_slice(&height.to_be_bytes());
    bytes.extend_from_slice(&width.to_be_bytes());
    bytes.extend_from_slice(&[0; 11]);
    bytes.extend_from_slice(&[0xff, 0xda, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00]);
    bytes.extend_from_slice(&[0xff, 0xd9]);
    bytes
}

fn capture_metadata_fixture(path: &Path, capture_time: &str) {
    let mut value = capture_time.as_bytes().to_vec();
    value.push(0);
    let data_offset = 8 + 2 + 12 + 4;
    let mut tiff = b"II*\0\x08\0\0\0".to_vec();
    tiff.extend_from_slice(&1_u16.to_le_bytes());
    tiff.extend_from_slice(&0x9003_u16.to_le_bytes());
    tiff.extend_from_slice(&2_u16.to_le_bytes());
    tiff.extend_from_slice(&(value.len() as u32).to_le_bytes());
    tiff.extend_from_slice(&(data_offset as u32).to_le_bytes());
    tiff.extend_from_slice(&0_u32.to_le_bytes());
    tiff.extend_from_slice(&value);
    let mut payload = b"Exif\0\0".to_vec();
    payload.extend_from_slice(&tiff);
    let length = u16::try_from(payload.len() + 2).unwrap();
    let mut bytes = b"\xff\xd8\xff\xe1".to_vec();
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.extend_from_slice(&payload);
    bytes.extend_from_slice(b"\xff\xd9");
    fs::write(path, bytes).unwrap();
}

#[tokio::test]
async fn album_and_state_protocol_persists_across_reopen() {
    let (base, mut config) = prepare_fixture();
    jpeg_fixture(&config.library_root.join("a.jpg"), 8, 4, [192, 64, 32]);
    jpeg_fixture(&config.library_root.join("b.jpg"), 8, 4, [32, 192, 64]);
    jpeg_fixture(&config.library_root.join("c.jpg"), 8, 4, [32, 64, 192]);
    config.port = 0;
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = create_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    assert_eq!(ids.len(), 3);

    let created = response_json(
        post_json(
            &router,
            "/api/albums",
            serde_json::json!({"name": " Picks "}),
            Some("http://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(created["albums"][0]["name"], "Picks");
    // Mutation responses expose bounded summaries only, never members.
    assert_eq!(created["albums"][0]["photoCount"], 0);
    assert_eq!(created["albums"][0]["hasSavedPosition"], false);
    assert!(created["albums"][0]["members"].is_null());
    assert!(created["albums"][0]["lastReviewedPhotoId"].is_null());
    let set_a = created["albums"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        send(
            &router,
            Request::builder()
                .method("POST")
                .uri(format!("http://camera.local/api/albums/{set_a}/members"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ORIGIN, "http://camera.local")
                .body(Body::from(serde_json::json!({"photoIds": ids}).to_string()))
                .unwrap(),
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        post_json(
            &router,
            &format!("http://camera.local/api/albums/{set_a}/order"),
            serde_json::json!({"photoIds": [&ids[2], &ids[0], &ids[1]]}),
            Some("http://camera.local"),
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        post_json(
            &router,
            &format!("http://camera.local/api/albums/{set_a}/progress"),
            serde_json::json!({"photoId": ids[0]}),
            Some("http://camera.local"),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let created_b = response_json(
        post_json(
            &router,
            "http://camera.local/api/albums",
            serde_json::json!({"name": "Other"}),
            Some("http://camera.local"),
        )
        .await,
    )
    .await;
    let set_b = created_b["albums"]
        .as_array()
        .unwrap()
        .iter()
        .find(|set| set["name"] == "Other")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        post_json(
            &router,
            &format!("http://camera.local/api/albums/{set_b}/members"),
            serde_json::json!({"photoIds": [&ids[0]]}),
            Some("http://camera.local"),
        )
        .await
        .status(),
        StatusCode::OK
    );

    let selected = response_json(
        post_json(
            &router,
            &format!("http://camera.local/api/photos/{}/state", ids[0]),
            serde_json::json!({"field": "selectionState", "value": "selected", "albumId": set_a}),
            Some("http://camera.local"),
        )
        .await,
    )
    .await;
    let undo = selected["undo"].clone();
    assert_eq!(selected["kind"], "applied");
    assert_eq!(
        post_json(
            &router,
            &format!("http://camera.local/api/photos/{}/state", ids[0]),
            serde_json::json!({"field": "rating", "value": 4}),
            None,
        )
        .await
        .status(),
        StatusCode::OK
    );
    // Membership order is observable only through a fresh Album
    // Browse Snapshot; the mutation responses stay summary-only.
    let ordered = browse_photo_ids(&application, BrowseSourceRequest::Album(set_a.clone())).await;
    assert_eq!(
        ordered,
        vec![ids[2].clone(), ids[0].clone(), ids[1].clone()]
    );
    let set_b_photos =
        browse_summaries(&application, BrowseSourceRequest::Album(set_b.clone())).await;
    let shared = set_b_photos
        .iter()
        .find(|photo| photo.id == ids[0])
        .unwrap();
    assert_eq!(shared.selection_state, "selected");
    assert_eq!(shared.rating, 4);
    assert_eq!(
        post_json(
            &router,
            &format!("http://camera.local/api/photos/{}/state", ids[0]),
            serde_json::json!({
                "field": undo["field"],
                "value": undo["priorValue"],
                "expectedCurrent": undo["expectedCurrent"]
            }),
            Some("http://camera.local"),
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        post_json(
            &router,
            &format!("http://camera.local/api/photos/{}/state", ids[0]),
            serde_json::json!({"field": "selectionState", "value": "rejected"}),
            None,
        )
        .await
        .status(),
        StatusCode::OK
    );
    let conflict = response_json(
            post_json(
                &router,
                &format!("http://camera.local/api/photos/{}/state", ids[0]),
                serde_json::json!({"field": "selectionState", "value": "selected", "expectedCurrent": "undecided"}),
                None,
            )
            .await,
        )
        .await;
    assert_eq!(
        conflict,
        serde_json::json!({"error": "Mutation conflicts with current state"})
    );

    assert_eq!(
        post_json(
            &router,
            &format!("http://camera.local/api/albums/{set_a}/members/remove"),
            serde_json::json!({"photoId": ids[0]}),
            None,
        )
        .await
        .status(),
        StatusCode::OK
    );
    // Removing the saved-position Photo clears the persisted progress;
    // the summary-only mutation response proves the cleared flag.
    let set_a_summary = application
        .albums()
        .await
        .unwrap()
        .albums
        .into_iter()
        .find(|set| set.id == set_a)
        .unwrap();
    assert!(!set_a_summary.has_saved_position);
    let before_original = fs::read(config.library_root.join("b.jpg")).unwrap();
    assert_eq!(
        post_json(
            &router,
            &format!("http://camera.local/api/albums/{set_a}/delete"),
            serde_json::json!({}),
            None,
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        fs::read(config.library_root.join("b.jpg")).unwrap(),
        before_original
    );
    application.shutdown().await.unwrap();

    let reopened = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&reopened).await;
    assert_eq!(
        browse_photo_ids(&reopened, BrowseSourceRequest::Library)
            .await
            .len(),
        3
    );
    let persisted = published_photo_summary(&reopened, &ids[0]).await;
    assert_eq!(persisted.selection_state, "rejected");
    assert_eq!(persisted.rating, 4);
    assert!(
        reopened
            .albums()
            .await
            .unwrap()
            .albums
            .iter()
            .all(|set| set.id != set_a)
    );
    reopened.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn unbounded_library_routes_are_retired() {
    let (base, config) = prepare_fixture();
    jpeg_fixture(&config.library_root.join("a.jpg"), 8, 4, [32, 64, 192]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = create_router(Arc::clone(&application), config.web_root());
    for uri in [
        "http://camera.local/api/photos",
        "http://camera.local/api/albums",
    ] {
        let response = send(
            &router,
            Request::builder().uri(uri).body(Body::empty()).unwrap(),
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
        Request::builder()
            .method("DELETE")
            .uri("http://camera.local/api/photos")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    // The request policy admits DELETE only for /api/browse/{token}, so
    // the retired list endpoint is rejected 405 before routing instead
    // of reaching the API 404 fallback.
    assert_eq!(deleted.status(), StatusCode::METHOD_NOT_ALLOWED);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn mutation_origin_validation_precedes_validation_and_scan_has_no_body() {
    let (base, config) = prepare_fixture();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = create_router(Arc::clone(&application), config.web_root());
    let foreign = response_json(
        post_json(
            &router,
            "http://camera.local/api/albums/not-an-id/delete",
            serde_json::json!("not-an-object"),
            Some("https://foreign.example"),
        )
        .await,
    )
    .await;
    assert_eq!(
        foreign,
        serde_json::json!({"error": "Cross-origin mutation rejected"})
    );
    let malformed = response_json(
        post_json(
            &router,
            "/api/albums",
            serde_json::json!({"name": "x"}),
            Some("not an origin"),
        )
        .await,
    )
    .await;
    assert_eq!(
        malformed,
        serde_json::json!({"error": "Cross-origin mutation rejected"})
    );
    assert_eq!(
        post_json(&router, "/api/scan", serde_json::json!(null), None,)
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
                None,
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
async fn mutation_body_limits_and_json_errors_are_rejected_before_writes() {
    let (base, config) = prepare_fixture();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = create_router(Arc::clone(&application), config.web_root());
    let request = |body: Body, length: Option<&str>| {
        let mut builder = Request::builder()
            .method("POST")
            .uri("http://camera.local/api/albums")
            .header(header::ORIGIN, "http://camera.local")
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
        request(Body::from(r#"{"name":"Never"}"#), Some("not-a-number"))
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
            Request::builder()
                .method("POST")
                .uri("http://camera.local/api/scan")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    // The scan response is Loading Status and carries no Photo facts.
    assert_eq!(
        scan,
        serde_json::json!({"state": "idle", "completed": 0, "total": 0})
    );
    assert_eq!(application.albums().await.unwrap().albums.len(), 0);
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
    let router = create_router(Arc::clone(&application), config.web_root());
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .into_iter()
        .next()
        .unwrap();
    let preview = response_json(
        send(
            &router,
            Request::builder()
                .uri(format!("http://camera.local/api/photos/{photo_id}/preview"))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(preview["state"], "ready");
    assert_eq!(preview["source"], "matching-jpeg");
    assert_eq!(preview["stale"], false);
    let url = preview["url"].as_str().unwrap().to_owned();
    let key = url.rsplit('/').next().unwrap().trim_end_matches(".jpg");
    let summary = published_photo_summary(&application, &photo_id).await;
    assert_eq!(summary.preview.state, "ready");
    assert_eq!(summary.preview.url.as_deref(), Some(url.as_str()));
    let derivative = send(
        &router,
        Request::builder()
            .uri(format!("http://camera.local{url}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(derivative.status(), StatusCode::OK);
    assert_eq!(derivative.headers()[header::CONTENT_TYPE], "image/jpeg");
    assert_eq!(
        derivative.headers()[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );
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
        .join("rust-vips-v1")
        .join(format!("{key}.jpg"));
    fs::write(&cache_path, marker_complete_corrupt_jpeg(90, 45)).unwrap();
    let repaired = send(
        &router,
        Request::builder()
            .uri(format!("http://camera.local{url}"))
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
            Request::builder()
                .uri(format!("http://camera.local/api/photos/{photo_id}/preview"))
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
            Request::builder()
                .uri(format!("http://camera.local{url}"))
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
        Request::builder()
            .method("HEAD")
            .uri(format!("http://camera.local{url}"))
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
        Request::builder()
            .uri(format!("http://camera.local{url}"))
            .header(header::IF_NONE_MATCH, etag)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(not_modified.status(), StatusCode::NOT_MODIFIED);
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
            Request::builder()
                .method("POST")
                .uri("http://camera.local/api/scan")
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
            Request::builder()
                .uri(format!("http://camera.local/api/photos/{photo_id}/preview"))
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
            Request::builder()
                .uri(format!("http://camera.local{url}"))
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
            Request::builder()
                .uri(format!("http://camera.local{changed_url}"))
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
        Request::builder()
            .method("POST")
            .uri("http://camera.local/api/scan")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let stale = response_json(
        send(
            &router,
            Request::builder()
                .uri(format!("http://camera.local/api/photos/{photo_id}/preview"))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(stale["state"], "ready");
    assert_eq!(stale["stale"], true);
    assert_eq!(stale["source"], "matching-jpeg");
    assert_eq!(stale["url"], changed_url);
    assert!(stale["message"].as_str().unwrap().contains("stale"));

    std::thread::sleep(std::time::Duration::from_millis(10));
    fs::remove_file(&original).unwrap();
    send(
        &router,
        Request::builder()
            .method("POST")
            .uri("http://camera.local/api/scan")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let unavailable = response_json(
        send(
            &router,
            Request::builder()
                .uri(format!("http://camera.local/api/photos/{photo_id}/preview"))
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
    let router = create_router(Arc::clone(&application), config.web_root());
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .into_iter()
        .next()
        .unwrap();

    let first = response_json(
        send(
            &router,
            Request::builder()
                .uri(format!("http://camera.local/api/photos/{photo_id}/preview"))
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
        assert_eq!(
            photo.preview_candidate,
            Some(PreviewCandidate::MatchingJpeg)
        );
        assert_eq!(photo.preview_source, Some(PreviewCandidate::MatchingJpeg));
        assert!(photo.preview_source_revision.is_some());
    }

    // The second request must use the durable seed without reopening the
    // Original. Removing it makes any accidental slow-path inspection visible
    // as a different Original-unavailable response.
    fs::remove_file(&original).unwrap();
    let second = response_json(
        send(
            &router,
            Request::builder()
                .uri(format!("http://camera.local/api/photos/{photo_id}/preview"))
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
    assert!(thumbnail_url.starts_with("/api/derivatives/"));
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
    let router = create_router(Arc::clone(&application), config.web_root());
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .into_iter()
        .next()
        .unwrap();
    let preview = response_json(
        send(
            &router,
            Request::builder()
                .uri(format!("http://camera.local/api/photos/{photo_id}/preview"))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(preview["state"], "ready");
    assert_eq!(preview["source"], "matching-jpeg");
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
        ("ready", Some("matching-jpeg"), Some(90), Some(45))
    );

    let thumbnail = response_json(
        send(
            &router,
            Request::builder()
                .uri(format!(
                    "http://camera.local/api/photos/{photo_id}/thumbnail"
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
    let router = create_router(Arc::clone(&application), config.web_root());
    let reopened = response_json(
        send(
            &router,
            Request::builder()
                .uri(format!(
                    "http://camera.local/api/photos/{photo_id}/thumbnail"
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
            Request::builder()
                .uri(format!("http://camera.local/api/photos/{photo_id}/preview"))
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
