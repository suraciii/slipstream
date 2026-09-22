use super::*;
use crate::{
    folders::valid_folder_location,
    queries::{
        CLI_CONTRACT_VERSION, CursorError, MAXIMUM_LIST_PAGE, MAXIMUM_RETAINED_IDS, QUERY_IDLE,
        RetainedKind, format_time,
    },
    wire::{
        AlbumListItemWire, CapabilitiesResponse, CapabilityLimitsWire, CliAlbumChangeWire,
        CliAlbumCreationWire, CliAlbumSummaryWire, CliFolderListResponse, CliListResponse,
        CliPhotoGetWire, CliPhotoItemWire, CliPhotoMetadataWire, CliScanStatusWire,
        CliStatusResponse, MissingItemWire, PhotoListItemWire,
    },
};

/// One manual recovery batch is bounded so a single request can never
/// rewrite an unbounded slice of the Library.
const MAXIMUM_RECOVERY_RELOCATIONS: usize = 10_000;
#[derive(Clone)]
pub(crate) struct WebRoot {
    path: PathBuf,
    descriptor: Option<Arc<OwnedFd>>,
    ready: bool,
}

#[derive(Clone)]
pub(crate) struct HttpState {
    pub(crate) application: Arc<Application>,
    pub(crate) web_root: Arc<WebRoot>,
}

pub(crate) struct CloseState {
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

pub async fn expand_library(config: ExpansionConfig) -> Result<(), ServerError> {
    validate_expansion_storage_layout(&config)?;
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

pub(crate) fn create_router_with_web_root(
    application: Arc<Application>,
    web_root: WebRoot,
) -> Router {
    let state = HttpState {
        application,
        web_root: Arc::new(web_root),
    };
    Router::new()
        .route(HEALTH_PATH, get(healthz))
        .route("/api/overview", get(overview))
        .route("/api/status", get(status))
        .route("/api/capabilities", get(capabilities))
        .route("/api/album-summaries", get(get_album_summaries))
        .route("/api/albums/{id}", get(get_album_summary))
        .route("/api/albums/{id}/changes", post(change_album))
        .route("/api/photo-queries", post(create_photo_query))
        .route("/api/photo-queries/{cursor}", get(get_photo_query_page))
        .route("/api/photos/{id}", get(get_photo))
        .route("/api/file-locations", get(get_file_locations))
        .route("/api/browse", post(open_browse))
        .route("/api/browse/{token}/position", get(get_browse_position))
        .route(
            "/api/browse/{token}",
            get(get_browse_window).delete(close_browse),
        )
        .route("/api/photos/{id}/preview", get(get_preview))
        .route("/api/photos/{id}/thumbnail", get(get_thumbnail))
        .route("/api/photos/{id}/metadata", get(get_photo_metadata))
        .route("/api/photos/{id}/albums", get(get_photo_albums))
        // The complete-membership list is retired; the path only creates Albums.
        .route("/api/albums", get(retired_album_list).post(create_album))
        .route(
            "/api/albums/{id}/rename",
            get(method_not_allowed).post(rename_album),
        )
        .route(
            "/api/albums/{id}/delete",
            get(method_not_allowed).post(delete_album),
        )
        .route(
            "/api/albums/{id}/members",
            get(method_not_allowed).post(add_album_members),
        )
        .route(
            "/api/albums/{id}/members/batch-remove",
            get(method_not_allowed).post(remove_added_album_members),
        )
        .route(
            "/api/albums/{id}/folder-members",
            get(method_not_allowed).post(add_folder_members),
        )
        .route(
            "/api/albums/{id}/members/remove",
            get(method_not_allowed).post(remove_album_member),
        )
        .route(
            "/api/albums/{id}/order",
            get(method_not_allowed).post(reorder_album),
        )
        .route(
            "/api/albums/{id}/progress",
            get(method_not_allowed).post(set_progress),
        )
        .route(
            "/api/photos/state",
            get(method_not_allowed).post(mutate_photo_state_batch),
        )
        .route(
            "/api/photos/{id}/state",
            get(method_not_allowed).post(mutate_photo_state),
        )
        .route("/api/scan", get(method_not_allowed).post(scan))
        .route("/api/recovery/unavailable", get(recovery_unavailable))
        .route(
            "/api/recovery/propose",
            get(method_not_allowed).post(recovery_propose),
        )
        .route(
            "/api/recovery/apply",
            get(method_not_allowed).post(recovery_apply),
        )
        .route(
            "/api/derivatives/{photo_id}/{target}/{filename}",
            get(get_derivative),
        )
        .fallback(static_web)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            request_policy,
        ))
        .with_state(state)
}

pub(crate) fn open_web_root(path: PathBuf) -> WebRoot {
    let descriptor = open_directory_descriptor(&path).ok().map(Arc::new);
    let mut root = WebRoot {
        descriptor,
        path,
        ready: false,
    };
    root.ready = web_root_has_index(&root);
    root
}

pub(crate) fn web_root_has_index(root: &WebRoot) -> bool {
    let Some(descriptor) = &root.descriptor else {
        return false;
    };
    let index = CString::new("index.html").expect("static filename has no NUL");
    open_confined_file(descriptor.as_raw_fd(), &index)
        .and_then(|file| fstat(file.as_raw_fd()))
        .is_ok_and(|facts| facts.st_mode & libc::S_IFMT == libc::S_IFREG)
}

#[derive(Clone, Copy, Debug, Serialize)]
pub(crate) struct HealthResponse {
    status: &'static str,
}

pub(crate) async fn healthz(State(state): State<HttpState>) -> Response<Body> {
    if !state.web_root.ready || !web_root_has_index(&state.web_root) {
        return plain_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "Web application is not built",
        );
    }
    Json(HealthResponse { status: "ok" }).into_response()
}

pub(crate) async fn overview(
    State(state): State<HttpState>,
) -> Result<Json<LibraryOverviewResponse>, ApiError> {
    Ok(Json(state.application.overview().await?))
}

pub(crate) async fn status(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
    if request.headers().contains_key(CLI_CONTRACT_HEADER) {
        if let Err(response) = require_cli_contract(&request) {
            return *response;
        }
        let scan = state.application.scan_status();
        let response = CliStatusResponse {
            server_version: env!("CARGO_PKG_VERSION"),
            cli_contract_version: CLI_CONTRACT_VERSION,
            published: state.application.shared.published.load(Ordering::Relaxed),
            publication: state.application.current_publication(),
            photo_count: state.application.published_photo_count(),
            scan: CliScanStatusWire::from(scan),
        };
        return json_response(StatusCode::OK, &response);
    }
    json_response(StatusCode::OK, &state.application.scan_status())
}

const CLI_CONTRACT_HEADER: &str = "slipstream-cli-contract";
const DEFAULT_LIST_PAGE: usize = 50;

type CliBoundaryResult<T> = Result<T, Box<Response<Body>>>;

fn require_cli_contract(request: &Request<Body>) -> CliBoundaryResult<()> {
    let mut values = request.headers().get_all(CLI_CONTRACT_HEADER).iter();
    if values.next().and_then(|value| value.to_str().ok()) == Some("1") && values.next().is_none() {
        return Ok(());
    }
    Err(Box::new(cli_error(
        StatusCode::UPGRADE_REQUIRED,
        "incompatible_server",
        "The server does not support the requested CLI contract; use a compatible client or server.",
        serde_json::json!({
            "requestedContractVersion": request
                .headers()
                .get(CLI_CONTRACT_HEADER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u16>().ok())
                .unwrap_or(0),
            "supportedContractVersions": [CLI_CONTRACT_VERSION]
        }),
    )))
}

fn cli_error(
    status: StatusCode,
    code: &'static str,
    message: &'static str,
    details: Value,
) -> Response<Body> {
    json_response(
        status,
        &serde_json::json!({
            "error": {
                "code": code,
                "message": message,
                "effect": "none",
                "details": details
            }
        }),
    )
}

fn invalid_cli(argument: &'static str, reason: &'static str) -> Response<Body> {
    cli_error(
        StatusCode::BAD_REQUEST,
        "invalid_input",
        "Correct the request and try again.",
        serde_json::json!({"argument": argument, "reason": reason}),
    )
}

fn require_published(application: &Application) -> CliBoundaryResult<()> {
    if application.shared.published.load(Ordering::Relaxed) {
        return Ok(());
    }
    Err(Box::new(cli_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "library_unavailable",
        "Wait for Library publication and query status again.",
        serde_json::json!({"scan": CliScanStatusWire::from(application.scan_status())}),
    )))
}

pub(crate) async fn capabilities(request: Request<Body>) -> Response<Body> {
    if let Err(response) = require_cli_contract(&request) {
        return *response;
    }
    json_response(
        StatusCode::OK,
        &CapabilitiesResponse {
            server_version: env!("CARGO_PKG_VERSION"),
            supported_cli_contract_versions: [CLI_CONTRACT_VERSION],
            limits: CapabilityLimitsWire {
                list_page_maximum: MAXIMUM_LIST_PAGE,
                mutation_photo_ids_maximum: slipstream_core::PHOTO_STATE_BATCH_MAX,
                album_reorder_members_maximum: ALBUM_PHOTO_IDS_MAX,
                retained_query_ids_maximum: MAXIMUM_RETAINED_IDS,
                retained_query_idle_seconds: QUERY_IDLE.as_secs(),
            },
        },
    )
}

