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
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use slipstream_core::{
    CacheDirectory, DerivativeTarget, Library, LibraryConfig, LibraryError, PhotoSetRecord,
    PhotoStateField, PhotoStateValue, PreviewCandidate, PreviewService, PreviewState, ScanLimits,
    ScanPhase, SelectionState,
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
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
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
    BrowseNotFound,
    BrowseLimit,
    Join(String),
    NotPublished,
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
            Self::BrowseNotFound => formatter.write_str("Browse source is no longer available"),
            Self::BrowseLimit => formatter.write_str("Browse window is invalid"),
            Self::Join(error) => formatter.write_str(error),
            Self::NotPublished => formatter.write_str(
                "Library is initializing; the first completed scan has not published a Library yet",
            ),
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

const MAX_BROWSE_WINDOW: usize = 60;
const MAX_BROWSE_SNAPSHOTS: usize = 8;
const BROWSE_SNAPSHOT_IDLE: Duration = Duration::from_secs(30 * 60);

static NEXT_BROWSE_NAMESPACE: AtomicU64 = AtomicU64::new(0);

/// The published Library plus id indices, rebuilt atomically on each snapshot
/// replacement so bounded window requests never rescan the whole Library.
struct Published {
    snapshot: slipstream_core::ScanSnapshot,
    photos_by_id: std::collections::HashMap<String, usize>,
    originals_by_id: std::collections::HashMap<String, usize>,
}

impl Published {
    fn new(snapshot: slipstream_core::ScanSnapshot) -> Self {
        let photos_by_id = snapshot
            .photos
            .iter()
            .enumerate()
            .map(|(position, photo)| (photo.id.clone(), position))
            .collect();
        let originals_by_id = snapshot
            .originals
            .iter()
            .enumerate()
            .map(|(position, original)| (original.id.clone(), position))
            .collect();
        Self {
            snapshot,
            photos_by_id,
            originals_by_id,
        }
    }
}

struct BrowseSnapshot {
    photo_ids: Vec<String>,
    last_used: Instant,
}

/// The published Library plus shared scan-lifecycle flags. The snapshot is
/// refreshed from persisted state at every publication, so facts committed
/// after a scan's apply (Selection State, Rating, Review Preview seeds) can
/// never be reverted by the completed scan. `publication` serializes
/// publications against in-place fact patches: a patch either happens before
/// the publication's persisted read (the read includes its committed fact) or
/// after the swap (the patch applies to the new snapshot).
struct SharedLibrary {
    snapshot: RwLock<Option<Published>>,
    published: AtomicBool,
    failed: AtomicBool,
    awaiting_scan: AtomicUsize,
    runs_started: AtomicU64,
    runs_completed: AtomicU64,
    publication: tokio::sync::Mutex<()>,
}

impl SharedLibrary {
    /// Replaces the published snapshot with the current persisted state read
    /// while holding `publication`. The scan has already applied its result
    /// transactionally, so the fresh read is the authoritative merge of
    /// scan-owned changes (availability, order, source selection, Preview
    /// invalidation for changed revisions) plus every later committed fact.
    async fn publish_fresh(&self, library: &Library) -> Result<(), LibraryError> {
        let _publication = self.publication.lock().await;
        let persisted = library.snapshot().await?;
        *self.snapshot.write().expect("published Library poisoned") =
            Some(Published::new(persisted));
        self.failed.store(false, Ordering::Relaxed);
        self.published.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Patches one mutable Photo fact in place. Called only after the
    /// owning SQLite write committed, under `publication`, so the patch can
    /// never be applied to a snapshot a concurrent publication is replacing.
    async fn patch_photo(
        &self,
        photo_id: &str,
        apply: impl FnOnce(&mut slipstream_core::PhotoRecord),
    ) {
        let _publication = self.publication.lock().await;
        let mut guard = self.snapshot.write().expect("published Library poisoned");
        let Some(published) = guard.as_mut() else {
            return;
        };
        let Some(position) = published.photos_by_id.get(photo_id).copied() else {
            return;
        };
        let Some(photo) = published.snapshot.photos.get_mut(position) else {
            return;
        };
        apply(photo);
    }

    /// One owned scan cycle shared by the background startup rescan and
    /// explicit rescan requests; the Library coalesces concurrent waiters.
    /// The optional publish gate (test-only) parks this cycle after the scan's
    /// apply and before publication so tests can commit facts in between.
    async fn run_scan(
        &self,
        library: &Library,
        publish_gate: Option<oneshot::Receiver<()>>,
    ) -> Result<(), LibraryError> {
        self.runs_started.fetch_add(1, Ordering::Relaxed);
        self.awaiting_scan.fetch_add(1, Ordering::Relaxed);
        let outcome = match library.scan().await {
            Ok(_) => {
                if let Some(gate) = publish_gate {
                    let _ = gate.await;
                }
                match self.publish_fresh(library).await {
                    Ok(()) => Ok(()),
                    Err(error @ (LibraryError::Closed | LibraryError::ScannerStopped)) => {
                        Err(error)
                    }
                    Err(error) => {
                        self.failed.store(true, Ordering::Relaxed);
                        Err(error)
                    }
                }
            }
            // Shutdown drained this admitted scan; it is not a Library failure.
            Err(error @ (LibraryError::Closed | LibraryError::ScannerStopped)) => Err(error),
            Err(error) => {
                self.failed.store(true, Ordering::Relaxed);
                Err(error)
            }
        };
        self.awaiting_scan.fetch_sub(1, Ordering::Relaxed);
        self.runs_completed.fetch_add(1, Ordering::Relaxed);
        outcome
    }
}

pub struct Application {
    library: Arc<Library>,
    preview: PreviewService,
    shared: Arc<SharedLibrary>,
    background_scan: Mutex<Option<JoinHandle<()>>>,
    browse_snapshots: Mutex<std::collections::HashMap<String, BrowseSnapshot>>,
    browse_namespace: u128,
    browse_counter: AtomicU64,
    shutdown: Mutex<bool>,
}

impl Application {
    pub async fn open(config: &Config) -> Result<Arc<Self>, ServerError> {
        Self::open_with_gate(config, ScanLimits::default(), None, None).await
    }

    async fn open_with_gate(
        config: &Config,
        scan_limits: ScanLimits,
        scan_gate: Option<oneshot::Receiver<()>>,
        publish_gate: Option<oneshot::Receiver<()>>,
    ) -> Result<Arc<Self>, ServerError> {
        validate_storage_layout(config)?;
        let cache = CacheDirectory::open(&config.cache_directory, &config.library_root)?;
        let library_config = LibraryConfig {
            library_root: config.library_root.clone(),
            state_directory: config.state_directory.clone(),
            database_basename: config.database_basename.clone(),
            limits: scan_limits,
            ..LibraryConfig::default()
        };
        let library = tokio::task::spawn_blocking(move || Library::open(library_config))
            .await
            .map_err(|error| ServerError::Join(error.to_string()))??;
        let library = Arc::new(library);
        // Admission is complete: serve the last committed Library immediately
        // while the ordinary startup rescan runs in the background. A store
        // without a published Library stays initializing until its first scan
        // publishes one.
        let persisted = library.snapshot().await?;
        let published_initial = !persisted.photos.is_empty() || !persisted.originals.is_empty();
        let shared = Arc::new(SharedLibrary {
            snapshot: RwLock::new(published_initial.then(|| Published::new(persisted))),
            published: AtomicBool::new(published_initial),
            failed: AtomicBool::new(false),
            awaiting_scan: AtomicUsize::new(0),
            runs_started: AtomicU64::new(0),
            runs_completed: AtomicU64::new(0),
            publication: tokio::sync::Mutex::new(()),
        });
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
        let browse_namespace = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            ^ (u128::from(std::process::id()) << 64)
            ^ u128::from(NEXT_BROWSE_NAMESPACE.fetch_add(1, Ordering::Relaxed));
        let background_scan = {
            let library = Arc::clone(&library);
            let shared = Arc::clone(&shared);
            tokio::spawn(async move {
                if let Some(gate) = scan_gate {
                    let _ = gate.await;
                }
                let _ = shared.run_scan(&library, publish_gate).await;
            })
        };
        Ok(Arc::new(Self {
            library,
            preview,
            shared,
            background_scan: Mutex::new(Some(background_scan)),
            browse_snapshots: Mutex::new(std::collections::HashMap::new()),
            browse_namespace,
            browse_counter: AtomicU64::new(0),
            shutdown: Mutex::new(false),
        }))
    }

    /// Truthful Library status: the scanner owns measurable phases and
    /// counters, and the shared flags decide idle, failed, or initializing.
    fn scan_status(&self) -> ScanStatusWire {
        let progress = self.library.scan_progress();
        match progress.phase {
            ScanPhase::Discovering => ScanStatusWire {
                state: "discovering",
                completed: Some(usize::try_from(progress.discovered).unwrap_or(usize::MAX)),
                total: None,
            },
            ScanPhase::Inspecting => ScanStatusWire {
                state: "inspecting",
                completed: Some(usize::try_from(progress.inspected).unwrap_or(usize::MAX)),
                total: progress
                    .inspect_total
                    .map(|total| usize::try_from(total).unwrap_or(usize::MAX)),
            },
            ScanPhase::Applying => ScanStatusWire {
                state: "applying",
                completed: None,
                total: None,
            },
            ScanPhase::Idle => {
                if self.shared.awaiting_scan.load(Ordering::Relaxed) > 0 {
                    // The scan finished; its result is being published.
                    ScanStatusWire {
                        state: "applying",
                        completed: None,
                        total: None,
                    }
                } else if self.shared.failed.load(Ordering::Relaxed) {
                    ScanStatusWire {
                        state: "failed",
                        completed: None,
                        total: None,
                    }
                } else if self.shared.published.load(Ordering::Relaxed) {
                    let photo_count = self.published_photo_count();
                    ScanStatusWire {
                        state: "idle",
                        completed: Some(photo_count),
                        total: Some(photo_count),
                    }
                } else {
                    ScanStatusWire {
                        state: "initializing",
                        completed: None,
                        total: None,
                    }
                }
            }
        }
    }

    fn published_photo_count(&self) -> usize {
        self.shared
            .snapshot
            .read()
            .expect("published Library poisoned")
            .as_ref()
            .map_or(0, |published| published.snapshot.photos.len())
    }

    pub async fn overview(&self) -> Result<LibraryOverviewResponse, ServerError> {
        let photo_sets = self
            .library
            .list_photo_sets()
            .await?
            .into_iter()
            .map(photo_set_summary)
            .collect();
        Ok(LibraryOverviewResponse {
            published: self.shared.published.load(Ordering::Relaxed),
            photo_count: self.published_photo_count(),
            scan: self.scan_status(),
            photo_sets,
        })
    }

    pub async fn browse_open(
        &self,
        source: BrowseSourceRequest,
        preferred_photo_id: Option<&str>,
    ) -> Result<BrowseOpenResponse, ServerError> {
        let (photo_ids, position): (Vec<String>, usize) = match source {
            BrowseSourceRequest::Library => {
                let guard = self
                    .shared
                    .snapshot
                    .read()
                    .expect("published Library poisoned");
                let Some(published) = guard.as_ref() else {
                    return Err(ServerError::NotPublished);
                };
                let photo_ids = published
                    .snapshot
                    .photos
                    .iter()
                    .map(|photo| photo.id.clone())
                    .collect::<Vec<_>>();
                let position = preferred_photo_id
                    .and_then(|preferred| photo_ids.iter().position(|id| id == preferred))
                    .unwrap_or(0);
                (photo_ids, position)
            }
            BrowseSourceRequest::PhotoSet(id) => {
                let set = self
                    .library
                    .list_photo_sets()
                    .await?
                    .into_iter()
                    .find(|set| set.id == id)
                    .ok_or(ServerError::BrowseNotFound)?;
                let preferred = preferred_photo_id.and_then(|preferred| {
                    set.members
                        .iter()
                        .position(|member| member.photo_id == preferred)
                });
                let saved = set.last_reviewed_photo_id.and_then(|saved| {
                    set.members
                        .iter()
                        .position(|member| member.photo_id == saved)
                });
                let position = preferred.unwrap_or_else(|| {
                    saved
                        .filter(|saved| set.members[*saved].available)
                        .or_else(|| {
                            saved.and_then(|saved| {
                                (1..=set.members.len())
                                    .map(|offset| (saved + offset) % set.members.len())
                                    .find(|index| set.members[*index].available)
                            })
                        })
                        .or_else(|| set.members.iter().position(|member| member.available))
                        .or(saved)
                        .unwrap_or(0)
                });
                (
                    set.members
                        .into_iter()
                        .map(|member| member.photo_id)
                        .collect(),
                    position,
                )
            }
        };
        let token = format!(
            "b{:032x}{:016x}",
            self.browse_namespace,
            self.browse_counter.fetch_add(1, Ordering::Relaxed)
        );
        let mut snapshots = self
            .browse_snapshots
            .lock()
            .expect("browse snapshots poisoned");
        let now = Instant::now();
        snapshots
            .retain(|_, snapshot| now.duration_since(snapshot.last_used) < BROWSE_SNAPSHOT_IDLE);
        while snapshots.len() >= MAX_BROWSE_SNAPSHOTS {
            let Some(oldest) = snapshots
                .iter()
                .min_by_key(|(_, snapshot)| snapshot.last_used)
                .map(|(token, _)| token.clone())
            else {
                break;
            };
            snapshots.remove(&oldest);
        }
        let total = photo_ids.len();
        snapshots.insert(
            token.clone(),
            BrowseSnapshot {
                photo_ids,
                last_used: now,
            },
        );
        Ok(BrowseOpenResponse {
            token,
            total,
            position,
        })
    }

    pub async fn browse_window(
        &self,
        token: &str,
        start: usize,
        limit: usize,
    ) -> Result<BrowseWindowResponse, ServerError> {
        if limit == 0 || limit > MAX_BROWSE_WINDOW {
            return Err(ServerError::BrowseLimit);
        }
        let (ids, total) = {
            let mut snapshots = self
                .browse_snapshots
                .lock()
                .expect("browse snapshots poisoned");
            let now = Instant::now();
            if snapshots.get(token).is_some_and(|snapshot| {
                now.duration_since(snapshot.last_used) >= BROWSE_SNAPSHOT_IDLE
            }) {
                snapshots.remove(token);
                return Err(ServerError::BrowseNotFound);
            }
            let snapshot = snapshots
                .get_mut(token)
                .ok_or(ServerError::BrowseNotFound)?;
            snapshot.last_used = now;
            let total = snapshot.photo_ids.len();
            let ids = snapshot
                .photo_ids
                .iter()
                .skip(start)
                .take(limit)
                .cloned()
                .collect::<Vec<_>>();
            (ids, total)
        };
        let source_guard = self
            .shared
            .snapshot
            .read()
            .expect("published Library poisoned");
        let Some(source) = source_guard.as_ref() else {
            return Err(ServerError::NotPublished);
        };
        let photos = ids
            .iter()
            .filter_map(|id| source.photos_by_id.get(id).copied())
            .filter_map(|position| source.snapshot.photos.get(position))
            .map(|photo| {
                photo_summary_indexed(photo, &source.snapshot.originals, &source.originals_by_id)
            })
            .collect();
        Ok(BrowseWindowResponse {
            start,
            total,
            photos,
        })
    }

    pub fn browse_close(&self, token: &str) {
        self.browse_snapshots
            .lock()
            .expect("browse snapshots poisoned")
            .remove(token);
    }

    pub async fn photo_sets(&self) -> Result<PhotoSetSummaryListResponse, ServerError> {
        Ok(PhotoSetSummaryListResponse {
            photo_sets: self
                .library
                .list_photo_sets()
                .await?
                .into_iter()
                .map(photo_set_summary)
                .collect(),
        })
    }

    pub async fn rescan(&self) -> Result<ScanStatusWire, ServerError> {
        self.shared.run_scan(&self.library, None).await?;
        Ok(self.scan_status())
    }

    pub async fn mutate_photo_set(
        &self,
        mutation: slipstream_core::PhotoSetMutation,
    ) -> Result<PhotoSetSummaryListResponse, ServerError> {
        self.library.mutate_photo_set(mutation).await?;
        self.photo_sets().await
    }

    pub async fn mutate_photo_state(
        &self,
        mutation: slipstream_core::PhotoStateMutation,
    ) -> Result<slipstream_core::PhotoStateMutationResult, ServerError> {
        let photo_id = mutation.photo_id.clone();
        let field = mutation.field;
        let value = mutation.value;
        let result = self.library.mutate_photo_state(mutation).await?;
        self.shared
            .patch_photo(&photo_id, |photo| match (field, value) {
                (PhotoStateField::SelectionState, PhotoStateValue::Selection(selection)) => {
                    photo.selection_state = selection;
                }
                (PhotoStateField::Rating, PhotoStateValue::Rating(rating)) => {
                    photo.rating = rating;
                }
                _ => {}
            })
            .await;
        Ok(result)
    }

    pub async fn preview(&self, photo_id: &str) -> Result<PreviewResponse, ServerError> {
        self.preview_with_priority(photo_id, slipstream_core::DerivativePriority::Current)
            .await
    }

    pub async fn preview_with_priority(
        &self,
        photo_id: &str,
        priority: slipstream_core::DerivativePriority,
    ) -> Result<PreviewResponse, ServerError> {
        self.preview_target(photo_id, DerivativeTarget::Review2560, priority)
            .await
    }

    pub async fn thumbnail(&self, photo_id: &str) -> Result<PreviewResponse, ServerError> {
        self.preview_target(
            photo_id,
            DerivativeTarget::Thumbnail512,
            slipstream_core::DerivativePriority::VisibleGrid,
        )
        .await
    }

    async fn preview_target(
        &self,
        photo_id: &str,
        target: DerivativeTarget,
        priority: slipstream_core::DerivativePriority,
    ) -> Result<PreviewResponse, ServerError> {
        if !valid_id(photo_id) {
            return Ok(PreviewResponse::unavailable("Unknown Photo"));
        }
        let result = self
            .preview
            .request(photo_id.to_owned(), target, priority)
            .await;
        let response = match result {
            Ok(slipstream_core::PreviewRequestResult::Current(ready)) => {
                if target == DerivativeTarget::Review2560 {
                    self.shared
                        .patch_photo(photo_id, |photo| {
                            photo.preview_state = PreviewState::Ready;
                            photo.preview_source = Some(ready.source);
                            photo.preview_width = Some(ready.width);
                            photo.preview_height = Some(ready.height);
                            photo.cache_revision = Some(ready.cache_key.clone());
                        })
                        .await;
                }
                PreviewResponse::ready(photo_id, &ready, false)
            }
            Ok(slipstream_core::PreviewRequestResult::Stale(ready)) => {
                PreviewResponse::ready(photo_id, &ready, true)
            }
            Ok(slipstream_core::PreviewRequestResult::Unavailable(unavailable)) => {
                if target == DerivativeTarget::Review2560
                    && unavailable.reason
                        == slipstream_core::PreviewUnavailableReason::NoUsableSource
                {
                    self.patch_preview_state(photo_id, PreviewState::Unavailable)
                        .await;
                }
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
                if target == DerivativeTarget::Review2560 {
                    self.patch_preview_state(photo_id, PreviewState::Failed)
                        .await;
                }
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
        };
        Ok(response)
    }

    async fn patch_preview_state(&self, photo_id: &str, state: PreviewState) {
        self.shared
            .patch_photo(photo_id, |photo| {
                photo.preview_state = state;
                photo.preview_source = None;
                photo.preview_width = None;
                photo.preview_height = None;
                photo.cache_revision = None;
            })
            .await;
    }

    pub async fn derivative(
        &self,
        photo_id: &str,
        cache_key: &str,
        target: DerivativeTarget,
    ) -> Result<Option<DerivativeDelivery>, ServerError> {
        if !valid_id(photo_id) || !is_hex_key(cache_key) {
            return Ok(None);
        }
        let result = self
            .preview
            .request(
                photo_id.to_owned(),
                target,
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
        // Drain the admitted background scan before closing the Library so a
        // completed scan is always published and shutdown observes no torn work.
        let background = self
            .background_scan
            .lock()
            .expect("background scan poisoned")
            .take();
        if let Some(handle) = background {
            let _ = handle.await;
        }
        let application = Arc::clone(self);
        tokio::task::spawn_blocking(move || application.shutdown_blocking())
            .await
            .map_err(|error| ServerError::Join(error.to_string()))?
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryOverviewResponse {
    pub published: bool,
    pub photo_count: usize,
    pub scan: ScanStatusWire,
    pub photo_sets: Vec<PhotoSetSummaryWire>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatusWire {
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoSetSummaryWire {
    pub id: String,
    pub name: String,
    pub photo_count: usize,
    pub has_saved_position: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseOpenResponse {
    pub token: String,
    pub total: usize,
    pub position: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseWindowResponse {
    pub start: usize,
    pub total: usize,
    pub photos: Vec<PhotoSummary>,
}

#[derive(Clone, Debug)]
pub enum BrowseSourceRequest {
    Library,
    PhotoSet(String),
}

/// Bounded Photo Set mutation response: the same summaries the Library
/// Overview exposes, never member lists.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoSetSummaryListResponse {
    pub photo_sets: Vec<PhotoSetSummaryWire>,
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

fn photo_summary_indexed(
    photo: &slipstream_core::PhotoRecord,
    originals: &[slipstream_core::OriginalRecord],
    originals_by_id: &std::collections::HashMap<String, usize>,
) -> PhotoSummary {
    let original = |id: &Option<String>, kind: &'static str| {
        id.as_ref().and_then(|id| {
            originals_by_id
                .get(id)
                .and_then(|position| originals.get(*position))
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

fn photo_set_summary(record: PhotoSetRecord) -> PhotoSetSummaryWire {
    PhotoSetSummaryWire {
        id: record.id,
        name: record.name,
        photo_count: record.members.len(),
        has_saved_position: record.last_reviewed_photo_id.is_some(),
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

fn derivative_target_name(target: DerivativeTarget) -> &'static str {
    match target {
        DerivativeTarget::Thumbnail512 => "thumbnail",
        DerivativeTarget::Review2560 => "review",
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
                "/api/derivatives/{}/{}/{}.jpg",
                photo_id,
                derivative_target_name(ready.target),
                ready.cache_key
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

pub async fn expand_library(config: Config) -> Result<(), ServerError> {
    validate_storage_layout(&config)?;
    let library_config = LibraryConfig {
        library_root: config.library_root,
        state_directory: config.state_directory,
        database_basename: config.database_basename,
        limits: ScanLimits::default(),
        ..LibraryConfig::default()
    };
    let expansion_config = library_config.clone();
    tokio::task::spawn_blocking(move || slipstream_core::expand_library(expansion_config))
        .await
        .map_err(|error| ServerError::Join(error.to_string()))??;
    let library = tokio::task::spawn_blocking(move || Library::open(library_config))
        .await
        .map_err(|error| ServerError::Join(error.to_string()))??;
    let scan_result = library.scan().await.map(|_| ());
    let close_result = tokio::task::spawn_blocking(move || library.shutdown())
        .await
        .map_err(|error| ServerError::Join(error.to_string()))?;
    scan_result?;
    close_result?;
    Ok(())
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
        .route("/api/overview", get(overview))
        .route("/api/status", get(status))
        .route("/api/browse", post(open_browse))
        .route(
            "/api/browse/{token}",
            get(get_browse_window).delete(close_browse),
        )
        .route("/api/photos/{id}/preview", get(get_preview))
        .route("/api/photos/{id}/thumbnail", get(get_thumbnail))
        // The complete-membership list is retired; the path only creates sets.
        .route(
            "/api/photo-sets",
            get(retired_photo_set_list).post(create_photo_set),
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
            "/api/derivatives/{photo_id}/{target}/{filename}",
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

async fn overview(
    State(state): State<HttpState>,
) -> Result<Json<LibraryOverviewResponse>, ApiError> {
    Ok(Json(state.application.overview().await?))
}

async fn status(State(state): State<HttpState>) -> Json<ScanStatusWire> {
    Json(state.application.scan_status())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowseOpenBody {
    source: String,
    #[serde(default)]
    photo_set_id: Option<String>,
    #[serde(default)]
    photo_id: Option<String>,
}

async fn open_browse(State(state): State<HttpState>, request: Request<Body>) -> Response<Body> {
    if !mutation_allowed(&request) {
        return api_error(StatusCode::FORBIDDEN, "Cross-origin mutation rejected");
    }
    let body = match read_json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Ok(body) = serde_json::from_value::<BrowseOpenBody>(body) else {
        return api_error(StatusCode::BAD_REQUEST, "Invalid browse source");
    };
    let preferred_photo_id = match body.photo_id {
        Some(id) if valid_id(&id) => Some(id),
        Some(_) => return api_error(StatusCode::BAD_REQUEST, "Invalid preferred Photo"),
        None => None,
    };
    let source = match body.source.as_str() {
        "library" if body.photo_set_id.is_none() => BrowseSourceRequest::Library,
        "photo-set" => match body.photo_set_id.filter(|id| valid_id(id)) {
            Some(id) => BrowseSourceRequest::PhotoSet(id),
            None => return api_error(StatusCode::BAD_REQUEST, "Invalid Photo Set source"),
        },
        _ => return api_error(StatusCode::BAD_REQUEST, "Invalid browse source"),
    };
    match state
        .application
        .browse_open(source, preferred_photo_id.as_deref())
        .await
    {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(error) => ApiError::from(error).into_response(),
    }
}

async fn get_browse_window(
    State(state): State<HttpState>,
    axum::extract::Path(token): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    let Some((start, limit)) = browse_query(request.uri().query()) else {
        return api_error(StatusCode::BAD_REQUEST, "Browse window is invalid");
    };
    match state.application.browse_window(&token, start, limit).await {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(error) => ApiError::from(error).into_response(),
    }
}

async fn close_browse(
    State(state): State<HttpState>,
    axum::extract::Path(token): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    if !mutation_allowed(&request) {
        return api_error(StatusCode::FORBIDDEN, "Cross-origin mutation rejected");
    }
    state.application.browse_close(&token);
    Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Body::empty())
        .expect("valid response")
}

fn browse_query(query: Option<&str>) -> Option<(usize, usize)> {
    let mut start = None;
    let mut limit = None;
    for part in query?.split('&') {
        let (key, value) = part.split_once('=')?;
        match key {
            "start" => start = value.parse().ok(),
            "limit" => limit = value.parse().ok(),
            _ => {}
        }
    }
    Some((start?, limit?))
}

async fn retired_photo_set_list() -> Response<Body> {
    api_error(StatusCode::NOT_FOUND, "Not found")
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

async fn get_thumbnail(
    State(state): State<HttpState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Response<Body> {
    match state.application.thumbnail(&id).await {
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

async fn get_preview(
    State(state): State<HttpState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    let priority = if request.uri().query() == Some("priority=adjacent") {
        slipstream_core::DerivativePriority::Adjacent
    } else {
        slipstream_core::DerivativePriority::Current
    };
    match state.application.preview_with_priority(&id, priority).await {
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
    axum::extract::Path((photo_id, target, filename)): axum::extract::Path<(
        String,
        String,
        String,
    )>,
    request: Request<Body>,
) -> Response<Body> {
    let target = match target.as_str() {
        "thumbnail" => DerivativeTarget::Thumbnail512,
        "review" => DerivativeTarget::Review2560,
        _ => return api_error(StatusCode::NOT_FOUND, "Derivative not found"),
    };
    let Some(key) = filename.strip_suffix(".jpg") else {
        return api_error(StatusCode::NOT_FOUND, "Derivative not found");
    };
    if !is_hex_key(key) {
        return api_error(StatusCode::NOT_FOUND, "Derivative not found");
    }
    let delivery = match state.application.derivative(&photo_id, key, target).await {
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
    if !matches!(
        request.method().as_str(),
        "GET" | "HEAD" | "POST" | "DELETE"
    ) {
        return api_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed");
    }
    if matches!(request.method().as_str(), "POST" | "DELETE") {
        let path = request.uri().path();
        let api_path = path == "/api" || path.starts_with("/api/");
        let admitted = api_path
            && (!request.method().as_str().eq("DELETE") || path.starts_with("/api/browse/"));
        if !admitted {
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
            ServerError::BrowseNotFound => Self {
                status: StatusCode::NOT_FOUND,
                message: "Browse source expired or not found",
            },
            ServerError::BrowseLimit => Self {
                status: StatusCode::BAD_REQUEST,
                message: "Browse window is invalid",
            },
            ServerError::NotPublished => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                message: "Library is initializing; retry after the first scan completes",
            },
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

    async fn browse_photo_ids(
        application: &Application,
        source: BrowseSourceRequest,
    ) -> Vec<String> {
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
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../compatibility/protocol/vectors.json"),
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
                    && let Some(id) = actual["photoSets"][0]["id"].as_str()
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
    async fn photo_set_browse_open_resolves_saved_position_without_members_response() {
        let (base, config) = prepare_fixture();
        for name in ["a.jpg", "b.jpg", "c.jpg"] {
            jpeg_fixture(&config.library_root.join(name), 8, 4, [32, 64, 192]);
        }
        let application = Application::open(&config).await.unwrap();
        wait_for_scan_settled(&application).await;
        application.rescan().await.unwrap();
        let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
        application
            .mutate_photo_set(slipstream_core::PhotoSetMutation::Create {
                name: "Picks".to_owned(),
            })
            .await
            .unwrap();
        let set_id = application
            .photo_sets()
            .await
            .unwrap()
            .photo_sets
            .into_iter()
            .find(|set| set.name == "Picks")
            .unwrap()
            .id;
        application
            .mutate_photo_set(slipstream_core::PhotoSetMutation::AddMembers {
                photo_set_id: set_id.clone(),
                photo_ids: ids.clone(),
            })
            .await
            .unwrap();
        application
            .mutate_photo_set(slipstream_core::PhotoSetMutation::SetProgress {
                photo_set_id: set_id.clone(),
                photo_id: ids[1].clone(),
            })
            .await
            .unwrap();
        let opened = application
            .browse_open(BrowseSourceRequest::PhotoSet(set_id), None)
            .await
            .unwrap();
        assert_eq!(opened.total, 3);
        assert_eq!(opened.position, 1);
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
            .mutate_photo_set(slipstream_core::PhotoSetMutation::Create {
                name: "Picks".to_owned(),
            })
            .await
            .unwrap();
        let set_id = application
            .photo_sets()
            .await
            .unwrap()
            .photo_sets
            .into_iter()
            .find(|set| set.name == "Picks")
            .unwrap()
            .id;
        application
            .mutate_photo_set(slipstream_core::PhotoSetMutation::AddMembers {
                photo_set_id: set_id.clone(),
                photo_ids: ids.clone(),
            })
            .await
            .unwrap();
        application
            .mutate_photo_set(slipstream_core::PhotoSetMutation::SetProgress {
                photo_set_id: set_id.clone(),
                photo_id: ids[0].clone(),
            })
            .await
            .unwrap();
        let preferred = application
            .browse_open(BrowseSourceRequest::PhotoSet(set_id), Some(&ids[2]))
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
        let application = Application::open_with_gate(
            &config,
            ScanLimits::default(),
            None,
            Some(publish_receiver),
        )
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
                    photo_set_id: None,
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
    async fn persisted_forty_thousand_photo_library_serves_bounded_overview_before_rescan_completes()
     {
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
        wait_for_scan_settled(&application).await;
        let router = create_router(Arc::clone(&application), config.web_root());
        let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
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
        // Mutation responses expose bounded summaries only, never members.
        assert_eq!(created["photoSets"][0]["photoCount"], 0);
        assert_eq!(created["photoSets"][0]["hasSavedPosition"], false);
        assert!(created["photoSets"][0]["members"].is_null());
        assert!(created["photoSets"][0]["lastReviewedPhotoId"].is_null());
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
        // Membership order is observable only through a fresh Photo Set
        // Browse Snapshot; the mutation responses stay summary-only.
        let ordered =
            browse_photo_ids(&application, BrowseSourceRequest::PhotoSet(set_a.clone())).await;
        assert_eq!(
            ordered,
            vec![ids[2].clone(), ids[0].clone(), ids[1].clone()]
        );
        let set_b_photos =
            browse_summaries(&application, BrowseSourceRequest::PhotoSet(set_b.clone())).await;
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
                &format!("http://camera.local/api/photo-sets/{set_a}/members/remove"),
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
            .photo_sets()
            .await
            .unwrap()
            .photo_sets
            .into_iter()
            .find(|set| set.id == set_a)
            .unwrap();
        assert!(!set_a_summary.has_saved_position);
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
    async fn unbounded_library_routes_are_retired() {
        let (base, config) = prepare_fixture();
        jpeg_fixture(&config.library_root.join("a.jpg"), 8, 4, [32, 64, 192]);
        let application = Application::open(&config).await.unwrap();
        wait_for_scan_settled(&application).await;
        let router = create_router(Arc::clone(&application), config.web_root());
        for uri in [
            "http://camera.local/api/photos",
            "http://camera.local/api/photo-sets",
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
        wait_for_scan_settled(&application).await;
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
}
