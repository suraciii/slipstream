use super::*;
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

mod browse;
mod cli;
mod export_routes;
mod lifecycle;
mod metadata;
mod preview;
mod processing_export;
mod processing_preview;
mod processing_recipe;
mod protocol;
mod recovery;
mod removal;
mod server;
mod startup;

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
        export_retained_output_bytes: None,
        metadata_supervisor: None,
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

fn configured_router(application: &Arc<Application>, web_root: impl Into<PathBuf>) -> Router {
    application.access.seed_test_token();
    crate::http::create_router_with_processing(
        Arc::clone(application),
        crate::http::open_web_root(web_root.into()),
        Some(unresolved_processing_config()),
    )
}

/// A processing configuration whose bundle directory never exists: no
/// engine work can run through it, so tests of non-processing surfaces
/// stay independent of a runnable engine while the deployment stays
/// configured and reports the bundle unavailable.
fn unresolved_processing_config() -> ProcessingConfig {
    ProcessingConfig {
        policy_sha256: "b".repeat(64),
        bundle_sha256: "c".repeat(64),
        bundle_root: PathBuf::from("/nonexistent-slipstream-photo-bundle"),
        film: None,
        failure: Some("bundle-unavailable"),
    }
}

// ---------------------------------------- Local Photo Development engine

/// The scripted engine program [`FakePhotoEngine`] installs. It speaks the
/// pinned MCP 2025-06-18 stdio contract the production client implements:
/// one JSON-RPC 2.0 object per line. The spawned environment is cleared, so
/// marker files inside the bundle are the only control channel; the run
/// counter the program maintains orders per-attempt scripting.
const FAKE_PHOTO_ENGINE: &str = r#"#!/usr/bin/env python3
import base64, json, os, shutil, sys, time

BUNDLE = os.path.dirname(os.path.dirname(os.path.dirname(os.path.realpath(__file__))))
OUTPUT = os.path.join(BUNDLE, "fixture-output.tif")
RUNS = os.path.join(BUNDLE, "engine-runs")
FAIL = os.path.join(BUNDLE, "script-fail")
HANG = os.path.join(BUNDLE, "script-hang")
CORRUPT = os.path.join(BUNDLE, "script-corrupt")
DIE = os.path.join(BUNDLE, "script-die")

def read_count():
    try:
        with open(RUNS) as handle:
            return int(handle.read().strip())
    except OSError:
        return 0

def respond(request_id, result):
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": request_id, "result": result}) + "\n")
    sys.stdout.flush()

def respond_error(request_id, message):
    frame = {"jsonrpc": "2.0", "id": request_id,
             "error": {"code": 1, "message": message}}
    sys.stdout.write(json.dumps(frame) + "\n")
    sys.stdout.flush()

def marker_run(path):
    try:
        with open(path) as handle:
            return int(handle.read().split("\0")[0].strip())
    except (OSError, ValueError):
        return None

run = read_count() + 1
with open(RUNS, "w") as handle:
    handle.write(str(run))

for line in sys.stdin:
    try:
        message = json.loads(line)
    except ValueError:
        continue
    if "id" not in message:
        continue
    request_id = message["id"]
    method = message.get("method")
    if method == "initialize":
        respond(request_id, {"protocolVersion": "2025-06-18", "capabilities": {},
                             "serverInfo": {"name": "fake", "version": "0"}})
    elif method == "tools/list":
        respond(request_id, {"tools": []})
    elif method == "shutdown":
        respond(request_id, "ok")
    elif method == "tools/call":
        name = message["params"]["name"]
        arguments = message["params"].get("arguments", {})
        if name == "list_modules":
            respond(request_id, {"content": [{"type": "text", "text": "[]"}],
                                 "isError": False})
        elif name == "module_schema":
            respond(request_id, {"content": [{"type": "text", "text": "{}"}],
                                 "isError": False})
        elif name == "render":
            png = (b"\x89PNG\r\n\x1a\n" + (13).to_bytes(4, "big") +
                   b"IHDR" + (1224).to_bytes(4, "big") +
                   (1224).to_bytes(4, "big") + b"\x08\x02\x00\x00\x00")
            respond(request_id, {"content": [{"type": "image", "mimeType": "image/png",
                                              "data": base64.b64encode(png).decode("ascii")}],
                                 "isError": False})
        elif name != "export_images":
            respond_error(request_id, "unsupported tool")
        else:
            hang = marker_run(HANG)
            if hang is not None and hang in (0, run):
                while os.path.exists(HANG):
                    time.sleep(0.05)
            fail = marker_run(FAIL)
            if fail is not None and fail in (0, run):
                with open(FAIL) as handle:
                    reason = handle.read().split("\0")[1]
                respond_error(request_id, reason)
                continue
            out = arguments["out_path"]
            if os.path.exists(DIE):
                with open(out, "wb") as handle:
                    handle.write(open(OUTPUT, "rb").read())
                sys.exit(1)
            if os.path.exists(CORRUPT):
                with open(out, "wb") as handle:
                    handle.write(b"not a development tiff")
            else:
                shutil.copyfile(OUTPUT, out)
            payload = {"paths": [out], "skipped": 0, "exported": 1, "ok": True}
            respond(request_id, {"content": [{"type": "text",
                                              "text": json.dumps(payload)}],
                                 "isError": False})
