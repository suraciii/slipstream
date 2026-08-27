//! Production HTTP boundary for the Slipstream Library core.
//!
//! This crate owns configuration, startup/shutdown, protocol mapping, and Web
//! delivery. It deliberately keeps filesystem indexing, persistence, and
//! Preview processing in `slipstream-core`.

use axum::{
    Router,
    body::Body,
    extract::State,
    http::{Request, Response, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Json},
    routing::get,
};
use serde::Serialize;
use slipstream_core::{
    CacheDirectory, Library, LibraryConfig, LibraryError, PhotoSetRecord, PreviewCandidate,
    PreviewService, PreviewState, ScanLimits, SelectionState,
};
use std::{
    env,
    ffi::CString,
    fmt, fs, io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::fs::OpenOptionsExt,
    },
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
};
use tokio::{
    net::TcpListener,
    sync::{OnceCell, oneshot},
    task::JoinHandle,
};

pub const HEALTH_PATH: &str = "/healthz";

const MAXIMUM_HEADER_BYTES: usize = 16 * 1024;
const DEFAULT_DATABASE_BASENAME: &str = "library.sqlite";
const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 3000;

/// Typed values accepted by the existing `SLIPSTREAM_*` startup contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub library_root: PathBuf,
    pub state_directory: PathBuf,
    pub cache_directory: PathBuf,
    pub database_basename: String,
    pub host: String,
    pub port: u16,
    /// Tests and packaged deployments may provide a built Web directory. When
    /// absent, the binary uses the repository's conventional `apps/web/dist`.
    pub web_root: Option<PathBuf>,
}

pub type StartupConfig = Config;
pub type ServerConfig = Config;

impl Config {
    pub fn from_env(
        environment: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, ConfigError> {
        let values = environment
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>();
        Self::from_lookup(|name| values.get(name).cloned())
    }

    pub fn from_process_environment() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| env::var(name).ok())
    }

    fn from_lookup(mut get: impl FnMut(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let library_root =
            absolute_path(get("SLIPSTREAM_LIBRARY_ROOT"), "SLIPSTREAM_LIBRARY_ROOT")?;
        let state_directory = absolute_path(
            get("SLIPSTREAM_STATE_DIRECTORY"),
            "SLIPSTREAM_STATE_DIRECTORY",
        )?;
        let cache_directory = absolute_path(
            get("SLIPSTREAM_CACHE_DIRECTORY"),
            "SLIPSTREAM_CACHE_DIRECTORY",
        )?;
        let database_basename = get("SLIPSTREAM_DATABASE_BASENAME")
            .unwrap_or_else(|| DEFAULT_DATABASE_BASENAME.to_owned());
        if !is_valid_database_basename(&database_basename) {
            return Err(ConfigError::Invalid("SLIPSTREAM_DATABASE_BASENAME"));
        }
        let host = get("SLIPSTREAM_HOST").unwrap_or_else(|| DEFAULT_HOST.to_owned());
        if host.is_empty()
            || host.len() > 255
            || host.chars().any(char::is_whitespace)
            || host.contains('/')
        {
            return Err(ConfigError::Invalid("SLIPSTREAM_HOST"));
        }
        let port = match get("SLIPSTREAM_PORT") {
            None => DEFAULT_PORT,
            Some(value) => value
                .parse::<u16>()
                .ok()
                .filter(|port| *port != 0)
                .ok_or(ConfigError::Invalid("SLIPSTREAM_PORT"))?,
        };
        let web_root = get("SLIPSTREAM_WEB_ROOT").map(PathBuf::from);
        if let Some(path) = &web_root
            && !path.is_absolute()
        {
            return Err(ConfigError::Invalid("SLIPSTREAM_WEB_ROOT"));
        }
        Ok(Self {
            library_root,
            state_directory,
            cache_directory,
            database_basename,
            host,
            port,
            web_root,
        })
    }

    pub fn web_root(&self) -> PathBuf {
        self.web_root
            .clone()
            .unwrap_or_else(|| PathBuf::from("apps/web/dist"))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigError {
    Missing(&'static str),
    NotAbsolute(&'static str),
    Invalid(&'static str),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(name) => write!(formatter, "{name} must be set"),
            Self::NotAbsolute(name) => write!(formatter, "{name} must be an absolute path"),
            Self::Invalid(name) => write!(formatter, "{name} is invalid"),
        }
    }
}

impl std::error::Error for ConfigError {}

fn absolute_path(value: Option<String>, name: &'static str) -> Result<PathBuf, ConfigError> {
    let value = value.ok_or(ConfigError::Missing(name))?;
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(ConfigError::NotAbsolute(name));
    }
    Ok(path)
}

fn is_valid_database_basename(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphanumeric()
                || byte == b'.'
                || byte == b'_'
                || byte == b'-' && index > 0
        })
        && value.as_bytes()[0].is_ascii_alphanumeric()
}

