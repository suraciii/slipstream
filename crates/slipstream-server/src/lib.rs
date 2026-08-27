//! Production HTTP boundary for the Slipstream Library core.
//!
//! This crate owns configuration, startup/shutdown, protocol mapping, and Web
//! delivery. It deliberately keeps filesystem indexing, persistence, and
//! Preview processing in `slipstream-core`.

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Request, Response, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Json},
    routing::get,
};
use serde::Serialize;
use serde_json::Value;
use slipstream_core::{
    CacheDirectory, Library, LibraryConfig, LibraryError, PhotoSetRecord, PreviewCandidate,
    PreviewService, PreviewState, ScanLimits, SelectionState,
};
use std::{
    env,
    ffi::{CString, OsString},
    fmt, fs, io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::fs::OpenOptionsExt,
    },
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
};
use tokio::{
    net::TcpListener,
    sync::{OnceCell, oneshot},
    task::JoinHandle,
};

pub const HEALTH_PATH: &str = "/healthz";

const MAXIMUM_HEADER_BYTES: usize = 16 * 1024;
const MAXIMUM_MUTATION_BODY_BYTES: usize = 64 * 1024;
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
    PreviewUnavailable,
    WebUnavailable,
    StorageLayout,
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
            Self::PreviewUnavailable => formatter.write_str("Preview service is unavailable"),
            Self::WebUnavailable => formatter.write_str("Web application is not built"),
            Self::StorageLayout => {
                formatter.write_str("Photo Library, state, and cache directories must not overlap")
            }
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

fn validate_storage_layout(config: &Config) -> Result<(), ServerError> {
    let paths = [
        config.library_root.as_path(),
        config.state_directory.as_path(),
        config.cache_directory.as_path(),
    ];
    let canonical = paths
        .iter()
        .map(|path| canonicalize_layout_path(path).map_err(|_| ServerError::StorageLayout))
        .collect::<Result<Vec<_>, _>>()?;
    for (index, left) in canonical.iter().enumerate() {
        for right in canonical.iter().skip(index + 1) {
            if left == right || left.starts_with(right) || right.starts_with(left) {
                return Err(ServerError::StorageLayout);
            }
        }
    }
    Ok(())
}

fn canonicalize_layout_path(path: &Path) -> io::Result<PathBuf> {
    if !path.is_absolute() {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    let mut components = Vec::<OsString>::new();
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::CurDir => {}
            Component::Normal(value) => components.push(value.to_owned()),
            Component::ParentDir => {
                components
                    .pop()
                    .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
            }
            Component::Prefix(_) => {
                return Err(io::Error::from(io::ErrorKind::InvalidInput));
            }
        }
    }

    let mut existing = PathBuf::from("/");
    let mut first_missing = components.len();
    for (index, component) in components.iter().enumerate() {
        if first_missing != components.len() {
            break;
        }
        let candidate = existing.join(component);
        match fs::canonicalize(&candidate) {
            Ok(canonical) => existing = canonical,
            Err(error) if error.kind() == io::ErrorKind::NotFound => first_missing = index,
            Err(error) => return Err(error),
        }
    }
    for component in components.iter().skip(first_missing) {
        existing.push(component);
    }
    Ok(existing)
}

pub struct Application {
    library: Arc<Library>,
    preview: PreviewService,
    snapshot: RwLock<slipstream_core::ScanSnapshot>,
    shutdown: Mutex<bool>,
}

