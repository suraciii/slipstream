//! Reviewed Location Recovery routes.
//!
//! Recovery reads one bounded review, retains its membership, and commits a
//! batch only when every submitted mapping still matches the identity the
//! Photographer reviewed. A CLI request carries the contract header and gets
//! the CLI error envelope; the Web gets the shared route envelope.

use super::cli::{CliBoundaryResult, query_token};
use super::*;
use crate::queries::{CursorError, QUERY_IDLE, RetainedKind, format_time};
use crate::recovery_review::{
    MAXIMUM_RECOVERY_APPLY, MAXIMUM_RECOVERY_MAPPINGS, MAXIMUM_RECOVERY_PAGE, RecoveryReviewItems,
};

/// One recovery route call: whether the CLI contract governs it, and the
/// operation name a bounded refusal reports.
#[derive(Clone, Copy)]
struct RecoveryRoute {
    cli: bool,
    operation: &'static str,
}

impl RecoveryRoute {
    fn unavailable(cli: bool) -> Self {
        Self {
            cli,
            operation: "recovery-unavailable",
        }
    }

    fn propose(cli: bool) -> Self {
        Self {
            cli,
            operation: "recovery-propose",
        }
    }

    fn apply(cli: bool) -> Self {
        Self {
            cli,
            operation: "recovery-apply",
        }
    }
}

/// Opens one bounded review of the active unavailable Photos.
pub(crate) async fn open_unavailable_review(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
    let route = RecoveryRoute::unavailable(request.headers().contains_key(CLI_CONTRACT_HEADER));
    if let Some(response) = recovery_boundary(&state, &request, route) {
        return response;
    }
    let body = match read_json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(object) = body.as_object() else {
        return invalid_recovery(route, "body", "Send a JSON object.");
    };
    let limit = match recovery_limit(route, object) {
        Ok(limit) => limit,
        Err(response) => return *response,
    };
    let items = match state.application.recovery_unavailable_items().await {
        Ok(items) => items,
        Err(error) => return recovery_server_error(route, error),
    };
    open_review(
        &state,
        route,
        RetainedKind::RecoveryUnavailable,
        RecoveryReviewItems::Unavailable(items),
        limit,
        SystemTime::now(),
    )
}

/// One later window of a retained unavailable-Photo review. Every item
/// reports the current state of its reviewed identity.
pub(crate) async fn unavailable_review_page(
    State(state): State<HttpState>,
    axum::extract::Path(cursor): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    let route = RecoveryRoute::unavailable(request.headers().contains_key(CLI_CONTRACT_HEADER));
    if let Some(response) = recovery_boundary(&state, &request, route) {
        return response;
    }
    let page = match review_page(&state, &cursor, RetainedKind::RecoveryUnavailable, route) {
        Ok(page) => page,
        Err(response) => return *response,
    };
    let RecoveryPageResponse {
        items,
        total,
        next_cursor,
        evaluated_at,
        expires_at,
    } = page;
    let RecoveryReviewItems::Unavailable(retained) = items else {
        unreachable!("one review retains one kind of membership");
    };
    let items = match state
        .application
        .recovery_identities(original_ids(&retained))
        .await
    {
        Ok(records) => retained
            .iter()
            .zip(records)
            .map(|(retained, record)| match record {
                Some(record) => {
                    RecoveryItemWire::from_record(&record, recovery_item_state(&record))
                }
                None => RecoveryItemWire::missing(retained),
            })
            .collect(),
        Err(error) => return recovery_server_error(route, error),
    };
    json_response(
        StatusCode::OK,
        &RecoveryListResponse {
            items,
            total,
            next_cursor,
            evaluated_at: format_time(evaluated_at),
            expires_at: expires_at.map(format_time),
        },
    )
}

