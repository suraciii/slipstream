// Browse route handlers.
use axum::{
    body::Body,
    extract::State,
    http::{Request, Response, StatusCode},
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::Value;
use std::time::SystemTime;

use super::{
    ApiError, CLI_CONTRACT_HEADER, HttpState, api_error, cli_error, has_exact_keys, invalid_cli,
    json_response, parse_permanent_deletion_ids, read_json_body, require_cli_contract,
    require_published, valid_id, valid_removal_markers, valid_selection,
};
use crate::{
    Application, BrowseSelectionFilter, BrowseSourceRequest, BrowseViewOrder, ServerError,
    folders::valid_folder_location,
    queries::format_time,
    wire::{CliFolderListResponse, CliScanStatusWire},
};
use slipstream_core::SelectionState;
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
    resume: bool,
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
        Some("unflagged") => BrowseSelectionFilter::Unflagged,
        Some("picked") => BrowseSelectionFilter::Picked,
        Some("rejected") => BrowseSelectionFilter::Rejected,
        Some(_) => return api_error(StatusCode::BAD_REQUEST, "Invalid browse selection"),
    };
    match state
        .application
        .browse_open_with_mode(
            source,
            order,
            selection,
            preferred_photo_id.as_deref(),
            body.resume,
        )
        .await
    {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(error) => ApiError::from(error).into_response(),
    }
}