#[derive(Debug)]
pub enum ServerError {
    Config(ConfigError),
    Library(LibraryError),
    Preview(String),
    Io(io::Error),
    Cache(String),
    Join(String),
}

impl fmt::Display for ServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => error.fmt(formatter),
            Self::Library(error) => error.fmt(formatter),
            Self::Preview(error) => formatter.write_str(error),
            Self::Io(error) => error.fmt(formatter),
            Self::Cache(error) => formatter.write_str(error),
            Self::Join(error) => formatter.write_str(error),
        }
    }
}

impl std::error::Error for ServerError {}
impl From<ConfigError> for ServerError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}
impl From<LibraryError> for ServerError {
    fn from(error: LibraryError) -> Self {
        Self::Library(error)
    }
}
impl From<io::Error> for ServerError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<slipstream_core::CacheError> for ServerError {
    fn from(error: slipstream_core::CacheError) -> Self {
        Self::Cache(error.to_string())
    }
}

pub struct Application {
    library: Arc<Library>,
    preview: PreviewService,
    snapshot: RwLock<slipstream_core::ScanSnapshot>,
    shutdown: Mutex<bool>,
}

impl Application {
    pub async fn open(config: &Config) -> Result<Arc<Self>, ServerError> {
        let cache = CacheDirectory::open(&config.cache_directory, &config.library_root)?;
        let library_config = LibraryConfig {
            library_root: config.library_root.clone(),
            state_directory: config.state_directory.clone(),
            database_basename: config.database_basename.clone(),
            limits: ScanLimits::default(),
            ..LibraryConfig::default()
        };
        let library = tokio::task::spawn_blocking(move || Library::open(library_config))
            .await
            .map_err(|error| ServerError::Join(error.to_string()))??;
        let library = Arc::new(library);
        let snapshot = match library.scan().await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                let library_for_close = Arc::clone(&library);
                let _ = tokio::task::spawn_blocking(move || library_for_close.shutdown()).await;
                return Err(error.into());
            }
        };
        let library_for_preview = Arc::clone(&library);
        let preview = match tokio::task::spawn_blocking(move || {
            PreviewService::from_cache(library_for_preview, cache)
        })
        .await
        .map_err(|error| ServerError::Join(error.to_string()))?
        {
            Ok(preview) => preview,
            Err(error) => {
                let library_for_close = Arc::clone(&library);
                let _ = tokio::task::spawn_blocking(move || library_for_close.shutdown()).await;
                return Err(ServerError::Preview(error.to_string()));
            }
        };
        Ok(Arc::new(Self {
            library,
            preview,
            snapshot: RwLock::new(snapshot),
            shutdown: Mutex::new(false),
        }))
    }

    pub fn photos(&self) -> PhotoListResponse {
        let snapshot = self.snapshot.read().expect("application snapshot poisoned");
        PhotoListResponse {
            photos: snapshot
                .photos
                .iter()
                .map(|photo| photo_summary(photo, &snapshot.originals))
                .collect(),
        }
    }

    pub async fn photo_sets(&self) -> Result<PhotoSetResponse, ServerError> {
        Ok(PhotoSetResponse {
            photo_sets: self
                .library
                .list_photo_sets()
                .await?
                .into_iter()
                .map(photo_set_wire)
                .collect(),
        })
    }

    fn shutdown_blocking(&self) -> Result<(), ServerError> {
        let mut closed = self.shutdown.lock().expect("application shutdown poisoned");
        if *closed {
            return Ok(());
        }
        *closed = true;
        drop(closed);
        let preview_result = self
            .preview
            .shutdown()
            .map_err(|error| ServerError::Preview(error.to_string()));
        let library_result = self.library.shutdown().map_err(ServerError::Library);
        preview_result.and(library_result)
    }

    pub async fn shutdown(self: &Arc<Self>) -> Result<(), ServerError> {
        let application = Arc::clone(self);
        tokio::task::spawn_blocking(move || application.shutdown_blocking())
            .await
            .map_err(|error| ServerError::Join(error.to_string()))?
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoListResponse {
    pub photos: Vec<PhotoSummary>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoSetResponse {
    pub photo_sets: Vec<PhotoSetWire>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoSetWire {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_reviewed_photo_id: Option<String>,
    pub members: Vec<PhotoSetMemberWire>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoSetMemberWire {
    pub photo_id: String,
    pub position: u32,
    pub available: bool,
    pub selection_state: &'static str,
    pub rating: u8,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoSummary {
    pub id: String,
    pub available: bool,
    pub ambiguous: bool,
    pub originals: Vec<OriginalWire>,
    pub selection_state: &'static str,
    pub rating: u8,
    pub preview: PreviewWire,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OriginalWire {
    pub kind: &'static str,
    pub available: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewWire {
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limited_detail: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<&'static str>,
}

fn photo_summary(
    photo: &slipstream_core::PhotoRecord,
    all_originals: &[slipstream_core::OriginalRecord],
) -> PhotoSummary {
    let original = |id: &Option<String>, kind: &'static str| {
        id.as_ref().and_then(|id| {
            all_originals
                .iter()
                .find(|original| &original.id == id)
                .map(|original| OriginalWire {
                    kind,
                    available: original.available,
                })
        })
    };
    let originals = [
        original(&photo.raw_original_id, "raw"),
        original(&photo.jpeg_original_id, "jpeg"),
    ]
    .into_iter()
    .flatten()
    .collect();
    let source = photo.preview_source.map(preview_candidate);
    let state = preview_state(photo.preview_state);
    PhotoSummary {
        id: photo.id.clone(),
        available: photo.available,
        ambiguous: photo.ambiguous,
        originals,
        selection_state: selection_state(photo.selection_state),
        rating: photo.rating,
        preview: PreviewWire {
            state,
            source,
            width: photo.preview_width,
            height: photo.preview_height,
            limited_detail: photo
                .preview_width
                .zip(photo.preview_height)
                .map(|(width, height)| width.max(height) < 2560),
            message: (!photo.available).then_some("Original File is unavailable"),
        },
    }
}

fn photo_set_wire(record: PhotoSetRecord) -> PhotoSetWire {
    PhotoSetWire {
        id: record.id,
        name: record.name,
        last_reviewed_photo_id: record.last_reviewed_photo_id,
        members: record
            .members
            .into_iter()
            .map(|member| PhotoSetMemberWire {
                photo_id: member.photo_id,
                position: member.position,
                available: member.available,
                selection_state: selection_state(member.selection_state),
                rating: member.rating,
            })
            .collect(),
    }
}

fn selection_state(state: SelectionState) -> &'static str {
    match state {
        SelectionState::Undecided => "undecided",
        SelectionState::Selected => "selected",
        SelectionState::Rejected => "rejected",
    }
}

fn preview_state(state: PreviewState) -> &'static str {
    match state {
        PreviewState::InspectionPending => "inspection-pending",
        PreviewState::Ready => "ready",
        PreviewState::Failed => "failed",
        PreviewState::Unavailable => "unavailable",
    }
}

fn preview_candidate(candidate: PreviewCandidate) -> &'static str {
    match candidate {
        PreviewCandidate::MatchingJpeg => "matching-jpeg",
        PreviewCandidate::EmbeddedRawJpeg => "embedded-raw-jpeg",
    }
}

#[derive(Clone)]
struct WebRoot {
    path: PathBuf,
    descriptor: Option<Arc<OwnedFd>>,
}

#[derive(Clone)]
struct HttpState {
    application: Arc<Application>,
    web_root: Arc<WebRoot>,
}

struct CloseState {
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
    server: Mutex<Option<JoinHandle<Result<(), String>>>>,
    application: Arc<Application>,
    completed: OnceCell<Result<(), String>>,
}

pub struct RunningServer {
    pub url: String,
    close_state: Arc<CloseState>,
}

impl RunningServer {
    pub async fn close(&self) -> Result<(), ServerError> {
        let result = self
            .close_state
            .completed
            .get_or_init(|| async {
                if let Some(sender) = self.close_state.shutdown.lock().unwrap().take() {
                    let _ = sender.send(());
                }
                let server = self.close_state.server.lock().unwrap().take();
                let server_result = if let Some(server) = server {
                    match server.await {
                        Ok(result) => result,
                        Err(error) => Err(error.to_string()),
                    }
                } else {
                    Ok(())
                };
                let application_result = self
                    .close_state
                    .application
                    .shutdown()
                    .await
                    .map_err(|error| error.to_string());
                match (server_result, application_result) {
                    (Err(server_error), _) => Err(server_error),
                    (Ok(()), Err(application_error)) => Err(application_error),
                    (Ok(()), Ok(())) => Ok(()),
                }
            })
            .await;
        result.clone().map_err(ServerError::Join)
    }
}

pub async fn start_server(config: Config) -> Result<RunningServer, ServerError> {
    let application = Application::open(&config).await?;
    let web_root = config.web_root();
    let listener = match TcpListener::bind((config.host.as_str(), config.port)).await {
        Ok(listener) => listener,
        Err(error) => {
            let _ = application.shutdown().await;
            return Err(error.into());
        }
    };
    let address = listener.local_addr()?;
    let router = create_router(Arc::clone(&application), web_root);
    let (sender, receiver) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = receiver.await;
            })
            .await
            .map_err(|error| error.to_string())
    });
    Ok(RunningServer {
        url: format!("http://{}:{}", config.host, address.port()),
        close_state: Arc::new(CloseState {
            shutdown: Mutex::new(Some(sender)),
            server: Mutex::new(Some(server)),
            application,
            completed: OnceCell::new(),
        }),
    })
}

