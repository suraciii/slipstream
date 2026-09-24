use super::*;
use crate::folders::MAXIMUM_FILE_LOCATION_WINDOW;
use std::{
    collections::{BTreeSet, HashMap},
    fs,
    io::{ErrorKind, Read, Write},
    path::PathBuf,
    sync::{
        Condvar, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
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
        public_origin: "https://camera.local".to_owned(),
        port,
        web_root: Some(web_root),
        processing: None,
    }
}

/// Waits until the background scan opened by `Application::open` has
/// completed, so tests observe the same published state an operator sees
/// once startup work settles. Deterministic even when the scan finishes
/// between status polls.
async fn wait_for_scan_settled(application: &Application) {
    application.access.seed_test_token();
    let started = application.shared.runs_started.load(Ordering::Relaxed);
    wait_for_scan_runs(application, started.max(1)).await;
}

async fn wait_for_scan_runs(application: &Application, target: u64) {
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

/// The default view order for one Browse source, matching the HTTP layer.
fn default_order(source: &BrowseSourceRequest) -> BrowseViewOrder {
    match source {
        BrowseSourceRequest::Album(_) => BrowseViewOrder::AlbumOrder,
        BrowseSourceRequest::Library | BrowseSourceRequest::Folder { .. } => {
            BrowseViewOrder::CaptureTimeAscending
        }
    }
}

/// Bounded traversal of one Browse source. Tests must observe Library
/// state through the bounded protocol, never a complete-Photo route.
async fn browse_summaries(
    application: &Application,
    source: BrowseSourceRequest,
) -> Vec<PhotoSummary> {
    let order = default_order(&source);
    let opened = application
        .browse_open(source, order, BrowseSelectionFilter::All, None)
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

/// The protocol success fixtures contain one RAW/JPEG pair and one JPEG-only
/// Photo. Their capture times make the descending view visibly reorder the
/// same two identities, while the pair proves RAW filename and Original
/// hydration at the HTTP boundary.
fn prepare_populated_fixture() -> (PathBuf, Config) {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    fs::write(root.join("pair.ARW"), b"compatibility raw fixture").unwrap();
    jpeg_fixture_with_capture_time(
        &root.join("pair.JPG"),
        90,
        45,
        [192, 64, 32],
        "2026:01:01 09:00:00",
    );
    jpeg_fixture_with_capture_time(
        &root.join("later.JPG"),
        120,
        60,
        [32, 192, 64],
        "2026:01:01 10:00:00",
    );
    (base, config)
}

fn environment(values: &[(&str, &str)]) -> HashMap<String, String> {
    values
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

#[test]
fn startup_defaults_to_loopback_and_allows_custom_host() {
    let defaults = Config::from_env(environment(&[
        ("SLIPSTREAM_LIBRARY_ROOT", "/photos"),
        ("SLIPSTREAM_STATE_DIRECTORY", "/state"),
        ("SLIPSTREAM_CACHE_DIRECTORY", "/cache"),
        ("SLIPSTREAM_PUBLIC_ORIGIN", "https://camera.local"),
    ]))
    .unwrap();
    assert_eq!(defaults.library_root, PathBuf::from("/photos"));
    assert_eq!(defaults.database_basename, "library.sqlite");
    assert_eq!((defaults.host.as_str(), defaults.port), ("127.0.0.1", 3000));
    assert_eq!(defaults.processing, None);
    let explicit = Config::from_env(environment(&[
        ("SLIPSTREAM_LIBRARY_ROOT", "/photos"),
        ("SLIPSTREAM_STATE_DIRECTORY", "/state"),
        ("SLIPSTREAM_CACHE_DIRECTORY", "/cache"),
        ("SLIPSTREAM_PUBLIC_ORIGIN", "https://camera.local"),
        ("SLIPSTREAM_DATABASE_BASENAME", "review.sqlite"),
        ("SLIPSTREAM_HOST", "0.0.0.0"),
        ("SLIPSTREAM_PORT", "8080"),
    ]))
    .unwrap();
    assert_eq!(explicit.database_basename, "review.sqlite");
    assert_eq!((explicit.host.as_str(), explicit.port), ("0.0.0.0", 8080));
}

#[test]
fn processing_startup_requires_complete_canonical_identity_pins() {
    let base = vec![
        ("SLIPSTREAM_LIBRARY_ROOT".to_owned(), "/photos".to_owned()),
        ("SLIPSTREAM_STATE_DIRECTORY".to_owned(), "/state".to_owned()),
        ("SLIPSTREAM_CACHE_DIRECTORY".to_owned(), "/cache".to_owned()),
        (
            "SLIPSTREAM_PUBLIC_ORIGIN".to_owned(),
            "https://camera.local".to_owned(),
        ),
    ];
    let mut values = base.clone();
    values.extend([
        (
            "SLIPSTREAM_PROCESSING_INSTANCE".to_owned(),
            "0123456789abcdef0123456789abcdef".to_owned(),
        ),
        (
            "SLIPSTREAM_PROCESSING_POLICY_SHA256".to_owned(),
            "b b".to_owned(),
        ),
        (
            "SLIPSTREAM_PROCESSING_BUNDLE_SHA256".to_owned(),
            "c".to_owned(),
        ),
    ]);
    assert_eq!(
        Config::from_env(values),
        Err(ConfigError::Invalid("SLIPSTREAM_PROCESSING_POLICY_SHA256"))
    );

    let mut values = base.clone();
    values.push((
        "SLIPSTREAM_PROCESSING_INSTANCE".to_owned(),
        "0123456789abcdef0123456789abcdef".to_owned(),
    ));
    assert_eq!(
        Config::from_env(values),
        Err(ConfigError::Missing("SLIPSTREAM_PROCESSING_POLICY_SHA256"))
    );

    let mut values = base;
    values.extend([
        (
            "SLIPSTREAM_PROCESSING_INSTANCE".to_owned(),
            "0123456789abcdef0123456789abcdef".to_owned(),
        ),
        (
            "SLIPSTREAM_PROCESSING_POLICY_SHA256".to_owned(),
            "b".repeat(64),
        ),
        (
            "SLIPSTREAM_PROCESSING_BUNDLE_SHA256".to_owned(),
            "c".repeat(64),
        ),
    ]);
    let config = Config::from_env(values).unwrap();
    let processing = config.processing.unwrap();
    assert_eq!(processing.instance, "0123456789abcdef0123456789abcdef");
    assert_eq!(
        processing.socket_path(),
        PathBuf::from("/run/slipstream-processing/0123456789abcdef0123456789abcdef/launcher.sock")
    );
}

#[test]
fn offline_expansion_config_requires_only_storage_settings() {
    let expansion = ExpansionConfig::from_env(environment(&[
        ("SLIPSTREAM_LIBRARY_ROOT", "/photos"),
        ("SLIPSTREAM_STATE_DIRECTORY", "/state"),
        ("SLIPSTREAM_CACHE_DIRECTORY", "/cache"),
    ]))
    .unwrap();
    assert_eq!(expansion.library_root, PathBuf::from("/photos"));
    assert_eq!(expansion.database_basename, "library.sqlite");
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
        ("SLIPSTREAM_PUBLIC_ORIGIN", "https://camera.local"),
    ]));
    assert_eq!(
        relative,
        Err(ConfigError::NotAbsolute("SLIPSTREAM_LIBRARY_ROOT"))
    );
    let invalid = Config::from_env(environment(&[
        ("SLIPSTREAM_LIBRARY_ROOT", "/photos"),
        ("SLIPSTREAM_STATE_DIRECTORY", "/state"),
        ("SLIPSTREAM_CACHE_DIRECTORY", "/cache"),
        ("SLIPSTREAM_PUBLIC_ORIGIN", "https://camera.local"),
        ("SLIPSTREAM_PORT", "65536"),
    ]));
    assert_eq!(invalid, Err(ConfigError::Invalid("SLIPSTREAM_PORT")));
}