/// Evaluates one Folder-prefix proposal batch, or one mapping for a single
/// Original. Proposals are inspectable and never write.
pub(crate) async fn open_proposal_review(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
    let route = RecoveryRoute::propose(request.headers().contains_key(CLI_CONTRACT_HEADER));
    if let Some(response) = recovery_boundary(&state, &request, route) {
        return response;
    }
    let body = match read_json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(object) = body.as_object() else {
        return invalid_recovery(route, "body", "Send a JSON object.");
    };
    if object.contains_key("oldPrefix") {
        if !object
            .keys()
            .all(|key| matches!(key.as_str(), "oldPrefix" | "newPrefix" | "limit"))
        {
            return invalid_recovery(route, "body", "Unknown key for a Folder-prefix proposal.");
        }
        let (Some(old_prefix), Some(new_prefix)) = (
            object["oldPrefix"].as_str(),
            object.get("newPrefix").and_then(Value::as_str),
        ) else {
            return invalid_recovery(route, "oldPrefix", "Name both prefixes as strings.");
        };
        if slipstream_core::parse_location_prefix(old_prefix).is_err() {
            return invalid_recovery(
                route,
                "oldPrefix",
                "A prefix is a Library-relative Folder Location with no leading or trailing separator.",
            );
        }
        if slipstream_core::parse_location_prefix(new_prefix).is_err() {
            return invalid_recovery(
                route,
                "newPrefix",
                "A prefix is a Library-relative Folder Location with no leading or trailing separator.",
            );
        }
        let limit = match recovery_limit(route, object) {
            Ok(limit) => limit,
            Err(response) => return *response,
        };
        let mappings = match state
            .application
            .recovery_propose_batch(old_prefix, new_prefix)
            .await
        {
            Ok(mappings) => mappings,
            Err(ServerError::RecoveryScope { evaluated }) => {
                return recovery_scope_exceeded(route, evaluated);
            }
            Err(error) => return recovery_server_error(route, error),
        };
        return open_review(
            &state,
            route,
            RetainedKind::RecoveryMappings,
            RecoveryReviewItems::Mappings(mappings),
            limit,
            SystemTime::now(),
        );
    }
    if !object
        .keys()
        .all(|key| matches!(key.as_str(), "originalId" | "newLocation"))
    {
        return invalid_recovery(route, "body", "Unknown key for a single mapping.");
    }
    let (Some(original_id), Some(new_location)) = (
        object.get("originalId").and_then(Value::as_str),
        object.get("newLocation").and_then(Value::as_str),
    ) else {
        return invalid_recovery(
            route,
            "originalId",
            "Name one Original and its new Location.",
        );
    };
    if !valid_id(original_id) {
        return invalid_recovery(
            route,
            "originalId",
            "The Original id is not a Library identity.",
        );
    }
    if slipstream_core::RelativeOriginalPath::parse(new_location.to_owned()).is_err() {
        return invalid_recovery(
            route,
            "newLocation",
            "The Location is a Library-relative Original Location including the filename.",
        );
    }
    match state
        .application
        .recovery_propose_single(original_id, new_location)
        .await
    {
        Ok(mapping) => json_response(
            StatusCode::OK,
            &RecoveryListResponse {
                items: vec![mapping],
                total: 1,
                next_cursor: None,
                evaluated_at: format_time(SystemTime::now()),
                expires_at: None,
            },
        ),
        Err(ServerError::PhotoNotFound) => recovery_unknown_original(route, original_id),
        Err(error) => recovery_server_error(route, error),
    }
}

/// One later window of a retained Folder-prefix proposal review.
pub(crate) async fn proposal_review_page(
    State(state): State<HttpState>,
    axum::extract::Path(cursor): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    let route = RecoveryRoute::propose(request.headers().contains_key(CLI_CONTRACT_HEADER));
    if let Some(response) = recovery_boundary(&state, &request, route) {
        return response;
    }
    let page = match review_page(&state, &cursor, RetainedKind::RecoveryMappings, route) {
        Ok(page) => page,
        Err(response) => return *response,
    };
    let RecoveryPageResponse {
        items,
        total,
        next_cursor,
        evaluated_at,
        expires_at,
    } = page;
    let RecoveryReviewItems::Mappings(items) = items else {
        unreachable!("one review retains one kind of membership");
    };
    json_response(
        StatusCode::OK,
        &RecoveryListResponse {
            items,
            total,
            next_cursor,
            evaluated_at: format_time(evaluated_at),
            expires_at: expires_at.map(format_time),
        },
    )
}