impl Application {
    pub async fn open(config: &Config) -> Result<Arc<Self>, ServerError> {
        validate_storage_layout(config)?;
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

    pub async fn rescan(&self) -> Result<PhotoListResponse, ServerError> {
        let snapshot = self.library.scan().await?;
        *self
            .snapshot
            .write()
            .expect("application snapshot poisoned") = snapshot;
        Ok(self.photos())
    }

    pub async fn mutate_photo_set(
        &self,
        mutation: slipstream_core::PhotoSetMutation,
    ) -> Result<PhotoSetResponse, ServerError> {
        self.library.mutate_photo_set(mutation).await?;
        self.photo_sets().await
    }

    pub async fn mutate_photo_state(
        &self,
        mutation: slipstream_core::PhotoStateMutation,
    ) -> Result<slipstream_core::PhotoStateMutationResult, ServerError> {
        let result = self.library.mutate_photo_state(mutation).await?;
        *self
            .snapshot
            .write()
            .expect("application snapshot poisoned") = self.library.snapshot().await?;
        Ok(result)
    }

    pub async fn preview(&self, photo_id: &str) -> Result<PreviewResponse, ServerError> {
        if !valid_id(photo_id) {
            return Ok(PreviewResponse::unavailable("Unknown Photo"));
        }
        let result = self
            .preview
            .review(
                photo_id.to_owned(),
                slipstream_core::DerivativePriority::Current,
            )
            .await;
        Ok(match result {
            Ok(slipstream_core::PreviewRequestResult::Current(ready)) => {
                PreviewResponse::ready(photo_id, &ready, false)
            }
            Ok(slipstream_core::PreviewRequestResult::Stale(ready)) => {
                PreviewResponse::ready(photo_id, &ready, true)
            }
            Ok(slipstream_core::PreviewRequestResult::Unavailable(unavailable)) => {
                let message = match unavailable.reason {
                    slipstream_core::PreviewUnavailableReason::PhotoNotFound => "Unknown Photo",
                    slipstream_core::PreviewUnavailableReason::OriginalUnavailable => {
                        "Original File is unavailable"
                    }
                    slipstream_core::PreviewUnavailableReason::NoUsableSource => {
                        "No usable camera-produced Preview"
                    }
                };
                PreviewResponse::unavailable(message)
            }
            Ok(slipstream_core::PreviewRequestResult::Failed(_)) => {
                PreviewResponse::failed("Preview generation failed")
            }
            Ok(slipstream_core::PreviewRequestResult::StaleIgnored) => {
                PreviewResponse::unavailable("Original File changed; rescan required")
            }
            Err(slipstream_core::PreviewServiceError::Changed) => {
                PreviewResponse::unavailable("Original File changed; rescan required")
            }
            Err(slipstream_core::PreviewServiceError::Saturated)
            | Err(slipstream_core::PreviewServiceError::Closed) => {
                return Err(ServerError::PreviewUnavailable);
            }
            Err(_) => PreviewResponse::failed("Request failed"),
        })
    }

    pub async fn derivative(
        &self,
        photo_id: &str,
        cache_key: &str,
    ) -> Result<Option<DerivativeDelivery>, ServerError> {
        if !valid_id(photo_id) || !is_hex_key(cache_key) {
            return Ok(None);
        }
        let result = self
            .preview
            .review(
                photo_id.to_owned(),
                slipstream_core::DerivativePriority::Current,
            )
            .await;
        let ready = match result {
            Ok(slipstream_core::PreviewRequestResult::Current(ready))
            | Ok(slipstream_core::PreviewRequestResult::Stale(ready))
                if ready.cache_key == cache_key =>
            {
                ready
            }
            _ => return Ok(None),
        };
        let cache = self.preview.scheduler().cache().clone();
        let cache_key = cache_key.to_owned();
        let bytes = tokio::task::spawn_blocking(move || cache.read_derivative(&cache_key))
            .await
            .map_err(|error| ServerError::Join(error.to_string()))?
            .ok();
        Ok(bytes.map(|bytes| DerivativeDelivery {
            cache_key: ready.cache_key,
            bytes,
        }))
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

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewResponse {
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limited_detail: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<&'static str>,
}

impl PreviewResponse {
    fn ready(photo_id: &str, ready: &slipstream_core::PreviewReady, stale: bool) -> Self {
        Self {
            state: "ready",
            source: Some(preview_candidate(ready.source)),
            stale: Some(stale),
            width: Some(ready.width),
            height: Some(ready.height),
            limited_detail: Some(ready.width.max(ready.height) < 2560),
            url: Some(format!(
                "/api/derivatives/{}/{}.jpg",
                photo_id, ready.cache_key
            )),
            message: stale.then_some("Showing a stale Preview because current generation failed"),
        }
    }

    fn unavailable(message: &'static str) -> Self {
        Self {
            state: "unavailable",
            source: None,
            stale: None,
            width: None,
            height: None,
            limited_detail: None,
            url: None,
            message: Some(message),
        }
    }

    fn failed(message: &'static str) -> Self {
        Self {
            state: "failed",
            source: None,
            stale: None,
            width: None,
            height: None,
            limited_detail: None,
            url: None,
            message: Some(message),
        }
    }
}

#[derive(Clone, Debug)]
pub struct DerivativeDelivery {
    pub cache_key: String,
    pub bytes: Vec<u8>,
}

#[derive(Clone)]
struct WebRoot {
    path: PathBuf,
    descriptor: Option<Arc<OwnedFd>>,
    ready: bool,
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
    let web_root = open_web_root(config.web_root());
    if !web_root.ready {
        return Err(ServerError::WebUnavailable);
    }
    let application = Application::open(&config).await?;
    let listener = match TcpListener::bind((config.host.as_str(), config.port)).await {
        Ok(listener) => listener,
        Err(error) => {
            let _ = application.shutdown().await;
            return Err(error.into());
        }
    };
    let address = listener.local_addr()?;
    let router = create_router_with_web_root(Arc::clone(&application), web_root);
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
    create_router_with_web_root(application, open_web_root(web_root.into()))
}

fn create_router_with_web_root(application: Arc<Application>, web_root: WebRoot) -> Router {
    Router::new()
        .route(HEALTH_PATH, get(healthz))
        .route("/api/photos", get(list_photos))
        .route("/api/photos/{id}/preview", get(get_preview))
        .route(
            "/api/photo-sets",
            get(list_photo_sets).post(create_photo_set),
        )
        .route(
            "/api/photo-sets/{id}/rename",
            get(method_not_allowed).post(rename_photo_set),
        )
        .route(
            "/api/photo-sets/{id}/delete",
            get(method_not_allowed).post(delete_photo_set),
        )
        .route(
            "/api/photo-sets/{id}/members",
            get(method_not_allowed).post(add_photo_set_members),
        )
        .route(
            "/api/photo-sets/{id}/members/remove",
            get(method_not_allowed).post(remove_photo_set_member),
        )
        .route(
            "/api/photo-sets/{id}/order",
            get(method_not_allowed).post(reorder_photo_set),
        )
        .route(
            "/api/photo-sets/{id}/progress",
            get(method_not_allowed).post(set_progress),
        )
        .route(
            "/api/photos/{id}/state",
            get(method_not_allowed).post(mutate_photo_state),
        )
        .route("/api/scan", get(method_not_allowed).post(scan))
        .route(
            "/api/derivatives/{photo_id}/{filename}",
            get(get_derivative),
        )
        .fallback(static_web)
        .layer(middleware::from_fn(request_policy))
        .with_state(HttpState {
            application,
            web_root: Arc::new(web_root),
        })
}

fn open_web_root(path: PathBuf) -> WebRoot {
    let descriptor = open_directory_descriptor(&path).ok().map(Arc::new);
    let mut root = WebRoot {
        descriptor,
        path,
        ready: false,
    };
    root.ready = web_root_has_index(&root);
    root
}

fn web_root_has_index(root: &WebRoot) -> bool {
    let Some(descriptor) = &root.descriptor else {
        return false;
    };
    let index = CString::new("index.html").expect("static filename has no NUL");
    open_confined_file(descriptor.as_raw_fd(), &index)
        .and_then(|file| fstat(file.as_raw_fd()))
        .is_ok_and(|facts| facts.st_mode & libc::S_IFMT == libc::S_IFREG)
}

#[derive(Clone, Copy, Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
}

async fn healthz(State(state): State<HttpState>) -> Response<Body> {
    if !state.web_root.ready || !web_root_has_index(&state.web_root) {
        return plain_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Web application is not built",
        );
    }
    Json(HealthResponse { status: "ok" }).into_response()
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

async fn method_not_allowed() -> Response<Body> {
    api_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed")
}

async fn scan(State(state): State<HttpState>, request: Request<Body>) -> Response<Body> {
    if !mutation_allowed(&request) {
        return api_error(StatusCode::FORBIDDEN, "Cross-origin mutation rejected");
    }
    match state.application.rescan().await {
        Ok(response) => json_response(StatusCode::OK, &response),
        Err(error) => ApiError::from(error).into_response(),
    }
}

async fn create_photo_set(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
    mutate_photo_set_route(&state, request, |body| {
        valid_name(body.get("name"))
            .map(|name| slipstream_core::PhotoSetMutation::Create { name })
            .ok_or("Invalid Photo Set name")
    })
    .await
}

async fn rename_photo_set(
    State(state): State<HttpState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    mutate_photo_set_route(&state, request, move |body| {
        if !valid_id(&id) {
            return Err("Invalid Photo Set mutation");
        }
        valid_name(body.get("name"))
            .map(|name| slipstream_core::PhotoSetMutation::Rename {
                photo_set_id: id.clone(),
                name,
            })
            .ok_or("Invalid Photo Set mutation")
    })
    .await
}

async fn delete_photo_set(
    State(state): State<HttpState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    if !mutation_allowed(&request) {
        return api_error(StatusCode::FORBIDDEN, "Cross-origin mutation rejected");
    }
    if !valid_id(&id) {
        return api_error(StatusCode::BAD_REQUEST, "Invalid Photo Set");
    }
    match state
        .application
        .mutate_photo_set(slipstream_core::PhotoSetMutation::Delete { photo_set_id: id })
        .await
    {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(error) => ApiError::from(error).into_response(),
    }
}

async fn add_photo_set_members(
    State(state): State<HttpState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    mutate_photo_set_route(&state, request, move |body| {
        if !valid_id(&id) {
            return Err("Invalid membership batch");
        }
        valid_ids(body.get("photoIds"))
            .map(|photo_ids| slipstream_core::PhotoSetMutation::AddMembers {
                photo_set_id: id.clone(),
                photo_ids,
            })
            .ok_or("Invalid membership batch")
    })
    .await
}

async fn remove_photo_set_member(
    State(state): State<HttpState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    mutate_photo_set_route(&state, request, move |body| {
        let photo_id = body.get("photoId").and_then(Value::as_str);
        if !valid_id(&id) || photo_id.is_none_or(|value| !valid_id(value)) {
            return Err("Invalid Photo Set member");
        }
        Ok(slipstream_core::PhotoSetMutation::RemoveMember {
            photo_set_id: id.clone(),
            photo_id: photo_id.unwrap().to_owned(),
        })
    })
    .await
}

async fn reorder_photo_set(
    State(state): State<HttpState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    mutate_photo_set_route(&state, request, move |body| {
        if !valid_id(&id) {
            return Err("Invalid Photo Set order");
        }
        valid_ids(body.get("photoIds"))
            .map(|photo_ids| slipstream_core::PhotoSetMutation::Reorder {
                photo_set_id: id.clone(),
                photo_ids,
            })
            .ok_or("Invalid Photo Set order")
    })
    .await
}

async fn set_progress(
    State(state): State<HttpState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    mutate_photo_set_route(&state, request, move |body| {
        let photo_id = body.get("photoId").and_then(Value::as_str);
        if !valid_id(&id) || photo_id.is_none_or(|value| !valid_id(value)) {
            return Err("Invalid review progress");
        }
        Ok(slipstream_core::PhotoSetMutation::SetProgress {
            photo_set_id: id.clone(),
            photo_id: photo_id.unwrap().to_owned(),
        })
    })
    .await
}

async fn mutate_photo_state(
    State(state): State<HttpState>,
    axum::extract::Path(photo_id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    if !mutation_allowed(&request) {
        return api_error(StatusCode::FORBIDDEN, "Cross-origin mutation rejected");
    }
    if !valid_id(&photo_id) {
        return api_error(StatusCode::BAD_REQUEST, "Invalid Photo state mutation");
    }
    let body = match read_json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let field = body.get("field").and_then(Value::as_str);
    let raw_value = body.get("value");
    let value = match (field, raw_value) {
        (Some("selectionState"), Some(value)) => {
            valid_selection(value).map(slipstream_core::PhotoStateValue::Selection)
        }
        (Some("rating"), Some(value)) => {
            valid_rating(value).map(slipstream_core::PhotoStateValue::Rating)
        }
        _ => None,
    };
    let Some(value) = value else {
        return api_error(StatusCode::BAD_REQUEST, "Invalid Photo state mutation");
    };
    let expected_current = match body.get("expectedCurrent") {
        None => Ok(None),
        Some(value) => match (field, value) {
            (Some("selectionState"), value) => valid_selection(value)
                .map(|v| Some(slipstream_core::PhotoStateValue::Selection(v)))
                .ok_or(()),
            (Some("rating"), value) => valid_rating(value)
                .map(|v| Some(slipstream_core::PhotoStateValue::Rating(v)))
                .ok_or(()),
            _ => Err(()),
        },
    };
    let Ok(expected_current) = expected_current else {
        return api_error(StatusCode::BAD_REQUEST, "Invalid Photo state mutation");
    };
    let photo_set_id = match body.get("photoSetId") {
        None => Ok(None),
        Some(value) => value
            .as_str()
            .filter(|id| valid_id(id))
            .map(str::to_owned)
            .map(Some)
            .ok_or(()),
    };
    let Ok(photo_set_id) = photo_set_id else {
        return api_error(StatusCode::BAD_REQUEST, "Invalid Photo state mutation");
    };
    let field = if matches!(field, Some("selectionState")) {
        slipstream_core::PhotoStateField::SelectionState
    } else {
        slipstream_core::PhotoStateField::Rating
    };
    let result = state
        .application
        .mutate_photo_state(slipstream_core::PhotoStateMutation {
            photo_id,
            field,
            value,
            expected_current,
            photo_set_id,
        })
        .await;
    match result {
        Ok(result) => json_response(StatusCode::OK, &photo_state_wire(&result)),
        Err(error) => ApiError::from(error).into_response(),
    }
}

fn photo_state_wire(result: &slipstream_core::PhotoStateMutationResult) -> Value {
    let (field, prior_value, expected_current) = match result.undo.field {
        slipstream_core::PhotoStateField::SelectionState => (
            "selectionState",
            selection_value(result.undo.prior_value),
            selection_value(result.undo.expected_current),
        ),
        slipstream_core::PhotoStateField::Rating => (
            "rating",
            rating_value(result.undo.prior_value),
            rating_value(result.undo.expected_current),
        ),
    };
    serde_json::json!({
        "kind": "applied",
        "photoId": result.photo_id,
        "undo": {
            "photoId": result.undo.photo_id,
            "field": field,
            "priorValue": prior_value,
            "expectedCurrent": expected_current,
        },
    })
}

fn selection_value(value: slipstream_core::PhotoStateValue) -> Value {
    match value {
        slipstream_core::PhotoStateValue::Selection(value) => {
            Value::String(selection_state(value).to_owned())
        }
        slipstream_core::PhotoStateValue::Rating(_) => Value::Null,
    }
}

fn rating_value(value: slipstream_core::PhotoStateValue) -> Value {
    match value {
        slipstream_core::PhotoStateValue::Rating(value) => Value::from(value),
        slipstream_core::PhotoStateValue::Selection(_) => Value::Null,
    }
}

async fn get_preview(
    State(state): State<HttpState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response<Body> {
    match state.application.preview(&id).await {
        Ok(result) => {
            let status = if result.state == "ready" {
                StatusCode::OK
            } else {
                StatusCode::NOT_FOUND
            };
            json_response(status, &result)
        }
        Err(error) => ApiError::from(error).into_response(),
    }
}

async fn get_derivative(
    State(state): State<HttpState>,
    axum::extract::Path((photo_id, filename)): axum::extract::Path<(String, String)>,
    request: Request<Body>,
) -> Response<Body> {
    let Some(key) = filename.strip_suffix(".jpg") else {
        return api_error(StatusCode::NOT_FOUND, "Derivative not found");
    };
    if !is_hex_key(key) {
        return api_error(StatusCode::NOT_FOUND, "Derivative not found");
    }
    let delivery = match state.application.derivative(&photo_id, key).await {
        Ok(Some(delivery)) => delivery,
        Ok(None) => return api_error(StatusCode::NOT_FOUND, "Derivative not found"),
        Err(error) => return ApiError::from(error).into_response(),
    };
    let entity_tag = format!("\"{}\"", delivery.cache_key);
    if request
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        == Some(entity_tag.as_str())
    {
        return Response::builder()
            .status(StatusCode::NOT_MODIFIED)
            .header(header::ETAG, entity_tag)
            .body(Body::empty())
            .expect("valid response");
    }
    let length = delivery.bytes.len().to_string();
    let body = if request.method() == http::Method::HEAD {
        Body::empty()
    } else {
        Body::from(delivery.bytes)
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CONTENT_LENGTH, length)
        .header(header::CACHE_CONTROL, "public, max-age=31536000, immutable")
        .header(header::ETAG, entity_tag)
        .header("x-content-type-options", "nosniff")
        .body(body)
        .expect("valid derivative response")
}

async fn mutate_photo_set_route(
    state: &HttpState,
    request: Request<Body>,
    build: impl FnOnce(
        &serde_json::Map<String, Value>,
    ) -> Result<slipstream_core::PhotoSetMutation, &'static str>,
) -> Response<Body> {
    if !mutation_allowed(&request) {
        return api_error(StatusCode::FORBIDDEN, "Cross-origin mutation rejected");
    }
    let body = match read_json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(body) = body.as_object() else {
        return api_error(StatusCode::BAD_REQUEST, "Invalid JSON body");
    };
    let mutation = match build(body) {
        Ok(mutation) => mutation,
        Err(message) => return api_error(StatusCode::BAD_REQUEST, message),
    };
    match state.application.mutate_photo_set(mutation).await {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(error) => ApiError::from(error).into_response(),
    }
}

async fn read_json_body(request: Request<Body>) -> Result<Value, Response<Body>> {
    let (parts, body) = request.into_parts();
    if let Some(length) = parts.headers.get(header::CONTENT_LENGTH) {
        let Ok(length) = length
            .to_str()
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or(())
        else {
            return Err(api_error(StatusCode::BAD_REQUEST, "Invalid request body"));
        };
        if length > MAXIMUM_MUTATION_BODY_BYTES as u64 {
            return Err(api_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "Request body is too large",
            ));
        }
    }
    let bytes = to_bytes(body, MAXIMUM_MUTATION_BODY_BYTES)
        .await
        .map_err(|error| {
            let status = if error.to_string().contains("length limit") {
                StatusCode::PAYLOAD_TOO_LARGE
            } else {
                StatusCode::BAD_REQUEST
            };
            let message = if status == StatusCode::PAYLOAD_TOO_LARGE {
                "Request body is too large"
            } else {
                "Invalid request body"
            };
            api_error(status, message)
        })?;
    serde_json::from_slice(&bytes)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "Invalid JSON body"))
}

fn json_response<T: Serialize>(status: StatusCode, value: &T) -> Response<Body> {
    let body = serde_json::to_vec(value).expect("response serializes");
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONTENT_LENGTH, body.len())
        .body(Body::from(body))
        .expect("valid JSON response")
}

