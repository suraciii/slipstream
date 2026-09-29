use super::*;
use crate::config::MAX_RESTORATION_PHOTOS;
mod browse;
mod cli;
mod export;
mod mutations;
mod photo;
mod recovery;
mod static_web;

pub(crate) use cli::{
    CLI_CONTRACT_HEADER, cli_error, invalid_cli, require_cli_contract, require_published,
};
pub(crate) use static_web::{
    fstat, open_confined_file, open_directory_descriptor, plain_error, static_web,
};

/// The largest number of Photo ids one bounded Album membership write accepts
/// at the HTTP boundary.
const ALBUM_PHOTO_IDS_MAX: usize = 100;

fn parse_permanent_deletion_ids(
    body: &serde_json::Map<String, Value>,
    key: &str,
) -> Option<Vec<String>> {
    let values = body.get(key)?.as_array()?;
    if values.len() > slipstream_core::PERMANENT_DELETION_MAX {
        return None;
    }
    let ids = values
        .iter()
        .map(|value| value.as_str().filter(|id| valid_id(id)).map(str::to_owned))
        .collect::<Option<Vec<_>>>()?;
    (ids.iter().collect::<std::collections::HashSet<_>>().len() == ids.len()).then_some(ids)
}

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
    pub(crate) processing: Option<ProcessingConfig>,
    pub(crate) edit_preview: Arc<crate::edit_preview::EditPreviewOwner>,
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
    let router = create_router_with_processing(
        Arc::clone(&application),
        web_root,
        config.processing.clone(),
    );
    let (sender, receiver) = oneshot::channel();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
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
    create_router_with_processing(application, open_web_root(web_root.into()), None)
}

pub(crate) fn create_router_with_processing(
    application: Arc<Application>,
    web_root: WebRoot,
    processing: Option<ProcessingConfig>,
) -> Router {
    let owner = Arc::new(crate::edit_preview::EditPreviewOwner::production(
        application.exports.as_ref().map(Arc::clone),
    ));
    create_router_with_preview(application, web_root, processing, owner)
}