/// Commits one reviewed relocation batch. The whole batch commits atomically
/// or is refused with one reason per mapping.
pub(crate) async fn apply(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
    let route = RecoveryRoute::apply(request.headers().contains_key(CLI_CONTRACT_HEADER));
    if let Some(response) = recovery_boundary(&state, &request, route) {
        return response;
    }
    let body = match read_json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let Some(object) = body.as_object() else {
        return invalid_recovery(route, "body", "Send a JSON object.");
    };
    if object.keys().any(|key| key != "mappings") {
        return invalid_recovery(route, "body", "Send exactly one mappings key.");
    }
    let Some(items) = object.get("mappings").and_then(Value::as_array) else {
        return invalid_recovery(route, "mappings", "Send a JSON array of mappings.");
    };
    if items.is_empty() || items.len() > MAXIMUM_RECOVERY_APPLY {
        return invalid_recovery(
            route,
            "mappings",
            "The batch contains between 1 and the advertised apply bound of mappings.",
        );
    }
    let mut parsed = Vec::with_capacity(items.len());
    let mut original_ids = std::collections::HashSet::new();
    let mut locations = std::collections::HashSet::new();
    for item in items {
        let Some(item) = item.as_object() else {
            return invalid_recovery(route, "mappings", "Every mapping is a JSON object.");
        };
        let known = [
            "originalId",
            "newLocation",
            "mappingId",
            "confirmUnverifiedContent",
            "retirePhotoId",
        ];
        if item.keys().any(|key| !known.contains(&key.as_str())) {
            return invalid_recovery(route, "mappings", "Unknown key in a submitted mapping.");
        }
        let (Some(original_id), Some(new_location), Some(mapping_id)) = (
            item.get("originalId").and_then(Value::as_str),
            item.get("newLocation").and_then(Value::as_str),
            item.get("mappingId").and_then(Value::as_str),
        ) else {
            return invalid_recovery(
                route,
                "mappings",
                "Every mapping names its Original, Location, and reviewed identity.",
            );
        };
        if !valid_id(original_id) || mapping_id.is_empty() {
            return invalid_recovery(route, "mappings", "A mapping identity is not reviewable.");
        }
        if slipstream_core::RelativeOriginalPath::parse(new_location.to_owned()).is_err() {
            return invalid_recovery(
                route,
                "mappings",
                "A submitted Location is not a Library Location.",
            );
        }
        let confirm_unverified_content = match item.get("confirmUnverifiedContent") {
            None => false,
            Some(value) => match value.as_bool() {
                Some(value) => value,
                None => {
                    return invalid_recovery(
                        route,
                        "confirmUnverifiedContent",
                        "The content acknowledgement is a boolean.",
                    );
                }
            },
        };
        let retire_photo_id = match item.get("retirePhotoId") {
            None => None,
            Some(value) => match value.as_str().filter(|value| valid_id(value)) {
                Some(value) => Some(value.to_owned()),
                None => {
                    return invalid_recovery(
                        route,
                        "retirePhotoId",
                        "The retire choice names one Photo identity.",
                    );
                }
            },
        };
        if !original_ids.insert(original_id.to_owned())
            || !locations.insert(new_location.to_owned())
        {
            return invalid_recovery(
                route,
                "mappings",
                "One Original and one destination appear at most once.",
            );
        }
        parsed.push(crate::app::RecoveryApplyItem {
            original_id: original_id.to_owned(),
            new_location: new_location.to_owned(),
            mapping_id: mapping_id.to_owned(),
            confirm_unverified_content,
            retire_photo_id,
        });
    }
    match state.application.recovery_apply(parsed).await {
        Ok(response) => json_response(StatusCode::OK, &response),
        Err(crate::app::RecoveryApplyError::Invalid) => {
            invalid_recovery(route, "mappings", "The batch is not a reviewable set.")
        }
        Err(crate::app::RecoveryApplyError::Rejected {
            message,
            rejections,
            refused_mappings,
        }) => {
            if route.cli {
                cli_error(
                    StatusCode::CONFLICT,
                    "recovery_conflict",
                    message,
                    serde_json::json!({
                        "appliedMappings": 0,
                        "refusedMappings": refused_mappings,
                        "rejections": rejections,
                    }),
                )
            } else {
                json_response(
                    StatusCode::CONFLICT,
                    &RecoveryRejectionResponseWire {
                        message,
                        rejections,
                        applied_mappings: 0,
                        refused_mappings,
                    },
                )
            }
        }
        Err(crate::app::RecoveryApplyError::OutcomeUnknown { mappings }) => {
            // The batch committed before publication failed, so the response
            // must not read as a confirmed refusal or storage failure: the
            // submitted mapping identities name what may have committed.
            if route.cli {
                cli_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "outcome_unknown",
                    "The recovery batch may have committed; inspect the review before submitting again.",
                    serde_json::json!({ "mappings": mappings }),
                )
            } else {
                api_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "The recovery batch may have committed; inspect the review before submitting again.",
                )
            }
        }
        Err(crate::app::RecoveryApplyError::Server(error)) => recovery_server_error(route, error),
    }
}

