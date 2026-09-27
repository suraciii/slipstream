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

// ---------------------------------------------------------------- Edit Preview

/// The generated Development Result edge used by the preview fixtures. The
/// route derives at the qualified 1224-pixel preview geometry; a smaller
/// artifact resamples at its own size, which keeps the fixture bounded.
const PREVIEW_FIXTURE_EDGE: u32 = 64;

/// A retention seam whose record a test swaps while the route runs.
struct ScriptedRetention {
    record: Mutex<Option<crate::edit_preview::RetainedDevelopmentResult>>,
}

impl crate::edit_preview::DevelopmentResultRetention for ScriptedRetention {
    fn resolve<'a>(
        &'a self,
        _photo_id: &'a str,
        _facts: &'a crate::edit_preview::PreviewFacts,
    ) -> std::pin::Pin<
        Box<
            dyn Future<Output = Option<crate::edit_preview::RetainedDevelopmentResult>> + Send + 'a,
        >,
    > {
        Box::pin(async { self.record.lock().unwrap().clone() })
    }
}

/// A render gate whose admissions a test scripts in order.
struct ScriptedGate {
    admissions: Mutex<std::collections::VecDeque<crate::edit_preview::RenderAdmission>>,
    settlements: Mutex<Vec<crate::edit_preview::RenderSettlement>>,
    calls: AtomicUsize,
}

impl crate::edit_preview::PreviewRenderGate for ScriptedGate {
    fn admit(
        &self,
        _request: crate::edit_preview::PreviewRenderRequest<'_>,
    ) -> crate::edit_preview::RenderAdmission {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.admissions
            .lock()
            .unwrap()
            .pop_front()
            .expect("admission script exhausted")
    }

    fn settle(
        &self,
        _request: crate::edit_preview::PreviewRenderRequest<'_>,
        settlement: crate::edit_preview::RenderSettlement,
    ) {
        self.settlements.lock().unwrap().push(settlement);
    }
}

fn scripted_retention(
    record: Option<crate::edit_preview::RetainedDevelopmentResult>,
) -> Arc<ScriptedRetention> {
    Arc::new(ScriptedRetention {
        record: Mutex::new(record),
    })
}

fn scripted_gate(
    admissions: std::collections::VecDeque<crate::edit_preview::RenderAdmission>,
) -> Arc<ScriptedGate> {
    Arc::new(ScriptedGate {
        admissions: Mutex::new(admissions),
        settlements: Mutex::new(Vec::new()),
        calls: AtomicUsize::new(0),
    })
}

fn preview_router(
    application: &Arc<Application>,
    web_root: impl Into<PathBuf>,
    retention: Arc<dyn crate::edit_preview::DevelopmentResultRetention>,
    gate: Arc<dyn crate::edit_preview::PreviewRenderGate>,
) -> (Router, Arc<crate::edit_preview::EditPreviewOwner>) {
    preview_router_bounded(application, web_root, retention, gate, None)
}

fn preview_router_bounded(
    application: &Arc<Application>,
    web_root: impl Into<PathBuf>,
    retention: Arc<dyn crate::edit_preview::DevelopmentResultRetention>,
    gate: Arc<dyn crate::edit_preview::PreviewRenderGate>,
    derivation_queue_bound: Option<std::time::Duration>,
) -> (Router, Arc<crate::edit_preview::EditPreviewOwner>) {
    application.access.seed_test_token();
    let owner = Arc::new(match derivation_queue_bound {
        Some(bound) => crate::edit_preview::EditPreviewOwner::new(retention, gate)
            .with_derivation_queue_bound(bound),
        None => crate::edit_preview::EditPreviewOwner::new(retention, gate),
    });
    let router = crate::http::create_router_with_preview(
        Arc::clone(application),
        crate::http::open_web_root(web_root.into()),
        Some(ProcessingConfig {
            instance: "f".repeat(32),
            policy_sha256: "b".repeat(64),
            bundle_sha256: "c".repeat(64),
            socket_override: None,
        }),
        Arc::clone(&owner),
    );
    (router, owner)
}