sys.exit(0)
"#;

/// A scripted local Photo Development bundle one test runs against: the
/// engine program, its metadata inventory, and the bundle ICC output
/// profile, all below `base`. A generated Development TIFF is the
/// successful output; marker files script per-attempt behavior.
pub(super) struct FakePhotoEngine {
    bundle_root: PathBuf,
}

impl FakePhotoEngine {
    pub(super) fn install(base: &Path) -> Self {
        let bundle_root = base.join("photo-bundle");
        let engine_directory = bundle_root.join("darktable/bin");
        fs::create_dir_all(&engine_directory).unwrap();
        let program = engine_directory.join("darktable-mcp");
        fs::write(&program, FAKE_PHOTO_ENGINE).unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(
            bundle_root.join("engine-metadata.json"),
            r#"{"tools":[],"modules":[],"schemas":{}}"#,
        )
        .unwrap();
        fs::create_dir_all(bundle_root.join("icc")).unwrap();
        fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../slipstream-core/assets/prophoto-linear-g10.icc"),
            bundle_root.join("icc/LargeRGB-elle-V2-g10.icc"),
        )
        .unwrap();
        let engine = Self { bundle_root };
        engine.set_output(&valid_development_tiff_fixture());
        engine
    }

    /// The engine bundle a previous [`install`](Self::install) placed at
    /// this base.
    pub(super) fn at(base: &Path) -> Self {
        Self {
            bundle_root: base.join("photo-bundle"),
        }
    }

    pub(super) fn processing_config(&self) -> ProcessingConfig {
        ProcessingConfig {
            policy_sha256: "b".repeat(64),
            bundle_sha256: "c".repeat(64),
            bundle_root: self.bundle_root.clone(),
            film: None,
            failure: None,
        }
    }

    /// The bytes the next successful engine run writes as its output.
    pub(super) fn set_output(&self, bytes: &[u8]) {
        fs::write(self.bundle_root.join("fixture-output.tif"), bytes).unwrap();
    }

    /// Scripts one engine run (1-based; 0 means every run) to refuse the
    /// development with an engine error naming `reason`.
    pub(super) fn fail_attempt(&self, run: u64, reason: &str) {
        fs::write(
            self.bundle_root.join("script-fail"),
            format!("{run}\0{reason}"),
        )
        .unwrap();
    }

    /// Scripts one engine run (1-based; 0 means every run) to hold the
    /// attempt open until [`release`](Self::release) removes the marker.
    pub(super) fn hang_attempt(&self, run: u64) {
        fs::write(self.bundle_root.join("script-hang"), run.to_string()).unwrap();
    }

    /// Releases a hung attempt and clears every scripted failure mode.
    pub(super) fn release(&self) {
        for marker in ["script-hang", "script-fail", "script-corrupt", "script-die"] {
            let _ = fs::remove_file(self.bundle_root.join(marker));
        }
    }

    /// How many engine processes this bundle has run: one per local
    /// development attempt.
    pub(super) fn runs(&self) -> u64 {
        fs::read_to_string(self.bundle_root.join("engine-runs"))
            .ok()
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(0)
    }

    /// Waits until the bundle has run `count` engine processes.
    pub(super) async fn wait_for_runs(&self, count: u64) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let observed = self.runs();
            if observed >= count {
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the engine must run {count} attempts, observed {observed}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

/// One valid Development TIFF the fake engine writes as its successful
/// output: 2x1 float32 RGB with the pinned embedded output profile.
fn valid_development_tiff_fixture() -> Vec<u8> {
    let path = std::env::temp_dir().join(format!(
        "development-tiff-fixture-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    crate::export_manager::development_tiff_decode::write_development_tiff(
        &path,
        &crate::export_manager::development_tiff_decode::stored_zlib(&[0_u8; 2 * 3 * 4]),
    );
    let bytes = fs::read(&path).unwrap();
    let _ = fs::remove_file(&path);
    bytes
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