#[tokio::test]
async fn post_to_static_path_is_rejected_without_reading_or_mutating() {
    let (base, config) = prepare_fixture();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let response = tower::ServiceExt::oneshot(
        router,
        authenticated_request()
            .method("POST")
            .uri("https://camera.local/")
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
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let response = tower::ServiceExt::oneshot(
        router,
        authenticated_request()
            .uri("https://camera.local/escape.txt")
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
        processing: None,
    });
    let response = tower::ServiceExt::oneshot(
        app.clone(),
        authenticated_request()
            .uri("https://camera.local/")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
    let response = tower::ServiceExt::oneshot(
        app,
        authenticated_request()
            .method("HEAD")
            .uri("https://camera.local/assets/app.js")
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

#[tokio::test]
async fn installation_resources_revalidate_and_never_fall_back_to_html() {
    let base = unique_base();
    let root = base.join("web");
    fs::create_dir_all(root.join("icons")).unwrap();
    fs::create_dir_all(base.join("originals")).unwrap();
    fs::write(root.join("index.html"), b"<main>web</main>").unwrap();
    fs::write(
        root.join("manifest.webmanifest"),
        b"{\"name\":\"Slipstream\"}",
    )
    .unwrap();
    fs::write(root.join("icons/app.png"), b"icon bytes").unwrap();
    fs::write(root.join("public.txt"), b"mutable public file").unwrap();
    let application = Application::open(&test_config(&base, root.clone(), 3000))
        .await
        .unwrap();
    let app = Router::new().fallback(static_web).with_state(HttpState {
        application: Arc::clone(&application),
        web_root: Arc::new(open_web_root(root.clone())),
        processing: None,
    });
    for (path, content_type, expected) in [
        (
            "/manifest.webmanifest",
            "application/manifest+json",
            "{\"name\":\"Slipstream\"}",
        ),
        ("/icons/app.png", "image/png", "icon bytes"),
        (
            "/public.txt",
            "application/octet-stream",
            "mutable public file",
        ),
        (
            "/review/session",
            "text/html; charset=utf-8",
            "<main>web</main>",
        ),
    ] {
        for method in ["GET", "HEAD"] {
            let response = tower::ServiceExt::oneshot(
                app.clone(),
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{method} {path}");
            assert_eq!(response.headers()[header::CONTENT_TYPE], content_type);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
            assert_eq!(
                response.headers()[header::CONTENT_LENGTH],
                expected.len().to_string()
            );
            let bytes = axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap();
            assert_eq!(
                bytes.as_ref(),
                if method == "HEAD" {
                    b""
                } else {
                    expected.as_bytes()
                }
            );
        }
    }
    // Stable metadata URLs must return the new deployment, not an immutable old body.
    fs::write(root.join("manifest.webmanifest"), b"{\"name\":\"Updated\"}").unwrap();
    let response = tower::ServiceExt::oneshot(
        app.clone(),
        Request::builder()
            .uri("/manifest.webmanifest")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap()
            .as_ref(),
        b"{\"name\":\"Updated\"}"
    );
    fs::remove_file(root.join("manifest.webmanifest")).unwrap();
    for path in ["/manifest.webmanifest", "/icons/missing.png"] {
        for method in ["GET", "HEAD"] {
            let response = tower::ServiceExt::oneshot(
                app.clone(),
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-cache");
            let bytes = axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap();
            assert_eq!(
                bytes.as_ref(),
                if method == "HEAD" {
                    b"".as_slice()
                } else {
                    b"Not found".as_slice()
                }
            );
        }
    }
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

fn substitute_protocol_captures(
    value: &serde_json::Value,
    album_id: &str,
    publication: &str,
) -> serde_json::Value {
    match value {
        serde_json::Value::String(text) => serde_json::Value::String(
            text.replace("$albumId", album_id)
                .replace("$publication", publication),
        ),
        serde_json::Value::Array(values) => serde_json::Value::Array(
            values
                .iter()
                .map(|item| substitute_protocol_captures(item, album_id, publication))
                .collect(),
        ),
        serde_json::Value::Object(entries) => serde_json::Value::Object(
            entries
                .iter()
                .map(|(name, item)| {
                    (
                        name.clone(),
                        substitute_protocol_captures(item, album_id, publication),
                    )
                })
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
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let vectors: Vec<serde_json::Value> = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../compatibility/protocol/vectors.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let mut captured_album_id = String::new();
    let mut captured_publication = String::new();
    for vector in vectors {
        let request_definition = &vector["request"];
        let method = request_definition["method"].as_str().unwrap();
        let path = request_definition["path"].as_str().unwrap();
        let mut builder = authenticated_request()
            .method(method)
            .uri(format!("https://camera.local{path}"));
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
            if captured_album_id.is_empty()
                && let Some(id) = actual["albums"][0]["id"].as_str()
            {
                captured_album_id = id.to_owned();
            }
            if captured_publication.is_empty()
                && let Some(publication) = actual
                    .get("publication")
                    .or_else(|| actual.get("scan")?.get("publication"))
                    .and_then(|value| value.as_str())
            {
                captured_publication = publication.to_owned();
            }
            let expected = substitute_protocol_captures(
                &serde_json::to_value(expected).unwrap(),
                &captured_album_id,
                &captured_publication,
            );
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

fn reverse_substitute(
    value: &serde_json::Value,
    captures: &HashMap<String, String>,
) -> serde_json::Value {
    // Replace the longest capture values first so URLs collapse before the
    // photo IDs they contain.
    let mut by_value: Vec<(&String, &String)> = captures.iter().collect();
    by_value.sort_by_key(|(_, value)| std::cmp::Reverse(value.len()));
    fn walk(value: &serde_json::Value, by_value: &[(&String, &String)]) -> serde_json::Value {
        match value {
            serde_json::Value::String(text) => {
                let mut result = text.clone();
                for (placeholder, value) in by_value {
                    result = result.replace(value.as_str(), &format!("${placeholder}"));
                }
                serde_json::Value::String(result)
            }
            serde_json::Value::Array(values) => {
                serde_json::Value::Array(values.iter().map(|item| walk(item, by_value)).collect())
            }
            serde_json::Value::Object(entries) => serde_json::Value::Object(
                entries
                    .iter()
                    .map(|(name, item)| (name.clone(), walk(item, by_value)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    walk(value, &by_value)
}

#[tokio::test]
async fn browse_protocol_fixtures_execute_with_captured_token() {
    let vectors: Vec<serde_json::Value> = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../compatibility/protocol/browse-vectors.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(vectors.len(), 53);
    fn substitute(
        value: &serde_json::Value,
        captures: &HashMap<String, String>,
    ) -> serde_json::Value {
        match value {
            serde_json::Value::String(text) => {
                let mut result = text.clone();
                for (placeholder, replacement) in captures {
                    result = result.replace(&format!("${placeholder}"), replacement);
                }
                serde_json::Value::String(result)
            }
            serde_json::Value::Array(values) => serde_json::Value::Array(
                values
                    .iter()
                    .map(|item| substitute(item, captures))
                    .collect(),
            ),
            serde_json::Value::Object(entries) => serde_json::Value::Object(
                entries
                    .iter()
                    .map(|(name, item)| (name.clone(), substitute(item, captures)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    let (base, config) = prepare_populated_fixture();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let photo_ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    assert_eq!(
        photo_ids.len(),
        3,
        "populated protocol fixture must have three Photos"
    );
    let album = application
        .mutate_album(slipstream_core::AlbumMutation::Create {
            name: "Compat Album".to_owned(),
        })
        .await
        .unwrap();
    let album_id = album.albums[0].id.clone();
    application
        .mutate_album(slipstream_core::AlbumMutation::AddMembers {
            album_id: album_id.clone(),
            photo_ids: vec![photo_ids[1].clone(), photo_ids[0].clone()],
        })
        .await
        .unwrap();
    // The two JPEG Photos preview from their own bytes; the RAW fixture is
    // arbitrary non-RAW bytes and stays terminally unavailable.
    for (index, photo_id) in photo_ids.iter().enumerate() {
        let preview = application.preview(photo_id).await.unwrap();
        if index == 2 {
            assert_ne!(preview.state, "ready", "RAW fixture cannot preview");
            continue;
        }
        assert_eq!(preview.state, "ready", "fixture Preview must be ready");
        let thumbnail = application.thumbnail(photo_id).await.unwrap();
        assert_eq!(thumbnail.state, "ready", "fixture Thumbnail must be ready");
    }
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let vectors: Vec<serde_json::Value> = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../compatibility/protocol/browse-vectors.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(vectors.len(), 53);
    let mut captures = HashMap::from([
        ("albumId".to_owned(), album_id),
        ("photoId".to_owned(), photo_ids[0].clone()),
        ("secondPhotoId".to_owned(), photo_ids[1].clone()),
        ("thirdPhotoId".to_owned(), photo_ids[2].clone()),
    ]);
    let regenerate = std::env::var("SLIPSTREAM_REGENERATE_PROTOCOL").is_ok();
    let mut regenerated: Vec<serde_json::Value> = Vec::new();
    let mut token = String::new();
    let mut publication = String::new();
    for vector in vectors {
        let name = vector["name"].as_str().unwrap().to_owned();
        let request_definition = &vector["request"];
        let method = request_definition["method"].as_str().unwrap();
        let path = substitute(
            &serde_json::Value::String(request_definition["path"].as_str().unwrap().to_owned()),
            &captures,
        )
        .as_str()
        .unwrap()
        .to_owned();
        let mut builder = authenticated_request()
            .method(method)
            .uri(format!("https://camera.local{path}"));
        if let Some(headers) = request_definition["headers"].as_object() {
            for (header_name, value) in headers {
                builder = builder.header(header_name, value.as_str().unwrap());
            }
        }
        let body = request_definition
            .get("body")
            .map(|body| Body::from(serde_json::to_vec(&substitute(body, &captures)).unwrap()))
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
        let body = axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap();
        let actual: Option<serde_json::Value> = if vector["expected"]["body"].is_object() {
            Some(serde_json::from_slice(&body).unwrap())
        } else {
            None
        };
        if let Some(actual_value) = actual.as_ref() {
            if publication.is_empty()
                && let Some(captured) = actual_value
                    .get("publication")
                    .or_else(|| {
                        actual_value
                            .get("scan")
                            .and_then(|scan| scan.get("publication"))
                    })
                    .and_then(|value| value.as_str())
            {
                publication = captured.to_owned();
                captures.insert("publication".to_owned(), publication.clone());
            }
            if method == "POST"
                && path == "/api/browse"
                && let Some(new_token) = actual_value.get("token").and_then(|value| value.as_str())
            {
                token = new_token.to_owned();
                captures.insert("token".to_owned(), token.clone());
                assert!(token.len() >= 36, "{name} token is not opaque");
            }
            if let Some(photos) = actual_value
                .get("photos")
                .and_then(|value| value.as_array())
            {
                for (index, photo) in photos.iter().take(3).enumerate() {
                    let fields = match index {
                        0 => [
                            ("photoId", "id"),
                            ("reviewUrl", "preview.url"),
                            ("thumbnailUrl", "preview.thumbnailUrl"),
                        ],
                        1 => [
                            ("secondPhotoId", "id"),
                            ("secondReviewUrl", "preview.url"),
                            ("secondThumbnailUrl", "preview.thumbnailUrl"),
                        ],
                        _ => [
                            ("thirdPhotoId", "id"),
                            ("thirdReviewUrl", "preview.url"),
                            ("thirdThumbnailUrl", "preview.thumbnailUrl"),
                        ],
                    };
                    for (placeholder, field) in fields {
                        let value = field
                            .split('.')
                            .try_fold(photo, |value, key| value.get(key))
                            .and_then(|value| value.as_str());
                        if let Some(value) = value {
                            captures
                                .entry(placeholder.to_owned())
                                .or_insert_with(|| value.to_owned());
                        }
                    }
                }
            }
            if let Some(url) = actual_value.get("url").and_then(|value| value.as_str()) {
                let placeholder = if url.contains("/thumbnail/") {
                    "thumbnailUrl"
                } else if url.contains("/review/") {
                    "reviewUrl"
                } else {
                    ""
                };
                if !placeholder.is_empty() {
                    captures
                        .entry(placeholder.to_owned())
                        .or_insert_with(|| url.to_owned());
                }
            }
        }
        if regenerate {
            let mut updated = vector.clone();
            if let Some(actual_value) = actual.as_ref() {
                updated["expected"]["body"] = reverse_substitute(actual_value, &captures);
            }
            regenerated.push(updated);
            continue;
        }
        if let (Some(actual_value), Some(expected)) =
            (actual.as_ref(), vector["expected"]["body"].as_object())
        {
            let expected = substitute(&serde_json::Value::Object(expected.clone()), &captures);
            assert_eq!(
                actual_value.as_object().unwrap(),
                expected.as_object().unwrap(),
                "{name}"
            );
        }
    }
    if regenerate {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../compatibility/protocol/browse-vectors.json");
        fs::write(&path, serde_json::to_vec_pretty(&regenerated).unwrap()).unwrap();
        application.shutdown().await.unwrap();
        let _ = fs::remove_dir_all(base);
        return;
    }
    assert!(!token.is_empty(), "fixtures must exercise a captured token");
    assert!(captures.contains_key("albumId"));
    assert!(captures.contains_key("photoId"));
    assert!(captures.contains_key("reviewUrl"));
    assert!(captures.contains_key("thumbnailUrl"));
    assert!(
        !publication.is_empty(),
        "fixtures must exercise a captured publication"
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn response_goldens_match_real_serialized_routes() {
    let goldens: Vec<serde_json::Value> = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../compatibility/protocol/responses.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(goldens.len(), 9);

    fn substitute(
        value: &serde_json::Value,
        captures: &HashMap<String, String>,
    ) -> serde_json::Value {
        match value {
            serde_json::Value::String(text) => {
                let mut result = text.clone();
                for (placeholder, replacement) in captures {
                    result = result.replace(&format!("${placeholder}"), replacement);
                }
                serde_json::Value::String(result)
            }
            serde_json::Value::Array(values) => serde_json::Value::Array(
                values
                    .iter()
                    .map(|item| substitute(item, captures))
                    .collect(),
            ),
            serde_json::Value::Object(entries) => serde_json::Value::Object(
                entries
                    .iter()
                    .map(|(name, item)| (name.clone(), substitute(item, captures)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    fn assert_golden(
        goldens: &[serde_json::Value],
        index: usize,
        actual: &serde_json::Value,
        captures: &HashMap<String, String>,
    ) {
        if std::env::var("SLIPSTREAM_REGENERATE_PROTOCOL").is_ok() {
            let updated = reverse_substitute(actual, captures);
            let slot = goldens
                .last()
                .and_then(|last| last.as_object())
                .and_then(|object| object.get("__regenerated"))
                .and_then(|value| value.as_array())
                .map(|values| values.len())
                .unwrap_or(0);
            let _ = slot;
            REGOLDED.with(|cell| {
                let mut map = cell.borrow_mut();
                map.insert(index, updated);
            });
            return;
        }
        assert_eq!(
            actual,
            &substitute(&goldens[index], captures),
            "response golden {index}"
        );
    }

    thread_local! {
        static REGOLDED: std::cell::RefCell<HashMap<usize, serde_json::Value>> =
            std::cell::RefCell::new(HashMap::new());
    }

    let (base, config) = prepare_populated_fixture();
    let root = &config.library_root;
    oversized_jpeg_fixture(&root.join("failed.JPG"));
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let summaries = browse_summaries(&application, BrowseSourceRequest::Library).await;
    let photo_id_for = |filename: &str| {
        summaries
            .iter()
            .find(|photo| photo.original_filename.as_deref() == Some(filename))
            .unwrap_or_else(|| panic!("fixture is missing {filename}"))
            .id
            .clone()
    };
    let photo_id = photo_id_for("pair.JPG");
    let later_id = photo_id_for("later.JPG");
    let failed_id = photo_id_for("failed.JPG");

    let created = response_json(
        post_json(
            &router,
            "/api/albums",
            serde_json::json!({"name":"Review"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let album_id = created["albums"][0]["id"].as_str().unwrap().to_owned();
    let added = response_json(
        post_json(
            &router,
            &format!("/api/albums/{album_id}/members"),
            serde_json::json!({"photoIds":[photo_id.clone()]}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let mut captures = HashMap::from([
        ("albumId".to_owned(), album_id),
        ("photoId".to_owned(), photo_id.clone()),
    ]);
    assert_golden(&goldens, 2, &added, &captures);

    let opened = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source":"album","albumId":captures["albumId"]}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let token = opened["token"].as_str().unwrap().to_owned();
    let pending = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/browse/{token}?start=0&limit=1"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_golden(&goldens, 0, &pending, &captures);

    let pair_current = response_json(
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
    assert_eq!(pair_current["state"], "ready");
    captures.insert(
        "reviewUrl".to_owned(),
        pair_current["url"].as_str().unwrap().to_owned(),
    );

    let current = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{later_id}/preview"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(current["state"], "ready");
    assert_eq!(current["width"], 120);
    assert_eq!(current["height"], 60);
    captures.insert("secondPhotoId".to_owned(), later_id.clone());
    captures.insert(
        "secondReviewUrl".to_owned(),
        current["url"].as_str().unwrap().to_owned(),
    );
    let thumbnail = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{photo_id}/thumbnail"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(thumbnail["state"], "ready");
    captures.insert(
        "thumbnailUrl".to_owned(),
        thumbnail["url"].as_str().unwrap().to_owned(),
    );
    let ready = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/browse/{token}?start=0&limit=1"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_golden(&goldens, 1, &ready, &captures);
    assert_golden(&goldens, 3, &current, &captures);

    let set_two = response_json(
        post_json(
            &router,
            &format!("/api/photos/{photo_id}/state"),
            serde_json::json!({"field":"rating","value":2}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(set_two["kind"], "applied");
    let set_four = response_json(
        post_json(
            &router,
            &format!("/api/photos/{photo_id}/state"),
            serde_json::json!({"field":"rating","value":4}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_golden(&goldens, 7, &set_four, &captures);
    let batch = response_json(
        post_json(
            &router,
            "/api/photos/state",
            serde_json::json!({
                "photos":[
                    {"photoId":photo_id.clone(),"expectedCurrent":"undecided"},
                    {"photoId":"00000000-0000-4000-8000-000000000000","expectedCurrent":"undecided"}
                ],
                "selectionState":"selected"
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_golden(&goldens, 8, &batch, &captures);

    jpeg_fixture(&root.join("later.JPG"), 140, 70, [32, 192, 64]);
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
                    "https://camera.local/api/photos/{later_id}/preview"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(changed["state"], "ready");
    assert_eq!(changed["width"], 140);
    assert_eq!(changed["height"], 70);
    captures.insert(
        "secondReviewUrl".to_owned(),
        changed["url"].as_str().unwrap().to_owned(),
    );
    assert_eq!(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local{}",
                    changed["url"].as_str().unwrap()
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .status(),
        StatusCode::OK
    );

    fs::write(root.join("later.JPG"), b"malformed replacement").unwrap();
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
    let stale = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{later_id}/preview"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_golden(&goldens, 4, &stale, &captures);

    let failed = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{failed_id}/preview"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_golden(&goldens, 5, &failed, &captures);

    fs::remove_file(root.join("later.JPG")).unwrap();
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
    let unavailable = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{later_id}/preview"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_golden(&goldens, 6, &unavailable, &captures);

    if std::env::var("SLIPSTREAM_REGENERATE_PROTOCOL").is_ok() {
        REGOLDED.with(|cell| {
            let map = cell.borrow();
            let mut updated: Vec<serde_json::Value> = goldens.clone();
            for (index, value) in map.iter() {
                updated[*index] = value.clone();
            }
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../compatibility/protocol/responses.json");
            fs::write(&path, serde_json::to_vec_pretty(&updated).unwrap()).unwrap();
        });
        application.shutdown().await.unwrap();
        let _ = fs::remove_dir_all(base);
        return;
    }

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn cache_protocol_fixtures_execute_with_declared_headers() {
    let (base, config) = prepare_fixture();
    jpeg_fixture(
        &config.library_root.join("photo.jpg"),
        90,
        45,
        [192, 64, 32],
    );
    let web_root = config.web_root();
    fs::create_dir_all(web_root.join("assets")).unwrap();
    fs::write(web_root.join("assets/app.js"), b"console.log(1)").unwrap();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let vectors: Vec<serde_json::Value> = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../compatibility/protocol/cache-vectors.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(vectors.len(), 2);
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
    let preview_url = preview["url"].as_str().unwrap().to_owned();
    for vector in vectors {
        let name = vector["name"].as_str().unwrap();
        let request_definition = &vector["request"];
        let method = request_definition["method"].as_str().unwrap();
        let target = match vector["setup"].as_str().unwrap() {
            "jpeg-original" => {
                assert_eq!(request_definition["target"], "generated-derivative");
                format!("https://camera.local{preview_url}")
            }
            "web-asset" => {
                assert_eq!(request_definition["path"], "/assets/app.js");
                "https://camera.local/assets/app.js".to_owned()
            }
            other => panic!("unknown cache fixture setup {other}"),
        };
        let expected = &vector["expected"];
        let declared_headers = expected["headers"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            declared_headers,
            BTreeSet::from(["cache-control", "content-type", "x-content-type-options"]),
            "{name} must declare the complete cache header contract"
        );
        let response = send(
            &router,
            authenticated_request()
                .method(method)
                .uri(target.clone())
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(
            response.status().as_u16(),
            expected["status"].as_u64().unwrap() as u16,
            "{name}"
        );
        for (header_name, value) in expected["headers"].as_object().unwrap() {
            assert_eq!(
                response.headers()[header_name.as_str()],
                value.as_str().unwrap(),
                "{name} {header_name}"
            );
        }
        if let Some(pattern) = expected["etagPattern"].as_str() {
            assert_etag_pattern(
                pattern,
                response.headers()[header::ETAG].to_str().unwrap(),
                name,
            );
        }
        if vector["setup"] == "jpeg-original" {
            let cache_key = preview_url
                .rsplit('/')
                .next()
                .and_then(|filename| filename.strip_suffix(".jpg"))
                .unwrap();
            assert_eq!(
                response.headers()[header::ETAG],
                format!("\"{cache_key}\""),
                "{name} ETag must identify the requested derivative cache key"
            );
        }
        let etag = response
            .headers()
            .get(header::ETAG)
            .map(|value| value.to_str().unwrap().to_owned());
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024 * 1024)
            .await
            .unwrap();
        if let Some(minimum) = expected["minimumBodyBytes"].as_u64() {
            assert!(body.len() >= minimum as usize, "{name}");
        }
        if let Some(revalidation) = vector.get("revalidation") {
            assert_eq!(revalidation["header"], "if-none-match");
            let revalidated = send(
                &router,
                authenticated_request()
                    .method(method)
                    .uri(target)
                    .header(header::IF_NONE_MATCH, etag.unwrap())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
            assert_eq!(
                revalidated.status().as_u16(),
                revalidation["expectedStatus"].as_u64().unwrap() as u16,
                "{name} revalidation"
            );
            assert_eq!(
                axum::body::to_bytes(revalidated.into_body(), 1024)
                    .await
                    .unwrap()
                    .len(),
                0,
                "{name} revalidation body"
            );
        }
    }
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

fn assert_etag_pattern(pattern: &str, etag: &str, name: &str) {
    assert_eq!(pattern, "^\"[a-f0-9]{64}\"$", "{name} etag pattern");
    let key = etag
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or_else(|| panic!("{name} etag must be quoted"));
    assert_eq!(key.len(), 64, "{name} etag key length");
    assert!(
        key.bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "{name} etag key must be lowercase hex"
    );
}

#[tokio::test]
async fn photo_json_omits_optional_values_and_preserves_original_order() {
    let contract: serde_json::Value = serde_json::from_str(include_str!(
        "../../../compatibility/protocol/capture-order-omission.json"
    ))
    .unwrap();
    let ordered_paths = contract["orderedPaths"].as_array().unwrap();
    let allowed_keys = contract["allowedKeys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|key| key.as_str().unwrap())
        .collect::<BTreeSet<_>>();
    let optional_keys = contract["optionalKeys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|key| key.as_str().unwrap())
        .collect::<BTreeSet<_>>();
    let (base, config) = prepare_fixture();
    capture_metadata_fixture(&config.library_root.join("z.JPG"), "2026:01:01 09:00:00");
    capture_metadata_fixture(&config.library_root.join("a.jpg"), "2026:01:01 10:00:00");
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
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
        assert_eq!(photo["original"]["kind"], "jpeg");
        assert_eq!(
            photo["preview"],
            serde_json::json!({"state": "inspection-pending"})
        );
        assert!(!photo.to_string().contains(":null"));
        let keys = photo
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        for key in &keys {
            assert!(allowed_keys.contains(key), "{key} leaked into protocol");
        }
        for key in allowed_keys.difference(&optional_keys) {
            assert!(keys.contains(key), "{key} missing from protocol");
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

/// Bounded traversal for one explicit view order. Tests observe order only
/// through the bounded protocol, never a complete-Photo route.
async fn browse_ids_in_order(
    application: &Application,
    source: BrowseSourceRequest,
    order: BrowseViewOrder,
) -> Vec<String> {
    browse_ids_in_pages(application, source, order, 60).await
}

async fn browse_ids_in_pages(
    application: &Application,
    source: BrowseSourceRequest,
    order: BrowseViewOrder,
    limit: usize,
) -> Vec<String> {
    let opened = application
        .browse_open(source, order, BrowseSelectionFilter::All, None)
        .await
        .expect("browse open succeeds");
    let mut ids = Vec::new();
    let mut start = 0;
    loop {
        let window = application
            .browse_window(&opened.token, start, limit)
            .await
            .expect("browse window succeeds");
        let count = window.photos.len();
        ids.extend(window.photos.into_iter().map(|photo| photo.id));
        start += count;
        if count == 0 || start >= opened.total {
            break;
        }
    }
    application.browse_close(&opened.token);
    ids
}

/// Maps Photo IDs to their ordering Location names from one Library snapshot
/// read, so order assertions compare real persisted facts rather than test
/// guesses about generated identities.
async fn ordering_locations(application: &Application, ids: &[String]) -> Vec<String> {
    let snapshot = application.library.snapshot().await.unwrap();
    let locations: HashMap<&str, &str> = snapshot
        .photos
        .iter()
        .map(|photo| (photo.id.as_str(), photo.sort_path.as_str()))
        .collect();
    ids.iter()
        .map(|id| locations[id.as_str()].to_owned())
        .collect()
}

/// Reads the Photo an Album Snapshot resumes at, exactly like a browser:
/// open the Snapshot, then read its reported position.
async fn album_resume(
    application: &Application,
    album_id: &str,
    order: BrowseViewOrder,
) -> (usize, String) {
    let opened = application
        .browse_open(
            BrowseSourceRequest::Album(album_id.to_owned()),
            order,
            BrowseSelectionFilter::All,
            None,
        )
        .await
        .expect("album browse open succeeds");
    let window = application
        .browse_window(&opened.token, opened.position, 1)
        .await
        .expect("album resume window succeeds");
    let photo_id = window.photos[0].id.clone();
    application.browse_close(&opened.token);
    (opened.position, photo_id)
}

/// Photo ID by ordering Location name for one set of IDs.
async fn photo_ids_by_location(
    application: &Application,
    ids: &[String],
) -> HashMap<String, String> {
    let snapshot = application.library.snapshot().await.unwrap();
    let locations: HashMap<&str, &str> = snapshot
        .photos
        .iter()
        .map(|photo| (photo.id.as_str(), photo.sort_path.as_str()))
        .collect();
    ids.iter()
        .map(|id| (locations[id.as_str()].to_owned(), id.clone()))
        .collect()
}

async fn get_json(router: &Router, uri: &str) -> (StatusCode, serde_json::Value) {
    let response = send(
        router,
        authenticated_request()
            .uri(format!("https://camera.local{uri}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let status = response.status();
    (status, response_json(response).await)
}

#[tokio::test]
async fn browse_view_order_reverses_only_capture_time_and_keeps_missing_last() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    capture_metadata_fixture(&root.join("a.jpg"), "2026:01:01 09:00:00");
    capture_metadata_fixture(&root.join("c.jpg"), "2026:01:01 09:00:00");
    capture_metadata_fixture(&root.join("b.jpg"), "2026:01:01 10:00:00");
    jpeg_fixture(&root.join("d.jpg"), 8, 4, [1, 2, 3]);
    jpeg_fixture(&root.join("z.jpg"), 8, 4, [4, 5, 6]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;

    let ascending = browse_ids_in_order(
        &application,
        BrowseSourceRequest::Library,
        BrowseViewOrder::CaptureTimeAscending,
    )
    .await;
    assert_eq!(
        ordering_locations(&application, &ascending).await,
        vec!["a.jpg", "c.jpg", "b.jpg", "d.jpg", "z.jpg"]
    );
    let descending = browse_ids_in_order(
        &application,
        BrowseSourceRequest::Library,
        BrowseViewOrder::CaptureTimeDescending,
    )
    .await;
    // Equal Capture Times keep the Location tie-breaker ascending and the
    // missing-time partition stays last instead of leading the view.
    assert_eq!(
        ordering_locations(&application, &descending).await,
        vec!["b.jpg", "a.jpg", "c.jpg", "d.jpg", "z.jpg"]
    );
    assert_eq!(ascending.len(), 5);
    // The Published Library keeps its natural ascending order: a view order
    // is a projection, not a rewrite.
    let snapshot = application.library.snapshot().await.unwrap();
    assert_eq!(
        snapshot
            .photos
            .iter()
            .map(|photo| photo.sort_path.as_str())
            .collect::<Vec<_>>(),
        vec!["a.jpg", "c.jpg", "b.jpg", "d.jpg", "z.jpg"]
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn folder_view_order_reverses_only_capture_time_within_the_subtree() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    fs::create_dir_all(root.join("sub")).unwrap();
    capture_metadata_fixture(&root.join("sub/a.jpg"), "2026:01:01 09:00:00");
    capture_metadata_fixture(&root.join("sub/b.jpg"), "2026:01:01 10:00:00");
    jpeg_fixture(&root.join("sub/c.jpg"), 8, 4, [1, 2, 3]);
    capture_metadata_fixture(&root.join("other.jpg"), "2026:01:01 08:00:00");
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let publication = application
        .file_locations(None, "", 0, 60)
        .await
        .unwrap()
        .publication;
    let folder = || BrowseSourceRequest::Folder {
        location: "sub".to_owned(),
        publication: publication.clone(),
    };
    let ascending = browse_ids_in_order(
        &application,
        folder(),
        BrowseViewOrder::CaptureTimeAscending,
    )
    .await;
    assert_eq!(
        ordering_locations(&application, &ascending).await,
        vec!["sub/a.jpg", "sub/b.jpg", "sub/c.jpg"]
    );
    let descending = browse_ids_in_order(
        &application,
        folder(),
        BrowseViewOrder::CaptureTimeDescending,
    )
    .await;
    assert_eq!(
        ordering_locations(&application, &descending).await,
        vec!["sub/b.jpg", "sub/a.jpg", "sub/c.jpg"]
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn album_time_views_order_members_without_rewriting_membership_positions() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    capture_metadata_fixture(&root.join("a.jpg"), "2026:01:01 09:00:00");
    capture_metadata_fixture(&root.join("b.jpg"), "2026:01:01 10:00:00");
    jpeg_fixture(&root.join("c.jpg"), 8, 4, [1, 2, 3]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let library_ids = browse_ids_in_order(
        &application,
        BrowseSourceRequest::Library,
        BrowseViewOrder::CaptureTimeAscending,
    )
    .await;
    let locations = ordering_locations(&application, &library_ids).await;
    let by_location: HashMap<&str, &String> = locations
        .iter()
        .map(|location| location.as_str())
        .zip(library_ids.iter())
        .collect();
    let a = by_location["a.jpg"].clone();
    let b = by_location["b.jpg"].clone();
    let c = by_location["c.jpg"].clone();

    application
        .mutate_album(slipstream_core::AlbumMutation::Create {
            name: "Picks".to_owned(),
        })
        .await
        .unwrap();
    let album_id = application
        .albums()
        .await
        .unwrap()
        .albums
        .into_iter()
        .find(|album| album.name == "Picks")
        .unwrap()
        .id;
    // Membership order is deliberately not Capture Time order.
    application
        .mutate_album(slipstream_core::AlbumMutation::AddMembers {
            album_id: album_id.clone(),
            photo_ids: vec![c.clone(), b.clone(), a.clone()],
        })
        .await
        .unwrap();

    let album_order = browse_ids_in_order(
        &application,
        BrowseSourceRequest::Album(album_id.clone()),
        BrowseViewOrder::AlbumOrder,
    )
    .await;
    assert_eq!(album_order, vec![c.clone(), b.clone(), a.clone()]);
    let time_ascending = browse_ids_in_order(
        &application,
        BrowseSourceRequest::Album(album_id.clone()),
        BrowseViewOrder::CaptureTimeAscending,
    )
    .await;
    assert_eq!(time_ascending, vec![a.clone(), b.clone(), c.clone()]);
    let time_descending = browse_ids_in_order(
        &application,
        BrowseSourceRequest::Album(album_id.clone()),
        BrowseViewOrder::CaptureTimeDescending,
    )
    .await;
    assert_eq!(time_descending, vec![b.clone(), a.clone(), c.clone()]);

    // A preferred Photo resolves by identity inside the requested view.
    let preferred = application
        .browse_open(
            BrowseSourceRequest::Album(album_id.clone()),
            BrowseViewOrder::CaptureTimeDescending,
            BrowseSelectionFilter::All,
            Some(&a),
        )
        .await
        .unwrap();
    assert_eq!(preferred.position, 1);
    application.browse_close(&preferred.token);

    // Persisted membership positions keep the Album's own order.
    let album = application
        .library
        .list_albums()
        .await
        .unwrap()
        .into_iter()
        .find(|album| album.id == album_id)
        .unwrap();
    assert_eq!(
        album
            .members
            .iter()
            .map(|member| member.photo_id.as_str())
            .collect::<Vec<_>>(),
        vec![c.as_str(), b.as_str(), a.as_str()]
    );
    assert_eq!(
        album
            .members
            .iter()
            .map(|member| member.position)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn browse_open_rejects_unknown_and_source_invalid_order() {
    let (base, config) = prepare_fixture();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let unknown = post_json(
        &router,
        "/api/browse",
        serde_json::json!({"source": "library", "order": "newest-first"}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(unknown).await,
        serde_json::json!({"error": "Invalid browse order"})
    );
    let album_order_on_library = post_json(
        &router,
        "/api/browse",
        serde_json::json!({"source": "library", "order": "album-order"}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(album_order_on_library.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(album_order_on_library).await,
        serde_json::json!({"error": "Invalid browse order"})
    );
    let folder_album_order = post_json(
        &router,
        "/api/browse",
        serde_json::json!({
            "source": "folder",
            "folderPath": "",
            "publication": "0123456789abcdef",
            "order": "album-order"
        }),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(folder_album_order.status(), StatusCode::BAD_REQUEST);
    let accepted = post_json(
        &router,
        "/api/browse",
        serde_json::json!({"source": "library", "order": "capture-time-desc"}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(accepted.status(), StatusCode::OK);
    // The source/order compatibility rule lives in the application boundary,
    // so a direct caller cannot silently reinterpret `album-order`.
    assert!(matches!(
        application
            .browse_open(
                BrowseSourceRequest::Library,
                BrowseViewOrder::AlbumOrder,
                BrowseSelectionFilter::All,
                None,
            )
            .await,
        Err(ServerError::BrowseOrder)
    ));
    assert!(matches!(
        application
            .browse_open(
                BrowseSourceRequest::Folder {
                    location: "".to_owned(),
                    publication: "0123456789abcdef".to_owned(),
                },
                BrowseViewOrder::AlbumOrder,
                BrowseSelectionFilter::All,
                None,
            )
            .await,
        Err(ServerError::BrowseOrder)
    ));
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn photo_albums_route_reports_true_membership_from_the_owner() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    jpeg_fixture(&root.join("a.jpg"), 8, 4, [1, 2, 3]);
    jpeg_fixture(&root.join("b.jpg"), 8, 4, [4, 5, 6]);
    jpeg_fixture(&root.join("c.jpg"), 8, 4, [7, 8, 9]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_ids_in_order(
        &application,
        BrowseSourceRequest::Library,
        BrowseViewOrder::CaptureTimeAscending,
    )
    .await;
    assert_eq!(ids.len(), 3);
    for name in ["Picks", "Later"] {
        application
            .mutate_album(slipstream_core::AlbumMutation::Create {
                name: name.to_owned(),
            })
            .await
            .unwrap();
    }
    let albums = application.albums().await.unwrap().albums;
    let picks = albums
        .iter()
        .find(|album| album.name == "Picks")
        .unwrap()
        .id
        .clone();
    let later = albums
        .iter()
        .find(|album| album.name == "Later")
        .unwrap()
        .id
        .clone();
    application
        .mutate_album(slipstream_core::AlbumMutation::AddMembers {
            album_id: picks.clone(),
            photo_ids: vec![ids[0].clone()],
        })
        .await
        .unwrap();
    application
        .mutate_album(slipstream_core::AlbumMutation::AddMembers {
            album_id: later.clone(),
            photo_ids: vec![ids[0].clone(), ids[1].clone()],
        })
        .await
        .unwrap();
    // Re-adding an existing member must not duplicate membership.
    application
        .mutate_album(slipstream_core::AlbumMutation::AddMembers {
            album_id: picks.clone(),
            photo_ids: vec![ids[0].clone()],
        })
        .await
        .unwrap();

    let (status, body) = get_json(&router, &format!("/api/photos/{}/albums", ids[0])).await;
    assert_eq!(status, StatusCode::OK);
    // Both routes order Albums by creation time and ID. Equal timestamps are
    // resolved by the generated IDs, not by the order of create calls.
    let expected = albums
        .iter()
        .map(|album| serde_json::json!({"id": album.id, "name": album.name}))
        .collect::<Vec<_>>();
    assert_eq!(body, serde_json::json!({"albums": expected}));
    let (status, body) = get_json(&router, &format!("/api/photos/{}/albums", ids[1])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        serde_json::json!({"albums": [{"id": later, "name": "Later"}]})
    );
    let (status, body) = get_json(&router, &format!("/api/photos/{}/albums", ids[2])).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, serde_json::json!({"albums": []}));
    let (status, body) = get_json(
        &router,
        "/api/photos/00000000-0000-4000-8000-000000000000/albums",
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body, serde_json::json!({"error": "Photo not found"}));
    let (status, body) = get_json(&router, "/api/photos/NOT-A-ID/albums").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body, serde_json::json!({"error": "Invalid Photo"}));
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn album_saved_position_falls_back_by_membership_position_in_time_views() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    // `c` deliberately has no Capture Time so a time view puts it last.
    capture_metadata_fixture(&root.join("a.jpg"), "2026:01:01 09:00:00");
    capture_metadata_fixture(&root.join("b.jpg"), "2026:01:01 10:00:00");
    jpeg_fixture(&root.join("c.jpg"), 8, 4, [1, 2, 3]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let ids = browse_ids_in_order(
        &application,
        BrowseSourceRequest::Library,
        BrowseViewOrder::CaptureTimeAscending,
    )
    .await;
    let by_name = photo_ids_by_location(&application, &ids).await;
    let (a, b, c) = (
        by_name["a.jpg"].clone(),
        by_name["b.jpg"].clone(),
        by_name["c.jpg"].clone(),
    );

    // Membership order starts with the Photo that will become unavailable.
    let album = application
        .mutate_album(slipstream_core::AlbumMutation::Create {
            name: "Picks".to_owned(),
        })
        .await
        .unwrap()
        .albums
        .into_iter()
        .find(|album| album.name == "Picks")
        .unwrap()
        .id;
    application
        .mutate_album(slipstream_core::AlbumMutation::AddMembers {
            album_id: album.clone(),
            photo_ids: vec![c.clone(), a.clone(), b.clone()],
        })
        .await
        .unwrap();
    application
        .mutate_album(slipstream_core::AlbumMutation::SetProgress {
            album_id: album.clone(),
            photo_id: c.clone(),
        })
        .await
        .unwrap();
    fs::remove_file(root.join("c.jpg")).unwrap();
    application.rescan().await.unwrap();

    // Saved `c` is unavailable, so every order resumes at the next available
    // member by membership position: `a`.
    assert_eq!(
        album_resume(&application, &album, BrowseViewOrder::AlbumOrder).await,
        (1, a.clone())
    );
    let ascending = browse_ids_in_order(
        &application,
        BrowseSourceRequest::Album(album.clone()),
        BrowseViewOrder::CaptureTimeAscending,
    )
    .await;
    assert_eq!(
        ordering_locations(&application, &ascending).await,
        vec!["a.jpg", "b.jpg", "c.jpg"]
    );
    assert_eq!(
        album_resume(&application, &album, BrowseViewOrder::CaptureTimeAscending).await,
        (0, a.clone())
    );
    let descending = browse_ids_in_order(
        &application,
        BrowseSourceRequest::Album(album.clone()),
        BrowseViewOrder::CaptureTimeDescending,
    )
    .await;
    assert_eq!(
        ordering_locations(&application, &descending).await,
        vec!["b.jpg", "a.jpg", "c.jpg"]
    );
    // Membership position picks `a` even though its view position differs.
    assert_eq!(
        album_resume(&application, &album, BrowseViewOrder::CaptureTimeDescending).await,
        (1, a.clone())
    );
    // The explicit preferred Photo still outranks the saved position, and the
    // persisted membership positions never move.
    let opened = application
        .browse_open(
            BrowseSourceRequest::Album(album.clone()),
            BrowseViewOrder::CaptureTimeDescending,
            BrowseSelectionFilter::All,
            Some(&b),
        )
        .await
        .unwrap();
    assert_eq!(opened.position, 0);
    application.browse_close(&opened.token);
    assert_eq!(
        browse_ids_in_order(
            &application,
            BrowseSourceRequest::Album(album),
            BrowseViewOrder::AlbumOrder,
        )
        .await,
        vec![c, a, b]
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn descending_paged_windows_stay_globally_ordered_without_duplicates() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    for index in 0..65u32 {
        let name = format!("n{index:02}.jpg");
        capture_metadata_fixture(
            &root.join(&name),
            &format!("2026:01:01 09:{:02}:{:02}", index / 60, index % 60),
        );
    }
    // Two Photos without a valid Capture Time stay last in both directions.
    jpeg_fixture(&root.join("d.jpg"), 8, 4, [1, 2, 3]);
    jpeg_fixture(&root.join("z.jpg"), 8, 4, [4, 5, 6]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;

    let descending = browse_ids_in_pages(
        &application,
        BrowseSourceRequest::Library,
        BrowseViewOrder::CaptureTimeDescending,
        7,
    )
    .await;
    let mut expected_timed = (0..65u32)
        .map(|index| format!("n{index:02}.jpg"))
        .collect::<Vec<_>>();
    expected_timed.reverse();
    let mut expected = expected_timed;
    expected.extend(["d.jpg".to_owned(), "z.jpg".to_owned()]);
    assert_eq!(descending.len(), 67);
    assert_eq!(
        ordering_locations(&application, &descending).await,
        expected
    );
    let unique = descending.iter().collect::<std::collections::BTreeSet<_>>();
    assert_eq!(unique.len(), descending.len(), "windows repeated a Photo");

    // The same paged traversal in ascending order is the exact inverse of the
    // time partition, proving both directions page one global order.
    let ascending = browse_ids_in_pages(
        &application,
        BrowseSourceRequest::Library,
        BrowseViewOrder::CaptureTimeAscending,
        7,
    )
    .await;
    let ascending_timed = &ascending[..65];
    let mut reversed_timed = ascending_timed.to_vec();
    reversed_timed.reverse();
    assert_eq!(reversed_timed, descending[..65].to_vec());
    assert_eq!(
        ordering_locations(&application, &ascending[65..]).await,
        vec!["d.jpg", "z.jpg"]
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

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
    let router = authorized_router(Arc::clone(&application), config.web_root());

    let overview_response = send(
        &router,
        authenticated_request()
            .uri("https://camera.local/api/overview")
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
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(opened.status(), StatusCode::OK);
    let opened: serde_json::Value = response_json(opened).await;
    let token = opened["token"].as_str().unwrap();
    let window = send(
        &router,
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/browse/{}?start=39940&limit=60",
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
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/browse/{}?start=0&limit=61",
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
    let album_id = application
        .albums()
        .await
        .unwrap()
        .albums
        .into_iter()
        .find(|album| album.name == "Picks")
        .unwrap()
        .id;
    application
        .mutate_album(slipstream_core::AlbumMutation::AddMembers {
            album_id: album_id.clone(),
            photo_ids: ids.clone(),
        })
        .await
        .unwrap();
    application
        .mutate_album(slipstream_core::AlbumMutation::SetProgress {
            album_id: album_id.clone(),
            photo_id: ids[1].clone(),
        })
        .await
        .unwrap();
    let opened = application
        .browse_open(
            BrowseSourceRequest::Album(album_id),
            BrowseViewOrder::AlbumOrder,
            BrowseSelectionFilter::All,
            None,
        )
        .await
        .unwrap();
    assert_eq!(opened.total, 3);
    assert_eq!(opened.position, 1);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn file_location_windows_derive_bounded_folders_from_one_publication() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    jpeg_fixture(&root.join("b.JPG"), 8, 4, [32, 64, 192]);
    fs::write(root.join("a.ARW"), b"raw-bytes-a").unwrap();
    jpeg_fixture(&root.join("a.JPG"), 8, 4, [64, 32, 192]);
    fs::create_dir_all(root.join("shoot/sub")).unwrap();
    jpeg_fixture(&root.join("shoot/c.JPG"), 8, 4, [1, 2, 3]);
    fs::write(root.join("shoot/d.ARW"), b"raw-bytes-d").unwrap();
    jpeg_fixture(&root.join("shoot/d.JPG"), 8, 4, [4, 5, 6]);
    jpeg_fixture(&root.join("shoot/sub/e.JPG"), 8, 4, [7, 8, 9]);
    fs::create_dir_all(root.join("a")).unwrap();
    jpeg_fixture(&root.join("a/f.JPG"), 8, 4, [9, 8, 7]);
    fs::create_dir_all(root.join("ab")).unwrap();
    jpeg_fixture(&root.join("ab/g.JPG"), 8, 4, [6, 5, 4]);
    fs::create_dir_all(root.join("\u{76f8}\u{518c}")).unwrap();
    jpeg_fixture(&root.join("\u{76f8}\u{518c}/h.JPG"), 8, 4, [3, 2, 1]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    application.rescan().await.unwrap();
    wait_for_scan_settled(&application).await;

    // The first window binds to the current publication without supplying one.
    let first = application.file_locations(None, "", 0, 60).await.unwrap();
    assert_eq!(first.parent, "");
    assert_eq!(first.total, 4);
    assert_eq!(first.children.len(), 4);
    let names: Vec<&str> = first
        .children
        .iter()
        .map(|child| child.name.as_str())
        .collect();
    assert_eq!(names, vec!["a", "ab", "shoot", "\u{76f8}\u{518c}"]);
    let by_location = |location: &str| {
        first
            .children
            .iter()
            .find(|child| child.location == location)
            .unwrap()
    };
    // Folder counts are recursive and count independent Photos.
    assert_eq!(by_location("a").photo_count, 1);
    assert!(!by_location("a").has_descendant_folders);
    assert_eq!(by_location("ab").photo_count, 1);
    assert_eq!(by_location("shoot").photo_count, 4);
    assert!(by_location("shoot").has_descendant_folders);
    assert_eq!(by_location("\u{76f8}\u{518c}").photo_count, 1);
    let publication = first.publication.clone();
    assert!(!publication.is_empty());

    // A retained window with the same publication stays coherent.
    let shoot = application
        .file_locations(Some(&publication), "shoot", 0, 60)
        .await
        .unwrap();
    assert_eq!(shoot.total, 1);
    assert_eq!(shoot.children[0].location, "shoot/sub");
    assert_eq!(shoot.children[0].photo_count, 1);

    // Window bounds are enforced.
    assert!(matches!(
        application
            .file_locations(Some(&publication), "", 0, 0)
            .await,
        Err(ServerError::FileLocationWindow)
    ));
    assert!(matches!(
        application
            .file_locations(Some(&publication), "", 0, MAXIMUM_FILE_LOCATION_WINDOW + 1)
            .await,
        Err(ServerError::FileLocationWindow)
    ));
    // Malformed and unknown parents are rejected without fallback.
    for malformed in ["/abs", "a/../b", "a//b", "a/.", "\0"] {
        assert!(matches!(
            application
                .file_locations(Some(&publication), malformed, 0, 60)
                .await,
            Err(ServerError::FolderInvalid)
        ));
    }
    assert!(matches!(
        application
            .file_locations(Some(&publication), "missing", 0, 60)
            .await,
        Err(ServerError::FolderNotFound)
    ));

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn file_location_counts_propagate_through_deep_ancestors() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    fs::create_dir_all(root.join("a/b/c")).unwrap();
    jpeg_fixture(&root.join("a/b/c/deep.JPG"), 8, 4, [1, 2, 3]);
    jpeg_fixture(&root.join("a/top.JPG"), 8, 4, [4, 5, 6]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    application.rescan().await.unwrap();
    wait_for_scan_settled(&application).await;
    let publication = application
        .file_locations(None, "", 0, 60)
        .await
        .unwrap()
        .publication;
    let root_window = application
        .file_locations(Some(&publication), "", 0, 60)
        .await
        .unwrap();
    assert_eq!(root_window.children[0].photo_count, 2);
    let a = application
        .file_locations(Some(&publication), "a", 0, 60)
        .await
        .unwrap();
    // Direct-child Folders only: files directly in `a` are not Folders.
    assert_eq!(
        a.children
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec!["b"]
    );
    // The intermediate chain aggregates upward: a/b inherits c's Photo.
    assert_eq!(a.children[0].photo_count, 1);
    // The intermediate chain aggregates upward: a/b inherits c's Photo.
    let ids = browse_photo_ids(
        &application,
        BrowseSourceRequest::Folder {
            location: "a".to_owned(),
            publication,
        },
    )
    .await;
    assert_eq!(ids.len(), 2);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn file_location_queries_decode_spaces_and_report_exact_expiry() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    fs::create_dir_all(root.join("My Photos")).unwrap();
    jpeg_fixture(&root.join("My Photos/one.JPG"), 8, 4, [1, 2, 3]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    application.rescan().await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let publication = application
        .file_locations(None, "", 0, 60)
        .await
        .unwrap()
        .publication;

    // `+` decodes as space in query values, so the spaced Folder opens.
    let response = tower::ServiceExt::oneshot(
        router.clone(),
        authenticated_request()
            .method("GET")
            .uri(format!(
                "https://camera.local/api/file-locations?publication={publication}&parent=My+Photos&start=0&limit=60"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["total"], 0);

    // A superseded publication reports the exact expiry contract.
    let response = tower::ServiceExt::oneshot(
        router.clone(),
        authenticated_request()
            .method("GET")
            .uri("https://camera.local/api/file-locations?publication=0000000000000000&start=0&limit=60")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: serde_json::Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(body["error"], "File Locations expired");
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn folder_sources_filter_ancestry_and_expire_with_publication() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    jpeg_fixture(&root.join("b.JPG"), 8, 4, [32, 64, 192]);
    fs::write(root.join("a.ARW"), b"raw-bytes-a").unwrap();
    jpeg_fixture(&root.join("a.JPG"), 8, 4, [64, 32, 192]);
    fs::create_dir_all(root.join("a")).unwrap();
    jpeg_fixture(&root.join("a/f.JPG"), 8, 4, [9, 8, 7]);
    fs::create_dir_all(root.join("ab")).unwrap();
    jpeg_fixture(&root.join("ab/g.JPG"), 8, 4, [6, 5, 4]);
    fs::create_dir_all(root.join("shoot/sub")).unwrap();
    jpeg_fixture(&root.join("shoot/c.JPG"), 8, 4, [1, 2, 3]);
    jpeg_fixture(&root.join("shoot/sub/e.JPG"), 8, 4, [7, 8, 9]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    application.rescan().await.unwrap();
    wait_for_scan_settled(&application).await;
    let publication = application
        .file_locations(None, "", 0, 60)
        .await
        .unwrap()
        .publication;

    // Persisted Photo IDs are opaque allocations; read the Published snapshot
    // through the in-crate boundary to build path expectations.
    let ids_by_path = |application: &Application| {
        let guard = application.shared.snapshot.read().unwrap();
        let published = guard.as_ref().unwrap();
        let mut map = std::collections::HashMap::new();
        for photo in &published.snapshot.photos {
            let position = published.originals_by_id[&photo.original_id];
            map.insert(
                published.snapshot.originals[position]
                    .relative_path
                    .as_str()
                    .to_owned(),
                photo.id.clone(),
            );
        }
        map
    };
    let ids = ids_by_path(&application);
    let photo_a = ids["a.ARW"].clone();
    let photo_f = ids["a/f.JPG"].clone();
    let photo_c = ids["shoot/c.JPG"].clone();
    let photo_e = ids["shoot/sub/e.JPG"].clone();

    // Component-aware ancestry: folder "a" never includes sibling "ab".
    let folder_a = browse_photo_ids(
        &application,
        BrowseSourceRequest::Folder {
            location: "a".to_owned(),
            publication: publication.clone(),
        },
    )
    .await;
    assert_eq!(folder_a, vec![photo_f.clone()]);
    // Recursive subtree membership in Capture Time order.
    let shoot = browse_photo_ids(
        &application,
        BrowseSourceRequest::Folder {
            location: "shoot".to_owned(),
            publication: publication.clone(),
        },
    )
    .await;
    assert_eq!(shoot.len(), 2);
    assert!(shoot.contains(&photo_c));
    assert!(shoot.contains(&photo_e));
    // The root Folder location covers the whole Published Library.
    let root_source = browse_photo_ids(
        &application,
        BrowseSourceRequest::Folder {
            location: String::new(),
            publication: publication.clone(),
        },
    )
    .await;
    assert_eq!(root_source.len(), 7);
    assert!(root_source.contains(&photo_a));
    assert!(root_source.contains(&ids["b.JPG"]));

    // A rescan that removes one Original supersedes the publication: every
    // retained File Location value and Folder-source open fails as expired.
    fs::remove_file(root.join("shoot/sub/e.JPG")).unwrap();
    application.rescan().await.unwrap();
    wait_for_scan_settled(&application).await;
    assert!(matches!(
        application
            .file_locations(Some(&publication), "", 0, 60)
            .await,
        Err(ServerError::FileLocationsExpired)
    ));
    assert!(matches!(
        application
            .browse_open(
                BrowseSourceRequest::Folder {
                    location: "a".to_owned(),
                    publication: publication.clone(),
                },
                BrowseViewOrder::CaptureTimeAscending,
                BrowseSelectionFilter::All,
                None,
            )
            .await,
        Err(ServerError::FileLocationsExpired)
    ));
    // A fresh window binds to the new publication and still projects the
    // remembered unavailable Photo at its last known Location.
    let fresh = application.file_locations(None, "", 0, 60).await.unwrap();
    assert_ne!(fresh.publication, publication);
    let shoot = application
        .file_locations(Some(&fresh.publication), "shoot", 0, 60)
        .await
        .unwrap();
    // The child window keeps projecting the remembered unavailable Photo.
    assert_eq!(shoot.children[0].photo_count, 1);
    let root_counts = application
        .file_locations(Some(&fresh.publication), "", 0, 60)
        .await
        .unwrap();
    let shoot_child = root_counts
        .children
        .iter()
        .find(|child| child.location == "shoot")
        .unwrap();
    assert_eq!(shoot_child.photo_count, 2);
    let reopened = browse_photo_ids(
        &application,
        BrowseSourceRequest::Folder {
            location: "shoot".to_owned(),
            publication: fresh.publication.clone(),
        },
    )
    .await;
    assert_eq!(
        reopened.len(),
        2,
        "remembered unavailable Photo is retained"
    );
    assert!(reopened.contains(&photo_c));
    assert!(reopened.contains(&photo_e));

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn folder_album_add_uses_recursive_publication_and_is_idempotent() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    fs::create_dir_all(root.join("shoot/nested")).unwrap();
    jpeg_fixture(&root.join("shoot/first.JPG"), 8, 4, [1, 2, 3]);
    jpeg_fixture(&root.join("shoot/nested/second.JPG"), 8, 4, [4, 5, 6]);
    jpeg_fixture(&root.join("outside.JPG"), 8, 4, [7, 8, 9]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    application.rescan().await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let publication = application
        .file_locations(None, "", 0, 60)
        .await
        .unwrap()
        .publication;
    let folder_ids = browse_photo_ids(
        &application,
        BrowseSourceRequest::Folder {
            location: "shoot".to_owned(),
            publication: publication.clone(),
        },
    )
    .await;
    assert_eq!(folder_ids.len(), 2);

    let created: serde_json::Value = response_json(
        post_json(
            &router,
            "/api/albums",
            serde_json::json!({"name": "Folder Picks"}),
            None,
        )
        .await,
    )
    .await;
    let album_id = created["albums"][0]["id"].as_str().unwrap().to_owned();
    let first: serde_json::Value = response_json(
        post_json(
            &router,
            &format!("/api/albums/{album_id}/folder-members"),
            serde_json::json!({
                "folderPath": "shoot",
                "publication": publication.clone(),
            }),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(first["albumId"], album_id);
    assert_eq!(first["folderPath"], "shoot");
    assert_eq!(first["matchedCount"], 2);
    assert_eq!(first["addedCount"], 2);
    assert_eq!(first["alreadyMemberCount"], 0);
    assert_eq!(
        browse_photo_ids(&application, BrowseSourceRequest::Album(album_id.clone()),).await,
        folder_ids
    );

    let repeated: serde_json::Value = response_json(
        post_json(
            &router,
            &format!("/api/albums/{album_id}/folder-members"),
            serde_json::json!({
                "folderPath": "shoot",
                "publication": publication.clone(),
            }),
            None,
        )
        .await,
    )
    .await;
    assert_eq!(repeated["matchedCount"], 2);
    assert_eq!(repeated["addedCount"], 0);
    assert_eq!(repeated["alreadyMemberCount"], 2);
    let stale = post_json(
        &router,
        &format!("/api/albums/{album_id}/folder-members"),
        serde_json::json!({
            "folderPath": "shoot",
            "publication": "0000000000000000",
        }),
        None,
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    assert_eq!(
        browse_photo_ids(&application, BrowseSourceRequest::Album(album_id)).await,
        folder_ids
    );
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
        .browse_open(
            BrowseSourceRequest::Album(album_id.clone()),
            BrowseViewOrder::AlbumOrder,
            BrowseSelectionFilter::All,
            None,
        )
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
        .browse_open(
            BrowseSourceRequest::Album(summary.id.clone()),
            BrowseViewOrder::AlbumOrder,
            BrowseSelectionFilter::All,
            None,
        )
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
        .browse_open(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::All,
            None,
        )
        .await
        .unwrap();
    let opened_b = application_b
        .browse_open(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::All,
            None,
        )
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
            .retained_queries
            .lock()
            .expect("retained queries poisoned");
        let snapshot = snapshots.entries.get_mut(&opened_a.token).unwrap();
        snapshot.last_used -= BROWSE_SNAPSHOT_IDLE + Duration::from_secs(1);
    }
    assert!(matches!(
        application_a.browse_window(&opened_a.token, 0, 10).await,
        Err(ServerError::BrowseNotFound)
    ));
    assert!(
        !application_a
            .retained_queries
            .lock()
            .unwrap()
            .entries
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
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let opened = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source":"library"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let token = opened["token"].as_str().unwrap().to_owned();
    async fn window(router: &Router, token: &str) -> Response<Body> {
        send(
            router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/browse/{token}?start=0&limit=10"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
    }
    assert_eq!(window(&router, &token).await.status(), StatusCode::OK);
    let foreign_origin = send(
        &router,
        authenticated_request()
            .method("DELETE")
            .uri(format!("https://camera.local/api/browse/{token}"))
            .header(header::ORIGIN, "http://elsewhere.example")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(foreign_origin.status(), StatusCode::FORBIDDEN);
    assert_eq!(window(&router, &token).await.status(), StatusCode::OK);
    let deleted = send(
        &router,
        Request::builder()
            .method("DELETE")
            .uri(format!("/api/browse/{token}"))
            .header(
                "Authorization",
                format!("Bearer {}", crate::access::TEST_TOKEN),
            )
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
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
        .browse_open(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::All,
            Some(&ids[2]),
        )
        .await
        .unwrap();
    assert_eq!(library.position, 2);
    let fallback = application
        .browse_open(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::All,
            None,
        )
        .await
        .unwrap();
    assert_eq!(fallback.position, 0);
    application
        .mutate_album(slipstream_core::AlbumMutation::Create {
            name: "Picks".to_owned(),
        })
        .await
        .unwrap();
    let album_id = application
        .albums()
        .await
        .unwrap()
        .albums
        .into_iter()
        .find(|album| album.name == "Picks")
        .unwrap()
        .id;
    application
        .mutate_album(slipstream_core::AlbumMutation::AddMembers {
            album_id: album_id.clone(),
            photo_ids: ids.clone(),
        })
        .await
        .unwrap();
    application
        .mutate_album(slipstream_core::AlbumMutation::SetProgress {
            album_id: album_id.clone(),
            photo_id: ids[0].clone(),
        })
        .await
        .unwrap();
    let preferred = application
        .browse_open(
            BrowseSourceRequest::Album(album_id),
            BrowseViewOrder::AlbumOrder,
            BrowseSelectionFilter::All,
            Some(&ids[2]),
        )
        .await
        .unwrap();
    assert_eq!(preferred.position, 2);
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let invalid = post_json(
        &router,
        "/api/browse",
        serde_json::json!({"source":"library","photoId":"NOT-A-ID"}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn browse_position_resolves_identity_within_one_snapshot() {
    let (base, config) = prepare_fixture();
    for name in ["a.jpg", "b.jpg", "c.jpg"] {
        jpeg_fixture(&config.library_root.join(name), 8, 4, [32, 64, 192]);
    }
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let opened = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source":"library"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let token = opened["token"].as_str().unwrap().to_owned();
    let get_position = |photo_id: String| {
        let uri = format!("https://camera.local/api/browse/{token}/position?photoId={photo_id}");
        let router = &router;
        async move {
            response_json(
                send(
                    router,
                    authenticated_request()
                        .uri(uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await,
            )
            .await
        }
    };
    assert_eq!(get_position(ids[1].clone()).await["position"], 1);
    assert!(get_position("f".repeat(64)).await["position"].is_null());

    let invalid = send(
        &router,
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/browse/{token}/position?photoId=bad"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);

    application.browse_close(&token);
    let expired = send(
        &router,
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/browse/{token}/position?photoId={}",
                ids[0]
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(expired.status(), StatusCode::NOT_FOUND);
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
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let opened = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source":"library"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let token = opened["token"].as_str().unwrap().to_owned();
    let window = || async {
        response_json(
            send(
                &router,
                authenticated_request()
                    .uri(format!(
                        "https://camera.local/api/browse/{token}?start=0&limit=10"
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
            &format!("https://camera.local/api/photos/{}/state", ids[0]),
            serde_json::json!({"field": "selectionState", "value": "selected"}),
            Some("https://camera.local"),
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

/// One bounded batch Selection State write reports exactly one outcome per
/// requested Photo, presents the confirmed states in the open Browse Snapshot,
/// and moves the source's counts when the source is reopened.
#[tokio::test]
async fn batch_photo_state_applies_to_every_requested_photo() {
    let (base, config) = prepare_fixture();
    for name in ["a.jpg", "b.jpg", "c.jpg"] {
        jpeg_fixture(&config.library_root.join(name), 8, 4, [32, 64, 192]);
    }
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    assert_eq!(ids.len(), 3);
    let opened = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source":"library"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let token = opened["token"].as_str().unwrap().to_owned();
    assert_eq!(opened["selectionCounts"]["undecided"], 3);

    let applied = response_json(
        post_json(
            &router,
            "/api/photos/state",
            serde_json::json!({
                "photos": ids
                    .iter()
                    .map(|photo_id| serde_json::json!({"photoId": photo_id, "expectedCurrent": "undecided"}))
                    .collect::<Vec<_>>(),
                "selectionState": "rejected"
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(applied["changedElsewhere"].as_array().unwrap().len(), 0);
    assert_eq!(applied["missing"].as_array().unwrap().len(), 0);
    let entries = applied["applied"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    for (entry, id) in entries.iter().zip(&ids) {
        assert_eq!(entry["photoId"], *id);
        assert_eq!(entry["priorValue"], "undecided");
    }

    // The open Snapshot presents the confirmed states without a reload.
    let window = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/browse/{token}?start=0&limit=10"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert!(
        window["photos"]
            .as_array()
            .unwrap()
            .iter()
            .all(|photo| photo["selectionState"] == "rejected")
    );

    // A later writer changes one Photo after the browser's confirmed state.
    // The next batch reports that identity and leaves its newer value intact.
    let external = response_json(
        post_json(
            &router,
            &format!("/api/photos/{}/state", ids[0]),
            serde_json::json!({
                "field": "selectionState",
                "value": "selected",
                "expectedCurrent": "rejected"
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(external["kind"], "applied");
    let changed = response_json(
        post_json(
            &router,
            "/api/photos/state",
            serde_json::json!({
                "photos": ids
                    .iter()
                    .map(|photo_id| serde_json::json!({"photoId": photo_id, "expectedCurrent": "rejected"}))
                    .collect::<Vec<_>>(),
                "selectionState": "selected"
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(
        changed["changedElsewhere"],
        serde_json::json!([{"photoId": ids[0], "currentValue": "selected"}])
    );
    assert_eq!(changed["applied"].as_array().unwrap().len(), 2);
    assert!(changed["missing"].as_array().unwrap().is_empty());
    let changed_window = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/browse/{token}?start=0&limit=10"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert!(
        changed_window["photos"]
            .as_array()
            .unwrap()
            .iter()
            .all(|photo| photo["selectionState"] == "selected")
    );

    // Repeating the now-confirmed state is idempotent: every Photo reports the
    // state it already holds as its prior value.
    let repeated = response_json(
        post_json(
            &router,
            "/api/photos/state",
            serde_json::json!({
                "photos": ids
                    .iter()
                    .map(|photo_id| serde_json::json!({"photoId": photo_id, "expectedCurrent": "selected"}))
                    .collect::<Vec<_>>(),
                "selectionState": "selected"
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert!(
        repeated["applied"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["priorValue"] == "selected")
    );
    let reopened = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source":"library"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(reopened["selectionCounts"]["selected"], 3);
    assert_eq!(reopened["selectionCounts"]["rejected"], 0);
    assert_eq!(reopened["selectionCounts"]["undecided"], 0);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A Photo that is no longer in the current Library is reported as missing
/// and never rolls back the confirmed Photos of the same batch.
#[tokio::test]
async fn batch_photo_state_reports_a_missing_photo_without_blocking_the_rest() {
    let (base, config) = prepare_fixture();
    for name in ["a.jpg", "b.jpg"] {
        jpeg_fixture(&config.library_root.join(name), 8, 4, [32, 64, 192]);
    }
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let missing = "00000000-0000-4000-8000-000000000000";

    let result = response_json(
        post_json(
            &router,
            "/api/photos/state",
            serde_json::json!({
                "photos": [
                    {"photoId": ids[0], "expectedCurrent": "undecided"},
                    {"photoId": missing, "expectedCurrent": "undecided"},
                    {"photoId": ids[1], "expectedCurrent": "undecided"}
                ],
                "selectionState": "selected"
            }),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let applied = result["applied"].as_array().unwrap();
    assert_eq!(applied.len(), 2);
    assert_eq!(applied[0]["photoId"], ids[0]);
    assert_eq!(applied[1]["photoId"], ids[1]);
    let missing_outcomes = result["missing"].as_array().unwrap();
    assert_eq!(missing_outcomes.len(), 1);
    assert_eq!(missing_outcomes[0]["photoId"], missing);
    assert_eq!(
        missing_outcomes[0],
        serde_json::json!({ "photoId": missing })
    );
    assert!(result["changedElsewhere"].as_array().unwrap().is_empty());
    // The Library no longer holds a state for that Photo, so the missing
    // outcome names it and nothing else.

    // The confirmed Photos persisted; the unknown Photo changed nothing.
    let reopened = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source":"library"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(reopened["selectionCounts"]["selected"], 2);
    assert_eq!(reopened["selectionCounts"]["undecided"], 0);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// The batch bound, identifier shape, uniqueness, and value vocabulary are
/// rejected before any write.
#[tokio::test]
async fn batch_photo_state_rejects_over_limit_duplicate_and_unknown_requests() {
    let (base, config) = prepare_fixture();
    jpeg_fixture(&config.library_root.join("a.jpg"), 8, 4, [32, 64, 192]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;

    let over_limit: Vec<String> = (0..=slipstream_core::PHOTO_STATE_BATCH_MAX)
        .map(|index| format!("00000000-0000-4000-8000-{index:012}"))
        .collect();
    let valid_items = || {
        ids.iter()
            .map(
                |photo_id| serde_json::json!({"photoId": photo_id, "expectedCurrent": "undecided"}),
            )
            .collect::<Vec<_>>()
    };
    for body in [
        serde_json::json!({
            "photos": over_limit
                .into_iter()
                .map(|photo_id| serde_json::json!({"photoId": photo_id, "expectedCurrent": "undecided"}))
                .collect::<Vec<_>>(),
            "selectionState": "selected"
        }),
        serde_json::json!({
            "photos": [
                {"photoId": ids[0], "expectedCurrent": "undecided"},
                {"photoId": ids[0], "expectedCurrent": "undecided"}
            ],
            "selectionState": "selected"
        }),
        serde_json::json!({"photos": [], "selectionState": "selected"}),
        serde_json::json!({"photos": valid_items(), "selectionState": "undecided"}),
        serde_json::json!({"photos": valid_items(), "selectionState": "maybe"}),
        serde_json::json!({"photos": valid_items(), "rating": 3}),
        serde_json::json!({
            "photos": [{"photoId": ids[0]}],
            "selectionState": "selected"
        }),
        serde_json::json!({
            "photos": [{
                "photoId": ids[0],
                "expectedCurrent": "undecided",
                "unexpected": true
            }],
            "selectionState": "selected"
        }),
        serde_json::json!({
            "photos": [{"photoId": "NOT-A-PHOTO-ID", "expectedCurrent": "undecided"}],
            "selectionState": "selected"
        }),
    ] {
        assert_eq!(
            post_json(
                &router,
                "/api/photos/state",
                body,
                Some("https://camera.local")
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }

    // No rejected request wrote anything.
    let reopened = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source":"library"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(reopened["selectionCounts"]["undecided"], 1);
    assert_eq!(reopened["selectionCounts"]["selected"], 0);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// Commits one Selection State through the HTTP mutation route so a fixture
/// holds real persisted decisions rather than fabricated facts.
async fn decide_selection(router: &Router, photo_id: &str, value: &str) -> StatusCode {
    post_json(
        router,
        &format!("https://camera.local/api/photos/{photo_id}/state"),
        serde_json::json!({"field": "selectionState", "value": value}),
        Some("https://camera.local"),
    )
    .await
    .status()
}

/// Opens one source with one Selection State filter and reads the whole view
/// through bounded windows, exactly as the browser must.
async fn browse_filtered_summaries(
    application: &Application,
    source: BrowseSourceRequest,
    selection: BrowseSelectionFilter,
) -> (BrowseOpenResponse, Vec<PhotoSummary>) {
    let order = default_order(&source);
    let opened = application
        .browse_open(source, order, selection, None)
        .await
        .expect("filtered browse open succeeds");
    let mut photos = Vec::new();
    let mut start = 0;
    loop {
        let window = application
            .browse_window(&opened.token, start, 60)
            .await
            .expect("filtered browse window succeeds");
        assert_eq!(window.total, opened.total);
        let count = window.photos.len();
        photos.extend(window.photos);
        start += count;
        if count == 0 || start >= opened.total {
            break;
        }
    }
    assert_eq!(photos.len(), opened.total, "filtered traversal incomplete");
    application.browse_close(&opened.token);
    (opened, photos)
}

#[tokio::test]
async fn browse_selection_filter_selects_from_the_source_order_with_source_counts() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    for (name, capture_time) in [
        ("a.jpg", "2026:01:01 09:00:00"),
        ("b.jpg", "2026:01:01 10:00:00"),
        ("c.jpg", "2026:01:01 11:00:00"),
        ("d.jpg", "2026:01:01 12:00:00"),
        ("e.jpg", "2026:01:01 13:00:00"),
    ] {
        capture_metadata_fixture(&root.join(name), capture_time);
    }
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let by_location = photo_ids_by_location(&application, &ids).await;
    for (name, value) in [
        ("a.jpg", "selected"),
        ("c.jpg", "selected"),
        ("b.jpg", "rejected"),
    ] {
        assert_eq!(
            decide_selection(&router, &by_location[name], value).await,
            StatusCode::OK,
            "{name}"
        );
    }

    // Every view is a projection of the same source order: the filtered
    // sequence keeps Capture Time order and the counts stay source-wide.
    for (selection, expected_locations) in [
        (BrowseSelectionFilter::All, vec!["a", "b", "c", "d", "e"]),
        (BrowseSelectionFilter::Selected, vec!["a", "c"]),
        (BrowseSelectionFilter::Rejected, vec!["b"]),
        (BrowseSelectionFilter::Undecided, vec!["d", "e"]),
    ] {
        let (opened, photos) =
            browse_filtered_summaries(&application, BrowseSourceRequest::Library, selection).await;
        assert_eq!(opened.total, expected_locations.len(), "{selection:?}");
        assert_eq!(photos.len(), expected_locations.len(), "{selection:?}");
        assert_eq!(
            photos
                .iter()
                .map(|photo| photo
                    .original_filename
                    .clone()
                    .unwrap()
                    .split('.')
                    .next()
                    .unwrap()
                    .to_owned())
                .collect::<Vec<_>>(),
            expected_locations
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>(),
            "{selection:?}"
        );
        assert_eq!(
            opened.selection_counts,
            SelectionCountsWire {
                selected: 2,
                rejected: 1,
                undecided: 2,
            },
            "counts describe the source for {selection:?}"
        );
    }

    // Positions resolve inside the filtered sequence, so an anchor that the
    // filter excluded falls back to the first filtered Photo.
    let selected = application
        .browse_open(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::Selected,
            Some(&by_location["c.jpg"]),
        )
        .await
        .unwrap();
    assert_eq!(selected.position, 1);
    let window = application
        .browse_window(&selected.token, 0, 1)
        .await
        .unwrap();
    assert_eq!(window.photos[0].id, by_location["a.jpg"]);
    let filtered_out = application
        .browse_open(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::Selected,
            Some(&by_location["b.jpg"]),
        )
        .await
        .unwrap();
    assert_eq!(filtered_out.position, 0);
    application.browse_close(&selected.token);
    application.browse_close(&filtered_out.token);

    // The route rejects an unknown value before any Snapshot exists, and a
    // valid value reaches the same filtered projection as the direct call.
    let unknown = post_json(
        &router,
        "/api/browse",
        serde_json::json!({"source": "library", "selection": "maybe"}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        response_json(unknown).await,
        serde_json::json!({"error": "Invalid browse selection"})
    );
    let routed = response_json(
        post_json(
            &router,
            "/api/browse",
            serde_json::json!({"source": "library", "selection": "rejected"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(routed["total"], 1);
    assert_eq!(
        routed["selectionCounts"],
        serde_json::json!({"selected": 2, "rejected": 1, "undecided": 2})
    );
    let routed_token = routed["token"].as_str().unwrap().to_owned();
    let routed_window = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/browse/{routed_token}?start=0&limit=60"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(routed_window["photos"][0]["id"], by_location["b.jpg"]);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn browse_selection_filter_projects_album_and_folder_sources_without_writes() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    fs::create_dir_all(root.join("shoot")).unwrap();
    let shoot = root.join("shoot");
    for name in ["p1.jpg", "p2.jpg", "p3.jpg"] {
        jpeg_fixture(&shoot.join(name), 8, 4, [32, 64, 192]);
    }
    jpeg_fixture(&root.join("outside.jpg"), 8, 4, [12, 24, 36]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let by_location = photo_ids_by_location(&application, &ids).await;
    application
        .mutate_album(slipstream_core::AlbumMutation::Create {
            name: "Review".to_owned(),
        })
        .await
        .unwrap();
    let album_id = application
        .albums()
        .await
        .unwrap()
        .albums
        .into_iter()
        .find(|album| album.name == "Review")
        .unwrap()
        .id;
    application
        .mutate_album(slipstream_core::AlbumMutation::AddMembers {
            album_id: album_id.clone(),
            photo_ids: vec![
                by_location["shoot/p3.jpg"].clone(),
                by_location["shoot/p1.jpg"].clone(),
                by_location["shoot/p2.jpg"].clone(),
            ],
        })
        .await
        .unwrap();
    assert_eq!(
        decide_selection(&router, &by_location["shoot/p1.jpg"], "selected").await,
        StatusCode::OK
    );
    assert_eq!(
        decide_selection(&router, &by_location["shoot/p2.jpg"], "rejected").await,
        StatusCode::OK
    );

    // Album pages stay in membership position, so the filtered view keeps
    // the persisted member order instead of the Library order.
    let (opened, photos) = browse_filtered_summaries(
        &application,
        BrowseSourceRequest::Album(album_id.clone()),
        BrowseSelectionFilter::Selected,
    )
    .await;
    assert_eq!(opened.total, 1);
    assert_eq!(photos[0].id, by_location["shoot/p1.jpg"]);
    assert_eq!(
        opened.selection_counts,
        SelectionCountsWire {
            selected: 1,
            rejected: 1,
            undecided: 1,
        }
    );
    let (all_members, _) = browse_filtered_summaries(
        &application,
        BrowseSourceRequest::Album(album_id.clone()),
        BrowseSelectionFilter::All,
    )
    .await;
    assert_eq!(all_members.total, 3);
    // No filter value rewrites persisted membership position.
    let target = application
        .library
        .album_browse_target(&album_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        target
            .members
            .iter()
            .map(|member| member.photo_id.as_str())
            .collect::<Vec<_>>(),
        vec![
            by_location["shoot/p3.jpg"].as_str(),
            by_location["shoot/p1.jpg"].as_str(),
            by_location["shoot/p2.jpg"].as_str()
        ]
    );

    // A Folder source filters the same recursive projection, and Photos
    // outside the Folder never appear in it: a matching Photo elsewhere in
    // the Library must not leak into the Folder view or its counts.
    assert_eq!(
        decide_selection(&router, &by_location["outside.jpg"], "selected").await,
        StatusCode::OK
    );
    let publication = {
        let guard = application.shared.snapshot.read().unwrap();
        guard.as_ref().unwrap().publication_value()
    };
    let (folder_opened, folder_photos) = browse_filtered_summaries(
        &application,
        BrowseSourceRequest::Folder {
            location: "shoot".to_owned(),
            publication,
        },
        BrowseSelectionFilter::Selected,
    )
    .await;
    assert_eq!(folder_opened.total, 1);
    assert_eq!(folder_photos[0].id, by_location["shoot/p1.jpg"]);
    // The Folder's counts stay scoped to the Folder: `outside.jpg` is selected
    // in the Library but is not part of this source.
    assert_eq!(folder_opened.selection_counts.selected, 1);
    assert_eq!(folder_opened.selection_counts.rejected, 1);
    assert_eq!(folder_opened.selection_counts.undecided, 1);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn album_view_change_anchors_the_current_photo_instead_of_the_saved_position() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    capture_metadata_fixture(&root.join("a.jpg"), "2026:01:01 09:00:00");
    capture_metadata_fixture(&root.join("b.jpg"), "2026:01:01 10:00:00");
    capture_metadata_fixture(&root.join("c.jpg"), "2026:01:01 11:00:00");
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let by_location = photo_ids_by_location(&application, &ids).await;
    let (a, b, c) = (
        by_location["a.jpg"].clone(),
        by_location["b.jpg"].clone(),
        by_location["c.jpg"].clone(),
    );
    let album = application
        .mutate_album(slipstream_core::AlbumMutation::Create {
            name: "Review".to_owned(),
        })
        .await
        .unwrap()
        .albums
        .into_iter()
        .find(|album| album.name == "Review")
        .unwrap()
        .id;
    application
        .mutate_album(slipstream_core::AlbumMutation::AddMembers {
            album_id: album.clone(),
            photo_ids: vec![a.clone(), b.clone(), c.clone()],
        })
        .await
        .unwrap();
    // `c` is the durable saved position and `a` is the browser's current
    // Photo, so the saved member matches the filter while the anchor does not.
    application
        .mutate_album(slipstream_core::AlbumMutation::SetProgress {
            album_id: album.clone(),
            photo_id: c.clone(),
        })
        .await
        .unwrap();
    assert_eq!(
        decide_selection(&router, &b, "selected").await,
        StatusCode::OK
    );
    assert_eq!(
        decide_selection(&router, &c, "selected").await,
        StatusCode::OK
    );

    // A view change reopens with the browser's current Photo as the anchor.
    // The anchor no longer matches the filter, so the view starts at its first
    // Photo (`b`) instead of resuming at the saved Photo (`c`).
    let changed = application
        .browse_open(
            BrowseSourceRequest::Album(album.clone()),
            BrowseViewOrder::AlbumOrder,
            BrowseSelectionFilter::Selected,
            Some(&a),
        )
        .await
        .unwrap();
    assert_eq!(changed.total, 2);
    assert_eq!(changed.position, 0);
    let window = application
        .browse_window(&changed.token, changed.position, 1)
        .await
        .unwrap();
    assert_eq!(window.photos[0].id, b);
    application.browse_close(&changed.token);

    // An explicit anchor that matches the filter still outranks the saved
    // position, and an open without one keeps resuming at it.
    let anchored = application
        .browse_open(
            BrowseSourceRequest::Album(album.clone()),
            BrowseViewOrder::AlbumOrder,
            BrowseSelectionFilter::Selected,
            Some(&c),
        )
        .await
        .unwrap();
    assert_eq!(anchored.position, 1);
    application.browse_close(&anchored.token);
    let plain = application
        .browse_open(
            BrowseSourceRequest::Album(album),
            BrowseViewOrder::AlbumOrder,
            BrowseSelectionFilter::Selected,
            None,
        )
        .await
        .unwrap();
    assert_eq!(plain.position, 1);
    application.browse_close(&plain.token);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn browse_selection_filter_membership_is_frozen_until_the_source_reopens() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    for name in ["a.jpg", "b.jpg"] {
        jpeg_fixture(&root.join(name), 8, 4, [32, 64, 192]);
    }
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let by_location = photo_ids_by_location(&application, &ids).await;
    assert_eq!(
        decide_selection(&router, &by_location["a.jpg"], "selected").await,
        StatusCode::OK
    );
    let opened = application
        .browse_open(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::Selected,
            None,
        )
        .await
        .unwrap();
    assert_eq!(opened.total, 1);
    assert_eq!(opened.selection_counts.selected, 1);

    // A decision cannot change an open Snapshot's membership: the frozen
    // view still lists the Photo, and only reopening applies the filter to
    // the latest facts.
    assert_eq!(
        decide_selection(&router, &by_location["a.jpg"], "rejected").await,
        StatusCode::OK
    );
    let window = application
        .browse_window(&opened.token, 0, 60)
        .await
        .unwrap();
    assert_eq!(window.total, 1);
    assert_eq!(window.photos[0].id, by_location["a.jpg"]);
    assert_eq!(window.photos[0].selection_state, "rejected");

    let reopened = application
        .browse_open(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::Selected,
            Some(&by_location["a.jpg"]),
        )
        .await
        .unwrap();
    assert_eq!(reopened.total, 0);
    // The anchor no longer matches, so the reopened view reports the same
    // empty position as any other empty source.
    assert_eq!(reopened.position, 0);
    assert_eq!(
        reopened.selection_counts,
        SelectionCountsWire {
            selected: 0,
            rejected: 1,
            undecided: 1,
        }
    );
    application.browse_close(&opened.token);
    application.browse_close(&reopened.token);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn metadata_saturation_uses_shared_admission_and_safe_capture_fallback() {
    let (base, config) = prepare_fixture();
    generated_non_tiff_raw_fixture(&config.library_root.join("native.ARW"));
    capture_metadata_fixture(
        &config.library_root.join("known.jpg"),
        "2026:01:01 10:00:00",
    );
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let by_location = photo_ids_by_location(&application, &ids).await;
    let raw_id = by_location["native.ARW"].clone();
    let known_id = by_location["known.jpg"].clone();
    let raw = application
        .library
        .snapshot()
        .await
        .unwrap()
        .originals
        .into_iter()
        .find(|original| original.relative_path.as_str() == "native.ARW")
        .unwrap();
    assert_eq!(raw.kind, slipstream_core::OriginalKind::Raw);
    assert!(raw.capture.source_revision.is_some());

    let scheduled = Arc::new(AtomicUsize::new(0));
    let scheduled_hook = Arc::clone(&scheduled);
    let _hook = crate::app::install_metadata_inspection_test_hook(move |_| {
        scheduled_hook.fetch_add(1, Ordering::AcqRel);
    });
    let first = application
        .library
        .try_admit_native_work()
        .expect("first shared native slot");
    let second = application
        .library
        .try_admit_native_work()
        .expect("second shared native slot");

    let saturated =
        tokio::time::timeout(Duration::from_secs(1), application.photo_metadata(&raw_id))
            .await
            .expect("saturation fallback must not wait")
            .unwrap();
    assert_eq!(saturated, slipstream_core::CaptureReviewMetadata::default());
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let direct = tokio::time::timeout(
        Duration::from_secs(1),
        send(
            &router,
            authenticated_request()
                .uri(format!("https://camera.local/api/photos/{known_id}"))
                .header("Slipstream-CLI-Contract", "1")
                .body(Body::empty())
                .unwrap(),
        ),
    )
    .await
    .expect("CLI fallback must not wait");
    let direct = response_json(direct).await;
    assert_eq!(direct["metadata"]["state"], "known");
    assert_eq!(
        direct["metadata"]["captureTime"],
        "2026-01-01T10:00:00.000000000"
    );
    assert_eq!(scheduled.load(Ordering::Acquire), 0);

    drop((first, second));
    assert_eq!(
        application.photo_metadata(&raw_id).await.unwrap(),
        slipstream_core::CaptureReviewMetadata::default()
    );
    assert_eq!(scheduled.load(Ordering::Acquire), 1);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn cancelled_raw_metadata_requests_retain_admission_until_native_work_finishes() {
    let (base, config) = prepare_fixture();
    for name in ["cancel-a.ARW", "cancel-b.ARW", "cancel-c.ARW"] {
        generated_non_tiff_raw_fixture(&config.library_root.join(name));
    }
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let by_location = photo_ids_by_location(&application, &ids).await;

    // Enrollment shares native capacity. Wait for its actual completion before
    // asserting that both slots belong to these controlled metadata workers.
    let deadline = Instant::now() + Duration::from_secs(5);
    while application.library.fingerprint_counts().enrolled != 3 {
        assert!(
            Instant::now() < deadline,
            "fingerprint enrollment did not settle"
        );
        tokio::task::yield_now().await;
    }
    let gate = Arc::new((Mutex::new((0_usize, false)), Condvar::new()));
    struct ReleaseGate(Arc<(Mutex<(usize, bool)>, Condvar)>);
    impl Drop for ReleaseGate {
        fn drop(&mut self) {
            let (lock, signal) = &*self.0;
            lock.lock().unwrap().1 = true;
            signal.notify_all();
        }
    }
    // A failed assertion must also release native workers before Tokio teardown.
    let _release = ReleaseGate(Arc::clone(&gate));
    let hook_gate = Arc::clone(&gate);
    let _hook = crate::app::install_metadata_inspection_test_hook(move |path| {
        if !path.as_str().starts_with("cancel-") {
            return;
        }
        let (lock, signal) = &*hook_gate;
        let mut state = lock.lock().unwrap();
        state.0 += 1;
        signal.notify_all();
        while !state.1 {
            state = signal.wait(state).unwrap();
        }
    });

    let first_application = Arc::clone(&application);
    let first_id = by_location["cancel-a.ARW"].clone();
    let first = tokio::spawn(async move { first_application.photo_metadata(&first_id).await });
    let second_application = Arc::clone(&application);
    let second_id = by_location["cancel-b.ARW"].clone();
    let second = tokio::spawn(async move { second_application.photo_metadata(&second_id).await });
    let deadline = Instant::now() + Duration::from_secs(5);
    while gate.0.lock().unwrap().0 != 2 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(gate.0.lock().unwrap().0, 2, "two metadata workers admitted");

    let third = tokio::time::timeout(
        Duration::from_secs(1),
        application.photo_metadata(&by_location["cancel-c.ARW"]),
    )
    .await
    .expect("third request must fall back instead of queueing")
    .unwrap();
    assert_eq!(third, slipstream_core::CaptureReviewMetadata::default());
    assert_eq!(gate.0.lock().unwrap().0, 2, "no third worker scheduled");

    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    let after_cancellation = tokio::time::timeout(
        Duration::from_secs(1),
        application.photo_metadata(&by_location["cancel-c.ARW"]),
    )
    .await
    .expect("cancelled waiter must not release active native work")
    .unwrap();
    assert_eq!(
        after_cancellation,
        slipstream_core::CaptureReviewMetadata::default()
    );
    assert_eq!(gate.0.lock().unwrap().0, 2);
    assert!(application.library.try_admit_native_work().is_none());

    {
        let (lock, signal) = &*gate;
        lock.lock().unwrap().1 = true;
        signal.notify_all();
    }
    second.await.unwrap().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let recovered = loop {
        if let Some(first) = application.library.try_admit_native_work() {
            if let Some(second) = application.library.try_admit_native_work() {
                break (first, second);
            }
            drop(first);
        }
        assert!(
            Instant::now() < deadline,
            "native admission was not released"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    assert!(application.library.try_admit_native_work().is_none());
    drop(recovered);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

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
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;

    // Commit a Selection State and a Review Preview seed while the
    // completed scan is parked before publication.
    assert_eq!(
        post_json(
            &router,
            &format!("https://camera.local/api/photos/{}/state", ids[0]),
            serde_json::json!({"field": "selectionState", "value": "selected"}),
            Some("https://camera.local"),
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
        .browse_open(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::All,
            None,
        )
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
        .browse_open(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::All,
            None,
        )
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
async fn fresh_service_is_healthy_while_library_initializes_then_status_reaches_published_idle() {
    let (base, config) = prepare_fixture();
    let (gate_sender, gate_receiver) = tokio::sync::oneshot::channel();
    let application =
        Application::open_with_gate(&config, ScanLimits::default(), Some(gate_receiver), None)
            .await
            .unwrap();
    let router = authorized_router(Arc::clone(&application), config.web_root());

    let health = send(
        &router,
        authenticated_request()
            .uri("https://camera.local/healthz")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(health.status(), StatusCode::OK);

    let overview: serde_json::Value = response_json(
        send(
            &router,
            authenticated_request()
                .uri("https://camera.local/api/overview")
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
            authenticated_request()
                .uri("https://camera.local/api/status")
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
            authenticated_request()
                .uri("https://camera.local/api/overview")
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
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::SERVICE_UNAVAILABLE);

    // An Album open is no exception. `album-order` reads persisted membership
    // position, but anchor, filter membership, and counts still come from the
    // Published Library, so it fails with the same not-published response
    // instead of failing later at its first window.
    let early_album = application
        .mutate_album(slipstream_core::AlbumMutation::Create {
            name: "Early".to_owned(),
        })
        .await
        .unwrap()
        .albums
        .into_iter()
        .find(|album| album.name == "Early")
        .unwrap()
        .id;
    let rejected_album = post_json(
        &router,
        "/api/browse",
        serde_json::json!({"source":"album","albumId": early_album}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(rejected_album.status(), StatusCode::SERVICE_UNAVAILABLE);

    drop(gate_sender);
    wait_for_scan_settled(&application).await;
    let overview: serde_json::Value = response_json(
        send(
            &router,
            authenticated_request()
                .uri("https://camera.local/api/overview")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(overview["published"], true);
    assert_eq!(overview["scan"]["state"], "idle");
    let status: serde_json::Value = response_json(
        send(
            &router,
            authenticated_request()
                .uri("https://camera.local/api/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(status["state"], "idle");
    assert!(status["publication"].is_string());
    let opened = post_json(
        &router,
        "/api/browse",
        serde_json::json!({"source":"library"}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(opened.status(), StatusCode::OK);
    application.shutdown().await.unwrap();

    // A completed empty publication is durable: restart serves the empty
    // Library immediately instead of regressing to initializing.
    let (restart_gate_sender, restart_gate_receiver) = tokio::sync::oneshot::channel();
    let reopened = Application::open_with_gate(
        &config,
        ScanLimits::default(),
        Some(restart_gate_receiver),
        None,
    )
    .await
    .unwrap();
    let overview = reopened.overview().await.unwrap();
    assert!(overview.published);
    assert_eq!(overview.photo_count, 0);
    assert_eq!(overview.scan.state, "idle");
    let opened = reopened
        .browse_open(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::All,
            None,
        )
        .await
        .unwrap();
    assert_eq!(opened.total, 0);
    restart_gate_sender.send(()).unwrap();
    reopened.shutdown().await.unwrap();
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
    let router = authorized_router(Arc::clone(&application), config.web_root());

    // The published Library must be served before the background rescan
    // has run at all.
    let overview: serde_json::Value = response_json(
        send(
            &router,
            authenticated_request()
                .uri("https://camera.local/api/overview")
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
        Some("https://camera.local"),
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
            authenticated_request()
                .uri("https://camera.local/api/overview")
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
    let router = authorized_router(Arc::clone(&application), config.web_root());

    assert_eq!(application.scan_status().state, "failed");
    let overview: serde_json::Value = response_json(
        send(
            &router,
            authenticated_request()
                .uri("https://camera.local/api/overview")
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
        authenticated_request()
            .method("POST")
            .uri("https://camera.local/api/scan")
            .header(header::ORIGIN, "https://camera.local")
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
async fn startup_and_explicit_waiters_share_one_scan_cycle_and_terminal_status() {
    let (base, config) = prepare_fixture();
    capture_metadata_fixture(&config.library_root.join("a.jpg"), "2026:01:01 09:00:00");
    let (gate_sender, gate_receiver) = tokio::sync::oneshot::channel();
    let application =
        Application::open_with_gate(&config, ScanLimits::default(), Some(gate_receiver), None)
            .await
            .unwrap();

    // Startup was admitted synchronously but its leader is parked. Every
    // explicit waiter must join that same application-owned cycle.
    let receivers = (0..8)
        .map(|_| application.admit_scan_cycle(None, None).unwrap())
        .collect::<Vec<_>>();
    gate_sender.send(()).unwrap();

    let mut terminal = None;
    for receiver in receivers {
        let status = receiver.await.unwrap().unwrap();
        let facts = (status.state, status.completed, status.total);
        if let Some(expected) = terminal {
            assert_eq!(facts, expected);
        } else {
            terminal = Some(facts);
        }
    }
    assert_eq!(terminal, Some(("idle", Some(1), Some(1))));
    assert_eq!(application.shared.runs_started.load(Ordering::Relaxed), 1);
    assert_eq!(application.shared.runs_completed.load(Ordering::Relaxed), 1);
    assert_eq!(application.shared.awaiting_scan.load(Ordering::Relaxed), 0);

    // A terminal cycle releases admission for one later independent cycle.
    let next = application.rescan().await.unwrap();
    assert_eq!(
        (next.state, next.completed, next.total),
        ("idle", Some(1), Some(1))
    );
    assert_eq!(application.shared.runs_started.load(Ordering::Relaxed), 2);
    assert_eq!(application.shared.runs_completed.load(Ordering::Relaxed), 2);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn dropped_scan_waiters_do_not_cancel_the_leader_or_leak_applying_status() {
    let (base, config) = prepare_fixture();
    capture_metadata_fixture(&config.library_root.join("a.jpg"), "2026:01:01 09:00:00");
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let baseline = application.shared.runs_completed.load(Ordering::Relaxed);

    let (gate_sender, gate_receiver) = tokio::sync::oneshot::channel();
    let first = application
        .admit_scan_cycle(Some(gate_receiver), None)
        .unwrap();
    let mut abandoned = vec![first];
    for _ in 1..64 {
        abandoned.push(application.admit_scan_cycle(None, None).unwrap());
    }
    // These receivers model HTTP request futures dropped after admission.
    drop(abandoned);
    // Cancellation must release bounded waiter capacity before the physical
    // cycle completes; this live caller joins the same leader.
    let live = application.admit_scan_cycle(None, None).unwrap();
    gate_sender.send(()).unwrap();
    assert_eq!(live.await.unwrap().unwrap().state, "idle");

    wait_for_scan_runs(&application, baseline + 1).await;
    assert_eq!(
        application.shared.runs_started.load(Ordering::Relaxed),
        baseline + 1
    );
    assert_eq!(application.shared.awaiting_scan.load(Ordering::Relaxed), 0);
    assert_eq!(application.scan_status().state, "idle");

    // Cleanup of the abandoned cycle must leave the next admission usable.
    let next = application.rescan().await.unwrap();
    assert_eq!(next.state, "idle");
    assert_eq!(
        application.shared.runs_completed.load(Ordering::Relaxed),
        baseline + 2
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn shutdown_returns_only_after_live_scan_waiters_receive_terminal_status() {
    let (base, config) = prepare_fixture();
    capture_metadata_fixture(&config.library_root.join("a.jpg"), "2026:01:01 09:00:00");
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;

    let (gate_sender, gate_receiver) = tokio::sync::oneshot::channel();
    let mut receive = application
        .admit_scan_cycle(Some(gate_receiver), None)
        .unwrap();
    let closing = {
        let application = Arc::clone(&application);
        tokio::spawn(async move { application.shutdown().await })
    };
    tokio::task::yield_now().await;
    gate_sender.send(()).unwrap();
    closing.await.unwrap().unwrap();

    let terminal = receive
        .try_recv()
        .expect("shutdown returned before scan waiter fan-out")
        .unwrap();
    assert_eq!(terminal.state, "idle");
    assert_eq!(application.shared.awaiting_scan.load(Ordering::Relaxed), 0);
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
    let router = authorized_router(Arc::clone(&application), config.web_root());

    // Served from the persisted Library before the background rescan runs.
    let overview_response = send(
        &router,
        authenticated_request()
            .uri("https://camera.local/api/overview")
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
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(opened.status(), StatusCode::OK);
    let opened: serde_json::Value = response_json(opened).await;
    let token = opened["token"].as_str().unwrap();
    let window: serde_json::Value = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/browse/{token}?start=39940&limit=60"
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
            authenticated_request()
                .uri("https://camera.local/api/overview")
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

/// Deterministic manual-recovery HTTP fixture: one unavailable Photo with a
/// retained Rating, Selection State, and Album membership, seeded before the
/// Application opens. `fingerprint` optionally seeds a matching fingerprint
/// for the remembered bytes.
fn recovery_http_fixture(fingerprint: bool) -> (PathBuf, Config) {
    let (base, config) = prepare_fixture();
    fs::create_dir_all(&config.state_directory).unwrap();
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
        .execute_batch(include_str!("../../../compatibility/sqlite/schema-v6.sql"))
        .unwrap();
    database
        .execute(
            "INSERT INTO library_metadata VALUES('canonical_root',?)",
            [config.library_root.to_str().unwrap()],
        )
        .unwrap();
    database.execute(
        "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,capture_metadata_state,capture_source_revision) VALUES('aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa','shoot/a.JPG','jpeg',11,1.0,0,'missing','remembered-revision')",
        [],
    ).unwrap();
    database.execute(
        "INSERT INTO photos(id,original_id,available,preview_state,sort_path,selection_state,rating) VALUES('bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb','aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',0,'unavailable','shoot/a.JPG','selected',3)",
        [],
    ).unwrap();
    database
        .execute("INSERT INTO albums VALUES('set','Trip',1)", [])
        .unwrap();
    database
        .execute(
            "INSERT INTO album_members VALUES('set','bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',0)",
            [],
        )
        .unwrap();
    if fingerprint {
        database.execute(
            "INSERT INTO original_fingerprints(original_id,digest,size,mtime_ms) VALUES('aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',?,11,1.0)",
            [slipstream_core::digest_bytes(b"jpeg-bytes-a")],
        ).unwrap();
    }
    drop(database);
    (base, config)
}

async fn library_window(router: &Router) -> serde_json::Value {
    let opened = response_json(
        post_json(
            router,
            "/api/browse",
            serde_json::json!({"source":"library"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let token = opened["token"].as_str().unwrap();
    response_json(
        send(
            router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/browse/{token}?start=0&limit=60"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await
}

#[tokio::test]
async fn recovery_http_restores_unavailable_photo_without_fingerprint() {
    let (base, config) = recovery_http_fixture(false);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());

    // The moved file appears only after the settled scan, so no automatic
    // recovery can act and the persisted record stays unavailable.
    fs::create_dir_all(config.library_root.join("moved")).unwrap();
    fs::write(config.library_root.join("moved/a.JPG"), b"jpeg-bytes-a").unwrap();

    let unavailable = response_json(
        send(
            &router,
            authenticated_request()
                .uri("https://camera.local/api/recovery/unavailable")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(unavailable["unavailable"].as_array().unwrap().len(), 1);
    let record = &unavailable["unavailable"][0];
    assert_eq!(record["location"], "shoot/a.JPG");
    assert_eq!(record["kind"], "jpeg");
    assert_eq!(record["rating"], 3);
    assert_eq!(record["selectionState"], "selected");
    assert_eq!(record["fingerprintEnrolled"], false);
    assert_eq!(record["albumCount"], 1);

    let proposals = response_json(
        post_json(
            &router,
            "/api/recovery/propose",
            serde_json::json!({"oldPrefix":"shoot","newPrefix":"moved"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let proposal = &proposals["proposals"][0];
    assert_eq!(proposal["outcome"], "matched");
    assert_eq!(proposal["verified"], false);
    assert_eq!(proposal["toLocation"], "moved/a.JPG");

    let applied = response_json(
        post_json(
            &router,
            "/api/recovery/apply",
            serde_json::json!({"relocations":[{"originalId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","newLocation":"moved/a.JPG"}]}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(applied["relocatedPhotos"], 1);
    assert_eq!(applied["unavailablePhotos"], 0);

    // The restored Photo keeps its identity, decisions, and Album membership.
    let window = library_window(&router).await;
    assert_eq!(window["total"], 1);
    let photo = &window["photos"][0];
    assert_eq!(photo["id"], "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
    assert_eq!(photo["available"], true);
    assert_eq!(photo["originalFilename"], "a.JPG");
    assert_eq!(photo["rating"], 3);
    assert_eq!(photo["selectionState"], "selected");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn recovery_http_retires_discovered_destination_photo() {
    let (base, config) = recovery_http_fixture(false);
    // The moved file exists before open, so the initial scan discovers it as
    // a new default-state Photo occupying the destination.
    fs::create_dir_all(config.library_root.join("moved")).unwrap();
    fs::write(config.library_root.join("moved/a.JPG"), b"jpeg-bytes-a").unwrap();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());

    // Both records are visible: the remembered unavailable Photo and the
    // newly discovered occupier.
    let window = library_window(&router).await;
    assert_eq!(window["total"], 2);
    let discovered = window["photos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|photo| photo["id"].as_str().unwrap().to_owned())
        .find(|id| id != "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb")
        .unwrap();

    let proposals = response_json(
        post_json(
            &router,
            "/api/recovery/propose",
            serde_json::json!({"oldPrefix":"shoot","newPrefix":"moved"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let proposal = &proposals["proposals"][0];
    assert_eq!(proposal["outcome"], "occupied");
    assert_eq!(proposal["retire"]["photoId"].as_str().unwrap(), discovered);

    // Without the explicit retire the whole batch is refused with a 409.
    let refused = post_json(
        &router,
        "/api/recovery/apply",
        serde_json::json!({"relocations":[{"originalId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","newLocation":"moved/a.JPG"}]}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);

    let applied = response_json(
        post_json(
            &router,
            "/api/recovery/apply",
            serde_json::json!({"relocations":[{"originalId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","newLocation":"moved/a.JPG","retireDestination":true}]}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(applied["relocatedPhotos"], 1);

    let window = library_window(&router).await;
    assert_eq!(window["total"], 1);
    let photo = &window["photos"][0];
    assert_eq!(photo["id"], "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
    assert_eq!(photo["rating"], 3);
    assert_eq!(photo["selectionState"], "selected");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn recovery_http_verifies_fingerprints_and_rejects_mismatches() {
    let (base, config) = recovery_http_fixture(true);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    fs::create_dir_all(config.library_root.join("moved")).unwrap();
    // Different content than the enrolled fingerprint.
    fs::write(config.library_root.join("moved/a.JPG"), b"other-bytes").unwrap();

    let proposals = response_json(
        post_json(
            &router,
            "/api/recovery/propose",
            serde_json::json!({"oldPrefix":"shoot","newPrefix":"moved"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(proposals["proposals"][0]["outcome"], "content-mismatch");

    let refused = post_json(
        &router,
        "/api/recovery/apply",
        serde_json::json!({"relocations":[{"originalId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","newLocation":"moved/a.JPG"}]}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    let body = response_json(refused).await;
    assert_eq!(body["rejections"][0]["reason"], "content-mismatch");

    // Matching content verifies and commits.
    fs::write(config.library_root.join("moved/a.JPG"), b"jpeg-bytes-a").unwrap();
    let proposals = response_json(
        post_json(
            &router,
            "/api/recovery/propose",
            serde_json::json!({"oldPrefix":"shoot","newPrefix":"moved"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(proposals["proposals"][0]["outcome"], "matched");
    assert_eq!(proposals["proposals"][0]["verified"], true);

    let single = response_json(
        post_json(
            &router,
            "/api/recovery/propose",
            serde_json::json!({"originalId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","newLocation":"moved/a.JPG"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(single["outcome"], "matched");
    assert_eq!(single["verified"], true);

    let applied = response_json(
        post_json(
            &router,
            "/api/recovery/apply",
            serde_json::json!({"relocations":[{"originalId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","newLocation":"moved/a.JPG"}]}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(applied["relocatedPhotos"], 1);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn recovery_http_rejects_duplicate_source_mappings() {
    let (base, config) = recovery_http_fixture(false);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    fs::create_dir_all(config.library_root.join("moved")).unwrap();
    fs::write(config.library_root.join("moved/a.JPG"), b"jpeg-bytes-a").unwrap();
    fs::write(config.library_root.join("moved/b.JPG"), b"jpeg-bytes-b").unwrap();

    // Two mappings for one Original File are a colliding batch refused with
    // a per-mapping reason.
    let refused = post_json(
        &router,
        "/api/recovery/apply",
        serde_json::json!({"relocations":[
            {"originalId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","newLocation":"moved/a.JPG"},
            {"originalId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","newLocation":"moved/b.JPG"}
        ]}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    let body = response_json(refused).await;
    assert_eq!(body["rejections"][0]["reason"], "colliding");

    // The refusal leaves the Library untouched.
    let unavailable = response_json(
        send(
            &router,
            authenticated_request()
                .uri("https://camera.local/api/recovery/unavailable")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(unavailable["unavailable"].as_array().unwrap().len(), 1);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn recovery_http_validates_requests() {
    let (base, config) = recovery_http_fixture(false);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());

    let escape = post_json(
        &router,
        "/api/recovery/propose",
        serde_json::json!({"oldPrefix":"..","newPrefix":"moved"}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(escape.status(), StatusCode::BAD_REQUEST);

    let unknown = post_json(
        &router,
        "/api/recovery/propose",
        serde_json::json!({"originalId":"cccccccc-cccc-4ccc-8ccc-cccccccccccc","newLocation":"moved/a.JPG"}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

    let empty = post_json(
        &router,
        "/api/recovery/apply",
        serde_json::json!({"relocations":[]}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(empty.status(), StatusCode::BAD_REQUEST);

    let stale = post_json(
        &router,
        "/api/recovery/apply",
        serde_json::json!({"relocations":[{"originalId":"cccccccc-cccc-4ccc-8ccc-cccccccccccc","newLocation":"moved/a.JPG"}]}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let body = response_json(stale).await;
    assert_eq!(body["rejections"][0]["reason"], "stale");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn recovery_http_reports_missing_destination_without_fingerprint() {
    let (base, config) = recovery_http_fixture(false);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());

    // Nothing exists at the destination: the batch must not present it as a
    // usable mapping, and it must stay explicitly unverified.
    let proposals = response_json(
        post_json(
            &router,
            "/api/recovery/propose",
            serde_json::json!({"oldPrefix":"shoot","newPrefix":"moved"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let proposal = &proposals["proposals"][0];
    assert_eq!(proposal["outcome"], "missing");
    assert_eq!(proposal["verified"], false);
    assert_eq!(proposal["toLocation"], "moved/a.JPG");

    // Applying the same mapping is refused, and the refusal changes nothing.
    // The apply path judges the candidate in its own vocabulary: absent and
    // unreadable are both refusals.
    let refused = post_json(
        &router,
        "/api/recovery/apply",
        serde_json::json!({"relocations":[{"originalId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","newLocation":"moved/a.JPG"}]}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    let body = response_json(refused).await;
    assert_eq!(body["rejections"][0]["reason"], "unreadable");

    let unavailable = response_json(
        send(
            &router,
            authenticated_request()
                .uri("https://camera.local/api/recovery/unavailable")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(unavailable["unavailable"].as_array().unwrap().len(), 1);

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
async fn cli_album_reorder_refuses_an_album_larger_than_the_complete_order_bound() {
    let (base, config) = prepare_fixture();
    for index in 0..=slipstream_core::ALBUM_MEMBERSHIP_BATCH_MAX {
        jpeg_fixture(
            &config.library_root.join(format!("{index:03}.jpg")),
            8,
            4,
            [32, 64, 192],
        );
    }
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let photo_ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    assert_eq!(photo_ids.len(), 101);
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let created = response_json(
        post_cli_json(&router, "/api/albums", serde_json::json!({"name": "Large"})).await,
    )
    .await;
    let album_id = created["album"]["id"].as_str().unwrap().to_owned();
    let initial_version = created["album"]["albumVersion"]
        .as_str()
        .unwrap()
        .to_owned();
    let first_add = response_json(
        post_cli_json(
            &router,
            &format!("/api/albums/{album_id}/changes"),
            serde_json::json!({
                "operation": "add",
                "photoIds": photo_ids[..100],
                "ifVersion": initial_version
            }),
        )
        .await,
    )
    .await;
    let first_version = first_add["album"]["albumVersion"]
        .as_str()
        .unwrap()
        .to_owned();
    let second_add = response_json(
        post_cli_json(
            &router,
            &format!("/api/albums/{album_id}/changes"),
            serde_json::json!({
                "operation": "add",
                "photoIds": [photo_ids[100]],
                "ifVersion": first_version
            }),
        )
        .await,
    )
    .await;
    let complete_version = second_add["album"]["albumVersion"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(second_add["album"]["photoCount"], 101);

    let refused = post_cli_json(
        &router,
        &format!("/api/albums/{album_id}/changes"),
        serde_json::json!({
            "operation": "reorder",
            "photoIds": photo_ids[..100],
            "ifVersion": complete_version
        }),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        response_json(refused).await["error"],
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
    assert_eq!(unchanged.photo_count, 101);
    assert_eq!(unchanged.album_version, complete_version);

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
                "retainedQueryIdsMaximum": 1_000_000,
                "retainedQueryIdleSeconds": 900
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
        Some(ProcessingConfig {
            instance: "f".repeat(32),
            policy_sha256: "b".repeat(64),
            bundle_sha256: "c".repeat(64),
        }),
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
            "state": "launcher-unavailable",
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
            "originalAvailable".to_owned(),
            "originalKind".to_owned(),
            "preview".to_owned(),
            "rating".to_owned(),
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
        format!("https://camera.local{uri}")
    } else {
        uri.to_owned()
    };
    let mut builder = authenticated_request()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(origin) = origin {
        builder = builder.header(header::ORIGIN, origin);
    }
    send(router, builder.body(Body::from(body.to_string())).unwrap()).await
}

async fn post_cli_json(router: &Router, uri: &str, body: serde_json::Value) -> Response<Body> {
    send(
        router,
        authenticated_request()
            .method("POST")
            .uri(format!("https://camera.local{uri}"))
            .header("Slipstream-CLI-Contract", "1")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

async fn get_cli_json(router: &Router, uri: &str) -> Response<Body> {
    send(
        router,
        authenticated_request()
            .uri(format!("http://camera.local{uri}"))
            .header("Slipstream-CLI-Contract", "1")
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

async fn cli_photo_read(router: &Router, photo_id: &str) -> serde_json::Value {
    let response = get_cli_json(router, &format!("/api/photos/{photo_id}")).await;
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await
}

fn jpeg_fixture(path: &Path, width: u32, height: u32, color: [u8; 3]) {
    image::RgbImage::from_pixel(width, height, image::Rgb(color))
        .save_with_format(path, image::ImageFormat::Jpeg)
        .unwrap();
}

fn jpeg_fixture_with_capture_time(
    path: &Path,
    width: u32,
    height: u32,
    color: [u8; 3],
    capture_time: &str,
) {
    jpeg_fixture(path, width, height, color);
    let jpeg = fs::read(path).unwrap();
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
    let mut bytes = jpeg[..2].to_vec();
    bytes.extend_from_slice(b"\xff\xe1");
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.extend_from_slice(&payload);
    bytes.extend_from_slice(&jpeg[2..]);
    fs::write(path, bytes).unwrap();
}

/// A sparse JPEG-suffixed file beyond the preview input budget. The scanner
/// can retain its identity, while preview inspection returns the real request
/// failure state without allocating 128 MiB of fixture data.
fn oversized_jpeg_fixture(path: &Path) {
    let file = fs::File::create(path).unwrap();
    file.set_len(128 * 1024 * 1024 + 1).unwrap();
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

fn generated_non_tiff_raw_fixture(path: &Path) {
    let bytes = b"Slipstream generated non-TIFF RAW metadata fixture";
    assert!(!matches!(&bytes[..4], b"II*\0" | b"MM\0*"));
    fs::write(path, bytes).unwrap();
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
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    assert_eq!(ids.len(), 3);

    let created = response_json(
        post_json(
            &router,
            "/api/albums",
            serde_json::json!({"name": " Picks "}),
            Some("https://camera.local"),
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
    let album_a = created["albums"][0]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        send(
            &router,
            authenticated_request()
                .method("POST")
                .uri(format!("https://camera.local/api/albums/{album_a}/members"))
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ORIGIN, "https://camera.local")
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
            &format!("https://camera.local/api/albums/{album_a}/order"),
            serde_json::json!({"photoIds": [&ids[2], &ids[0], &ids[1]]}),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        post_json(
            &router,
            &format!("https://camera.local/api/albums/{album_a}/progress"),
            serde_json::json!({"photoId": ids[0]}),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let created_b = response_json(
        post_json(
            &router,
            "https://camera.local/api/albums",
            serde_json::json!({"name": "Other"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let album_b = created_b["albums"]
        .as_array()
        .unwrap()
        .iter()
        .find(|album| album["name"] == "Other")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let first_b_add = response_json(
        post_json(
            &router,
            &format!("https://camera.local/api/albums/{album_b}/members"),
            serde_json::json!({"photoIds": [&ids[0]]}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(first_b_add["albumId"], album_b);
    assert_eq!(first_b_add["addedPhotoIds"], serde_json::json!([ids[0]]));
    assert_eq!(first_b_add["alreadyMemberPhotoIds"], serde_json::json!([]));

    let mixed_b_add = response_json(
        post_json(
            &router,
            &format!("https://camera.local/api/albums/{album_b}/members"),
            serde_json::json!({"photoIds": [&ids[0], &ids[1]]}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(mixed_b_add["addedPhotoIds"], serde_json::json!([ids[1]]));
    assert_eq!(
        mixed_b_add["alreadyMemberPhotoIds"],
        serde_json::json!([ids[0]])
    );
    assert_eq!(
        post_json(
            &router,
            &format!("https://camera.local/api/albums/{album_b}/progress"),
            serde_json::json!({"photoId": ids[1]}),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let removed_b = response_json(
        post_json(
            &router,
            &format!("https://camera.local/api/albums/{album_b}/members/batch-remove"),
            serde_json::json!({"photoIds": [ids[1]]}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(removed_b["removedPhotoIds"], serde_json::json!([ids[1]]));
    assert_eq!(removed_b["alreadyAbsentPhotoIds"], serde_json::json!([]));
    assert_eq!(
        removed_b["albums"]
            .as_array()
            .unwrap()
            .iter()
            .find(|album| album["id"] == album_b)
            .unwrap()["hasSavedPosition"],
        false
    );
    let repeated_removed_b = response_json(
        post_json(
            &router,
            &format!("https://camera.local/api/albums/{album_b}/members/batch-remove"),
            serde_json::json!({"photoIds": [ids[1]]}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(repeated_removed_b["removedPhotoIds"], serde_json::json!([]));
    assert_eq!(
        repeated_removed_b["alreadyAbsentPhotoIds"],
        serde_json::json!([ids[1]])
    );
    assert_eq!(
        post_json(
            &router,
            &format!("https://camera.local/api/albums/{album_b}/members"),
            serde_json::json!({"photoIds": [&ids[2], &ids[2]]}),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_json(
            &router,
            &format!("https://camera.local/api/albums/{album_b}/members/batch-remove"),
            serde_json::json!({"photoIds": [&ids[0], &ids[0]]}),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_json(
            &router,
            &format!("https://camera.local/api/albums/{album_b}/members/batch-remove"),
            serde_json::json!({"photoIds": []}),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    let over_limit = (0..=100)
        .map(|index| format!("00000000-0000-4000-8000-{index:012}"))
        .collect::<Vec<_>>();
    assert_eq!(
        post_json(
            &router,
            &format!("https://camera.local/api/albums/{album_b}/members/batch-remove"),
            serde_json::json!({"photoIds": over_limit}),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post_json(
            &router,
            &format!("https://camera.local/api/albums/{album_b}/members/batch-remove"),
            serde_json::json!({"photoIds": ["00000000-0000-4000-8000-00000000dead"]}),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    let selected = response_json(
        post_json(
            &router,
            &format!("https://camera.local/api/photos/{}/state", ids[0]),
            serde_json::json!({"field": "selectionState", "value": "selected", "albumId": album_a}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let undo = selected["undo"].clone();
    assert_eq!(selected["kind"], "applied");
    assert_eq!(
        post_json(
            &router,
            &format!("https://camera.local/api/photos/{}/state", ids[0]),
            serde_json::json!({"field": "rating", "value": 4}),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::OK
    );
    // Membership order is observable only through a fresh Album Browse
    // Snapshot; the bounded membership response carries identities, not
    // member lists.
    let ordered = browse_photo_ids(&application, BrowseSourceRequest::Album(album_a.clone())).await;
    assert_eq!(
        ordered,
        vec![ids[2].clone(), ids[0].clone(), ids[1].clone()]
    );
    let album_b_photos =
        browse_summaries(&application, BrowseSourceRequest::Album(album_b.clone())).await;
    let shared = album_b_photos
        .iter()
        .find(|photo| photo.id == ids[0])
        .unwrap();
    assert_eq!(shared.selection_state, "selected");
    assert_eq!(shared.rating, 4);
    assert_eq!(
        post_json(
            &router,
            &format!("https://camera.local/api/photos/{}/state", ids[0]),
            serde_json::json!({
                "field": undo["field"],
                "value": undo["priorValue"],
                "expectedCurrent": undo["expectedCurrent"]
            }),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        post_json(
            &router,
            &format!("https://camera.local/api/photos/{}/state", ids[0]),
            serde_json::json!({"field": "selectionState", "value": "rejected"}),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let conflict = response_json(
            post_json(
                &router,
                &format!("https://camera.local/api/photos/{}/state", ids[0]),
                serde_json::json!({"field": "selectionState", "value": "selected", "expectedCurrent": "undecided"}),
                Some("https://camera.local"),
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
            &format!("https://camera.local/api/albums/{album_a}/members/remove"),
            serde_json::json!({"photoId": ids[0]}),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::OK
    );
    // Removing the saved-position Photo clears the persisted progress;
    // the summary-only mutation response proves the cleared flag.
    let album_a_summary = application
        .albums()
        .await
        .unwrap()
        .albums
        .into_iter()
        .find(|album| album.id == album_a)
        .unwrap();
    assert!(!album_a_summary.has_saved_position);
    let before_original = fs::read(config.library_root.join("b.jpg")).unwrap();
    assert_eq!(
        post_json(
            &router,
            &format!("https://camera.local/api/albums/{album_a}/delete"),
            serde_json::json!({}),
            Some("https://camera.local"),
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
            .all(|album| album.id != album_a)
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
    // The request policy admits DELETE only for /api/browse/{token}, so
    // the retired list endpoint is rejected 405 before routing instead
    // of reaching the API 404 fallback.
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
    assert_eq!(derivative.headers()[header::CACHE_CONTROL], "no-store");
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
async fn photo_metadata_protocol_returns_capture_time_when_available() {
    let (base, config) = prepare_fixture();
    capture_metadata_fixture(
        &config.library_root.join("metadata.jpg"),
        "2026:02:03 04:05:06",
    );
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .into_iter()
        .next()
        .unwrap();
    let response = send(
        &router,
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/photos/{photo_id}/metadata"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let metadata = response_json(response).await;
    assert_eq!(metadata["captureTime"], "2026-02-03T04:05:06.000000000");
    assert!(metadata["aperture"].is_null());
    fs::remove_file(config.library_root.join("metadata.jpg")).unwrap();
    let unavailable = send(
        &router,
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/photos/{photo_id}/metadata"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(unavailable.status(), StatusCode::OK);
    assert_eq!(response_json(unavailable).await, serde_json::json!({}));
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

#[tokio::test]
async fn browse_windows_report_the_ordering_original_filename() {
    let (base, config) = prepare_fixture();
    let root = &config.library_root;
    fs::create_dir_all(root.join("shoot")).unwrap();
    fs::write(root.join("shoot/IMG_4521.ARW"), b"raw-bytes").unwrap();
    jpeg_fixture(&root.join("shoot/IMG_4521.JPG"), 8, 4, [64, 32, 192]);
    jpeg_fixture(&root.join("shoot/IMG_4522.JPG"), 8, 4, [32, 64, 192]);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());

    let opened = post_json(
        &router,
        "/api/browse",
        serde_json::json!({"source":"library"}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(opened.status(), StatusCode::OK);
    let opened: serde_json::Value = response_json(opened).await;
    let token = opened["token"].as_str().unwrap();
    let window = send(
        &router,
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/browse/{}?start=0&limit=60",
                token
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(window.status(), StatusCode::OK);
    let window: serde_json::Value = response_json(window).await;
    let photos = window["photos"].as_array().unwrap();
    assert_eq!(photos.len(), 3);

    // Each Photo carries its own Original's filename. Both are basenames, so
    // the relative Location never crosses the boundary.
    let by_name = |name: &str| {
        photos
            .iter()
            .find(|photo| photo["originalFilename"] == name)
            .unwrap_or_else(|| panic!("window is missing {name}"))
    };
    assert_eq!(by_name("IMG_4521.ARW")["original"]["kind"], "raw");
    assert_eq!(by_name("IMG_4522.JPG")["original"]["kind"], "jpeg");
    assert!(!window.to_string().contains("shoot/"));

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
    assert!(thumbnail_url.starts_with("/api/private/derivatives/"));
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
        ("ready", Some("jpeg-original"), Some(90), Some(45))
    );

    let thumbnail = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{photo_id}/thumbnail"
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
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let reopened = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{photo_id}/thumbnail"
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
    assert_eq!(review["url"].as_str().unwrap(), review_url);
    assert_eq!(facts(&application, &photo_id).await, established);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

fn authorized_router(application: Arc<Application>, web_root: impl Into<PathBuf>) -> Router {
    application.access.seed_test_token();
    create_router(application, web_root)
}

fn authenticated_request() -> ::http::request::Builder {
    Request::builder().header(
        "Authorization",
        format!("Bearer {}", crate::access::TEST_TOKEN),
    )
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
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

// ---------------------------------------------------------------- Edit Recipe

/// A generated TIFF-headered RAW fixture carrying the approved SONY ILCE-7RM5
/// camera identity, so the source-profile classifier admits the class. The
/// bounded TIFF metadata parser reads it without LibRaw.
fn approved_raw_fixture_bytes() -> Vec<u8> {
    let make = b"SONY\0";
    let model = b"ILCE-7RM5\0";
    let capture_time = b"2026:01:01 09:00:00\0";
    let entries = 3_u32;
    let ifd_offset = 8_u32;
    let ifd_size = 2 + entries * 12 + 4;
    let make_offset = ifd_offset + ifd_size;
    let model_offset = make_offset + make.len() as u32;
    let time_offset = model_offset + model.len() as u32;
    let entry = |tag: u16, value_offset: u32, count: u32| {
        let mut bytes = Vec::with_capacity(12);
        bytes.extend_from_slice(&tag.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(&value_offset.to_le_bytes());
        bytes
    };
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"II*\0");
    bytes.extend_from_slice(&ifd_offset.to_le_bytes());
    bytes.extend_from_slice(&(entries as u16).to_le_bytes());
    bytes.extend_from_slice(&entry(0x010f, make_offset, make.len() as u32));
    bytes.extend_from_slice(&entry(0x0110, model_offset, model.len() as u32));
    bytes.extend_from_slice(&entry(0x9003, time_offset, capture_time.len() as u32));
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(make);
    bytes.extend_from_slice(model);
    bytes.extend_from_slice(capture_time);
    bytes
}

fn approved_raw_fixture(path: &Path) -> Vec<u8> {
    let bytes = approved_raw_fixture_bytes();
    fs::write(path, &bytes).unwrap();
    bytes
}

fn unapproved_raw_fixture(path: &Path) {
    let mut bytes = approved_raw_fixture_bytes();
    let model_offset = 8 + 2 + 3 * 12 + 4 + 5;
    let replacement = b"ACME-1\0\0\0\0";
    bytes[model_offset..model_offset + replacement.len()].copy_from_slice(replacement);
    fs::write(path, &bytes).unwrap();
}

fn configured_router(application: &Arc<Application>, web_root: impl Into<PathBuf>) -> Router {
    application.access.seed_test_token();
    crate::http::create_router_with_processing(
        Arc::clone(application),
        crate::http::open_web_root(web_root.into()),
        Some(ProcessingConfig {
            instance: "f".repeat(32),
            policy_sha256: "b".repeat(64),
            bundle_sha256: "c".repeat(64),
        }),
    )
}

fn edit_recipe_uri(photo_id: &str) -> String {
    format!("https://camera.local/api/photos/{photo_id}/edit-recipe")
}

async fn get_edit_recipe(router: &Router, photo_id: &str) -> (StatusCode, serde_json::Value) {
    let response = get_cli_json(router, &format!("/api/photos/{photo_id}/edit-recipe")).await;
    let status = response.status();
    (status, response_json(response).await)
}

async fn save_recipe(
    router: &Router,
    photo_id: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response =
        post_cli_json(router, &format!("/api/photos/{photo_id}/edit-recipe"), body).await;
    let status = response.status();
    (status, response_json(response).await)
}

async fn rebind_recipe(
    router: &Router,
    photo_id: &str,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let response = post_cli_json(
        router,
        &format!("/api/photos/{photo_id}/edit-recipe/rebind"),
        body,
    )
    .await;
    let status = response.status();
    (status, response_json(response).await)
}

fn save_body(
    request_id: &str,
    expected_recipe_revision: Option<&str>,
    expected_source_revision: &str,
    exposure_ev: f64,
) -> serde_json::Value {
    serde_json::json!({
        "requestId": request_id,
        "expectedRecipeRevision": expected_recipe_revision,
        "expectedSourceRevision": expected_source_revision,
        "settings": {"exposureEv": exposure_ev, "whiteBalance": {"mode": "as-shot"}}
    })
}

fn error_code(body: &serde_json::Value) -> &str {
    body["error"]["code"].as_str().unwrap_or("")
}

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
    assert_eq!(read["processingAvailable"], true);
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
            "expectedRecipeRevision": "00000000-0000-4000-8000-000000000000",
            "expectedSourceRevision": source_revision,
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
        serde_json::json!({
            "expectedRecipeRevision": second_revision,
            "expectedSourceRevision": new_source,
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(rebound["outcome"], "saved");
    let rebound_revision = rebound["recipeVersion"].as_str().unwrap().to_owned();
    assert_ne!(rebound_revision, second_revision);
    assert_eq!(rebound["sourceRevision"], new_source);
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
                "expectedRecipeRevision": "00000000-0000-4000-8000-000000000000",
                "expectedSourceRevision": "00000000-0000-4000-8000-000000000000",
            }),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(error_code(&rebound), "unsupported_photo");
    }

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A RAW Photo whose camera identity cannot be observed reads as
/// unavailable with the closed unreadable-Original reason, reports a null
/// source revision, and refuses every guarded write with the closed
/// resource refusal.
#[tokio::test]
async fn edit_recipe_reports_unobservable_camera_identity_as_unavailable() {
    let (base, config) = prepare_fixture();
    generated_non_tiff_raw_fixture(&config.library_root.join("opaque.ARW"));
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = configured_router(&application, config.web_root());
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .into_iter()
        .next()
        .unwrap();

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
            "expectedRecipeRevision": "00000000-0000-4000-8000-000000000000",
            "expectedSourceRevision": "00000000-0000-4000-8000-000000000000",
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

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// An Original missing from its remembered Location reports unavailable with
/// the closed missing-Original reason and the same null source revision.
#[tokio::test]
async fn edit_recipe_reports_missing_original_as_unavailable() {
    let (base, config) = prepare_fixture();
    let raw_path = config.library_root.join("vanishing.ARW");
    approved_raw_fixture(&raw_path);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = configured_router(&application, config.web_root());
    let by_location = photo_ids_by_location(
        &application,
        &browse_photo_ids(&application, BrowseSourceRequest::Library).await,
    )
    .await;
    let photo_id = by_location["vanishing.ARW"].clone();

    let (_, before) = get_edit_recipe(&router, &photo_id).await;
    assert_eq!(before["sourceSupport"], "supported");

    fs::remove_file(&raw_path).unwrap();
    let scanned = post_json(&router, "/api/scan", serde_json::json!({}), None).await;
    assert_eq!(scanned.status(), StatusCode::OK);
    wait_for_scan_settled(&application).await;

    let (_, read) = get_edit_recipe(&router, &photo_id).await;
    assert_eq!(read["sourceSupport"], "unavailable");
    assert_eq!(read["supportReason"], "original-missing");
    assert!(read["sourceRevision"].is_null());
    assert_eq!(read["processingAvailable"], false);

    let (status, refused) = save_recipe(
        &router,
        &photo_id,
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
        serde_json::json!({"expectedRecipeRevision": "", "expectedSourceRevision": source_revision}),
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(error_code(&refused), "invalid_settings");
    let (status, refused) = rebind_recipe(
        &router,
        &photo_id,
        serde_json::json!({
            "expectedRecipeRevision": "00000000-0000-4000-8000-000000000000",
            "expectedSourceRevision": source_revision,
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