fn mutation_allowed(request: &Request<Body>) -> bool {
    let Some(origin) = request.headers().get(header::ORIGIN) else {
        return true;
    };
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let Ok(supplied) = origin.parse::<http::Uri>() else {
        return false;
    };
    let Some(scheme) = supplied.scheme_str() else {
        return false;
    };
    let Some(authority) = supplied.authority() else {
        return false;
    };
    if request.uri().scheme_str() == Some(scheme) && request.uri().authority() == Some(authority) {
        return true;
    }
    request
        .headers()
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
        .is_some_and(|host| request.uri().scheme_str().is_none() && host == authority.as_str())
}

fn valid_id(value: &str) -> bool {
    value.len() >= 36
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte) || byte == b'-')
}

fn is_hex_key(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_name(value: Option<&Value>) -> Option<String> {
    let value = value?.as_str()?.trim();
    (!value.is_empty() && value.chars().count() <= 120).then(|| value.to_owned())
}

fn valid_ids(value: Option<&Value>) -> Option<Vec<String>> {
    let values = value?.as_array()?;
    if values.len() > 100 {
        return None;
    }
    let ids = values
        .iter()
        .map(Value::as_str)
        .collect::<Option<Vec<_>>>()?;
    if ids.iter().any(|id| !valid_id(id)) {
        return None;
    }
    let unique = ids.iter().collect::<std::collections::BTreeSet<_>>().len();
    (unique == ids.len()).then(|| ids.into_iter().map(str::to_owned).collect())
}

fn valid_selection(value: &Value) -> Option<SelectionState> {
    match value.as_str()? {
        "undecided" => Some(SelectionState::Undecided),
        "selected" => Some(SelectionState::Selected),
        "rejected" => Some(SelectionState::Rejected),
        _ => None,
    }
}

fn valid_rating(value: &Value) -> Option<u8> {
    let rating = value.as_u64()?;
    (rating <= 5).then_some(rating as u8)
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

#[derive(Clone, Copy, Debug)]
struct ApiError {
    status: StatusCode,
    message: &'static str,
}

impl From<ServerError> for ApiError {
    fn from(error: ServerError) -> Self {
        if let ServerError::Library(LibraryError::Mutation(error)) = error {
            return match error {
                slipstream_core::MutationError::NotFound => Self {
                    status: StatusCode::NOT_FOUND,
                    message: "Mutation target not found",
                },
                slipstream_core::MutationError::Conflict => Self {
                    status: StatusCode::CONFLICT,
                    message: "Mutation conflicts with current state",
                },
                slipstream_core::MutationError::Persistence
                | slipstream_core::MutationError::Saturated
                | slipstream_core::MutationError::Closed => Self {
                    status: StatusCode::SERVICE_UNAVAILABLE,
                    message: "Mutation could not be persisted",
                },
            };
        }
        match error {
            ServerError::PreviewUnavailable => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                message: "Preview service unavailable",
            },
            _ => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Request failed",
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response<Body> {
        api_error(self.status, self.message)
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
    let served_path = if is_index {
        root.path.join("index.html")
    } else {
        actual
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type(&served_path))
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
    async fn shared_protocol_vectors_execute_all_requests_with_exact_results() {
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
        let contract: serde_json::Value = serde_json::from_str(include_str!(
            "../../../compatibility/protocol/capture-order-omission.json"
        ))
        .unwrap();
        let ordered_paths = contract["orderedPaths"].as_array().unwrap();
        let (base, config) = prepare_fixture();
        capture_metadata_fixture(&config.library_root.join("z.JPG"), "2026:01:01 09:00:00");
        capture_metadata_fixture(&config.library_root.join("a.jpg"), "2026:01:01 10:00:00");
        let application = Application::open(&config).await.unwrap();
        let photos = serde_json::to_value(application.photos()).unwrap();
        let list = photos["photos"].as_array().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(
            list.iter()
                .map(|photo| photo["id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ordered_paths
                .iter()
                .map(|path| {
                    slipstream_core::standalone_photo_id(&slipstream_core::original_id(
                        path.as_str().unwrap(),
                    ))
                })
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
    async fn photo_set_and_state_protocol_persists_across_reopen() {
        let (base, mut config) = prepare_fixture();
        jpeg_fixture(&config.library_root.join("a.jpg"), 8, 4, [192, 64, 32]);
        jpeg_fixture(&config.library_root.join("b.jpg"), 8, 4, [32, 192, 64]);
        jpeg_fixture(&config.library_root.join("c.jpg"), 8, 4, [32, 64, 192]);
        config.port = 0;
        let application = Application::open(&config).await.unwrap();
        let router = create_router(Arc::clone(&application), config.web_root());
        let ids = application
            .photos()
            .photos
            .into_iter()
            .map(|photo| photo.id)
            .collect::<Vec<_>>();
        assert_eq!(ids.len(), 3);

        let created = response_json(
            post_json(
                &router,
                "/api/photo-sets",
                serde_json::json!({"name": " Picks "}),
                Some("http://camera.local"),
            )
            .await,
        )
        .await;
        assert_eq!(created["photoSets"][0]["name"], "Picks");
        let set_a = created["photoSets"][0]["id"].as_str().unwrap().to_owned();
        assert_eq!(
            send(
                &router,
                Request::builder()
                    .method("POST")
                    .uri(format!(
                        "http://camera.local/api/photo-sets/{set_a}/members"
                    ))
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
                &format!("http://camera.local/api/photo-sets/{set_a}/order"),
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
                &format!("http://camera.local/api/photo-sets/{set_a}/progress"),
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
                "http://camera.local/api/photo-sets",
                serde_json::json!({"name": "Other"}),
                Some("http://camera.local"),
            )
            .await,
        )
        .await;
        let set_b = created_b["photoSets"]
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
                &format!("http://camera.local/api/photo-sets/{set_b}/members"),
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
                serde_json::json!({"field": "selectionState", "value": "selected", "photoSetId": set_a}),
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
        let sets = response_json(
            send(
                &router,
                Request::builder()
                    .uri("http://camera.local/api/photo-sets")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await,
        )
        .await;
        let members = sets["photoSets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|set| set["id"] == set_a)
            .unwrap()["members"]
            .as_array()
            .unwrap();
        assert_eq!(
            members
                .iter()
                .map(|member| member["photoId"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![ids[2].as_str(), ids[0].as_str(), ids[1].as_str()]
        );
        assert!(sets["photoSets"].as_array().unwrap().iter().all(|set| {
            set["members"].as_array().unwrap().iter().all(|member| {
                member["photoId"] != ids[0]
                    || (member["selectionState"] == "selected" && member["rating"] == 4)
            })
        }));
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
                &format!("http://camera.local/api/photo-sets/{set_a}/members/remove"),
                serde_json::json!({"photoId": ids[0]}),
                None,
            )
            .await
            .status(),
            StatusCode::OK
        );
        let sets = response_json(
            send(
                &router,
                Request::builder()
                    .uri("http://camera.local/api/photo-sets")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await,
        )
        .await;
        assert!(
            !sets["photoSets"]
                .as_array()
                .unwrap()
                .iter()
                .find(|set| set["id"] == set_a)
                .unwrap()
                .as_object()
                .unwrap()
                .contains_key("lastReviewedPhotoId")
        );
        let before_original = fs::read(config.library_root.join("b.jpg")).unwrap();
        assert_eq!(
            post_json(
                &router,
                &format!("http://camera.local/api/photo-sets/{set_a}/delete"),
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
        let reopened_photos = reopened.photos();
        assert_eq!(reopened_photos.photos.len(), 3);
        let persisted = reopened_photos
            .photos
            .iter()
            .find(|photo| photo.id == ids[0])
            .unwrap();
        assert_eq!(persisted.selection_state, "rejected");
        assert_eq!(persisted.rating, 4);
        assert!(
            reopened
                .photo_sets()
                .await
                .unwrap()
                .photo_sets
                .iter()
                .all(|set| set.id != set_a)
        );
        reopened.shutdown().await.unwrap();
        let _ = fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn mutation_origin_validation_precedes_validation_and_scan_has_no_body() {
        let (base, config) = prepare_fixture();
        let application = Application::open(&config).await.unwrap();
        let router = create_router(Arc::clone(&application), config.web_root());
        let foreign = response_json(
            post_json(
                &router,
                "http://camera.local/api/photo-sets/not-an-id/delete",
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
                "/api/photo-sets",
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
                    "/api/photo-sets",
                    serde_json::json!({"name": ""}),
                    None,
                )
                .await,
            )
            .await,
            serde_json::json!({"error": "Invalid Photo Set name"})
        );
        application.shutdown().await.unwrap();
        let _ = fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn mutation_body_limits_and_json_errors_are_rejected_before_writes() {
        let (base, config) = prepare_fixture();
        let application = Application::open(&config).await.unwrap();
        let router = create_router(Arc::clone(&application), config.web_root());
        let request = |body: Body, length: Option<&str>| {
            let mut builder = Request::builder()
                .method("POST")
                .uri("http://camera.local/api/photo-sets")
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
        assert_eq!(
            response_json(
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
            .await["photos"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        assert_eq!(application.photo_sets().await.unwrap().photo_sets.len(), 0);
        application.shutdown().await.unwrap();
        let _ = fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn preview_derivative_protocol_revalidates_source_and_reports_stale_truth() {
        let (base, config) = prepare_fixture();
        let original = config.library_root.join("photo.jpg");
        jpeg_fixture(&original, 90, 45, [192, 64, 32]);
        let application = Application::open(&config).await.unwrap();
        let router = create_router(Arc::clone(&application), config.web_root());
        let photo_id = application.photos().photos[0].id.clone();
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
}