fn cli_query_pairs(query: Option<&str>) -> CliBoundaryResult<Vec<(String, String)>> {
    let mut pairs = Vec::new();
    if let Some(query) = query {
        if query.is_empty() {
            return Ok(pairs);
        }
        for part in query.split('&') {
            let Some((name, value)) = part.split_once('=') else {
                return Err(Box::new(invalid_cli(
                    "query",
                    "Each query parameter must have a value.",
                )));
            };
            let Some(name) = percent_decode(name) else {
                return Err(Box::new(invalid_cli(
                    "query",
                    "A query parameter name is not valid UTF-8.",
                )));
            };
            let Some(value) = percent_decode(value) else {
                return Err(Box::new(invalid_cli(
                    "query",
                    "A query parameter value is not valid UTF-8.",
                )));
            };
            if pairs.iter().any(|(existing, _)| existing == &name) {
                return Err(Box::new(invalid_cli(
                    "query",
                    "Duplicate query parameters are not allowed.",
                )));
            }
            pairs.push((name, value));
        }
    }
    Ok(pairs)
}

fn list_limit(value: Option<&str>) -> CliBoundaryResult<usize> {
    match value {
        None => Ok(DEFAULT_LIST_PAGE),
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|limit| (1..=MAXIMUM_LIST_PAGE).contains(limit))
            .ok_or_else(|| Box::new(invalid_cli("limit", "The limit must be from 1 through 60."))),
    }
}

fn query_token(application: &Application, kind: RetainedKind) -> String {
    let prefix = match kind {
        RetainedKind::Album => 'a',
        RetainedKind::Photo => 'p',
        RetainedKind::Browse => 'b',
    };
    format!(
        "{prefix}{:032x}{:016x}",
        application.browse_namespace,
        application.browse_counter.fetch_add(1, Ordering::Relaxed)
    )
}

fn map_query_error(
    error: LibraryError,
    operation: &'static str,
    missing_resource: &'static str,
    missing_reference: &str,
) -> Response<Body> {
    match error {
        LibraryError::Query(slipstream_core::PhotoQueryError::Invalid) => {
            invalid_cli("query", "The query combination is invalid.")
        }
        LibraryError::Query(slipstream_core::PhotoQueryError::SourceNotFound) => cli_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "Inspect the current source and try again.",
            serde_json::json!({"resource": missing_resource, "reference": missing_reference}),
        ),
        LibraryError::Query(slipstream_core::PhotoQueryError::ResultLimitExceeded { .. }) => {
            cli_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "server_busy",
                "Narrow the query before trying again.",
                serde_json::json!({"operation": operation, "retryAfterSeconds": null}),
            )
        }
        _ => cli_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_failed",
            "Inspect server health and try again.",
            serde_json::json!({"operation": operation}),
        ),
    }
}

fn query_cursor_error(kind: &'static str, error: CursorError) -> Response<Body> {
    match error {
        CursorError::Invalid => {
            invalid_cli("cursor", "The cursor is malformed or has the wrong kind.")
        }
        CursorError::ProcessRestarted => cli_error(
            StatusCode::GONE,
            "cursor_expired",
            "Start a new query after the server restart.",
            serde_json::json!({"cursorKind": kind, "reason": "process_restarted"}),
        ),
        CursorError::PublicationReplaced => cli_error(
            StatusCode::GONE,
            "cursor_expired",
            "List Folders again from the current Published Library.",
            serde_json::json!({"cursorKind": "folder", "reason": "publication_replaced"}),
        ),
    }
}

pub(crate) async fn get_album_summaries(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
    if let Err(response) = require_cli_contract(&request) {
        return *response;
    }
    if let Err(response) = require_published(&state.application) {
        return *response;
    }
    let pairs = match cli_query_pairs(request.uri().query()) {
        Ok(pairs) => pairs,
        Err(response) => return *response,
    };
    if let Some(cursor) = pairs
        .iter()
        .find(|(name, _)| name == "cursor")
        .map(|(_, value)| value)
    {
        if pairs.len() != 1 {
            return invalid_cli("cursor", "A continuation cannot include new filters.");
        }
        return album_query_page(&state.application, cursor).await;
    }
    if pairs
        .iter()
        .any(|(name, _)| !matches!(name.as_str(), "name" | "photoId" | "limit"))
    {
        return invalid_cli("query", "The Album query contains an unknown parameter.");
    }
    let name = pairs
        .iter()
        .find(|(key, _)| key == "name")
        .map(|(_, value)| value);
    let photo = pairs
        .iter()
        .find(|(key, _)| key == "photoId")
        .map(|(_, value)| value);
    if name.is_some() && photo.is_some() {
        return invalid_cli("query", "Album name and Photo filters cannot be combined.");
    }
    if name.is_some_and(|value| value.trim().is_empty() || value.chars().count() > 120) {
        return invalid_cli("name", "The Album name is invalid.");
    }
    if photo.is_some_and(|value| !valid_id(value)) {
        return invalid_cli("photoId", "The Photo ID is invalid.");
    }
    let limit = match list_limit(
        pairs
            .iter()
            .find(|(key, _)| key == "limit")
            .map(|(_, value)| value.as_str()),
    ) {
        Ok(limit) => limit,
        Err(response) => return *response,
    };
    let filter = match (name, photo) {
        (Some(name), None) => slipstream_core::AlbumQueryFilter::ExactName(name.to_owned()),
        (None, Some(photo)) => slipstream_core::AlbumQueryFilter::ContainsPhoto(photo.to_owned()),
        (None, None) => slipstream_core::AlbumQueryFilter::All,
        (Some(_), Some(_)) => unreachable!(),
    };
    let missing_reference = photo.map_or("", String::as_str);
    let ids = match state
        .application
        .library
        .create_album_query(filter, MAXIMUM_RETAINED_IDS)
        .await
    {
        Ok(ids) => ids,
        Err(error) => return map_query_error(error, "albums-list", "photo", missing_reference),
    };
    let evaluated_at = SystemTime::now();
    let total = ids.len();
    let token = query_token(&state.application, RetainedKind::Album);
    let page_ids = ids.iter().take(limit).cloned().collect::<Vec<_>>();
    let next_cursor = if total > limit {
        if state
            .application
            .retained_queries
            .lock()
            .expect("retained queries poisoned")
            .insert(
                token.clone(),
                RetainedKind::Album,
                ids,
                Instant::now(),
                evaluated_at,
            )
            .is_err()
        {
            return cli_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "server_busy",
                "Narrow the query before trying again.",
                serde_json::json!({"operation": "albums-list", "retryAfterSeconds": null}),
            );
        }
        Some(state.application.cursor_signer.query_cursor(
            state.application.browse_namespace,
            RetainedKind::Album,
            &token,
            limit,
            limit,
        ))
    } else {
        None
    };
    let expires_at = next_cursor.as_ref().map(|_| evaluated_at + QUERY_IDLE);
    album_list_response(
        &state.application,
        page_ids,
        total,
        evaluated_at,
        next_cursor,
        expires_at,
    )
    .await
}

async fn album_query_page(application: &Application, cursor: &str) -> Response<Body> {
    let cursor = match application.cursor_signer.parse_query_cursor(
        cursor,
        application.browse_namespace,
        RetainedKind::Album,
    ) {
        Ok(cursor) => cursor,
        Err(error) => return query_cursor_error("album", error),
    };
    let page = application
        .retained_queries
        .lock()
        .expect("retained queries poisoned")
        .page(
            &cursor.token,
            RetainedKind::Album,
            cursor.offset,
            cursor.limit,
            Instant::now(),
            SystemTime::now(),
        );
    let Some(page) = page else {
        return cli_error(
            StatusCode::GONE,
            "cursor_expired",
            "Start a new Album query.",
            serde_json::json!({"cursorKind": "album", "reason": "idle_or_evicted"}),
        );
    };
    let next_offset = cursor.offset.saturating_add(page.ids.len());
    let next = (next_offset < page.total).then(|| {
        application.cursor_signer.query_cursor(
            application.browse_namespace,
            RetainedKind::Album,
            &cursor.token,
            next_offset,
            cursor.limit,
        )
    });
    let expires_at = next.as_ref().map(|_| page.expires_at);
    album_list_response(
        application,
        page.ids,
        page.total,
        page.evaluated_at,
        next,
        expires_at,
    )
    .await
}