/// Confirms the removal of one reviewed rejected result. The body names the
/// Browse Snapshot and the browser-generated operation id; the reviewed result
/// itself never travels back from the client.
pub(crate) async fn remove_photos(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
    let body = match read_json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(body) = body.as_object() else {
        return api_error(StatusCode::BAD_REQUEST, "Invalid removal request");
    };
    if !has_exact_keys(body, &["token", "operationId"]) {
        return api_error(StatusCode::BAD_REQUEST, "Invalid removal request");
    }
    let Some(token) = body
        .get("token")
        .and_then(Value::as_str)
        .filter(|token| valid_id(token))
    else {
        return api_error(StatusCode::BAD_REQUEST, "Invalid removal request");
    };
    let Some(operation_id) = body
        .get("operationId")
        .and_then(Value::as_str)
        .filter(|operation_id| valid_id(operation_id))
    else {
        return api_error(StatusCode::BAD_REQUEST, "Invalid removal request");
    };
    match state.application.remove_photos(token, operation_id).await {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(error) => ApiError::from(error).into_response(),
    }
}
/// Applies an explicit, caller-reviewed selection of Photos. This route is
/// CLI-only so its operation and evidence are always authenticated as one client attempt.
pub(crate) async fn remove_photos_explicit(
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
        Err(response) => return response,
    };
    let Some(body) = body.as_object() else {
        return invalid_cli("input", "The explicit removal request must be an object.");
    };
    if !has_exact_keys(body, &["operationId", "photos"]) {
        return invalid_cli(
            "input",
            "The explicit removal request must contain operationId and photos only.",
        );
    }
    let Some(operation_id) = body
        .get("operationId")
        .and_then(Value::as_str)
        .filter(|value| valid_id(value))
    else {
        return invalid_cli("operationId", "The operation ID is invalid.");
    };
    let Some(items) = body.get("photos").and_then(Value::as_array) else {
        return invalid_cli("photos", "The explicit target list is invalid.");
    };
    if items.is_empty() {
        return invalid_cli("photos", "The explicit target list must not be empty.");
    }
    if items.len() > slipstream_core::PHOTO_REMOVAL_MAX {
        return cli_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "limit_exceeded",
            "Reduce the Photo target set and try again.",
            serde_json::json!({
                "limitName": "removalPhotoIdsMaximum",
                "limit": slipstream_core::PHOTO_REMOVAL_MAX,
                "actual": items.len(),
            }),
        );
    }
    let mut photos = Vec::with_capacity(items.len());
    let mut ids = std::collections::BTreeSet::new();
    for item in items {
        let Some(item) = item.as_object() else {
            return invalid_cli("photos", "Each target must be an object.");
        };
        if !has_exact_keys(
            item,
            &[
                "photoId",
                "selectionState",
                "decisionVersion",
                "removedAtMs",
            ],
        ) {
            return invalid_cli(
                "photos",
                "Each target must contain exactly its evidence fields.",
            );
        }
        let Some(photo_id) = item
            .get("photoId")
            .and_then(Value::as_str)
            .filter(|value| valid_id(value))
        else {
            return invalid_cli("photoId", "The Photo ID is invalid.");
        };
        if !ids.insert(photo_id) {
            return invalid_cli("photos", "Photo IDs must be distinct.");
        }
        let Some(selection_state) = item
            .get("selectionState")
            .and_then(valid_selection)
            .filter(|value| *value == SelectionState::Rejected)
        else {
            return invalid_cli("selectionState", "Removal requires rejected evidence.");
        };
        let Some(decision_version) = item
            .get("decisionVersion")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            return invalid_cli("decisionVersion", "The decision version is required.");
        };
        let removed_at_ms = match item.get("removedAtMs") {
            Some(Value::Null) => None,
            Some(value) => match value.as_i64().filter(|value| *value >= 0) {
                Some(value) => Some(value),
                None => return invalid_cli("removedAtMs", "The removal marker is invalid."),
            },
            None => return invalid_cli("removedAtMs", "The removal state is required."),
        };
        photos.push(slipstream_core::PhotoRemovalTarget {
            photo_id: photo_id.to_owned(),
            expected_selection_state: selection_state,
            expected_decision_version: decision_version.to_owned(),
            expected_removed_at_ms: removed_at_ms,
        });
    }
    match state
        .application
        .remove_photos_explicit(slipstream_core::ExplicitPhotoRemovalMutation {
            operation_id: operation_id.to_owned(),
            photos,
        })
        .await
    {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub(crate) async fn get_photo_removal_operation(
    State(state): State<HttpState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    if let Err(response) = require_cli_contract(&request) {
        return *response;
    }
    if !valid_id(&id) {
        return invalid_cli("operationId", "The operation ID is invalid.");
    }
    match state.application.photo_removal_operation(id.clone()).await {
        Ok(Some(result)) => json_response(StatusCode::OK, &result),
        Ok(None) => cli_error(
            StatusCode::CONFLICT,
            "outcome_unknown",
            "Inspect the original operation before submitting a replacement.",
            serde_json::json!({"operationId": id}),
        ),
        Err(error) => ApiError::from(error).into_response(),
    }
}

/// Restores one removal operation or one explicit bounded list of Photos.
/// Applies one explicit, caller-reviewed Restore attempt. This route is
/// CLI-only and retains the attempt identity for read-only reconciliation.
pub(crate) async fn restore_photos_explicit(
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
        Err(response) => return response,
    };
    let Some(body) = body.as_object() else {
        return invalid_cli("input", "The explicit Restore request must be an object.");
    };
    if !has_exact_keys(body, &["operationId", "photos"]) {
        return invalid_cli(
            "input",
            "The explicit Restore request must contain operationId and photos only.",
        );
    }
    let Some(operation_id) = body
        .get("operationId")
        .and_then(Value::as_str)
        .filter(|value| valid_id(value))
    else {
        return invalid_cli("operationId", "The operation ID is invalid.");
    };
    let Some(items) = body.get("photos").and_then(Value::as_array) else {
        return invalid_cli("photos", "The explicit Restore target list is invalid.");
    };
    if items.is_empty() {
        return invalid_cli(
            "photos",
            "The explicit Restore target list must not be empty.",
        );
    }
    if items.len() > slipstream_core::PHOTO_REMOVAL_MAX {
        return cli_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "limit_exceeded",
            "Reduce the Photo target set and try again.",
            serde_json::json!({
                "limitName": "removalPhotoIdsMaximum",
                "limit": slipstream_core::PHOTO_REMOVAL_MAX,
                "actual": items.len(),
            }),
        );
    }
    let mut photos = Vec::with_capacity(items.len());
    let mut ids = std::collections::BTreeSet::new();
    for item in items {
        let Some(item) = item.as_object() else {
            return invalid_cli("photos", "Each Restore target must be an object.");
        };
        if !has_exact_keys(item, &["photoId", "removedAtMs"]) {
            return invalid_cli(
                "photos",
                "Each Restore target must contain its marker fields.",
            );
        }
        let Some(photo_id) = item
            .get("photoId")
            .and_then(Value::as_str)
            .filter(|value| valid_id(value))
        else {
            return invalid_cli("photoId", "The Photo ID is invalid.");
        };
        if !ids.insert(photo_id) {
            return invalid_cli("photos", "Photo IDs must be distinct.");
        }
        let Some(removed_at_ms) = item
            .get("removedAtMs")
            .and_then(Value::as_i64)
            .filter(|value| *value >= 0)
        else {
            return invalid_cli("removedAtMs", "The removal marker is invalid.");
        };
        photos.push(slipstream_core::PhotoRemovalMarker {
            photo_id: photo_id.to_owned(),
            removed_at_ms,
        });
    }
    match state
        .application
        .restore_photos_explicit(slipstream_core::ExplicitPhotoRestoreMutation {
            operation_id: operation_id.to_owned(),
            photos,
        })
        .await
    {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub(crate) async fn get_photo_restore_operation(
    State(state): State<HttpState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    if let Err(response) = require_cli_contract(&request) {
        return *response;
    }
    if !valid_id(&id) {
        return invalid_cli("operationId", "The operation ID is invalid.");
    }
    match state.application.photo_restore_operation(id.clone()).await {
        Ok(Some(result)) => json_response(StatusCode::OK, &result),
        Ok(None) => cli_error(
            StatusCode::CONFLICT,
            "outcome_unknown",
            "Inspect the original Restore attempt before submitting a replacement.",
            serde_json::json!({"operationId": id}),
        ),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub(crate) async fn restore_photos(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
    let body = match read_json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(body) = body.as_object() else {
        return api_error(StatusCode::BAD_REQUEST, "Invalid restore request");
    };
    if body.is_empty() || !body.keys().all(|key| key == "operation" || key == "photos") {
        return api_error(StatusCode::BAD_REQUEST, "Invalid restore request");
    }
    let operation = match body.get("operation") {
        None | Some(Value::Null) => None,
        Some(Value::String(operation)) if valid_id(operation.as_str()) => {
            Some(operation.to_owned())
        }
        Some(_) => return api_error(StatusCode::BAD_REQUEST, "Invalid restore request"),
    };
    // An explicit restore names each Photo together with the removal marker it
    // was reviewed at, so the request is a compare-and-set instead of a clear
    // of whatever removal the Photo carries now.
    let photos = match body.get("photos") {
        None | Some(Value::Null) => None,
        Some(value) => match valid_removal_markers(value) {
            Some(markers) if !markers.is_empty() => Some(markers),
            _ => return api_error(StatusCode::BAD_REQUEST, "Invalid restore request"),
        },
    };
    // One named operation or one named Photo list, never both and never
    // neither: a restore that could mean two different sets is refused.
    let restoration = match (operation, photos) {
        (Some(operation), None) => slipstream_core::PhotoRestoration::Operation(operation),
        (None, Some(markers)) => slipstream_core::PhotoRestoration::Photos(markers),
        _ => return api_error(StatusCode::BAD_REQUEST, "Invalid restore request"),
    };
    match state.application.restore_photos(restoration).await {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(error) => ApiError::from(error).into_response(),
    }
}

/// One bounded page of removed Photos, newest removal first.
pub(crate) async fn get_removed_photos(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
    let Some((start, limit)) = browse_query(request.uri().query()) else {
        return api_error(StatusCode::BAD_REQUEST, "Removed Photos window is invalid");
    };
    match state.application.removed_photos(start, limit).await {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(error) => ApiError::from(error).into_response(),
    }
}
/// Captures a fixed Trash selection and returns the exact files and affected
/// Albums that require confirmation before deletion.
pub(crate) async fn review_permanent_deletion(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
    let (operation_id, selection) = {
        let body = match read_json_body(request).await {
            Ok(body) => body,
            Err(response) => return response,
        };
        let Some(body) = body.as_object() else {
            return api_error(StatusCode::BAD_REQUEST, "Invalid Trash review request");
        };
        if !has_exact_keys(body, &["operationId", "all", "photoIds", "excludePhotoIds"]) {
            return api_error(StatusCode::BAD_REQUEST, "Invalid Trash review request");
        }
        let Some(operation_id) = body
            .get("operationId")
            .and_then(Value::as_str)
            .filter(|value| valid_id(value))
        else {
            return api_error(StatusCode::BAD_REQUEST, "Invalid Trash review request");
        };
        let Some(all) = body.get("all").and_then(Value::as_bool) else {
            return api_error(StatusCode::BAD_REQUEST, "Invalid Trash review request");
        };
        let Some(photo_ids) = parse_permanent_deletion_ids(body, "photoIds") else {
            return api_error(StatusCode::BAD_REQUEST, "Invalid Trash review request");
        };
        let Some(exclude_photo_ids) = parse_permanent_deletion_ids(body, "excludePhotoIds") else {
            return api_error(StatusCode::BAD_REQUEST, "Invalid Trash review request");
        };
        let selection = if all {
            if !photo_ids.is_empty() {
                return api_error(StatusCode::BAD_REQUEST, "Invalid Trash review request");
            }
            slipstream_core::PermanentDeletionSelection::All { exclude_photo_ids }
        } else {
            if photo_ids.is_empty() || !exclude_photo_ids.is_empty() {
                return api_error(StatusCode::BAD_REQUEST, "Invalid Trash review request");
            }
            slipstream_core::PermanentDeletionSelection::Photos(photo_ids)
        };
        (operation_id.to_owned(), selection)
    };
    match state
        .application
        .prepare_permanent_deletion(operation_id, selection)
        .await
    {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub(crate) async fn permanently_delete(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
    let operation_id = {
        let body = match read_json_body(request).await {
            Ok(body) => body,
            Err(response) => return response,
        };
        let Some(body) = body.as_object() else {
            return api_error(
                StatusCode::BAD_REQUEST,
                "Invalid Permanent Deletion request",
            );
        };
        if !has_exact_keys(body, &["operationId"]) {
            return api_error(
                StatusCode::BAD_REQUEST,
                "Invalid Permanent Deletion request",
            );
        }
        let Some(operation_id) = body
            .get("operationId")
            .and_then(Value::as_str)
            .filter(|value| valid_id(value))
        else {
            return api_error(
                StatusCode::BAD_REQUEST,
                "Invalid Permanent Deletion request",
            );
        };
        operation_id.to_owned()
    };
    match state
        .application
        .permanently_delete(operation_id.to_owned())
        .await
    {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub(crate) async fn get_permanent_deletion(
    State(state): State<HttpState>,
    axum::extract::Path(operation_id): axum::extract::Path<String>,
) -> Response<Body> {
    if !valid_id(&operation_id) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "Invalid Permanent Deletion operation",
        );
    }
    match state
        .application
        .read_permanent_deletion(operation_id)
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
    let pairs = match super::cli::cli_query_pairs(request.uri().query()) {
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
            Err(error) => return super::cli::query_cursor_error("folder", error),
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
        let limit = match super::cli::list_limit(
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