/// The shared boundary of a recovery route: the CLI contract header for a CLI
/// request, and a published Library for one that evaluates the Library.
fn recovery_boundary(
    state: &HttpState,
    request: &Request<Body>,
    route: RecoveryRoute,
) -> Option<Response<Body>> {
    if route.cli {
        if let Err(response) = require_cli_contract(request) {
            return Some(*response);
        }
        if let Err(response) = require_published(&state.application) {
            return Some(*response);
        }
    }
    None
}

fn invalid_recovery(
    route: RecoveryRoute,
    argument: &'static str,
    reason: &'static str,
) -> Response<Body> {
    if route.cli {
        return invalid_cli(argument, reason);
    }
    api_error(StatusCode::BAD_REQUEST, "Invalid recovery request")
}

fn recovery_unknown_original(route: RecoveryRoute, original_id: &str) -> Response<Body> {
    if route.cli {
        return cli_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "The Original is not an active unavailable Photo.",
            serde_json::json!({"resource": "original", "reference": original_id}),
        );
    }
    api_error(StatusCode::NOT_FOUND, "Unknown unavailable Original")
}

fn recovery_scope_exceeded(route: RecoveryRoute, evaluated: usize) -> Response<Body> {
    if route.cli {
        return cli_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "recovery_scope_exceeded",
            "Narrow the Folder prefix before reviewing this scope.",
            serde_json::json!({"evaluated": evaluated, "limit": MAXIMUM_RECOVERY_MAPPINGS}),
        );
    }
    api_error(
        StatusCode::PAYLOAD_TOO_LARGE,
        "Recovery scope exceeds the advertised bound; narrow the Folder prefix",
    )
}

fn recovery_server_error(route: RecoveryRoute, error: ServerError) -> Response<Body> {
    if route.cli {
        return match &error {
            ServerError::QueryCapacity => cli_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "server_busy",
                "Narrow the review before trying again.",
                serde_json::json!({"operation": route.operation, "retryAfterSeconds": null}),
            ),
            ServerError::RecoveryScope { evaluated } => recovery_scope_exceeded(route, *evaluated),
            _ => cli_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "storage_failed",
                "Inspect server health and try again.",
                serde_json::json!({"operation": route.operation}),
            ),
        };
    }
    ApiError::from(error).into_response()
}

/// One requested review window, or an error response the caller returns.
fn recovery_limit(
    route: RecoveryRoute,
    object: &serde_json::Map<String, Value>,
) -> CliBoundaryResult<usize> {
    match object.get("limit") {
        None => Ok(MAXIMUM_RECOVERY_PAGE),
        Some(value) => match value
            .as_u64()
            .filter(|limit| (1..=MAXIMUM_RECOVERY_PAGE as u64).contains(limit))
        {
            Some(limit) => Ok(limit as usize),
            None => Err(Box::new(invalid_recovery(
                route,
                "limit",
                "The limit is between 1 and the advertised review page bound.",
            ))),
        },
    }
}