async fn album_list_response(
    application: &Application,
    ids: Vec<String>,
    total: usize,
    evaluated_at: SystemTime,
    next_cursor: Option<String>,
    expires_at: Option<SystemTime>,
) -> Response<Body> {
    let facts = match application.library.albums_by_id(ids.clone()).await {
        Ok(facts) => facts,
        Err(error) => return map_query_error(error, "albums-list", "album", ""),
    };
    let items = ids
        .into_iter()
        .zip(facts)
        .map(|(id, summary)| match summary {
            Some(summary) => AlbumListItemWire::Present(CliAlbumSummaryWire::from(summary)),
            None => AlbumListItemWire::Missing(MissingItemWire {
                id,
                state: "missing",
            }),
        })
        .collect();
    json_response(
        StatusCode::OK,
        &CliListResponse {
            items,
            total,
            next_cursor,
            evaluated_at: format_time(evaluated_at),
            expires_at: expires_at.map(format_time),
        },
    )
}

pub(crate) async fn get_album_summary(
    State(state): State<HttpState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    if let Err(response) = require_cli_contract(&request) {
        return *response;
    }
    if let Err(response) = require_published(&state.application) {
        return *response;
    }
    if !valid_id(&id) {
        return invalid_cli("albumId", "The Album ID is invalid.");
    }
    match state.application.library.album(&id).await {
        Ok(Some(summary)) => json_response(StatusCode::OK, &CliAlbumSummaryWire::from(summary)),
        Ok(None) => cli_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "Query Albums and use a current Album ID.",
            serde_json::json!({"resource": "album", "reference": id}),
        ),
        Err(error) => map_query_error(error, "albums-get", "album", &id),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PhotoQueryBody {
    #[serde(default)]
    source: Option<PhotoQuerySourceBody>,
    #[serde(default)]
    selection: Option<String>,
    #[serde(default)]
    rating_minimum: Option<u8>,
    #[serde(default)]
    rating_maximum: Option<u8>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    available: Option<bool>,
    #[serde(default)]
    captured_from: Option<String>,
    #[serde(default)]
    captured_before: Option<String>,
    #[serde(default)]
    order: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum PhotoQuerySourceBody {
    All,
    Album { album_id: String },
    Folder { location: String },
}

pub(crate) async fn create_photo_query(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
    if let Err(response) = require_cli_contract(&request) {
        return *response;
    }
    if let Err(response) = require_published(&state.application) {
        return *response;
    }
    let body = match read_json_body(request).await {
        Ok(body) => body,
        Err(_) => return invalid_cli("body", "The request body must be one valid JSON object."),
    };
    let body = match serde_json::from_value::<PhotoQueryBody>(body) {
        Ok(body) => body,
        Err(_) => return invalid_cli("body", "The Photo query body is invalid."),
    };
    let limit = match body.limit {
        Some(limit) if (1..=MAXIMUM_LIST_PAGE).contains(&limit) => limit,
        None => DEFAULT_LIST_PAGE,
        Some(_) => return invalid_cli("limit", "The limit must be from 1 through 60."),
    };
    if body.rating_minimum.is_some_and(|rating| rating > 5)
        || body.rating_maximum.is_some_and(|rating| rating > 5)
        || body
            .rating_minimum
            .zip(body.rating_maximum)
            .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        return invalid_cli(
            "rating",
            "Rating bounds must be from 0 through 5 and ordered.",
        );
    }
    let selection = match body.selection.as_deref() {
        None | Some("all") => None,
        Some("undecided") => Some(SelectionState::Undecided),
        Some("selected") => Some(SelectionState::Selected),
        Some("rejected") => Some(SelectionState::Rejected),
        Some(_) => return invalid_cli("selection", "The Selection State filter is invalid."),
    };
    let original_kind = match body.kind.as_deref() {
        None => None,
        Some("raw") => Some(slipstream_core::OriginalKind::Raw),
        Some("jpeg") => Some(slipstream_core::OriginalKind::Jpeg),
        Some(_) => return invalid_cli("kind", "The Original kind filter is invalid."),
    };
    let captured_from = match body.captured_from {
        None => None,
        Some(value) => match slipstream_core::CaptureTimeBound::parse(value) {
            Ok(value) => Some(value),
            Err(_) => {
                return invalid_cli(
                    "capturedFrom",
                    "Use a valid camera-local YYYY-MM-DDTHH:MM:SS value.",
                );
            }
        },
    };
    let captured_before = match body.captured_before {
        None => None,
        Some(value) => match slipstream_core::CaptureTimeBound::parse(value) {
            Ok(value) => Some(value),
            Err(_) => {
                return invalid_cli(
                    "capturedBefore",
                    "Use a valid camera-local YYYY-MM-DDTHH:MM:SS value.",
                );
            }
        },
    };
    if captured_from
        .as_ref()
        .zip(captured_before.as_ref())
        .is_some_and(|(from, before)| from.as_str() >= before.as_str())
    {
        return invalid_cli(
            "capturedBefore",
            "The upper Capture Time bound must follow the lower bound.",
        );
    }
    let source = match body.source.unwrap_or(PhotoQuerySourceBody::All) {
        PhotoQuerySourceBody::All => slipstream_core::PhotoQuerySource::AllPhotos,
        PhotoQuerySourceBody::Album { album_id } if valid_id(&album_id) => {
            slipstream_core::PhotoQuerySource::Album(album_id)
        }
        PhotoQuerySourceBody::Folder { location } if valid_folder_location(&location) => {
            slipstream_core::PhotoQuerySource::Folder(location)
        }
        PhotoQuerySourceBody::Album { .. } => {
            return invalid_cli("albumId", "The Album ID is invalid.");
        }
        PhotoQuerySourceBody::Folder { .. } => {
            return invalid_cli("location", "The Original Folder Location is invalid.");
        }
    };
    let album_source = matches!(source, slipstream_core::PhotoQuerySource::Album(_));
    let order = match body.order.as_deref() {
        None if album_source => slipstream_core::PhotoQueryOrder::AlbumOrder,
        None => slipstream_core::PhotoQueryOrder::CaptureTimeAscending,
        Some("capture-time-asc") => slipstream_core::PhotoQueryOrder::CaptureTimeAscending,
        Some("capture-time-desc") => slipstream_core::PhotoQueryOrder::CaptureTimeDescending,
        Some("album-order") if album_source => slipstream_core::PhotoQueryOrder::AlbumOrder,
        Some(_) => return invalid_cli("order", "Album order is valid only for an Album source."),
    };
    let (missing_resource, missing_reference) = match &source {
        slipstream_core::PhotoQuerySource::Album(id) => ("album", id.clone()),
        slipstream_core::PhotoQuerySource::Folder(location) => ("folder", location.clone()),
        slipstream_core::PhotoQuerySource::AllPhotos => ("folder", String::new()),
    };
    let ids = match state
        .application
        .create_photo_query(
            slipstream_core::PhotoQuery {
                source,
                selection_state: selection,
                rating_minimum: body.rating_minimum,
                rating_maximum: body.rating_maximum,
                original_kind,
                original_available: body.available,
                captured_from,
                captured_before,
                order,
            },
            MAXIMUM_RETAINED_IDS,
        )
        .await
    {
        Ok(ids) => ids,
        Err(error) => {
            return map_query_error(error, "photos-list", missing_resource, &missing_reference);
        }
    };
    let evaluated_at = SystemTime::now();
    let total = ids.len();
    let token = query_token(&state.application, RetainedKind::Photo);
    let page_ids = ids.iter().take(limit).cloned().collect::<Vec<_>>();
    let next_cursor = if total > limit {
        if state
            .application
            .retained_queries
            .lock()
            .expect("retained queries poisoned")
            .insert(
                token.clone(),
                RetainedKind::Photo,
                ids,
                Instant::now(),
                evaluated_at,
            )
            .is_err()
        {
            return cli_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "server_busy",
                "Narrow the query before trying again.",
                serde_json::json!({"operation": "photos-list", "retryAfterSeconds": null}),
            );
        }
        Some(state.application.cursor_signer.query_cursor(
            state.application.browse_namespace,
            RetainedKind::Photo,
            &token,
            limit,
            limit,
        ))
    } else {
        None
    };
    let expires_at = next_cursor.as_ref().map(|_| evaluated_at + QUERY_IDLE);
    photo_list_response(
        &state.application,
        page_ids,
        total,
        evaluated_at,
        next_cursor,
        expires_at,
    )
    .await
}

pub(crate) async fn get_photo_query_page(
    State(state): State<HttpState>,
    axum::extract::Path(cursor): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    if let Err(response) = require_cli_contract(&request) {
        return *response;
    }
    let cursor = match state.application.cursor_signer.parse_query_cursor(
        &cursor,
        state.application.browse_namespace,
        RetainedKind::Photo,
    ) {
        Ok(cursor) => cursor,
        Err(error) => return query_cursor_error("photo", error),
    };
    let page = state
        .application
        .retained_queries
        .lock()
        .expect("retained queries poisoned")
        .page(
            &cursor.token,
            RetainedKind::Photo,
            cursor.offset,
            cursor.limit,
            Instant::now(),
            SystemTime::now(),
        );
    let Some(page) = page else {
        return cli_error(
            StatusCode::GONE,
            "cursor_expired",
            "Start a new Photo query.",
            serde_json::json!({"cursorKind": "photo", "reason": "idle_or_evicted"}),
        );
    };
    let next_offset = cursor.offset.saturating_add(page.ids.len());
    let next = (next_offset < page.total).then(|| {
        state.application.cursor_signer.query_cursor(
            state.application.browse_namespace,
            RetainedKind::Photo,
            &cursor.token,
            next_offset,
            cursor.limit,
        )
    });
    let expires_at = next.as_ref().map(|_| page.expires_at);
    photo_list_response(
        &state.application,
        page.ids,
        page.total,
        page.evaluated_at,
        next,
        expires_at,
    )
    .await
}

