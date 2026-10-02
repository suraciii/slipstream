use super::*;
use sha2::{Digest, Sha256};

fn environment(values: &[(&str, &str)]) -> HashMap<String, String> {
    values
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect()
}

#[test]
fn optional_origin_defaults_to_local_http_without_changing_listener() {
    let base = environment(&[
        ("SLIPSTREAM_LIBRARY_ROOT", "/photos"),
        ("SLIPSTREAM_STATE_DIRECTORY", "/state"),
        ("SLIPSTREAM_CACHE_DIRECTORY", "/cache"),
        ("SLIPSTREAM_PORT", "8123"),
    ]);
    let config = Config::from_env(base.clone()).unwrap();
    assert_eq!(config.public_origin, "http://localhost:8123");
    assert_eq!(config.host, "127.0.0.1");
    for origin in ["http://camera.local:8123", "https://camera.local"] {
        let mut values = base.clone();
        values.insert("SLIPSTREAM_PUBLIC_ORIGIN".into(), origin.into());
        let config = Config::from_env(values).unwrap();
        assert_eq!(config.public_origin, origin);
        assert_eq!(config.host, "127.0.0.1");
    }
    for origin in [
        "",
        "ftp://camera.local",
        "http://user@camera.local",
        "http://camera.local/path",
    ] {
        let mut values = base.clone();
        values.insert("SLIPSTREAM_PUBLIC_ORIGIN".into(), origin.into());
        assert_eq!(
            Config::from_env(values),
            Err(ConfigError::Invalid("SLIPSTREAM_PUBLIC_ORIGIN"))
        );
    }
}