pub fn create_router(application: Arc<Application>, web_root: impl Into<PathBuf>) -> Router {
    Router::new()
        .route(HEALTH_PATH, get(healthz))
        .route("/api/photos", get(list_photos))
        .route("/api/photo-sets", get(list_photo_sets))
        .fallback(static_web)
        .layer(middleware::from_fn(request_policy))
        .with_state(HttpState {
            application,
            web_root: Arc::new(open_web_root(web_root.into())),
        })
}

fn open_web_root(path: PathBuf) -> WebRoot {
    WebRoot {
        descriptor: open_directory_descriptor(&path).ok().map(Arc::new),
        path,
    }
}

#[derive(Clone, Copy, Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
}

async fn healthz() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn list_photos(State(state): State<HttpState>) -> Json<PhotoListResponse> {
    Json(state.application.photos())
}

async fn list_photo_sets(
    State(state): State<HttpState>,
) -> Result<Json<PhotoSetResponse>, ApiError> {
    let response = state.application.photo_sets().await?;
    Ok(Json(response))
}

async fn request_policy(request: Request<Body>, next: Next) -> Response<Body> {
    let header_bytes = request
        .headers()
        .iter()
        .map(|(name, value)| name.as_str().len() + value.as_bytes().len())
        .sum::<usize>();
    if header_bytes > MAXIMUM_HEADER_BYTES {
        return api_error(
            StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE,
            "Request headers are too large",
        );
    }
    if !matches!(request.method().as_str(), "GET" | "HEAD" | "POST") {
        return api_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed");
    }
    if request.method() == http::Method::POST {
        let path = request.uri().path();
        if !(path == "/api" || path.starts_with("/api/")) {
            return api_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed");
        }
    }
    next.run(request).await
}