async fn photo_list_response(
    application: &Application,
    ids: Vec<String>,
    total: usize,
    evaluated_at: SystemTime,
    next_cursor: Option<String>,
    expires_at: Option<SystemTime>,
) -> Response<Body> {
    let facts = match application.published_photos_by_id(ids.clone()).await {
        Ok(facts) => facts,
        Err(error) => return map_query_error(error, "photos-list", "photo", ""),
    };
    let items = ids
        .into_iter()
        .zip(facts)
        .map(|(id, photo)| match photo {
            Some(photo) => PhotoListItemWire::Present(CliPhotoItemWire::from(photo)),
            None => PhotoListItemWire::Missing(MissingItemWire {
                id,
                state: "missing",
            }),
        })
        .collect();
    json_response(
        StatusCode::OK,
        &CliListResponse {
            items,
            total,
            next_cursor,
            evaluated_at: format_time(evaluated_at),
            expires_at: expires_at.map(format_time),
        },
    )
}

pub(crate) async fn get_photo(
    State(state): State<HttpState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    if let Err(response) = require_cli_contract(&request) {
        return *response;
    }
    if let Err(response) = require_published(&state.application) {
        return *response;
    }
    if !valid_id(&id) {
        return invalid_cli("photoId", "The Photo ID is invalid.");
    }
    let detail = match state.application.published_photo_detail(&id).await {
        Ok(Some(detail)) => detail,
        Ok(None) => {
            return cli_error(
                StatusCode::NOT_FOUND,
                "not_found",
                "Query Photos and use a current Photo ID.",
                serde_json::json!({"resource": "photo", "reference": id}),
            );
        }
        Err(error) => return map_query_error(error, "photos-get", "photo", &id),
    };
    let (photo, metadata) = state
        .application
        .inspect_published_photo_detail(detail)
        .await;
    let metadata_state = match photo.capture.state {
        slipstream_core::CaptureMetadataState::Pending => "pending",
        slipstream_core::CaptureMetadataState::Known => "known",
        slipstream_core::CaptureMetadataState::Missing => "missing",
        slipstream_core::CaptureMetadataState::Invalid => "invalid",
        slipstream_core::CaptureMetadataState::Failed => "failed",
    };
    let persisted_capture_time = photo.capture.order_key.clone();
    json_response(
        StatusCode::OK,
        &CliPhotoGetWire {
            photo: CliPhotoItemWire::from(photo),
            metadata: CliPhotoMetadataWire {
                state: metadata_state,
                capture_time: metadata.capture_time.or(persisted_capture_time),
                aperture: metadata.aperture,
                shutter_speed: metadata.shutter_speed,
                focal_length: metadata.focal_length,
                iso: metadata.iso,
            },
        },
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowseOpenBody {
    source: String,
    #[serde(default)]
    album_id: Option<String>,
    #[serde(default)]
    folder_path: Option<String>,
    #[serde(default)]
    publication: Option<String>,
    #[serde(default)]
    photo_id: Option<String>,
    #[serde(default)]
    order: Option<String>,
    #[serde(default)]
    selection: Option<String>,
}

pub(crate) async fn open_browse(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
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
        "library" if body.album_id.is_none() && body.folder_path.is_none() => {
            BrowseSourceRequest::Library
        }
        "album" if body.folder_path.is_none() => match body.album_id.filter(|id| valid_id(id)) {
            Some(id) => BrowseSourceRequest::Album(id),
            None => return api_error(StatusCode::BAD_REQUEST, "Invalid Album source"),
        },
        "folder" if body.album_id.is_none() => match (body.folder_path, body.publication) {
            (Some(location), Some(publication))
                if !publication.is_empty() && valid_folder_location(&location) =>
            {
                BrowseSourceRequest::Folder {
                    location,
                    publication,
                }
            }
            _ => return api_error(StatusCode::BAD_REQUEST, "Invalid Folder source"),
        },
        _ => return api_error(StatusCode::BAD_REQUEST, "Invalid browse source"),
    };
    let album_source = matches!(source, BrowseSourceRequest::Album(_));
    let order = match body.order.as_deref() {
        None => {
            if album_source {
                BrowseViewOrder::AlbumOrder
            } else {
                BrowseViewOrder::CaptureTimeAscending
            }
        }
        Some("album-order") => BrowseViewOrder::AlbumOrder,
        Some("capture-time-asc") => BrowseViewOrder::CaptureTimeAscending,
        Some("capture-time-desc") => BrowseViewOrder::CaptureTimeDescending,
        Some(_) => return api_error(StatusCode::BAD_REQUEST, "Invalid browse order"),
    };
    let selection = match body.selection.as_deref() {
        None | Some("all") => BrowseSelectionFilter::All,
        Some("undecided") => BrowseSelectionFilter::Undecided,
        Some("selected") => BrowseSelectionFilter::Selected,
        Some("rejected") => BrowseSelectionFilter::Rejected,
        Some(_) => return api_error(StatusCode::BAD_REQUEST, "Invalid browse selection"),
    };
    match state
        .application
        .browse_open(source, order, selection, preferred_photo_id.as_deref())
        .await
    {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub(crate) async fn get_browse_window(
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

pub(crate) async fn get_browse_position(
    State(state): State<HttpState>,
    axum::extract::Path(token): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    let Some(photo_id) =
        browse_position_query(request.uri().query()).and_then(|value| percent_decode(&value))
    else {
        return api_error(StatusCode::BAD_REQUEST, "Browse position is invalid");
    };
    if !valid_id(&photo_id) {
        return api_error(StatusCode::BAD_REQUEST, "Invalid Photo");
    }
    match state.application.browse_position(&token, &photo_id) {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub(crate) async fn get_file_locations(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
    if request.headers().contains_key(CLI_CONTRACT_HEADER) {
        return get_cli_folders(&state.application, request).await;
    }
    let Some((publication, parent, start, limit)) = file_locations_query(request.uri().query())
    else {
        return api_error(StatusCode::BAD_REQUEST, "File Location window is invalid");
    };
    let Some(parent) = percent_decode(&parent) else {
        return api_error(StatusCode::BAD_REQUEST, "Invalid Original Folder");
    };
    match state
        .application
        .file_locations(publication.as_deref(), &parent, start, limit)
        .await
    {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(error) => ApiError::from(error).into_response(),
    }
}

async fn get_cli_folders(application: &Application, request: Request<Body>) -> Response<Body> {
    if let Err(response) = require_cli_contract(&request) {
        return *response;
    }
    if let Err(response) = require_published(application) {
        return *response;
    }
    let pairs = match cli_query_pairs(request.uri().query()) {
        Ok(pairs) => pairs,
        Err(response) => return *response,
    };
    let (publication, parent, start, limit) = if let Some(cursor) = pairs
        .iter()
        .find(|(name, _)| name == "cursor")
        .map(|(_, value)| value)
    {
        if pairs.len() != 1 {
            return invalid_cli("cursor", "A continuation cannot include a parent or limit.");
        }
        match application
            .cursor_signer
            .parse_folder_cursor(cursor, application.browse_namespace)
        {
            Ok(cursor) => (
                Some(cursor.publication),
                cursor.parent,
                cursor.offset,
                cursor.limit,
            ),
            Err(error) => return query_cursor_error("folder", error),
        }
    } else {
        if pairs
            .iter()
            .any(|(name, _)| !matches!(name.as_str(), "parent" | "limit"))
        {
            return invalid_cli("query", "The Folder query contains an unknown parameter.");
        }
        let parent = pairs
            .iter()
            .find(|(name, _)| name == "parent")
            .map_or_else(String::new, |(_, value)| value.clone());
        if !valid_folder_location(&parent) {
            return invalid_cli("parent", "The Original Folder Location is invalid.");
        }
        let limit = match list_limit(
            pairs
                .iter()
                .find(|(name, _)| name == "limit")
                .map(|(_, value)| value.as_str()),
        ) {
            Ok(limit) => limit,
            Err(response) => return *response,
        };
        (None, parent, 0, limit)
    };
    let result = match application
        .file_locations(publication.as_deref(), &parent, start, limit)
        .await
    {
        Ok(result) => result,
        Err(ServerError::FileLocationsExpired) => {
            return cli_error(
                StatusCode::GONE,
                "cursor_expired",
                "List Folders again from the current Published Library.",
                serde_json::json!({"cursorKind": "folder", "reason": "publication_replaced"}),
            );
        }
        Err(ServerError::FolderNotFound) => {
            return cli_error(
                StatusCode::NOT_FOUND,
                "not_found",
                "List the parent Folder and use a current Location.",
                serde_json::json!({"resource": "folder", "reference": parent}),
            );
        }
        Err(ServerError::FolderInvalid | ServerError::FileLocationWindow) => {
            return invalid_cli("parent", "The Folder request is invalid.");
        }
        Err(_) => {
            return cli_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_failed",
                "Inspect server health and try again.",
                serde_json::json!({"operation": "folders-list"}),
            );
        }
    };
    let next_offset = start.saturating_add(result.children.len());
    let next_cursor = (next_offset < result.total).then(|| {
        application.cursor_signer.folder_cursor(
            application.browse_namespace,
            &result.publication,
            &result.parent,
            next_offset,
            limit,
        )
    });
    let evaluated_at = application
        .publication_evaluated_at(&result.publication)
        .unwrap_or_else(SystemTime::now);
    json_response(
        StatusCode::OK,
        &CliFolderListResponse {
            items: result.children,
            total: result.total,
            next_cursor,
            evaluated_at: format_time(evaluated_at),
            expires_at: None,
            publication: result.publication,
            parent: result.parent,
        },
    )
}

pub(crate) async fn close_browse(
    State(state): State<HttpState>,
    axum::extract::Path(token): axum::extract::Path<String>,
    _request: Request<Body>,
) -> Response<Body> {
    state.application.browse_close(&token);
    Response::builder()
        .status(StatusCode::NO_CONTENT)
        .body(Body::empty())
        .expect("valid response")
}

pub(crate) fn file_locations_query(
    query: Option<&str>,
) -> Option<(Option<String>, String, usize, usize)> {
    let mut publication = None;
    let mut parent = String::new();
    let mut start = None;
    let mut limit = None;
    for part in query?.split('&') {
        let (key, value) = part.split_once('=')?;
        match key {
            "publication" if !value.is_empty() => publication = Some(value.to_owned()),
            "parent" if !value.is_empty() => parent = value.to_owned(),
            "start" => start = value.parse().ok(),
            "limit" => limit = value.parse().ok(),
            _ => {}
        }
    }
    Some((publication, parent, start?, limit?))
}

/// Decodes one query value as UTF-8 using `application/x-www-form-urlencoded`
/// semantics: `+` means space and `%HH` escapes decode to bytes.
pub(crate) fn percent_decode(value: &str) -> Option<String> {
    let bytes: Vec<u8> = value
        .bytes()
        .map(|byte| if byte == b'+' { b' ' } else { byte })
        .collect();
    let bytes = bytes.as_slice();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                if index + 2 > bytes.len() {
                    return None;
                }
                let hex = bytes.get(index + 1..index + 3)?;
                let high = (hex[0] as char).to_digit(16)?;
                let low = (hex[1] as char).to_digit(16)?;
                decoded.push((high * 16 + low) as u8);
                index += 3;
            }
            byte => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(decoded).ok()
}

pub(crate) fn browse_query(query: Option<&str>) -> Option<(usize, usize)> {
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

pub(crate) fn browse_position_query(query: Option<&str>) -> Option<String> {
    let mut photo_id = None;
    for part in query?.split('&') {
        let (key, value) = part.split_once('=')?;
        match key {
            "photoId" if photo_id.is_none() && !value.is_empty() => {
                photo_id = Some(value.to_owned())
            }
            _ => {}
        }
    }
    photo_id
}

pub(crate) async fn retired_album_list() -> Response<Body> {
    api_error(StatusCode::NOT_FOUND, "Not found")
}

pub(crate) async fn method_not_allowed() -> Response<Body> {
    api_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed")
}

pub(crate) async fn scan(State(state): State<HttpState>, request: Request<Body>) -> Response<Body> {
    let cli_request = request.headers().contains_key(CLI_CONTRACT_HEADER);
    if cli_request && let Err(response) = require_cli_contract(&request) {
        return *response;
    }
    match state.application.rescan().await {
        Ok(response) if cli_request => {
            json_response(StatusCode::OK, &CliScanStatusWire::from(response))
        }
        Ok(response) => json_response(StatusCode::OK, &response),
        Err(_error) if cli_request => cli_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "library_unavailable",
            "Inspect the returned scan status before trying another Library check.",
            serde_json::json!({
                "scan": CliScanStatusWire::from(state.application.scan_status())
            }),
        ),
        Err(error) => ApiError::from(error).into_response(),
    }
}

/// Lists every unavailable Photo with its remembered folder, filename,
/// format, and retained decisions for the bounded recovery review entry.
pub(crate) async fn recovery_unavailable(
    State(state): State<HttpState>,
    _request: Request<Body>,
) -> Response<Body> {
    match state.application.recovery_survey().await {
        Ok(response) => json_response(StatusCode::OK, &response),
        Err(error) => ApiError::from(error).into_response(),
    }
}

/// Proposes a batch of folder-prefix relocations, or one mapping for a
/// single Original. Proposals are inspectable and never write.
pub(crate) async fn recovery_propose(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
    let body = match read_json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(body) = body.as_object() else {
        return api_error(StatusCode::BAD_REQUEST, "Invalid JSON body");
    };
    let valid_prefix = |value: &Value| {
        value
            .as_str()
            .is_some_and(|text| slipstream_core::parse_location_prefix(text).is_ok())
    };
    if has_exact_keys(body, &["oldPrefix", "newPrefix"]) {
        let (Some(old_prefix), Some(new_prefix)) =
            (body["oldPrefix"].as_str(), body["newPrefix"].as_str())
        else {
            return api_error(StatusCode::BAD_REQUEST, "Invalid recovery prefix");
        };
        if !valid_prefix(&body["oldPrefix"]) || !valid_prefix(&body["newPrefix"]) {
            return api_error(StatusCode::BAD_REQUEST, "Invalid recovery prefix");
        }
        return match state
            .application
            .recovery_propose_batch(old_prefix, new_prefix)
            .await
        {
            Ok(proposals) => json_response(
                StatusCode::OK,
                &serde_json::json!({ "proposals": proposals }),
            ),
            Err(error) => ApiError::from(error).into_response(),
        };
    }
    if has_exact_keys(body, &["originalId", "newLocation"]) {
        let (Some(original_id), Some(new_location)) =
            (body["originalId"].as_str(), body["newLocation"].as_str())
        else {
            return api_error(StatusCode::BAD_REQUEST, "Invalid recovery request");
        };
        if !valid_id(original_id)
            || slipstream_core::RelativeOriginalPath::parse(new_location.to_owned()).is_err()
        {
            return api_error(StatusCode::BAD_REQUEST, "Invalid recovery request");
        }
        return match state
            .application
            .recovery_propose_single(original_id, new_location)
            .await
        {
            Ok(proposal) => json_response(StatusCode::OK, &proposal),
            Err(error) => ApiError::from(error).into_response(),
        };
    }
    api_error(StatusCode::BAD_REQUEST, "Invalid recovery request")
}

/// Commits one confirmed manual relocation batch. The whole batch commits
/// atomically or is refused with per-mapping reasons.
pub(crate) async fn recovery_apply(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
    let body = match read_json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(object) = body.as_object() else {
        return api_error(StatusCode::BAD_REQUEST, "Invalid JSON body");
    };
    if !has_exact_keys(object, &["relocations"]) {
        return api_error(StatusCode::BAD_REQUEST, "Invalid recovery request");
    }
    let Some(items) = object["relocations"].as_array() else {
        return api_error(StatusCode::BAD_REQUEST, "Invalid recovery request");
    };
    if items.is_empty() || items.len() > MAXIMUM_RECOVERY_RELOCATIONS {
        return api_error(StatusCode::BAD_REQUEST, "Invalid recovery request");
    }
    let mut parsed = Vec::with_capacity(items.len());
    for item in items {
        let Some(item) = item.as_object() else {
            return api_error(StatusCode::BAD_REQUEST, "Invalid recovery request");
        };
        let valid_item = has_exact_keys(item, &["originalId", "newLocation"])
            || has_exact_keys(item, &["originalId", "newLocation", "retireDestination"]);
        if !valid_item {
            return api_error(StatusCode::BAD_REQUEST, "Invalid recovery request");
        }
        let (Some(original_id), Some(new_location)) =
            (item["originalId"].as_str(), item["newLocation"].as_str())
        else {
            return api_error(StatusCode::BAD_REQUEST, "Invalid recovery request");
        };
        let retire_destination = match item.get("retireDestination") {
            None => false,
            Some(value) => value.as_bool().unwrap_or(false),
        };
        if !valid_id(original_id)
            || slipstream_core::RelativeOriginalPath::parse(new_location.to_owned()).is_err()
        {
            return api_error(StatusCode::BAD_REQUEST, "Invalid recovery request");
        }
        parsed.push(crate::app::RecoveryApplyItem {
            original_id: original_id.to_owned(),
            new_location: new_location.to_owned(),
            retire_destination,
        });
    }
    match state.application.recovery_apply(parsed).await {
        Ok(response) => json_response(StatusCode::OK, &response),
        Err(crate::app::RecoveryApplyError::Invalid) => {
            api_error(StatusCode::BAD_REQUEST, "Invalid recovery request")
        }
        Err(crate::app::RecoveryApplyError::Rejected {
            message,
            rejections,
        }) => json_response(
            StatusCode::CONFLICT,
            &RecoveryRejectionResponseWire {
                message,
                rejections,
            },
        ),
        Err(crate::app::RecoveryApplyError::Server(error)) => ApiError::from(error).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CliAlbumCreateBody {
    name: String,
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "kebab-case", deny_unknown_fields)]
enum CliAlbumChangeBody {
    Rename {
        name: String,
        #[serde(rename = "ifVersion")]
        if_version: String,
    },
    Delete {
        #[serde(rename = "ifVersion")]
        if_version: String,
    },
    Add {
        #[serde(rename = "photoIds")]
        photo_ids: Vec<String>,
        #[serde(rename = "ifVersion")]
        if_version: String,
    },
    Remove {
        #[serde(rename = "photoIds")]
        photo_ids: Vec<String>,
        #[serde(rename = "ifVersion")]
        if_version: String,
    },
    Reorder {
        #[serde(rename = "photoIds")]
        photo_ids: Vec<String>,
        #[serde(rename = "ifVersion")]
        if_version: String,
    },
}

impl CliAlbumChangeBody {
    fn operation(&self) -> &'static str {
        match self {
            Self::Rename { .. } => "albums-rename",
            Self::Delete { .. } => "albums-delete",
            Self::Add { .. } => "albums-add",
            Self::Remove { .. } => "albums-remove",
            Self::Reorder { .. } => "albums-reorder",
        }
    }

    fn limit_name(&self) -> &'static str {
        match self {
            Self::Reorder { .. } => "albumReorderMembersMaximum",
            _ => "mutationPhotoIdsMaximum",
        }
    }

    fn into_mutation(
        self,
        album_id: String,
    ) -> CliBoundaryResult<slipstream_core::CheckedAlbumMutation> {
        match self {
            Self::Rename { name, if_version } => {
                let Some(name) = valid_name(Some(&Value::String(name))) else {
                    return Err(Box::new(invalid_cli("name", "The Album name is invalid.")));
                };
                if if_version.is_empty() {
                    return Err(Box::new(invalid_cli(
                        "ifVersion",
                        "The Album version is required.",
                    )));
                }
                Ok(slipstream_core::CheckedAlbumMutation::Rename {
                    album_id,
                    name,
                    expected_version: if_version,
                })
            }
            Self::Delete { if_version } => {
                if if_version.is_empty() {
                    return Err(Box::new(invalid_cli(
                        "ifVersion",
                        "The Album version is required.",
                    )));
                }
                Ok(slipstream_core::CheckedAlbumMutation::Delete {
                    album_id,
                    expected_version: if_version,
                })
            }
            Self::Add {
                photo_ids,
                if_version,
            } => checked_membership_mutation(album_id, photo_ids, if_version, false, false),
            Self::Remove {
                photo_ids,
                if_version,
            } => checked_membership_mutation(album_id, photo_ids, if_version, true, false),
            Self::Reorder {
                photo_ids,
                if_version,
            } => checked_membership_mutation(album_id, photo_ids, if_version, false, true),
        }
    }
}

fn checked_membership_mutation(
    album_id: String,
    photo_ids: Vec<String>,
    if_version: String,
    remove: bool,
    reorder: bool,
) -> CliBoundaryResult<slipstream_core::CheckedAlbumMutation> {
    if photo_ids.len() > ALBUM_PHOTO_IDS_MAX {
        return Err(Box::new(cli_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "limit_exceeded",
            "Reduce the Photo ID list and try again.",
            serde_json::json!({
                "limitName": if reorder {
                    "albumReorderMembersMaximum"
                } else {
                    "mutationPhotoIdsMaximum"
                },
                "limit": ALBUM_PHOTO_IDS_MAX,
                "actual": photo_ids.len()
            }),
        )));
    }
    if photo_ids.is_empty()
        || photo_ids.iter().any(|photo_id| !valid_id(photo_id))
        || photo_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != photo_ids.len()
    {
        return Err(Box::new(invalid_cli(
            "photoIds",
            "Photo IDs must be a nonempty ordered list of distinct valid IDs.",
        )));
    }
    if if_version.is_empty() {
        return Err(Box::new(invalid_cli(
            "ifVersion",
            "The Album version is required.",
        )));
    }
    if reorder {
        Ok(slipstream_core::CheckedAlbumMutation::Reorder {
            album_id,
            photo_ids,
            expected_version: if_version,
        })
    } else if remove {
        Ok(slipstream_core::CheckedAlbumMutation::RemoveMembers {
            album_id,
            photo_ids,
            expected_version: if_version,
        })
    } else {
        Ok(slipstream_core::CheckedAlbumMutation::AddMembers {
            album_id,
            photo_ids,
            expected_version: if_version,
        })
    }
}

pub(crate) async fn create_album(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
    if request.headers().contains_key(CLI_CONTRACT_HEADER) {
        if let Err(response) = require_cli_contract(&request) {
            return *response;
        }
        let body: CliAlbumCreateBody = match read_cli_json_body(request).await {
            Ok(body) => body,
            Err(response) => return response,
        };
        let Some(name) = valid_name(Some(&Value::String(body.name))) else {
            return invalid_cli("name", "The Album name is invalid.");
        };
        return match state.application.create_album_checked(name).await {
            Ok(result) => json_response(
                StatusCode::OK,
                &CliAlbumCreationWire {
                    album: result.album.into(),
                },
            ),
            Err(error) => map_album_write_error(error, "albums-create", "mutationPhotoIdsMaximum"),
        };
    }
    mutate_album_route(&state, request, |body| {
        valid_name(body.get("name"))
            .map(|name| slipstream_core::AlbumMutation::Create { name })
            .ok_or("Invalid Album name")
    })
    .await
}

pub(crate) async fn change_album(
    State(state): State<HttpState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    if let Err(response) = require_cli_contract(&request) {
        return *response;
    }
    if !valid_id(&id) {
        return invalid_cli("albumId", "The Album ID is invalid.");
    }
    let body: CliAlbumChangeBody = match read_cli_json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let operation = body.operation();
    let limit_name = body.limit_name();
    let mutation = match body.into_mutation(id) {
        Ok(mutation) => mutation,
        Err(response) => return *response,
    };
    match state.application.mutate_album_checked(mutation).await {
        Ok(result) => json_response(StatusCode::OK, &CliAlbumChangeWire::from(result)),
        Err(error) => map_album_write_error(error, operation, limit_name),
    }
}

fn map_album_write_error(
    error: LibraryError,
    operation: &'static str,
    limit_name: &'static str,
) -> Response<Body> {
    match error {
        LibraryError::AlbumWrite(slipstream_core::AlbumWriteError::Invalid) => {
            invalid_cli("body", "The Album change is invalid.")
        }
        LibraryError::AlbumWrite(slipstream_core::AlbumWriteError::AlbumNotFound { album_id }) => {
            cli_error(
                StatusCode::NOT_FOUND,
                "not_found",
                "Query Albums and use a current Album ID.",
                serde_json::json!({"resource": "album", "reference": album_id}),
            )
        }
        LibraryError::AlbumWrite(slipstream_core::AlbumWriteError::PhotoNotFound { photo_id }) => {
            cli_error(
                StatusCode::NOT_FOUND,
                "not_found",
                "Query Photos and submit only current Photo IDs.",
                serde_json::json!({"resource": "photo", "reference": photo_id}),
            )
        }
        LibraryError::AlbumWrite(
            slipstream_core::AlbumWriteError::VersionConflict {
                album_id,
                current_version,
            }
            | slipstream_core::AlbumWriteError::MembershipConflict {
                album_id,
                current_version,
            },
        ) => cli_error(
            StatusCode::CONFLICT,
            "conflict",
            "Read the current Album and decide whether to submit a new change.",
            serde_json::json!({
                "resource": "album",
                "reference": album_id,
                "currentVersion": current_version
            }),
        ),
        LibraryError::AlbumWrite(slipstream_core::AlbumWriteError::NameConflict {
            name,
            album_id,
        }) => cli_error(
            StatusCode::CONFLICT,
            "name_conflict",
            "Inspect the existing Album before choosing a different name.",
            serde_json::json!({"name": name, "albumId": album_id}),
        ),
        LibraryError::AlbumWrite(slipstream_core::AlbumWriteError::LimitExceeded {
            limit,
            actual,
        }) => cli_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "limit_exceeded",
            "Reduce the Photo ID list and try again.",
            serde_json::json!({"limitName": limit_name, "limit": limit, "actual": actual}),
        ),
        _ => cli_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_failed",
            "Inspect server health and the current Album before trying again.",
            serde_json::json!({"operation": operation}),
        ),
    }
}