#[test]
fn photo_development_startup_resolves_the_local_bundle_identity() {
    let base = vec![
        ("SLIPSTREAM_LIBRARY_ROOT", "/photos"),
        ("SLIPSTREAM_STATE_DIRECTORY", "/state"),
        ("SLIPSTREAM_CACHE_DIRECTORY", "/cache"),
        ("SLIPSTREAM_PUBLIC_ORIGIN", "https://camera.local"),
    ];

    // An unknown enablement value is refused before anything resolves.
    let mut values = base.clone();
    values.push(("SLIPSTREAM_PHOTO_DEVELOPMENT", "on"));
    assert_eq!(
        Config::from_env(environment(&values)),
        Err(ConfigError::Invalid("SLIPSTREAM_PHOTO_DEVELOPMENT"))
    );

    // A bundle directory override must be absolute.
    let mut values = base.clone();
    values.push(("SLIPSTREAM_PHOTO_DEVELOPMENT", "enabled"));
    values.push(("SLIPSTREAM_PHOTO_BUNDLE_DIRECTORY", "relative/photo"));
    assert_eq!(
        Config::from_env(environment(&values)),
        Err(ConfigError::Invalid("SLIPSTREAM_PHOTO_BUNDLE_DIRECTORY"))
    );

    // An explicit opt-out of both runtimes configures no processing
    // deployment; the darktable opt-out alone still opens the extension
    // for an independently configured film runtime (auto by default,
    // truthfully reporting a missing runtime here).
    let mut values = base.clone();
    values.push(("SLIPSTREAM_PHOTO_DEVELOPMENT", "disabled"));
    values.push(("SLIPSTREAM_FILM_MODULE", "disabled"));
    assert_eq!(
        Config::from_env(environment(&values)).unwrap().processing,
        None
    );

    // The darktable opt-out preserves independently configured Film.
    let mut values = base.clone();
    values.push(("SLIPSTREAM_PHOTO_DEVELOPMENT", "disabled"));
    let processing = Config::from_env(environment(&values))
        .unwrap()
        .processing
        .expect("film auto opens the extension");
    assert_eq!(processing.failure, Some("darktable-disabled"));
    assert_eq!(
        processing.film.as_ref().unwrap().failure,
        Some("film-runtime-missing")
    );

    let defaults = Config::from_env(environment(&[
        ("SLIPSTREAM_LIBRARY_ROOT", "/photos"),
        ("SLIPSTREAM_STATE_DIRECTORY", "/state"),
        ("SLIPSTREAM_CACHE_DIRECTORY", "/cache"),
        ("SLIPSTREAM_PUBLIC_ORIGIN", "https://camera.local"),
    ]))
    .unwrap();
    assert_eq!(
        defaults.export_retained_output_bytes,
        Some(8 * 1024 * 1024 * 1024)
    );

    // An override that names no installed bundle resolves the default
    // bundle location with the bundle unavailable.
    let mut values = base.clone();
    values.push(("SLIPSTREAM_PHOTO_DEVELOPMENT", "enabled"));
    let missing = std::env::temp_dir().join(format!(
        "slipstream-missing-photo-bundle-{}",
        std::process::id()
    ));
    values.push((
        "SLIPSTREAM_PHOTO_BUNDLE_DIRECTORY",
        missing.to_str().unwrap(),
    ));
    let config = Config::from_env(environment(&values)).unwrap();
    let processing = config.processing.unwrap();
    assert_eq!(processing.bundle_root, missing);
    assert_eq!(processing.failure, Some("bundle-unavailable"));
    assert_eq!(processing.bundle_sha256, "");

    // A complete local bundle whose manifest and named asset digests match.
    let installed =
        std::env::temp_dir().join(format!("slipstream-photo-bundle-{}", std::process::id()));
    let _ = fs::remove_dir_all(&installed);
    fs::create_dir_all(installed.join("darktable/bin")).unwrap();
    fs::create_dir_all(installed.join("icc")).unwrap();
    let engine = installed.join("darktable/bin/darktable-mcp");
    fs::write(&engine, b"engine").unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&engine, fs::Permissions::from_mode(0o755)).unwrap();
    let metadata_path = installed.join("engine-metadata.json");
    fs::write(&metadata_path, br#"{"tools":[],"modules":[],"schemas":{}}"#).unwrap();
    let icc_path = installed.join("icc/LargeRGB-elle-V2-g10.icc");
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../slipstream-core/assets/prophoto-linear-g10.icc"),
        &icc_path,
    )
    .unwrap();
    let commit_path = installed.join("darktable-commit");
    fs::write(&commit_path, format!("{}\n", "a".repeat(40))).unwrap();
    let packages_path = installed.join("os-packages.txt");
    fs::write(&packages_path, b"test-package\n").unwrap();
    let digest = |path: &Path| format!("{:x}", Sha256::digest(fs::read(path).unwrap()));
    let manifest = serde_json::json!({
        "format": 1,
        "darktable_commit": "a".repeat(40),
        "engine": "/opt/darktable/bin/darktable-mcp",
        "native": {"bin/darktable-mcp": digest(&engine)},
        "files": {
            "/opt/slipstream-photo/engine-metadata.json": digest(&metadata_path),
            "/opt/slipstream-photo/icc/LargeRGB-elle-V2-g10.icc": digest(&icc_path),
            "/opt/slipstream-photo/darktable-commit": digest(&commit_path),
            "/opt/os-packages.txt": digest(&packages_path),
        },
        "metadata": digest(&metadata_path),
        "icc": slipstream_processing::local_photo::ICC_ASSET_SHA256,
    });
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    fs::write(installed.join("bundle-manifest.json"), &manifest_bytes).unwrap();
    fs::write(
        installed.join("bundle"),
        format!("{:x}\n", Sha256::digest(&manifest_bytes)),
    )
    .unwrap();
    let mut values = base;
    values.push(("SLIPSTREAM_PHOTO_DEVELOPMENT", "auto"));
    values.push((
        "SLIPSTREAM_PHOTO_BUNDLE_DIRECTORY",
        installed.to_str().unwrap(),
    ));
    let config = Config::from_env(environment(&values)).unwrap();
    let processing = config.processing.unwrap();
    assert_eq!(processing.bundle_root, installed);
    assert_eq!(processing.failure, None);
    assert_eq!(
        processing.bundle_sha256,
        format!("{:x}", Sha256::digest(&manifest_bytes))
    );
    assert_eq!(processing.policy_sha256.len(), 64);
    let _ = fs::remove_dir_all(installed);
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
        // No checked-in vector pins a photo bundle directory, so the typed
        // config resolves the default bundle location for every vector.
        assert_eq!(
            config
                .processing
                .as_ref()
                .map(|processing| processing.bundle_root.clone()),
            Some(PathBuf::from("/opt/slipstream-photo"))
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
