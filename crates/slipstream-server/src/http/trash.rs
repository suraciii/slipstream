use super::cli::{
    cli_outcome_unknown, cli_query_pairs, list_limit, query_cursor_error, query_token,
};
use super::{ApiError, HttpState, cli_error, invalid_cli, json_response, valid_id};
use crate::queries::{CursorError, QUERY_IDLE, RetainedKind};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{Request, Response, StatusCode},
    response::IntoResponse,
};
use serde::Deserialize;
use std::time::{Instant, SystemTime};

pub(crate) async fn list(State(state): State<HttpState>, request: Request<Body>) -> Response<Body> {
    let pairs = match cli_query_pairs(request.uri().query()) {
        Ok(pairs) => pairs,
        Err(response) => return *response,
    };
    if pairs
        .iter()
        .any(|(key, _)| !matches!(key.as_str(), "start" | "limit" | "cursor"))
    {
        return invalid_cli("query", "The Trash query is invalid.");
    }
    let cursor = pairs
        .iter()
        .find(|(key, _)| key == "cursor")
        .map(|(_, value)| value.as_str());
    let application = &state.application;
    let (items, start, limit, total, evaluated_at, expires_at, next_cursor, operation) =
        if let Some(cursor) = cursor {
            if pairs.len() != 1 {
                return invalid_cli(
                    "cursor",
                    "A cursor cannot be combined with other parameters.",
                );
            }
            let parsed = match application.cursor_signer.parse_query_cursor(
                cursor,
                application.browse_namespace,
                RetainedKind::Trash,
            ) {
                Ok(parsed) => parsed,
                Err(error) => return query_cursor_error("trash", error),
            };
            let Some(page) = application.retained_queries.page_trash(
                &parsed.token,
                parsed.offset,
                parsed.limit,
                Instant::now(),
                SystemTime::now(),
            ) else {
                return query_cursor_error("trash", CursorError::Idle);
            };
            let next_offset = parsed.offset.saturating_add(page.items.len());
            let next = (next_offset < page.total).then(|| {
                application.cursor_signer.query_cursor(
                    application.browse_namespace,
                    RetainedKind::Trash,
                    &parsed.token,
                    next_offset,
                    parsed.limit,
                )
            });
            let expires = next.as_ref().map(|_| page.expires_at);
            (
                page.items,
                parsed.offset,
                parsed.limit,
                page.total,
                page.evaluated_at,
                expires,
                next,
                page.operation,
            )
        } else {
            let start = match pairs.iter().find(|(key, _)| key == "start") {
                Some((_, value)) => match value.parse::<usize>() {
                    Ok(start) => start,
                    Err(_) => {
                        return invalid_cli("start", "The start must be a nonnegative integer.");
                    }
                },
                None => 0,
            };
            let limit = match list_limit(
                pairs
                    .iter()
                    .find(|(key, _)| key == "limit")
                    .map(|(_, value)| value.as_str()),
            ) {
                Ok(limit) => limit,
                Err(response) => return *response,
            };
            let operation = match application.library.removed_photos(0, 1).await {
                Ok((_, _, operation)) => {
                    operation.map(|operation| (operation.operation_id, operation.removed))
                }
                Err(error) => {
                    return ApiError::from(crate::ServerError::from(error)).into_response();
                }
            };
            let records = match application.library.trash_inspection().await {
                Ok(records) => records,
                Err(error) => {
                    return ApiError::from(crate::ServerError::from(error)).into_response();
                }
            };
            let total = records.len();
            let items = records
                .iter()
                .skip(start)
                .take(limit)
                .cloned()
                .collect::<Vec<_>>();
            let evaluated_at = SystemTime::now();
            let next_offset = start.saturating_add(items.len());
            let next = if next_offset < total {
                let token = query_token(application, RetainedKind::Trash);
                if application
                    .retained_queries
                    .insert_trash(
                        token.clone(),
                        records,
                        operation.clone(),
                        Instant::now(),
                        evaluated_at,
                    )
                    .is_err()
                {
                    return cli_error(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "server_busy",
                        "Start a new Trash listing later.",
                        serde_json::json!({"operation":"trash-list","retryAfterSeconds":null}),
                    );
                }
                Some(application.cursor_signer.query_cursor(
                    application.browse_namespace,
                    RetainedKind::Trash,
                    &token,
                    next_offset,
                    limit,
                ))
            } else {
                None
            };
            let expires = next.as_ref().map(|_| evaluated_at + QUERY_IDLE);
            (
                items,
                start,
                limit,
                total,
                evaluated_at,
                expires,
                next,
                operation,
            )
        };
    match application
        .trash_page(crate::app::trash::TrashPageCapture {
            captured: items,
            start,
            limit,
            total,
            evaluated_at,
            expires_at,
            next_cursor,
            operation,
        })
        .await
    {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(error) => ApiError::from(error).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CleanupRequest {
    operation_id: String,
    items: Vec<CleanupItem>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CleanupItem {
    photo_id: String,
    removed_at_ms: i64,
    original_id: String,
    original_location: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PreviewRequest {
    items: Vec<CleanupItem>,
}

fn cleanup_targets(
    items: Vec<CleanupItem>,
) -> Result<Vec<slipstream_core::TrashCleanupTarget>, Box<Response<Body>>> {
    if items.is_empty() || items.len() > slipstream_core::TRASH_CLEANUP_MAX {
        return Err(Box::new(invalid_cli(
            "input",
            "The cleanup operation or item count is invalid.",
        )));
    }
    let mut ids = std::collections::HashSet::new();
    let mut targets = Vec::with_capacity(items.len());
    for item in items {
        if !valid_id(&item.photo_id)
            || !valid_id(&item.original_id)
            || item.removed_at_ms < 0
            || !ids.insert(item.photo_id.clone())
        {
            return Err(Box::new(invalid_cli(
                "items",
                "The cleanup item identity or marker is invalid.",
            )));
        }
        let relative_path =
            match slipstream_core::RelativeOriginalPath::parse(item.original_location) {
                Ok(path) => path,
                Err(_) => {
                    return Err(Box::new(invalid_cli(
                        "items",
                        "The Original Location is invalid.",
                    )));
                }
            };
        targets.push(slipstream_core::TrashCleanupTarget {
            photo_id: item.photo_id,
            removed_at_ms: item.removed_at_ms,
            original_id: item.original_id,
            relative_path,
            absence: slipstream_core::TrashAbsence::Uninspectable,
            blockers: Vec::new(),
        });
    }
    Ok(targets)
}

pub(crate) async fn preview(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
    const PREVIEW_BODY_MAX: usize = 2 * 1024 * 1024;
    let bytes = match axum::body::to_bytes(request.into_body(), PREVIEW_BODY_MAX).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return cli_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "limit_exceeded",
                "Reduce the preview request body.",
                serde_json::json!({"limitName":"requestBodyBytesMaximum","limit":PREVIEW_BODY_MAX,"actual":PREVIEW_BODY_MAX + 1}),
            );
        }
    };
    let body: PreviewRequest = match serde_json::from_slice(&bytes) {
        Ok(body) => body,
        Err(_) => {
            return invalid_cli(
                "body",
                "The JSON body is malformed or contains unknown or duplicate keys.",
            );
        }
    };
    let targets = match cleanup_targets(body.items) {
        Ok(targets) => targets,
        Err(response) => return *response,
    };
    match state.application.preview_trash_cleanup(targets).await {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub(crate) async fn cleanup(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
    // This explicit 5000-item operation is larger than ordinary mutations.
    const CLEANUP_BODY_MAX: usize = 2 * 1024 * 1024;
    let bytes = match axum::body::to_bytes(request.into_body(), CLEANUP_BODY_MAX).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return cli_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "limit_exceeded",
                "Reduce the cleanup request body.",
                serde_json::json!({"limitName":"requestBodyBytesMaximum","limit":CLEANUP_BODY_MAX,"actual":CLEANUP_BODY_MAX + 1}),
            );
        }
    };
    let body: CleanupRequest = match serde_json::from_slice(&bytes) {
        Ok(body) => body,
        Err(_) => {
            return invalid_cli(
                "body",
                "The JSON body is malformed or contains unknown or duplicate keys.",
            );
        }
    };
    if !valid_id(&body.operation_id) {
        return invalid_cli("input", "The cleanup operation or item count is invalid.");
    }
    let targets = match cleanup_targets(body.items) {
        Ok(targets) => targets,
        Err(response) => return *response,
    };
    let operation_id = body.operation_id;
    match state
        .application
        .cleanup_trash(operation_id.clone(), targets)
        .await
    {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(crate::ServerError::Library(slipstream_core::LibraryError::Mutation(
            slipstream_core::MutationError::Conflict,
        ))) => cli_error(
            StatusCode::CONFLICT,
            "conflict",
            "The operation ID already names different cleanup intent.",
            serde_json::json!({"operationId":operation_id}),
        ),
        Err(error) => ApiError::from(error).into_response(),
    }
}

pub(crate) async fn operation(
    State(state): State<HttpState>,
    Path(id): Path<String>,
) -> Response<Body> {
    if !valid_id(&id) {
        return invalid_cli("operationId", "The operation ID is invalid.");
    }
    match state.application.read_trash_cleanup(id.clone()).await {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(crate::ServerError::Library(slipstream_core::LibraryError::Mutation(
            slipstream_core::MutationError::NotFound,
        ))) => cli_outcome_unknown(
            "Inspect the original cleanup attempt before submitting a replacement.",
            serde_json::json!({"operationId": id}),
        ),
        Err(error) => ApiError::from(error).into_response(),
    }
}
