// CLI route handlers.
use axum::{
    body::Body,
    extract::State,
    http::{Request, Response, StatusCode},
};
use serde::Deserialize;
use serde_json::Value;
use slipstream_core::{LibraryError, SelectionState};
use std::{
    sync::atomic::Ordering,
    time::{Instant, SystemTime},
};

use super::browse::percent_decode;
use super::{ALBUM_PHOTO_IDS_MAX, HttpState, json_response, read_json_body, valid_id};
use crate::{
    Application,
    folders::valid_folder_location,
    queries::{
        CLI_CONTRACT_VERSION, CursorError, MAXIMUM_LIST_PAGE, MAXIMUM_RETAINED_IDS, QUERY_IDLE,
        RetainedKind, format_time,
    },
    wire::{
        AlbumListItemWire, CapabilitiesResponse, CapabilityLimitsWire, CliAlbumSummaryWire,
        CliListResponse, CliPhotoGetWire, CliPhotoItemWire, CliPhotoMetadataWire,
        CliScanStatusWire, CliStatusResponse, MissingItemWire, PhotoListItemWire,
    },
};
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

pub(crate) const CLI_CONTRACT_HEADER: &str = "slipstream-cli-contract";
const DEFAULT_LIST_PAGE: usize = 50;

pub(super) type CliBoundaryResult<T> = Result<T, Box<Response<Body>>>;

pub(crate) fn require_cli_contract(request: &Request<Body>) -> CliBoundaryResult<()> {
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

pub(crate) fn cli_error(
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

pub(crate) fn invalid_cli(argument: &'static str, reason: &'static str) -> Response<Body> {
    cli_error(
        StatusCode::BAD_REQUEST,
        "invalid_input",
        "Correct the request and try again.",
        serde_json::json!({"argument": argument, "reason": reason}),
    )
}

pub(crate) fn require_published(application: &Application) -> CliBoundaryResult<()> {
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
                removal_photo_ids_maximum: slipstream_core::PHOTO_REMOVAL_MAX,
                album_reorder_members_maximum: ALBUM_PHOTO_IDS_MAX,
                retained_query_ids_maximum: MAXIMUM_RETAINED_IDS,
                retained_query_idle_seconds: QUERY_IDLE.as_secs(),
                recovery_page_maximum: crate::recovery_review::MAXIMUM_RECOVERY_PAGE,
                recovery_mappings_maximum: crate::recovery_review::MAXIMUM_RECOVERY_MAPPINGS,
                recovery_apply_maximum: crate::recovery_review::MAXIMUM_RECOVERY_APPLY,
                recovery_review_idle_seconds: QUERY_IDLE.as_secs(),
            },
        },
    )
}

pub(super) fn cli_query_pairs(query: Option<&str>) -> CliBoundaryResult<Vec<(String, String)>> {
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

pub(super) fn list_limit(value: Option<&str>) -> CliBoundaryResult<usize> {
    match value {
        None => Ok(DEFAULT_LIST_PAGE),
        Some(value) => value
            .parse::<usize>()
            .ok()
            .filter(|limit| (1..=MAXIMUM_LIST_PAGE).contains(limit))
            .ok_or_else(|| Box::new(invalid_cli("limit", "The limit must be from 1 through 60."))),
    }
}

pub(super) fn query_token(application: &Application, kind: RetainedKind) -> String {
    let prefix = match kind {
        RetainedKind::Album => 'a',
        RetainedKind::Photo => 'p',
        RetainedKind::Browse => 'b',
        RetainedKind::RecoveryUnavailable => 'u',
        RetainedKind::RecoveryMappings => 'm',
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

pub(super) fn query_cursor_error(kind: &'static str, error: CursorError) -> Response<Body> {
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
        CursorError::Idle => cli_error(
            StatusCode::GONE,
            "cursor_expired",
            "Start a new query with the same filter.",
            serde_json::json!({"cursorKind": kind, "reason": "idle_or_evicted"}),
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
                None,
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
                None,
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
            Some(photo) => PhotoListItemWire::Present(Box::new(CliPhotoItemWire::from(photo))),
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
                make: metadata.make,
                model: metadata.model,
            },
        },
    )
}