/// Writes one generated float32 Development TIFF and returns its path and the
/// content evidence a retained record must carry.
fn development_result_fixture(path: &Path, value: f32) -> (PathBuf, String, u64) {
    let samples = vec![value; (PREVIEW_FIXTURE_EDGE * PREVIEW_FIXTURE_EDGE * 3) as usize];
    let bytes = slipstream_core::development_tiff_fixture(
        &samples,
        PREVIEW_FIXTURE_EDGE,
        PREVIEW_FIXTURE_EDGE,
    );
    fs::write(path, &bytes).unwrap();
    use sha2::{Digest, Sha256};
    let sha256 = crate::queries::hex_encode(Sha256::digest(&bytes).as_slice());
    let length = bytes.len() as u64;
    (path.to_path_buf(), sha256, length)
}

fn retained_result(
    path: PathBuf,
    sha256: String,
    byte_length: u64,
    recipe_revision: Option<String>,
    exposure_milli_ev: i64,
    source_revision: String,
) -> crate::edit_preview::RetainedDevelopmentResult {
    crate::edit_preview::RetainedDevelopmentResult {
        sha256,
        byte_length,
        recipe_revision,
        exposure_milli_ev,
        white_balance: "as-shot",
        source_revision,
        bundle_sha256: "c".repeat(64),
        path,
        width: 1,
        height: 1,
    }
}