/// Builds the router with explicit Edit Preview seams. Production resolves
/// retained Development Results from the Export lifecycle and admits
/// unretained identities through the preview-class render gate; route tests
/// may inject scripted seams.
pub(crate) fn create_router_with_preview(
    application: Arc<Application>,
    web_root: WebRoot,
    processing: Option<ProcessingConfig>,
    edit_preview: Arc<crate::edit_preview::EditPreviewOwner>,
) -> Router {
    let state = HttpState {
        application,
        web_root: Arc::new(web_root),
        processing,
        edit_preview,
    };
    Router::new()
        .route(HEALTH_PATH, get(healthz))
        .route("/api/overview", get(overview))
        .route("/api/status", get(cli::status))
        .route("/api/capabilities", get(cli::capabilities))
        .route(
            "/api/processing/capability",
            get(crate::processing_capability::get_processing_capability),
        )
        .route("/api/album-summaries", get(cli::get_album_summaries))
        .route("/api/albums/{id}", get(cli::get_album_summary))
        .route("/api/albums/{id}/changes", post(mutations::change_album))
        .route(
            "/api/photo-decisions",
            post(mutations::mutate_photo_decision),
        )
        .route("/api/photo-queries", post(cli::create_photo_query))
        .route(
            "/api/photo-queries/{cursor}",
            get(cli::get_photo_query_page),
        )
        .route("/api/photos/{id}", get(cli::get_photo))
        .route(
            "/api/photos/{id}/development-proxy",
            get(crate::development_proxy::get_development_proxy)
                .post(crate::development_proxy::post_development_proxy)
                .delete(crate::development_proxy::delete_development_proxy),
        )
        .route(
            "/api/photos/{id}/edit-recipe",
            get(crate::edit_recipe::get_edit_recipe).post(crate::edit_recipe::post_edit_recipe),
        )
        .route(
            "/api/photos/{id}/edit-recipe/rebind",
            get(browse::method_not_allowed).post(crate::edit_recipe::post_edit_recipe_rebind),
        )
        .route(
            "/api/photos/{id}/edit-preview/{stage}",
            get(crate::edit_preview::get_edit_preview),
        )
        .route("/api/file-locations", get(browse::get_file_locations))
        .route("/api/browse", post(browse::open_browse))
        .route(
            "/api/browse/{token}/position",
            get(browse::get_browse_position),
        )
        .route(
            "/api/browse/{token}",
            get(browse::get_browse_window).delete(browse::close_browse),
        )
        .route("/api/photos/{id}/preview", get(photo::get_preview))
        .route("/api/photos/{id}/thumbnail", get(photo::get_thumbnail))
        .route("/api/photos/{id}/metadata", get(photo::get_photo_metadata))
        .route(
            "/api/photos/{id}/external-metadata",
            get(photo::get_external_metadata).post(photo::post_external_metadata),
        )
        .route("/api/photos/{id}/albums", get(photo::get_photo_albums))
        .route(
            "/api/photos/{id}/exports",
            get(export::list_photo_exports).post(export::submit_export),
        )
        .route("/api/exports/{id}", get(export::get_export))
        .route(
            "/api/exports/{id}/cancel",
            get(browse::method_not_allowed).post(export::cancel_export),
        )
        .route(
            "/api/exports/{id}/retry",
            get(browse::method_not_allowed).post(export::retry_export),
        )
        .route(
            "/api/exports/{id}/artifact",
            get(export::get_export_artifact),
        )
        // The complete-membership list is retired; the path only creates Albums.
        .route(
            "/api/albums",
            get(browse::retired_album_list).post(mutations::create_album),
        )
        .route(
            "/api/albums/{id}/rename",
            get(browse::method_not_allowed).post(mutations::rename_album),
        )
        .route(
            "/api/albums/{id}/delete",
            get(browse::method_not_allowed).post(mutations::delete_album),
        )
        .route(
            "/api/albums/{id}/members",
            get(browse::method_not_allowed).post(mutations::add_album_members),
        )
        .route(
            "/api/albums/{id}/members/batch-remove",
            get(browse::method_not_allowed).post(mutations::remove_added_album_members),
        )
        .route(
            "/api/albums/{id}/folder-members",
            get(browse::method_not_allowed).post(mutations::add_folder_members),
        )
        .route(
            "/api/albums/{id}/members/remove",
            get(browse::method_not_allowed).post(mutations::remove_album_member),
        )
        .route(
            "/api/albums/{id}/order",
            get(browse::method_not_allowed).post(mutations::reorder_album),
        )
        .route(
            "/api/albums/{id}/progress",
            get(browse::method_not_allowed).post(mutations::set_progress),
        )
        .route(
            "/api/photos/state",
            get(browse::method_not_allowed).post(mutations::mutate_photo_state_batch),
        )
        .route(
            "/api/photos/remove",
            get(browse::method_not_allowed).post(browse::remove_photos),
        )
        .route(
            "/api/photos/remove-explicit",
            get(browse::method_not_allowed).post(browse::remove_photos_explicit),
        )
        .route(
            "/api/photos/removal-operations/{id}",
            get(browse::get_photo_removal_operation),
        )
        .route(
            "/api/photos/restore",
            get(browse::method_not_allowed).post(browse::restore_photos),
        )
        .route(
            "/api/photos/restore-explicit",
            get(browse::method_not_allowed).post(browse::restore_photos_explicit),
        )
        .route(
            "/api/photos/restore-operations/{id}",
            get(browse::get_photo_restore_operation),
        )
        .route("/api/photos/removed", get(browse::get_removed_photos))
        .route("/api/trash", get(browse::get_removed_photos))
        .route("/api/trash/review", post(browse::review_permanent_deletion))
        .route("/api/trash/delete", post(browse::permanently_delete))
        .route(
            "/api/trash/operations/{id}",
            get(browse::get_permanent_deletion),
        )
        .route(
            "/api/photos/{id}/state",
            get(browse::method_not_allowed).post(mutations::mutate_photo_state),
        )
        .route(
            "/api/scan",
            get(browse::method_not_allowed).post(browse::scan),
        )
        .route(
            "/api/recovery/unavailable",
            get(browse::method_not_allowed).post(recovery::open_unavailable_review),
        )
        .route(
            "/api/recovery/unavailable/{cursor}",
            get(recovery::unavailable_review_page),
        )
        .route(
            "/api/recovery/propose",
            get(browse::method_not_allowed).post(recovery::open_proposal_review),
        )
        .route(
            "/api/recovery/proposals/{cursor}",
            get(recovery::proposal_review_page),
        )
        .route(
            "/api/recovery/apply",
            get(browse::method_not_allowed).post(recovery::apply),
        )
        .route(
            "/api/private/derivatives/{photo_id}/{target}/{filename}",
            get(photo::get_derivative),
        )
        .fallback(static_web)
        .layer(middleware::from_fn_with_state(
            state.clone(),
            request_policy,
        ))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            crate::access::boundary,
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

pub(crate) async fn read_cli_json_body<T: serde::de::DeserializeOwned>(
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

/// One explicit restore list: each Photo named with the removal marker the
/// caller reviewed. A marker must be a non-negative count of milliseconds, and
/// no Photo may be named twice.
pub(crate) fn valid_removal_markers(
    value: &Value,
) -> Option<Vec<slipstream_core::PhotoRemovalMarker>> {
    let values = value.as_array()?;
    if values.len() > MAX_RESTORATION_PHOTOS {
        return None;
    }
    let markers = values
        .iter()
        .map(|value| {
            let entry = value.as_object()?;
            if !has_exact_keys(entry, &["id", "removedAtMs"]) {
                return None;
            }
            let photo_id = entry.get("id")?.as_str()?;
            if !valid_id(photo_id) {
                return None;
            }
            let removed_at_ms = entry.get("removedAtMs")?.as_i64()?;
            if removed_at_ms < 0 {
                return None;
            }
            Some(slipstream_core::PhotoRemovalMarker {
                photo_id: photo_id.to_owned(),
                removed_at_ms,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    let unique = markers
        .iter()
        .map(|marker| marker.photo_id.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    (unique == markers.len()).then_some(markers)
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
        let admitted =
            api_path && (!request.method().as_str().eq("DELETE") || delete_is_admitted(path));
        if !admitted {
            return api_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed");
        }
    }
    next.run(request).await
}

/// DELETE is admitted only on the routes that declare it: the browsing session
/// release and the Development Proxy removal. Every other DELETE is refused
/// before routing, so a retired or renamed path can never be reached by a
/// method the API does not publish.
fn delete_is_admitted(path: &str) -> bool {
    path.starts_with("/api/browse/") || is_development_proxy_route(path)
}

/// One `/api/photos/{id}/development-proxy` path: exactly one nonempty Photo ID
/// segment before the fixed suffix, and no deeper path.
fn is_development_proxy_route(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/api/photos/") else {
        return false;
    };
    let Some(photo_id) = rest.strip_suffix("/development-proxy") else {
        return false;
    };
    !photo_id.is_empty() && !photo_id.contains('/')
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
            ServerError::RecoveryScope { .. } => Self {
                status: StatusCode::PAYLOAD_TOO_LARGE,
                message: "Recovery review scope exceeds the advertised bound; narrow the Folder prefix",
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
            ServerError::RemovalFilter => Self {
                status: StatusCode::BAD_REQUEST,
                message: "Removal requires a Browse Snapshot filtered to Rejected",
            },
            ServerError::RemovedWindow => Self {
                status: StatusCode::BAD_REQUEST,
                message: "Removed Photos window is invalid",
            },
            ServerError::RestorationInvalid => Self {
                status: StatusCode::BAD_REQUEST,
                message: "Restore names exactly one operation or a bounded Photo list",
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