struct ApiError;

impl From<ServerError> for ApiError {
    fn from(_error: ServerError) -> Self {
        Self
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response<Body> {
        api_error(StatusCode::INTERNAL_SERVER_ERROR, "Request failed")
    }
}

fn api_error(status: StatusCode, message: &'static str) -> Response<Body> {
    let body = serde_json::to_vec(&serde_json::json!({ "error": message }))
        .expect("static JSON serializes");
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONTENT_LENGTH, body.len())
        .body(Body::from(body))
        .expect("valid API response")
}

async fn static_web(State(state): State<HttpState>, request: Request<Body>) -> Response<Body> {
    let path = request.uri().path();
    if path == "/api" || path.starts_with("/api/") {
        return api_error(StatusCode::NOT_FOUND, "Not found");
    }
    let requested = path.strip_prefix('/').unwrap_or(path);
    let requested = if requested.is_empty() {
        "index.html"
    } else {
        requested
    };
    let root = state.web_root;
    let requested_file = safe_web_path(&root.path, requested);
    let actual = match requested_file {
        Some(path) => path,
        None => root.path.join("__invalid__"),
    };
    let (bytes, is_index) = match read_web_file(&root, &actual).await {
        Ok(value) => value,
        Err(_) => match read_web_file(&root, &root.path.join("index.html")).await {
            Ok(value) => value,
            Err(_) => {
                return plain_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Web application is not built",
                );
            }
        },
    };
    let cache_control = if is_index {
        "no-cache"
    } else {
        "public, max-age=31536000, immutable"
    };
    let content_length = bytes.len().to_string();
    let body = if request.method().as_str() == "HEAD" {
        Body::empty()
    } else {
        Body::from(bytes)
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type(&actual))
        .header(header::CONTENT_LENGTH, content_length)
        .header(header::CACHE_CONTROL, cache_control)
        .header("x-content-type-options", "nosniff")
        .body(body)
        .expect("valid static response")
}

