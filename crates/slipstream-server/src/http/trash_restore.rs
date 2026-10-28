use axum::{
    body::Body,
    extract::State,
    http::{Request, Response, StatusCode},
    response::IntoResponse,
};
use serde_json::Value;

use super::{
    ApiError, HttpState, cli_error, has_exact_keys, invalid_cli, json_response,
    parse_permanent_deletion_ids, read_cli_json_body, valid_id,
};
use crate::ServerError;

/// Restores a fixed admission-time Trash selection under a durable attempt ID.
pub(crate) async fn restore_trash(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
    let body: Value = match read_cli_json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(body) = body.as_object() else {
        return invalid_cli("input", "The Trash Restore request must be an object.");
    };
    if !has_exact_keys(body, &["operationId", "all", "photoIds", "excludePhotoIds"]) {
        return invalid_cli(
            "input",
            "The Trash Restore request must contain its selection fields.",
        );
    }
    let Some(operation_id) = body
        .get("operationId")
        .and_then(Value::as_str)
        .filter(|id| valid_id(id))
    else {
        return cli_error(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "Correct the request and try again.",
            serde_json::json!({"argument":"operationId", "reason":"The operation ID is invalid.", "operationId":body.get("operationId")}),
        );
    };
    let Some(all) = body.get("all").and_then(Value::as_bool) else {
        return invalid_cli("all", "The selection mode is invalid.");
    };
    if let Some(items) = body.get("photoIds").and_then(Value::as_array)
        && !all
        && items.len() > slipstream_core::PHOTO_REMOVAL_MAX
    {
        return cli_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "limit_exceeded",
            "Reduce the Photo target set and try again.",
            serde_json::json!({"limitName":"removalPhotoIdsMaximum", "limit":slipstream_core::PHOTO_REMOVAL_MAX, "actual":items.len(), "operationId":operation_id}),
        );
    }
    let Some(photo_ids) = parse_permanent_deletion_ids(body, "photoIds") else {
        return invalid_cli("photoIds", "The Photo ID list is invalid.");
    };
    let Some(exclude_photo_ids) = parse_permanent_deletion_ids(body, "excludePhotoIds") else {
        return invalid_cli("excludePhotoIds", "The exclusion list is invalid.");
    };
    let selection = if all && photo_ids.is_empty() {
        slipstream_core::PermanentDeletionSelection::All { exclude_photo_ids }
    } else if !all
        && !photo_ids.is_empty()
        && exclude_photo_ids.is_empty()
        && photo_ids.len() <= slipstream_core::PHOTO_REMOVAL_MAX
    {
        slipstream_core::PermanentDeletionSelection::Photos(photo_ids)
    } else {
        return invalid_cli("input", "The Trash Restore selection is invalid.");
    };
    match state
        .application
        .restore_trash(operation_id.to_owned(), selection)
        .await
    {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(ServerError::Library(slipstream_core::LibraryError::Mutation(
            slipstream_core::MutationError::Conflict,
        ))) => cli_error(
            StatusCode::CONFLICT,
            "conflict",
            "The Restore selection conflicts with current Trash or exceeds the review bound.",
            serde_json::json!({"operationId": operation_id}),
        ),
        Err(ServerError::Library(slipstream_core::LibraryError::Mutation(
            slipstream_core::MutationError::Invalid,
        ))) => cli_error(
            StatusCode::BAD_REQUEST,
            "invalid_input",
            "Correct the request and try again.",
            serde_json::json!({"argument":"selection", "reason":"The Trash Restore selection is invalid.", "operationId":operation_id}),
        ),
        Err(error) => ApiError::from(error).into_response(),
    }
}