async fn get_preview_response(router: &Router, path: &str) -> Response<Body> {
    send(
        router,
        authenticated_request()
            .uri(format!("http://camera.local{path}"))
            .header("Slipstream-CLI-Contract", "1")
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

fn preview_uri(photo_id: &str, stage: &str) -> String {
    format!("/api/photos/{photo_id}/edit-preview/{stage}")
}

fn header_value(response: &Response<Body>, name: &str) -> String {
    response
        .headers()
        .get(name)
        .expect("preview metadata header")
        .to_str()
        .unwrap()
        .to_owned()
}

async fn body_bytes(response: Response<Body>) -> Vec<u8> {
    axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec()
}

/// One approved Photo with a saved recipe, plus the retained Development
/// Result of that identity. The application stays open; callers shut it down.
async fn approved_photo_with_recipe_and_result(
    request_id: &str,
    exposure_ev: f64,
) -> (
    PathBuf,
    Config,
    Arc<Application>,
    String,
    String,
    crate::edit_preview::RetainedDevelopmentResult,
) {
    let (base, config) = prepare_fixture();
    approved_raw_fixture(&config.library_root.join("approved.ARW"));
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    application.access.seed_test_token();
    let bootstrap = crate::http::create_router_with_processing(
        Arc::clone(&application),
        crate::http::open_web_root(config.web_root()),
        Some(ProcessingConfig {
            instance: "f".repeat(32),
            policy_sha256: "b".repeat(64),
            bundle_sha256: "c".repeat(64),
            socket_override: None,
        }),
    );
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .into_iter()
        .next()
        .unwrap();
    let (_, read) = get_edit_recipe(&bootstrap, &photo_id).await;
    let source_revision = read["sourceRevision"].as_str().unwrap().to_owned();
    let (_, saved) = save_recipe(
        &bootstrap,
        &photo_id,
        save_body(request_id, None, &source_revision, exposure_ev),
    )
    .await;
    assert_eq!(saved["outcome"], "saved");
    let recipe_revision = saved["recipeVersion"].as_str().unwrap().to_owned();
    let (path, sha256, byte_length) =
        development_result_fixture(&base.join("development-result.tif"), 0.18);
    let record = retained_result(
        path,
        sha256,
        byte_length,
        Some(recipe_revision.clone()),
        (exposure_ev * 1000.0) as i64,
        source_revision,
    );
    (base, config, application, photo_id, recipe_revision, record)
}

/// The current rendition streams behind the closed typed metadata: response
/// headers carry every contract field and the JPEG stream follows them.
#[tokio::test]
async fn edit_preview_streams_the_current_rendition_with_the_closed_metadata() {
    use sha2::{Digest, Sha256};
    let (base, config, application, photo_id, recipe_revision, record) =
        approved_photo_with_recipe_and_result("preview-save", 0.5).await;
    let (router, _preview_owner) = preview_router(
        &application,
        config.web_root(),
        scripted_retention(Some(record.clone())),
        scripted_gate(std::collections::VecDeque::new()),
    );
    let (_, read) = get_edit_recipe(&router, &photo_id).await;
    let source_revision = read["sourceRevision"].as_str().unwrap().to_owned();

    let response = get_preview_response(&router, &preview_uri(&photo_id, "develop")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(header_value(&response, "content-type"), "image/jpeg");
    assert_eq!(header_value(&response, "cache-control"), "no-store");
    assert_eq!(
        header_value(&response, "slipstream-edit-preview-photo-id"),
        photo_id
    );
    assert_eq!(
        header_value(&response, "slipstream-edit-preview-stage"),
        "develop"
    );
    assert_eq!(
        header_value(&response, "slipstream-edit-preview-width"),
        "64"
    );
    assert_eq!(
        header_value(&response, "slipstream-edit-preview-height"),
        "64"
    );
    assert_eq!(
        header_value(&response, "slipstream-edit-preview-recipe-version"),
        recipe_revision
    );
    assert_eq!(
        header_value(&response, "slipstream-edit-preview-source-revision"),
        crate::queries::hex_encode(source_revision.as_bytes())
    );
    assert_eq!(
        header_value(&response, "slipstream-edit-preview-display-transform"),
        "display-transform-v1"
    );
    let expires_at = header_value(&response, "slipstream-edit-preview-expires-at");
    assert!(expires_at.contains('T') && expires_at.ends_with('Z'));
    let sha256 = header_value(&response, "slipstream-edit-preview-sha256");
    let content_length = header_value(&response, "content-length");
    let bytes = body_bytes(response).await;
    assert_eq!(content_length, bytes.len().to_string());
    assert_eq!(
        crate::queries::hex_encode(Sha256::digest(&bytes).as_slice()),
        sha256,
        "the stream body is exactly the rendition the sha256 header names"
    );
    assert_eq!(
        [&bytes[..2], &bytes[bytes.len() - 2..]],
        [&[0xFF, 0xD8][..], &[0xFF, 0xD9][..]],
        "the stream is a complete JPEG"
    );
    // A second request is served by the rendition owner without a new
    // admission and with the same identity facts.
    let second = get_preview_response(&router, &preview_uri(&photo_id, "develop")).await;
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(
        header_value(&second, "slipstream-edit-preview-sha256"),
        sha256
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn edit_preview_admits_the_film_stage() {
    let (base, config, application, photo_id, _, _) =
        approved_photo_with_recipe_and_result("film-preview", 0.25).await;
    let gate = scripted_gate(std::collections::VecDeque::from([
        crate::edit_preview::RenderAdmission::Queued,
    ]));
    let gate_dyn: Arc<dyn crate::edit_preview::PreviewRenderGate> = gate;
    let (router, _preview_owner) = preview_router(
        &application,
        config.web_root(),
        scripted_retention(None),
        gate_dyn,
    );
    let response = get_preview_response(&router, &preview_uri(&photo_id, "film")).await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(
        response_json(response).await,
        serde_json::json!({"state": "queued", "stage": "film"})
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// Every closed refusal of the route carries its exact status and code, and
/// admitted work reports the 202 queued and running states.
#[tokio::test]
async fn edit_preview_reports_refusals_and_admissions_with_exact_statuses() {
    let (base, config, application, photo_id, first_revision, _) =
        approved_photo_with_recipe_and_result("preview-save", 0.25).await;
    let gate = scripted_gate(std::collections::VecDeque::from([
        crate::edit_preview::RenderAdmission::Queued,
        crate::edit_preview::RenderAdmission::Indeterminate,
    ]));
    let gate_dyn: Arc<dyn crate::edit_preview::PreviewRenderGate> = gate.clone();
    let (router, _preview_owner) = preview_router(
        &application,
        config.web_root(),
        scripted_retention(None),
        gate_dyn,
    );
    let (_, read) = get_edit_recipe(&router, &photo_id).await;
    let source_revision = read["sourceRevision"].as_str().unwrap().to_owned();

    // 404 unknown_photo: an unknown Photo and a malformed Photo ID.
    for path in [
        preview_uri("00000000-0000-4000-8000-000000000000", "develop"),
        preview_uri("short", "develop"),
    ] {
        let response = get_preview_response(&router, &path).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(error_code(&response_json(response).await), "unknown_photo");
    }

    // 422 invalid_settings: a stage outside the closed set.
    for stage in ["grain"] {
        let response = get_preview_response(&router, &preview_uri(&photo_id, stage)).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = response_json(response).await;
        assert_eq!(error_code(&body), "invalid_settings");
        assert_eq!(body["error"]["details"]["argument"], "stage");
    }

    // 202 queued: the render is admitted. The second request coalesces into
    // the running state without a second admission.
    let queued = get_preview_response(&router, &preview_uri(&photo_id, "develop")).await;
    assert_eq!(queued.status(), StatusCode::ACCEPTED);
    assert_eq!(
        response_json(queued).await,
        serde_json::json!({"state": "queued", "stage": "develop"})
    );
    let running = get_preview_response(&router, &preview_uri(&photo_id, "develop")).await;
    assert_eq!(running.status(), StatusCode::ACCEPTED);
    assert_eq!(
        response_json(running).await,
        serde_json::json!({"state": "running", "stage": "develop"})
    );
    assert_eq!(
        gate.calls.load(Ordering::Relaxed),
        1,
        "equal identities coalesce onto one admission"
    );

    // A changed identity supersedes the pending intent. With no usable
    // retained result and an unknowable admission outcome, the route reports
    // 500 outcome_unknown.
    let (_, saved) = save_recipe(
        &router,
        &photo_id,
        save_body(
            "preview-save-2",
            Some(&first_revision),
            &source_revision,
            0.75,
        ),
    )
    .await;
    assert_eq!(saved["outcome"], "saved");
    let unknown = get_preview_response(&router, &preview_uri(&photo_id, "develop")).await;
    assert_eq!(unknown.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(error_code(&response_json(unknown).await), "outcome_unknown");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// The baseline comparison is its own owner: it reaches the gate as its own
/// admission, never coalesces with the current rendition of the same stage,
/// and a selector outside the closed set refuses before any admission.
#[tokio::test]
async fn edit_preview_owns_the_baseline_comparison_separately_from_the_current_rendition() {
    let (base, config, application, photo_id, _, _) =
        approved_photo_with_recipe_and_result("preview-save", 0.25).await;
    let gate = scripted_gate(std::collections::VecDeque::from([
        crate::edit_preview::RenderAdmission::Queued,
        crate::edit_preview::RenderAdmission::Queued,
    ]));
    let gate_dyn: Arc<dyn crate::edit_preview::PreviewRenderGate> = gate.clone();
    let (router, _preview_owner) = preview_router(
        &application,
        config.web_root(),
        scripted_retention(None),
        gate_dyn,
    );
    let baseline_uri = format!("{}?settings=baseline", preview_uri(&photo_id, "develop"));

    // 422 invalid_settings: a selector outside the closed set refuses before
    // any admission, so a client cannot ask for a rendition the contract does
    // not define.
    let refused = get_preview_response(
        &router,
        &format!("{}?settings=as-shot", preview_uri(&photo_id, "develop")),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = response_json(refused).await;
    assert_eq!(error_code(&body), "invalid_settings");
    assert_eq!(body["error"]["details"]["argument"], "settings");
    assert_eq!(
        gate.calls.load(Ordering::Relaxed),
        0,
        "a refused selector admits nothing"
    );

    // The current rendition and the comparison are separate owners: each one
    // is admitted, and neither coalesces into or supersedes the other.
    let current = get_preview_response(&router, &preview_uri(&photo_id, "develop")).await;
    assert_eq!(current.status(), StatusCode::ACCEPTED);
    let repeated_current = get_preview_response(&router, &preview_uri(&photo_id, "develop")).await;
    assert_eq!(
        response_json(repeated_current).await,
        serde_json::json!({"state": "running", "stage": "develop"})
    );
    let baseline = get_preview_response(&router, &baseline_uri).await;
    assert_eq!(baseline.status(), StatusCode::ACCEPTED);
    assert_eq!(
        response_json(baseline).await,
        serde_json::json!({"state": "queued", "stage": "develop"})
    );
    assert_eq!(
        gate.calls.load(Ordering::Relaxed),
        2,
        "the comparison is its own admission"
    );
    // The comparison neither superseded the current intent nor admitted
    // again: each selector still coalesces onto its own live admission.
    let current_again = get_preview_response(&router, &preview_uri(&photo_id, "develop")).await;
    assert_eq!(
        response_json(current_again).await,
        serde_json::json!({"state": "running", "stage": "develop"}),
        "the comparison left the current intent live"
    );
    let repeated = get_preview_response(&router, &baseline_uri).await;
    assert_eq!(
        response_json(repeated).await,
        serde_json::json!({"state": "running", "stage": "develop"})
    );
    assert_eq!(gate.calls.load(Ordering::Relaxed), 2);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A source class without an approved profile refuses with 422
/// `unsupported_photo`; an unobservable class refuses with 503
/// `resource_unavailable` naming the reason.
#[tokio::test]
async fn edit_preview_refuses_unsupported_and_unobservable_source_classes() {
    let (base, config) = prepare_fixture();
    unapproved_raw_fixture(&config.library_root.join("unapproved.ARW"));
    generated_non_tiff_raw_fixture(&config.library_root.join("opaque.ARW"));
    let application = Arc::new(Application::open(&config).await.unwrap());
    wait_for_scan_settled(&application).await;
    let (router, _preview_owner) = preview_router(
        &application,
        config.web_root(),
        scripted_retention(None),
        scripted_gate(std::collections::VecDeque::new()),
    );
    let mut unsupported_refused = false;
    let mut unobservable_refused = false;
    for photo_id in browse_photo_ids(&application, BrowseSourceRequest::Library).await {
        let (_, read) = get_edit_recipe(&router, &photo_id).await;
        let response = get_preview_response(&router, &preview_uri(&photo_id, "develop")).await;
        if read["sourceSupport"] == "unsupported" {
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            assert_eq!(
                error_code(&response_json(response).await),
                "unsupported_photo"
            );
            unsupported_refused = true;
        } else if read["supportReason"] == "original-unreadable" {
            assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
            let body = response_json(response).await;
            assert_eq!(error_code(&body), "resource_unavailable");
            assert_eq!(body["error"]["details"]["reason"], "original-unreadable");
            unobservable_refused = true;
        }
    }
    assert!(unsupported_refused, "the unapproved class was classified");
    assert!(
        unobservable_refused,
        "the unobservable class was classified"
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A rendition of a superseded identity is not served: after a recipe change
/// the retained result of the old identity is stale, and the changed identity
/// owns the Photo and stage.
#[tokio::test]
async fn edit_preview_does_not_serve_a_superseded_identity() {
    let (base, config, application, photo_id, first_revision, first_record) =
        approved_photo_with_recipe_and_result("preview-save", 0.5).await;
    let retention = scripted_retention(Some(first_record));
    let gate = scripted_gate(std::collections::VecDeque::from([
        crate::edit_preview::RenderAdmission::Queued,
    ]));
    let retention_dyn: Arc<dyn crate::edit_preview::DevelopmentResultRetention> = retention.clone();
    let gate_dyn: Arc<dyn crate::edit_preview::PreviewRenderGate> = gate.clone();
    let (router, _preview_owner) =
        preview_router(&application, config.web_root(), retention_dyn, gate_dyn);
    let (_, read) = get_edit_recipe(&router, &photo_id).await;
    let source_revision = read["sourceRevision"].as_str().unwrap().to_owned();

    // The current identity streams.
    let current = get_preview_response(&router, &preview_uri(&photo_id, "develop")).await;
    assert_eq!(current.status(), StatusCode::OK);
    let current_sha = header_value(&current, "slipstream-edit-preview-sha256");

    // A recipe change supersedes the identity: the stale retained result is
    // not served, and the changed identity is admitted as preview work.
    let (_, saved) = save_recipe(
        &router,
        &photo_id,
        save_body(
            "preview-save-2",
            Some(&first_revision),
            &source_revision,
            0.75,
        ),
    )
    .await;
    assert_eq!(saved["outcome"], "saved");
    let second_revision = saved["recipeVersion"].as_str().unwrap().to_owned();
    let superseded = get_preview_response(&router, &preview_uri(&photo_id, "develop")).await;
    assert_eq!(superseded.status(), StatusCode::ACCEPTED);
    assert_eq!(
        response_json(superseded).await,
        serde_json::json!({"state": "queued", "stage": "develop"})
    );

    // Completion of the changed identity republishes only while it is still
    // current: the retained result of the new identity streams.
    let (path, second_sha, second_length) =
        development_result_fixture(&base.join("development-result-2.tif"), 0.9);
    *retention.record.lock().unwrap() = Some(retained_result(
        path,
        second_sha.clone(),
        second_length,
        Some(second_revision.clone()),
        750,
        source_revision,
    ));
    let republished = get_preview_response(&router, &preview_uri(&photo_id, "develop")).await;
    assert_eq!(republished.status(), StatusCode::OK);
    assert_eq!(
        header_value(&republished, "slipstream-edit-preview-recipe-version"),
        second_revision,
        "the republished rendition carries the changed identity"
    );
    let republished_sha = header_value(&republished, "slipstream-edit-preview-sha256");
    let republished_bytes = body_bytes(republished).await;
    use sha2::{Digest, Sha256};
    assert_eq!(
        crate::queries::hex_encode(Sha256::digest(&republished_bytes).as_slice()),
        republished_sha,
    );
    assert_ne!(
        current_sha, republished_sha,
        "the superseded rendition is never the current rendition"
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// Concurrent requests with the same full identity coalesce onto one
/// derivation and both receive the current rendition.
#[tokio::test]
async fn edit_preview_coalesces_concurrent_derivations() {
    use sha2::{Digest, Sha256};
    let (base, config, application, photo_id, _, record) =
        approved_photo_with_recipe_and_result("preview-save", 0.5).await;
    let (router, preview_owner) = preview_router(
        &application,
        config.web_root(),
        scripted_retention(Some(record.clone())),
        scripted_gate(std::collections::VecDeque::new()),
    );
    let path = preview_uri(&photo_id, "develop");
    let (first, second) = tokio::join!(
        get_preview_response(&router, &path),
        get_preview_response(&router, &path),
    );
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(
        header_value(&first, "slipstream-edit-preview-sha256"),
        header_value(&second, "slipstream-edit-preview-sha256")
    );
    assert_eq!(
        preview_owner.derivations_started(),
        1,
        "the concurrent requests coalesced onto exactly one derivation"
    );
    let first_bytes = body_bytes(first).await;
    let second_bytes = body_bytes(second).await;
    assert_eq!(first_bytes, second_bytes);
    assert_eq!(
        crate::queries::hex_encode(Sha256::digest(&first_bytes).as_slice()),
        crate::queries::hex_encode(Sha256::digest(&second_bytes).as_slice())
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// Without a configured processing deployment the develop stage cannot
/// execute, and the route reports `processing_unavailable`.
#[tokio::test]
async fn edit_preview_reports_a_disabled_deployment_as_processing_unavailable() {
    let (base, config) = prepare_fixture();
    approved_raw_fixture(&config.library_root.join("approved.ARW"));
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    application.access.seed_test_token();
    let router = crate::http::create_router(Arc::clone(&application), config.web_root());
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .into_iter()
        .next()
        .unwrap();
    let response = get_preview_response(&router, &preview_uri(&photo_id, "develop")).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        error_code(&response_json(response).await),
        "processing_unavailable"
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A recipe save commits while a render is in flight: the in-flight request's
/// publication is refused against the persisted identity, the stale rendition
/// is never served, and the newer request reaches admission without waiting
/// behind the in-flight derivation.
#[tokio::test]
async fn edit_preview_races_a_recipe_save_against_an_in_flight_render() {
    let (base, config, application, photo_id, first_revision, record) =
        approved_photo_with_recipe_and_result("preview-save", 0.5).await;
    let (router, owner) = preview_router(
        &application,
        config.web_root(),
        scripted_retention(Some(record)),
        scripted_gate(std::collections::VecDeque::from([
            crate::edit_preview::RenderAdmission::Queued,
        ])),
    );
    let (_, read) = get_edit_recipe(&router, &photo_id).await;
    let source_revision = read["sourceRevision"].as_str().unwrap().to_owned();

    // Park the first render deterministically: holding the instance-wide
    // conversion permit stops it right before the native conversion, after
    // it has read the recipe identity it intends to publish.
    let parked = owner.try_derivation_permit().expect("parking permit");
    let parked_router = router.clone();
    let parked_uri = preview_uri(&photo_id, "develop");
    let in_flight =
        tokio::spawn(async move { get_preview_response(&parked_router, &parked_uri).await });
    for _ in 0..200 {
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }

    // The save commits while the render is in flight.
    let (_, saved) = save_recipe(
        &router,
        &photo_id,
        save_body(
            "preview-save-2",
            Some(&first_revision),
            &source_revision,
            0.75,
        ),
    )
    .await;
    assert_eq!(saved["outcome"], "saved");

    // A newer request reaches admission immediately: it is not queued behind
    // the in-flight derivation.
    let newer = get_preview_response(&router, &preview_uri(&photo_id, "develop")).await;
    assert_eq!(newer.status(), StatusCode::ACCEPTED);
    assert_eq!(
        response_json(newer).await,
        serde_json::json!({"state": "queued", "stage": "develop"})
    );

    // Releasing the conversion permit lets the in-flight render finish. Its
    // publication acceptance runs against persistence and refuses: the newer
    // recipe is already committed, so no stale rendition is served.
    drop(parked);
    let stale = in_flight.await.unwrap();
    assert_eq!(stale.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = response_json(stale).await;
    assert_eq!(error_code(&body), "resource_unavailable");
    assert_eq!(body["error"]["details"]["reason"], "preview-superseded");
    assert_eq!(
        owner.derivations_started(),
        0,
        "the superseded request must be cancelled before its native conversion starts"
    );

    // The stale rendition was never published: the next request goes to
    // admission for the newer identity again.
    let again = get_preview_response(&router, &preview_uri(&photo_id, "develop")).await;
    assert_eq!(again.status(), StatusCode::ACCEPTED);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// With the render gate unavailable the route refuses fail-closed, naming the
/// closed admission reason instead of any open-ended string.
#[tokio::test]
async fn edit_preview_refuses_render_admission_fail_closed() {
    let (base, config) = prepare_fixture();
    approved_raw_fixture(&config.library_root.join("approved.ARW"));
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .into_iter()
        .next()
        .unwrap();
    let (router, _owner) = preview_router(
        &application,
        config.web_root(),
        Arc::new(crate::edit_preview::UnlandedRetention),
        Arc::new(crate::edit_preview::UnlandedRenderGate),
    );
    let response = get_preview_response(&router, &preview_uri(&photo_id, "develop")).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = response_json(response).await;
    assert_eq!(error_code(&body), "processing_unavailable");
    assert_eq!(
        body["error"]["details"]["reason"],
        "preview-render-admission-unavailable"
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A same-owner request queued on the per-owner derive mutex while a newer
/// intent is admitted must not start a conversion of its own once it
/// acquires the mutex: the pending intent names a different identity.
#[tokio::test]
async fn edit_preview_per_owner_waiter_does_not_start_a_superseded_conversion() {
    let (base, config, application, photo_id, first_revision, record) =
        approved_photo_with_recipe_and_result("preview-save", 0.5).await;
    let (router, owner) = preview_router(
        &application,
        config.web_root(),
        scripted_retention(Some(record)),
        scripted_gate(std::collections::VecDeque::from([
            crate::edit_preview::RenderAdmission::Queued,
        ])),
    );
    let (_, read) = get_edit_recipe(&router, &photo_id).await;
    let source_revision = read["sourceRevision"].as_str().unwrap().to_owned();

    // The older derivation holds the per-owner derive mutex and parks on the
    // conversion slot; a same-identity waiter queues on the mutex behind it.
    let parked = owner.try_derivation_permit().expect("parking permit");
    let uri = preview_uri(&photo_id, "develop");
    let older = {
        let router = router.clone();
        let uri = uri.clone();
        tokio::spawn(async move { get_preview_response(&router, &uri).await })
    };
    for _ in 0..200 {
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    let waiter = {
        let router = router.clone();
        let uri = uri.clone();
        tokio::spawn(async move { get_preview_response(&router, &uri).await })
    };
    for _ in 0..200 {
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }

    // A newer intent is admitted while both stale requests are queued.
    let (_, saved) = save_recipe(
        &router,
        &photo_id,
        save_body(
            "preview-save-2",
            Some(&first_revision),
            &source_revision,
            0.75,
        ),
    )
    .await;
    assert_eq!(saved["outcome"], "saved");
    let admitted = get_preview_response(&router, &preview_uri(&photo_id, "develop")).await;
    assert_eq!(admitted.status(), StatusCode::ACCEPTED);

    // Both stale requests leave without any native conversion: the parked
    // derivation is cancelled by the admission, and the waiter discovers the
    // superseded intent when it acquires the mutex.
    drop(parked);
    let older_response = older.await.unwrap();
    assert_eq!(older_response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let waiter_response = waiter.await.unwrap();
    assert_eq!(waiter_response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        owner.derivations_started(),
        0,
        "neither stale request may start a conversion"
    );

    // The newer intent survived both refusals and is still coalescing.
    let again = get_preview_response(&router, &preview_uri(&photo_id, "develop")).await;
    assert_eq!(again.status(), StatusCode::ACCEPTED);
    assert_eq!(
        response_json(again).await,
        serde_json::json!({"state": "running", "stage": "develop"})
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A request queued for the conversion slot wakes on cancellation without
/// the slot ever being released.
#[tokio::test]
async fn edit_preview_stops_waiting_for_the_conversion_slot_when_superseded() {
    let (base, config, application, photo_id, first_revision, record) =
        approved_photo_with_recipe_and_result("preview-save", 0.5).await;
    let (router, owner) = preview_router(
        &application,
        config.web_root(),
        scripted_retention(Some(record)),
        scripted_gate(std::collections::VecDeque::from([
            crate::edit_preview::RenderAdmission::Queued,
        ])),
    );
    let (_, read) = get_edit_recipe(&router, &photo_id).await;
    let source_revision = read["sourceRevision"].as_str().unwrap().to_owned();

    let parked = owner.try_derivation_permit().expect("parking permit");
    let queued_router = router.clone();
    let queued_uri = preview_uri(&photo_id, "develop");
    let queued =
        tokio::spawn(async move { get_preview_response(&queued_router, &queued_uri).await });
    for _ in 0..200 {
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }

    let (_, saved) = save_recipe(
        &router,
        &photo_id,
        save_body(
            "preview-save-2",
            Some(&first_revision),
            &source_revision,
            0.75,
        ),
    )
    .await;
    assert_eq!(saved["outcome"], "saved");
    let admitted = get_preview_response(&router, &preview_uri(&photo_id, "develop")).await;
    assert_eq!(admitted.status(), StatusCode::ACCEPTED);

    // The queued request settles as cancelled while the slot stays held.
    let response = tokio::time::timeout(std::time::Duration::from_secs(5), queued)
        .await
        .expect("the queued request must wake on cancellation")
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = response_json(response).await;
    assert_eq!(error_code(&body), "resource_unavailable");
    assert_eq!(body["error"]["details"]["reason"], "preview-superseded");
    assert_eq!(owner.derivations_started(), 0);
    drop(parked);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// The conversion-slot wait is bounded: a stuck conversion cannot hold a
/// request forever, and the bounded wait falls through to admission.
#[tokio::test]
async fn edit_preview_bounds_the_conversion_queue_wait() {
    let (base, config, application, photo_id, _, record) =
        approved_photo_with_recipe_and_result("preview-save", 0.5).await;
    let (router, owner) = preview_router_bounded(
        &application,
        config.web_root(),
        scripted_retention(Some(record)),
        scripted_gate(std::collections::VecDeque::from([
            crate::edit_preview::RenderAdmission::Queued,
        ])),
        Some(std::time::Duration::from_millis(200)),
    );

    let parked = owner.try_derivation_permit().expect("parking permit");
    let response = get_preview_response(&router, &preview_uri(&photo_id, "develop")).await;
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(
        response_json(response).await,
        serde_json::json!({"state": "queued", "stage": "develop"}),
        "the bounded wait falls through to the admission path"
    );
    assert_eq!(owner.derivations_started(), 0);
    drop(parked);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
