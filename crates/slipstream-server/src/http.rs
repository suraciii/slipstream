use super::*;
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

pub(crate) fn create_router_with_web_root(
    application: Arc<Application>,
    web_root: WebRoot,
) -> Router {
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

pub(crate) async fn status(State(state): State<HttpState>) -> Json<ScanStatusWire> {
    Json(state.application.scan_status())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowseOpenBody {
    source: String,
    #[serde(default)]
    album_id: Option<String>,
    #[serde(default)]
    photo_id: Option<String>,
}

pub(crate) async fn open_browse(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
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
        "library" if body.album_id.is_none() => BrowseSourceRequest::Library,
        "album" => match body.album_id.filter(|id| valid_id(id)) {
            Some(id) => BrowseSourceRequest::Album(id),
            None => return api_error(StatusCode::BAD_REQUEST, "Invalid Album source"),
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

pub(crate) async fn close_browse(
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

pub(crate) async fn retired_album_list() -> Response<Body> {
    api_error(StatusCode::NOT_FOUND, "Not found")
}

pub(crate) async fn method_not_allowed() -> Response<Body> {
    api_error(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed")
}

pub(crate) async fn scan(State(state): State<HttpState>, request: Request<Body>) -> Response<Body> {
    if !mutation_allowed(&request) {
        return api_error(StatusCode::FORBIDDEN, "Cross-origin mutation rejected");
    }
    match state.application.rescan().await {
        Ok(response) => json_response(StatusCode::OK, &response),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub(crate) async fn create_album(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
    mutate_album_route(&state, request, |body| {
        valid_name(body.get("name"))
            .map(|name| slipstream_core::AlbumMutation::Create { name })
            .ok_or("Invalid Album name")
    })
    .await
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
    request: Request<Body>,
) -> Response<Body> {
    if !mutation_allowed(&request) {
        return api_error(StatusCode::FORBIDDEN, "Cross-origin mutation rejected");
    }
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
    mutate_album_route(&state, request, move |body| {
        if !valid_id(&id) {
            return Err("Invalid membership batch");
        }
        valid_ids(body.get("photoIds"))
            .map(|photo_ids| slipstream_core::AlbumMutation::AddMembers {
                album_id: id.clone(),
                photo_ids,
            })
            .ok_or("Invalid membership batch")
    })
    .await
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
        valid_ids(body.get("photoIds"))
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
    match state.application.mutate_album(mutation).await {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub(crate) async fn read_json_body(request: Request<Body>) -> Result<Value, Response<Body>> {
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

pub(crate) fn json_response<T: Serialize>(status: StatusCode, value: &T) -> Response<Body> {
    let body = serde_json::to_vec(value).expect("response serializes");
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONTENT_LENGTH, body.len())
        .body(Body::from(body))
        .expect("valid JSON response")
}

pub(crate) fn mutation_allowed(request: &Request<Body>) -> bool {
    let Some(origin) = request.headers().get(header::ORIGIN) else {
        return true;
    };
    let Ok(origin) = origin.to_str() else {
        return false;
    };
    let Ok(supplied) = origin.parse::<::http::Uri>() else {
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

pub(crate) fn valid_ids(value: Option<&Value>) -> Option<Vec<String>> {
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

pub(crate) fn valid_selection(value: &Value) -> Option<SelectionState> {
    match value.as_str()? {
        "undecided" => Some(SelectionState::Undecided),
        "selected" => Some(SelectionState::Selected),
        "rejected" => Some(SelectionState::Rejected),
        _ => None,
    }
}

pub(crate) fn valid_rating(value: &Value) -> Option<u8> {
    let rating = value.as_u64()?;
    (rating <= 5).then_some(rating as u8)
}

pub(crate) async fn request_policy(request: Request<Body>, next: Next) -> Response<Body> {
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
pub(crate) struct ApiError {
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