/// Retains one evaluated review and returns its first window.
fn open_review(
    state: &HttpState,
    route: RecoveryRoute,
    kind: RetainedKind,
    items: RecoveryReviewItems,
    limit: usize,
    evaluated_at: SystemTime,
) -> Response<Body> {
    let total = items.len();
    let token = query_token(&state.application, kind);
    let window = match &items {
        RecoveryReviewItems::Unavailable(items) => {
            RecoveryReviewItems::Unavailable(items.iter().take(limit).cloned().collect())
        }
        RecoveryReviewItems::Mappings(items) => {
            RecoveryReviewItems::Mappings(items.iter().take(limit).cloned().collect())
        }
    };
    let next_cursor = (total > limit).then(|| {
        state.application.cursor_signer.query_cursor(
            state.application.browse_namespace,
            kind,
            &token,
            limit,
            limit,
        )
    });
    if next_cursor.is_some()
        && state
            .application
            .recovery_reviews
            .lock()
            .expect("recovery reviews poisoned")
            .insert(token, items, Instant::now(), evaluated_at)
            .is_err()
    {
        return recovery_server_error(route, ServerError::QueryCapacity);
    }
    let expires_at = next_cursor.as_ref().map(|_| evaluated_at + QUERY_IDLE);
    let expires_at = expires_at.map(format_time);
    let evaluated_at = format_time(evaluated_at);
    match window {
        RecoveryReviewItems::Unavailable(items) => json_response(
            StatusCode::OK,
            &RecoveryListResponse {
                items,
                total,
                next_cursor,
                evaluated_at,
                expires_at,
            },
        ),
        RecoveryReviewItems::Mappings(items) => json_response(
            StatusCode::OK,
            &RecoveryListResponse {
                items,
                total,
                next_cursor,
                evaluated_at,
                expires_at,
            },
        ),
    }
}

/// One later window of a retained review.
fn review_page(
    state: &HttpState,
    cursor: &str,
    kind: RetainedKind,
    route: RecoveryRoute,
) -> CliBoundaryResult<RecoveryPageResponse> {
    let cursor = match state.application.cursor_signer.parse_query_cursor(
        cursor,
        state.application.browse_namespace,
        kind,
    ) {
        Ok(cursor) => cursor,
        Err(error) => return Err(Box::new(recovery_cursor_error(route, kind, error))),
    };
    let page = state
        .application
        .recovery_reviews
        .lock()
        .expect("recovery reviews poisoned")
        .page(
            &cursor.token,
            kind,
            cursor.offset,
            cursor.limit,
            Instant::now(),
            SystemTime::now(),
        );
    let Some(page) = page else {
        return Err(Box::new(recovery_cursor_error(
            route,
            kind,
            CursorError::Idle,
        )));
    };
    let next_offset = cursor.offset.saturating_add(page.items.len());
    let next_cursor = (next_offset < page.total).then(|| {
        state.application.cursor_signer.query_cursor(
            state.application.browse_namespace,
            kind,
            &cursor.token,
            next_offset,
            cursor.limit,
        )
    });
    let expires_at = next_cursor.as_ref().map(|_| page.expires_at);
    Ok(RecoveryPageResponse {
        items: page.items,
        total: page.total,
        next_cursor,
        evaluated_at: page.evaluated_at,
        expires_at,
    })
}

/// The reviewed cursor name one continuation reports in a refusal.
fn recovery_cursor_kind(kind: RetainedKind) -> &'static str {
    match kind {
        RetainedKind::RecoveryUnavailable => "unavailable",
        RetainedKind::RecoveryMappings => "mappings",
        _ => "review",
    }
}

fn recovery_cursor_error(
    route: RecoveryRoute,
    kind: RetainedKind,
    error: CursorError,
) -> Response<Body> {
    if route.cli {
        let reason = match error {
            CursorError::Invalid => {
                return invalid_cli("cursor", "The cursor is malformed or has the wrong kind.");
            }
            CursorError::ProcessRestarted => "process_restarted",
            CursorError::PublicationReplaced | CursorError::Idle => "idle_or_evicted",
        };
        return cli_error(
            StatusCode::GONE,
            "cursor_expired",
            "Open a fresh recovery review.",
            serde_json::json!({"cursorKind": recovery_cursor_kind(kind), "reason": reason}),
        );
    }
    api_error(
        StatusCode::CONFLICT,
        "The recovery review expired; open a fresh review",
    )
}

/// One retained review window before it becomes a wire response.
struct RecoveryPageResponse {
    items: RecoveryReviewItems,
    total: usize,
    next_cursor: Option<String>,
    evaluated_at: SystemTime,
    expires_at: Option<SystemTime>,
}

fn original_ids(items: &[RecoveryItemWire]) -> Vec<String> {
    items.iter().map(|item| item.original_id.clone()).collect()
}
