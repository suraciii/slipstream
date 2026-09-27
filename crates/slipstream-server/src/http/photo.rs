// Photo metadata, Preview, and derivative route handlers.
use axum::{
    body::Body,
    extract::State,
    http::{Request, Response, StatusCode, header},
    response::IntoResponse,
};
use slipstream_core::DerivativeTarget;
use std::sync::Arc;

use super::{
    ApiError, CLI_CONTRACT_HEADER, HttpState, api_error, cli_error, invalid_cli, is_hex_key,
    json_response, require_cli_contract, require_published, valid_id,
};
use crate::Application;
use crate::{
    app::CliPreviewRefusal,
    wire::{
        CliPreviewResponse, DerivativeDelivery, PhotoMetadataWire, derivative_target_name,
        photo_web_path,
    },
};
pub(crate) async fn get_thumbnail(
    State(state): State<HttpState>,
    axum::extract::Path(id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    if request.headers().contains_key(CLI_CONTRACT_HEADER) {
        if let Err(response) = require_cli_contract(&request) {
            return *response;
        }
        if let Err(response) = require_published(&state.application) {
            return *response;
        }
        return cli_preview_response(
            Arc::clone(&state.application),
            &id,
            DerivativeTarget::Thumbnail512,
        )
        .await;
    }
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

pub(crate) async fn get_external_metadata(
    State(state): State<HttpState>,
    axum::extract::Path(photo_id): axum::extract::Path<String>,
) -> Response<Body> {
    if !valid_id(&photo_id) {
        return metadata_invalid_input("Invalid Photo");
    }
    match state.application.external_metadata_read(&photo_id).await {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(failure) => metadata_error_response(failure),
    }
}

pub(crate) async fn post_external_metadata(
    State(state): State<HttpState>,
    axum::extract::Path(photo_id): axum::extract::Path<String>,
    body: Result<axum::body::Bytes, axum::extract::rejection::BytesRejection>,
) -> Response<Body> {
    if !valid_id(&photo_id) {
        return metadata_invalid_input("Invalid Photo");
    }
    let body = match body {
        Ok(body) => body,
        Err(failure) if failure.status() == StatusCode::PAYLOAD_TOO_LARGE => {
            return metadata_body_limit();
        }
        Err(_) => return metadata_invalid_input("Save Metadata request could not be read"),
    };
    if body.len() > 2 * 1024 * 1024 {
        return metadata_body_limit();
    }
    let request: crate::metadata_wire::MetadataSaveRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(failure) => {
            return metadata_invalid_input(format!("Invalid Save Metadata request: {failure}"));
        }
    };
    match state
        .application
        .external_metadata_save(&photo_id, request)
        .await
    {
        Ok(result) => json_response(StatusCode::OK, &result),
        Err(failure) => metadata_error_response(failure),
    }
}

fn metadata_body_limit() -> Response<Body> {
    metadata_error_response(crate::metadata_wire::MetadataError {
        code: crate::metadata_wire::MetadataErrorCode::ResourceLimit,
        message: "Save Metadata request exceeds the 2 MiB body limit".into(),
        details: serde_json::Value::Null,
    })
}

fn metadata_invalid_input(message: impl Into<String>) -> Response<Body> {
    metadata_error_response(crate::metadata_wire::MetadataError {
        code: crate::metadata_wire::MetadataErrorCode::InvalidInput,
        message: message.into(),
        details: serde_json::Value::Null,
    })
}

fn metadata_error_response(failure: crate::metadata_wire::MetadataError) -> Response<Body> {
    let status = StatusCode::from_u16(crate::metadata_wire::http_status(failure.code))
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    json_response(
        status,
        &crate::metadata_wire::MetadataErrorEnvelope { error: failure },
    )
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
    if request.headers().contains_key(CLI_CONTRACT_HEADER) {
        if let Err(response) = require_cli_contract(&request) {
            return *response;
        }
        if let Err(response) = require_published(&state.application) {
            return *response;
        }
        return cli_preview_response(
            Arc::clone(&state.application),
            &id,
            DerivativeTarget::Review2560,
        )
        .await;
    }
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

/// Answers one CLI Preview request. The CLI contract header is already validated
/// by the request policy, so a request that carries it never reaches the Web
/// answer.
pub(crate) async fn cli_preview_response(
    application: Arc<Application>,
    photo_id: &str,
    target: DerivativeTarget,
) -> Response<Body> {
    if !valid_id(photo_id) {
        return invalid_cli("photoId", "The Photo ID is invalid.");
    }
    match application.cli_preview(photo_id, target).await {
        Ok(ready) => json_response(
            StatusCode::OK,
            &CliPreviewResponse {
                photo_id: ready.photo_id,
                state: "ready",
                source: Some(ready.source.wire_name()),
                source_revision: Some(ready.source_revision),
                width: Some(ready.width),
                height: Some(ready.height),
                detail_limited: Some(ready.width.max(ready.height) < 2560),
                url: Some(format!(
                    "/api/private/derivatives/{}/{}/{}.jpg",
                    photo_id,
                    derivative_target_name(target),
                    ready.cache_key
                )),
                web_path: photo_web_path(photo_id),
            },
        ),
        Err(refusal) => cli_preview_refusal(photo_id, refusal),
    }
}

fn cli_preview_refusal(photo_id: &str, refusal: CliPreviewRefusal) -> Response<Body> {
    match refusal {
        CliPreviewRefusal::Missing => cli_error(
            StatusCode::NOT_FOUND,
            "not_found",
            "Query Photos and use a current Photo ID.",
            serde_json::json!({"resource": "photo", "reference": photo_id}),
        ),
        CliPreviewRefusal::Busy => cli_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "server_busy",
            "Preview work is at its shared limit; try again.",
            serde_json::json!({"operation": "photos-preview", "retryAfterSeconds": null}),
        ),
        CliPreviewRefusal::Unavailable => cli_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "preview_unavailable",
            "No allowed source can produce a current Preview for this Photo.",
            serde_json::json!({"photoId": photo_id, "state": "unavailable"}),
        ),
        CliPreviewRefusal::NotReady(state) => cli_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "preview_unavailable",
            "Request the current Preview again for this Photo.",
            serde_json::json!({"photoId": photo_id, "state": state}),
        ),
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
    let cli_request = request.headers().contains_key(CLI_CONTRACT_HEADER);
    if cli_request {
        if let Err(response) = require_cli_contract(&request) {
            return *response;
        }
        if let Err(response) = require_published(&state.application) {
            return *response;
        }
    }
    let delivery = if cli_request {
        state
            .application
            .cli_derivative(&photo_id, key, target)
            .await
    } else {
        state.application.derivative(&photo_id, key, target).await
    };
    let delivery = match delivery {
        Ok(Some(delivery)) => delivery,
        Ok(None) => return api_error(StatusCode::NOT_FOUND, "Derivative not found"),
        Err(error) => return ApiError::from(error).into_response(),
    };
    let repeated = cli_preview_headers(&delivery);
    let entity_tag = format!("\"{}\"", delivery.cache_key);
    if request
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        == Some(entity_tag.as_str())
    {
        let mut response = Response::builder()
            .status(StatusCode::NOT_MODIFIED)
            .header(header::ETAG, entity_tag);
        for (name, value) in &repeated {
            response = response.header(*name, value.clone());
        }
        return response.body(Body::empty()).expect("valid response");
    }
    let length = delivery.bytes.len().to_string();
    let body = if request.method() == ::http::Method::HEAD {
        Body::empty()
    } else {
        Body::from(delivery.bytes)
    };
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/jpeg")
        .header(header::CONTENT_LENGTH, length)
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::ETAG, entity_tag)
        .header("x-content-type-options", "nosniff");
    for (name, value) in &repeated {
        response = response.header(*name, value.clone());
    }
    response.body(body).expect("valid derivative response")
}

/// The repeated typed facts one CLI derivative download must match against the
/// metadata it was admitted with. The Web derivative response adds nothing.
pub(crate) fn cli_preview_headers(delivery: &DerivativeDelivery) -> Vec<(&'static str, String)> {
    delivery
        .cli_facts
        .as_ref()
        .map(|facts| facts.headers().to_vec())
        .unwrap_or_default()
}