fn safe_web_path(root: &Path, requested: &str) -> Option<PathBuf> {
    if requested.contains('\0') || requested.contains('\\') {
        return None;
    }
    let mut path = root.to_path_buf();
    for component in requested.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return None;
        }
        path.push(component);
    }
    Some(path)
}

async fn read_web_file(root: &WebRoot, candidate: &Path) -> io::Result<(Vec<u8>, bool)> {
    let Some(descriptor) = &root.descriptor else {
        return Err(io::Error::from(io::ErrorKind::NotFound));
    };
    let relative = candidate
        .strip_prefix(&root.path)
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let relative = CString::new(relative.as_os_str().as_encoded_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let file = open_confined_file(descriptor.as_raw_fd(), &relative)?;
    let mut bytes = Vec::new();
    let mut file = fs::File::from(file);
    io::Read::read_to_end(&mut file, &mut bytes)?;
    let is_index = relative.as_bytes() == b"index.html";
    Ok((bytes, is_index))
}

fn open_directory_descriptor(path: &Path) -> io::Result<OwnedFd> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let descriptor: OwnedFd = file.into();
    let facts = fstat(descriptor.as_raw_fd())?;
    if facts.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return Err(io::Error::from(io::ErrorKind::NotADirectory));
    }
    Ok(descriptor)
}

fn open_confined_file(root: i32, relative: &CString) -> io::Result<OwnedFd> {
    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }
    let how = OpenHow {
        flags: (libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW) as u64,
        mode: 0,
        resolve: 0x08 | 0x04 | 0x02,
    };
    // SAFETY: `relative` is NUL-terminated and `how` matches Linux open_how.
    let descriptor = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            root,
            relative.as_ptr(),
            &how,
            std::mem::size_of::<OpenHow>(),
        ) as libc::c_int
    };
    if descriptor < 0 {
        Err(io::Error::last_os_error())
    } else {
        // SAFETY: the descriptor is newly opened and uniquely owned.
        Ok(unsafe { OwnedFd::from_raw_fd(descriptor) })
    }
}