pub(crate) async fn rename_album(
    State(state): State<HttpState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    mutate_album_route(&state, request, move |body| {
        if !valid_id(&id) {
            return Err("Invalid Album mutation");
        }
        valid_name(body.get("name"))
            .map(|name| slipstream_core::AlbumMutation::Rename {
                album_id: id.clone(),
                name,
            })
            .ok_or("Invalid Album mutation")
    })
    .await
}

pub(crate) async fn delete_album(
    State(state): State<HttpState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    _request: Request<Body>,
) -> Response<Body> {
    if !valid_id(&id) {
        return api_error(StatusCode::BAD_REQUEST, "Invalid Album");
    }
    match state
        .application
        .mutate_album(slipstream_core::AlbumMutation::Delete { album_id: id })
        .await
    {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub(crate) async fn add_album_members(
    State(state): State<HttpState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    let body = match read_json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(body) = body.as_object() else {
        return api_error(StatusCode::BAD_REQUEST, "Invalid membership batch");
    };
    if !valid_id(&id) {
        return api_error(StatusCode::BAD_REQUEST, "Invalid membership batch");
    }
    let Some(photo_ids) = valid_ids(body.get("photoIds"), ALBUM_PHOTO_IDS_MAX) else {
        return api_error(StatusCode::BAD_REQUEST, "Invalid membership batch");
    };
    match state.application.add_album_members(&id, photo_ids).await {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub(crate) async fn remove_added_album_members(
    State(state): State<HttpState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    let body = match read_json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(body) = body.as_object() else {
        return api_error(StatusCode::BAD_REQUEST, "Invalid membership compensation");
    };
    if !valid_id(&id) {
        return api_error(StatusCode::BAD_REQUEST, "Invalid membership compensation");
    }
    let Some(photo_ids) = valid_ids(body.get("photoIds"), ALBUM_PHOTO_IDS_MAX) else {
        return api_error(StatusCode::BAD_REQUEST, "Invalid membership compensation");
    };
    match state
        .application
        .remove_added_album_members(&id, photo_ids)
        .await
    {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub(crate) async fn add_folder_members(
    State(state): State<HttpState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    let body = match read_json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(body) = body.as_object() else {
        return api_error(StatusCode::BAD_REQUEST, "Invalid Folder Album body");
    };
    let Some(folder_path) = body.get("folderPath").and_then(Value::as_str) else {
        return api_error(StatusCode::BAD_REQUEST, "Invalid Folder Album body");
    };
    let Some(publication) = body.get("publication").and_then(Value::as_str) else {
        return api_error(StatusCode::BAD_REQUEST, "Invalid Folder Album body");
    };
    if !valid_id(&id) || publication.is_empty() || !valid_folder_location(folder_path) {
        return api_error(StatusCode::BAD_REQUEST, "Invalid Folder Album body");
    }
    match state
        .application
        .add_folder_to_album(&id, folder_path, publication)
        .await
    {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub(crate) async fn remove_album_member(
    State(state): State<HttpState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    mutate_album_route(&state, request, move |body| {
        let photo_id = body.get("photoId").and_then(Value::as_str);
        if !valid_id(&id) || photo_id.is_none_or(|value| !valid_id(value)) {
            return Err("Invalid Album member");
        }
        Ok(slipstream_core::AlbumMutation::RemoveMember {
            album_id: id.clone(),
            photo_id: photo_id.unwrap().to_owned(),
        })
    })
    .await
}

pub(crate) async fn reorder_album(
    State(state): State<HttpState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    mutate_album_route(&state, request, move |body| {
        if !valid_id(&id) {
            return Err("Invalid Album order");
        }
        valid_ids(body.get("photoIds"), ALBUM_PHOTO_IDS_MAX)
            .map(|photo_ids| slipstream_core::AlbumMutation::Reorder {
                album_id: id.clone(),
                photo_ids,
            })
            .ok_or("Invalid Album order")
    })
    .await
}

pub(crate) async fn set_progress(
    State(state): State<HttpState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    mutate_album_route(&state, request, move |body| {
        let photo_id = body.get("photoId").and_then(Value::as_str);
        if !valid_id(&id) || photo_id.is_none_or(|value| !valid_id(value)) {
            return Err("Invalid review progress");
        }
        Ok(slipstream_core::AlbumMutation::SetProgress {
            album_id: id.clone(),
            photo_id: photo_id.unwrap().to_owned(),
        })
    })
    .await
}

pub(crate) async fn mutate_photo_state(
    State(state): State<HttpState>,
    axum::extract::Path(photo_id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
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
    let album_id = match body.get("albumId") {
        None => Ok(None),
        Some(value) => value
            .as_str()
            .filter(|id| valid_id(id))
            .map(str::to_owned)
            .map(Some)
            .ok_or(()),
    };
    let Ok(album_id) = album_id else {
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
            album_id,
        })
        .await;
    match result {
        Ok(result) => json_response(StatusCode::OK, &photo_state_wire(&result)),
        Err(error) => ApiError::from(error).into_response(),
    }
}

/// The largest number of Photo ids one bounded Album membership write accepts
/// at the HTTP boundary. The Library enforces the same bound for the album
/// mutations it applies.
const ALBUM_PHOTO_IDS_MAX: usize = 100;

/// One bounded batch Selection State write from the Grid's multi-selection.
/// Each item carries the Selection State the browser last confirmed for that
/// Photo. The response reports one outcome per item so the browser moves only
/// confirmed facts and counts.
pub(crate) async fn mutate_photo_state_batch(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
    let body = match read_json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(body) = body.as_object() else {
        return api_error(StatusCode::BAD_REQUEST, "Invalid Photo state batch");
    };
    if !has_exact_keys(body, &["selectionState", "photos"]) {
        return api_error(StatusCode::BAD_REQUEST, "Invalid Photo state batch");
    }
    let Some(value) = body.get("selectionState").and_then(valid_batch_selection) else {
        return api_error(StatusCode::BAD_REQUEST, "Invalid Photo state batch");
    };
    let Some(items) = body.get("photos").and_then(Value::as_array) else {
        return api_error(StatusCode::BAD_REQUEST, "Invalid Photo state batch");
    };
    if items.is_empty() || items.len() > slipstream_core::PHOTO_STATE_BATCH_MAX {
        return api_error(StatusCode::BAD_REQUEST, "Invalid Photo state batch");
    }
    let mut photos = Vec::with_capacity(items.len());
    for item in items {
        let Some(item) = item.as_object() else {
            return api_error(StatusCode::BAD_REQUEST, "Invalid Photo state batch");
        };
        if !has_exact_keys(item, &["photoId", "expectedCurrent"]) {
            return api_error(StatusCode::BAD_REQUEST, "Invalid Photo state batch");
        }
        let Some(photo_id) = item
            .get("photoId")
            .and_then(Value::as_str)
            .filter(|photo_id| valid_id(photo_id))
        else {
            return api_error(StatusCode::BAD_REQUEST, "Invalid Photo state batch");
        };
        let Some(expected_current) = item.get("expectedCurrent").and_then(valid_selection) else {
            return api_error(StatusCode::BAD_REQUEST, "Invalid Photo state batch");
        };
        photos.push(slipstream_core::PhotoStateBatchItem {
            photo_id: photo_id.to_owned(),
            expected_current,
        });
    }
    if photos
        .iter()
        .map(|photo| &photo.photo_id)
        .collect::<std::collections::BTreeSet<_>>()
        .len()
        != photos.len()
    {
        return api_error(StatusCode::BAD_REQUEST, "Invalid Photo state batch");
    }
    let result = state
        .application
        .mutate_photo_state_batch(slipstream_core::PhotoStateBatchMutation { photos, value })
        .await;
    match result {
        Ok(result) => json_response(StatusCode::OK, &photo_state_batch_wire(&result)),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub(crate) fn photo_state_batch_wire(result: &slipstream_core::PhotoStateBatchResult) -> Value {
    serde_json::json!({
        "applied": result
            .applied
            .iter()
            .map(|entry| serde_json::json!({
                "photoId": entry.photo_id,
                "priorValue": selection_state(entry.prior_value),
            }))
            .collect::<Vec<_>>(),
        "changedElsewhere": result
            .changed_elsewhere
            .iter()
            .map(|entry| serde_json::json!({
                "photoId": entry.photo_id,
                "currentValue": selection_state(entry.current_value),
            }))
            .collect::<Vec<_>>(),
        "missing": result
            .missing
            .iter()
            .map(|entry| serde_json::json!({ "photoId": entry.photo_id }))
            .collect::<Vec<_>>(),
    })
}

pub(crate) fn photo_state_wire(result: &slipstream_core::PhotoStateMutationResult) -> Value {
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

pub(crate) fn selection_value(value: slipstream_core::PhotoStateValue) -> Value {
    match value {
        slipstream_core::PhotoStateValue::Selection(value) => {
            Value::String(selection_state(value).to_owned())
        }
        slipstream_core::PhotoStateValue::Rating(_) => Value::Null,
    }
}

pub(crate) fn rating_value(value: slipstream_core::PhotoStateValue) -> Value {
    match value {
        slipstream_core::PhotoStateValue::Rating(value) => Value::from(value),
        slipstream_core::PhotoStateValue::Selection(_) => Value::Null,
    }
}

pub(crate) async fn get_thumbnail(
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

pub(crate) async fn get_photo_metadata(
    State(state): State<HttpState>,
    axum::extract::Path(photo_id): axum::extract::Path<String>,
) -> Response<Body> {
    if !valid_id(&photo_id) {
        return api_error(StatusCode::BAD_REQUEST, "Invalid Photo");
    }
    match state.application.photo_metadata(&photo_id).await {
        Ok(metadata) => json_response(StatusCode::OK, &PhotoMetadataWire::from(metadata)),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub(crate) async fn get_photo_albums(
    State(state): State<HttpState>,
    axum::extract::Path(photo_id): axum::extract::Path<String>,
) -> Response<Body> {
    if !valid_id(&photo_id) {
        return api_error(StatusCode::BAD_REQUEST, "Invalid Photo");
    }
    match state.application.photo_albums(&photo_id).await {
        Ok(albums) => json_response(StatusCode::OK, &albums),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub(crate) async fn get_preview(
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

pub(crate) async fn get_derivative(
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
    let body = if request.method() == ::http::Method::HEAD {
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

pub(crate) async fn mutate_album_route(
    state: &HttpState,
    request: Request<Body>,
    build: impl FnOnce(
        &serde_json::Map<String, Value>,
    ) -> Result<slipstream_core::AlbumMutation, &'static str>,
) -> Response<Body> {
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
    match state.application.mutate_album(mutation).await {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub(crate) async fn read_json_body(request: Request<Body>) -> Result<Value, Response<Body>> {
    let bytes = read_body_bytes(request).await?;
    serde_json::from_slice(&bytes)
        .map_err(|_| api_error(StatusCode::BAD_REQUEST, "Invalid JSON body"))
}

async fn read_cli_json_body<T: serde::de::DeserializeOwned>(
    request: Request<Body>,
) -> Result<T, Response<Body>> {
    let declared_length = match request.headers().get(header::CONTENT_LENGTH) {
        None => None,
        Some(length) => {
            let Some(length) = length
                .to_str()
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
            else {
                return Err(invalid_cli(
                    "body",
                    "The request Content-Length is invalid.",
                ));
            };
            Some(length)
        }
    };
    if let Some(actual) = declared_length.filter(|length| *length > MAXIMUM_MUTATION_BODY_BYTES) {
        return Err(cli_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "limit_exceeded",
            "Reduce the request body and try again.",
            serde_json::json!({
                "limitName": "requestBodyBytesMaximum",
                "limit": MAXIMUM_MUTATION_BODY_BYTES,
                "actual": actual
            }),
        ));
    }
    let bytes = read_body_bytes(request).await.map_err(|response| {
        if response.status() == StatusCode::PAYLOAD_TOO_LARGE {
            cli_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "limit_exceeded",
                "Reduce the request body and try again.",
                serde_json::json!({
                    "limitName": "requestBodyBytesMaximum",
                    "limit": MAXIMUM_MUTATION_BODY_BYTES,
                    "actual": MAXIMUM_MUTATION_BODY_BYTES + 1
                }),
            )
        } else {
            invalid_cli("body", "The request body is invalid.")
        }
    })?;
    serde_json::from_slice(&bytes).map_err(|_| {
        invalid_cli(
            "body",
            "The JSON body is malformed or contains unknown or duplicate keys.",
        )
    })
}

async fn read_body_bytes(request: Request<Body>) -> Result<Vec<u8>, Response<Body>> {
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
    to_bytes(body, MAXIMUM_MUTATION_BODY_BYTES)
        .await
        .map(|bytes| bytes.to_vec())
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
        })
}

pub(crate) fn json_response<T: Serialize>(status: StatusCode, value: &T) -> Response<Body> {
    let body = serde_json::to_vec(value).expect("response serializes");
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONTENT_LENGTH, body.len())
        .body(Body::from(body))
        .expect("valid JSON response")
}

fn has_exact_keys(object: &serde_json::Map<String, Value>, expected: &[&str]) -> bool {
    object.len() == expected.len() && expected.iter().all(|key| object.contains_key(*key))
}

pub(crate) fn valid_id(value: &str) -> bool {
    value.len() >= 36
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte) || byte == b'-')
}

pub(crate) fn is_hex_key(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) fn valid_name(value: Option<&Value>) -> Option<String> {
    let value = value?.as_str()?.trim();
    (!value.is_empty() && value.chars().count() <= 120).then(|| value.to_owned())
}

pub(crate) fn valid_ids(value: Option<&Value>, max_ids: usize) -> Option<Vec<String>> {
    let values = value?.as_array()?;
    if values.len() > max_ids {
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

pub(crate) fn valid_selection(value: &Value) -> Option<SelectionState> {
    match value.as_str()? {
        "undecided" => Some(SelectionState::Undecided),
        "selected" => Some(SelectionState::Selected),
        "rejected" => Some(SelectionState::Rejected),
        _ => None,
    }
}

fn valid_batch_selection(value: &Value) -> Option<SelectionState> {
    match valid_selection(value) {
        Some(SelectionState::Selected) => Some(SelectionState::Selected),
        Some(SelectionState::Rejected) => Some(SelectionState::Rejected),
        _ => None,
    }
}

pub(crate) fn valid_rating(value: &Value) -> Option<u8> {
    let rating = value.as_u64()?;
    (rating <= 5).then_some(rating as u8)
}

pub(crate) async fn request_policy(
    State(_state): State<HttpState>,
    request: Request<Body>,
    next: Next,
) -> Response<Body> {
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
    if request.headers().contains_key(CLI_CONTRACT_HEADER)
        && let Err(response) = require_cli_contract(&request)
    {
        return *response;
    }
    if !matches!(
        request.method().as_str(),
        "GET" | "HEAD" | "POST" | "DELETE"
    ) {
        return api_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed");
    }
    let mutation = matches!(request.method().as_str(), "POST" | "DELETE");
    if mutation {
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
pub(crate) struct ApiError {
    status: StatusCode,
    message: &'static str,
}

impl From<ServerError> for ApiError {
    fn from(error: ServerError) -> Self {
        if let ServerError::Library(LibraryError::Mutation(error)) = error {
            return match error {
                slipstream_core::MutationError::Invalid => Self {
                    status: StatusCode::BAD_REQUEST,
                    message: "Invalid mutation request",
                },
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
            ServerError::PhotoNotFound => Self {
                status: StatusCode::NOT_FOUND,
                message: "Photo not found",
            },
            ServerError::BrowseLimit => Self {
                status: StatusCode::BAD_REQUEST,
                message: "Browse window is invalid",
            },
            ServerError::BrowseOrder => Self {
                status: StatusCode::BAD_REQUEST,
                message: "Invalid browse order",
            },
            ServerError::FileLocationsExpired => Self {
                status: StatusCode::CONFLICT,
                message: "File Locations expired",
            },
            ServerError::FolderInvalid => Self {
                status: StatusCode::BAD_REQUEST,
                message: "Invalid Original Folder",
            },
            ServerError::FolderNotFound => Self {
                status: StatusCode::NOT_FOUND,
                message: "Unknown Original Folder",
            },
            ServerError::FileLocationWindow => Self {
                status: StatusCode::BAD_REQUEST,
                message: "File Location window is invalid",
            },
            ServerError::FolderAlbumLimit => Self {
                status: StatusCode::PAYLOAD_TOO_LARGE,
                message: "Original Folder contains too many Photos for one Album operation",
            },
            ServerError::QueryCapacity => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                message: "Retained query capacity is unavailable",
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

pub(crate) fn api_error(status: StatusCode, message: &'static str) -> Response<Body> {
    let body = serde_json::to_vec(&serde_json::json!({ "error": message }))
        .expect("static JSON serializes");
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONTENT_LENGTH, body.len())
        .body(Body::from(body))
        .expect("valid API response")
}

pub(crate) async fn static_web(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
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

pub(crate) fn safe_web_path(root: &Path, requested: &str) -> Option<PathBuf> {
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

pub(crate) async fn read_web_file(root: &WebRoot, candidate: &Path) -> io::Result<(Vec<u8>, bool)> {
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

pub(crate) fn open_directory_descriptor(path: &Path) -> io::Result<OwnedFd> {
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

pub(crate) fn open_confined_file(root: i32, relative: &CString) -> io::Result<OwnedFd> {
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

pub(crate) fn fstat(fd: i32) -> io::Result<libc::stat> {
    let mut facts = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: `facts` is valid writable storage and initialized on success.
    if unsafe { libc::fstat(fd, facts.as_mut_ptr()) } == 0 {
        // SAFETY: successful fstat initialized `facts`.
        Ok(unsafe { facts.assume_init() })
    } else {
        Err(io::Error::last_os_error())
    }
}

pub(crate) fn content_type(path: &Path) -> &'static str {
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

pub(crate) fn plain_error(status: StatusCode, message: &'static str) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(message))
        .expect("valid plain response")
}