fn fstat(fd: i32) -> io::Result<libc::stat> {
    let mut facts = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `facts` is valid writable storage and initialized on success.
    if unsafe { libc::fstat(fd, facts.as_mut_ptr()) } == 0 {
        // SAFETY: successful fstat initialized `facts`.
        Ok(unsafe { facts.assume_init() })
    } else {
        Err(io::Error::last_os_error())
    }
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("ico") => "image/x-icon",
        _ => "application/octet-stream",
    }
}

fn plain_error(status: StatusCode, message: &'static str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(message))
        .expect("valid plain response")
}

impl From<PhotoSetRecord> for PhotoSetWire {
    fn from(record: PhotoSetRecord) -> Self {
        photo_set_wire(record)
    }
}

#[cfg(test)]
mod tests {
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
        let vectors: Vec<serde_json::Value> =
            serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
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

    #[tokio::test]
    async fn shared_protocol_read_vectors_preserve_static_and_api_contracts() {
        let (base, config) = prepare_fixture();
        let application = Application::open(&config).await.unwrap();
        let router = create_router(Arc::clone(&application), config.web_root());
        let vectors: Vec<serde_json::Value> = serde_json::from_slice(
            &fs::read(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../compatibility/protocol/vectors.json"),
            )
            .unwrap(),
        )
        .unwrap();
        for vector in vectors {
            if vector["request"]["method"] != "GET" {
                continue;
            }
            let request = Request::builder()
                .method(vector["request"]["method"].as_str().unwrap())
                .uri(vector["request"]["path"].as_str().unwrap())
                .body(Body::empty())
                .unwrap();
            let response = tower::ServiceExt::oneshot(router.clone(), request)
                .await
                .unwrap();
            assert_eq!(
                response.status().as_u16(),
                vector["expected"]["status"].as_u64().unwrap() as u16,
                "{}",
                vector["name"]
            );
            let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
            if let Some(expected) = vector["expected"]["body"].as_object() {
                let actual: serde_json::Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(actual.as_object().unwrap(), expected, "{}", vector["name"]);
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
    async fn photo_json_omits_optional_values_and_preserves_original_order() {
        let (base, config) = prepare_fixture();
        let jpeg = image::RgbImage::from_pixel(8, 4, image::Rgb([32, 64, 192]));
        jpeg.save_with_format(config.library_root.join("z.JPG"), image::ImageFormat::Jpeg)
            .unwrap();
        jpeg.save_with_format(config.library_root.join("a.jpg"), image::ImageFormat::Jpeg)
            .unwrap();
        let application = Application::open(&config).await.unwrap();
        let photos = serde_json::to_value(application.photos()).unwrap();
        let list = photos["photos"].as_array().unwrap();
        assert_eq!(list.len(), 2);
        assert!(list[0]["id"].as_str().unwrap() < list[1]["id"].as_str().unwrap());
        for photo in list {
            assert_eq!(photo["originals"][0]["kind"], "jpeg");
            assert_eq!(
                photo["preview"],
                serde_json::json!({"state": "inspection-pending"})
            );
            assert!(!photo.to_string().contains(":null"));
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
        let response = tower::ServiceExt::oneshot(
            router,
            Request::builder()
                .method("HEAD")
                .uri("/api/photos")
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
        let router = create_router(Arc::clone(&application), config.web_root());
        let exact = "x".repeat(MAXIMUM_HEADER_BYTES - "x-test".len());
        let response = tower::ServiceExt::oneshot(
            router.clone(),
            Request::builder()
                .uri("/api/photos")
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
                .uri("/api/photos")
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
                let Some(headers_end) =
                    response.windows(4).position(|window| window == b"\r\n\r\n")
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
}
