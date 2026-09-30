use super::export::{read_export_json_body, valid_export_request_id};
use crate::http::{HttpState, cli_error};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{Request, Response, StatusCode, header},
};
use serde::Deserialize;
use slipstream_core::{WhiteBalanceIntent, XMP_CONTENT_TYPE, XmpCreateOutcome, XmpExportRecord};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BodyIn {
    request_id: String,
    expected_recipe_version: String,
    expected_source_revision: String,
}
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn error(status: StatusCode, code: &'static str, message: &'static str) -> Response<Body> {
    cli_error(status, code, message, serde_json::json!({}))
}
fn timestamp(value: i64) -> String {
    OffsetDateTime::from_unix_timestamp(value)
        .expect("persisted XMP timestamp")
        .format(&Rfc3339)
        .expect("RFC3339 timestamp")
}
fn wire(record: &XmpExportRecord) -> serde_json::Value {
    let standard = match record.white_balance {
        WhiteBalanceIntent::AsShot => vec!["Exposure2012", "WhiteBalance"],
        WhiteBalanceIntent::TemperatureTint { .. } => vec!["Exposure2012"],
    };
    serde_json::json!({
        "exportId": record.export_id, "photoId": record.photo_id,
        "target": "edit-state-xmp", "state": "succeeded",
        "recipeVersion": record.recipe_version, "sourceRevision": record.source_revision,
        "createdAt": timestamp(record.created_at), "expiresAt": timestamp(record.expires_at),
        "artifact": {"filename": record.filename, "contentType": XMP_CONTENT_TYPE, "byteLength": record.byte_length, "sha256": record.sha256},
        "parameterSupport": {"standard": standard, "slipstream": ["PhotoId", "RecipeVersion", "SourceRevision", "FilmRecipeSha256", "FilmProcedure", "WhiteBalance", "TemperatureKelvin", "TintMilli"]}
    })
}
pub(crate) async fn create(
    State(state): State<HttpState>,
    Path(photo_id): Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    let body: BodyIn = match read_export_json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    if !valid_export_request_id(&body.request_id)
        || body.expected_recipe_version.is_empty()
        || body.expected_source_revision.is_empty()
    {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_settings",
            "The request body is outside the closed wire shape",
        );
    }
    match state
        .application
        .library
        .create_xmp_export(
            &photo_id,
            &body.request_id,
            &body.expected_recipe_version,
            &body.expected_source_revision,
            now(),
        )
        .await
    {
        Ok(XmpCreateOutcome::Created(record)) => {
            crate::http::json_response(StatusCode::CREATED, &wire(&record))
        }
        Ok(XmpCreateOutcome::Replay(record)) => {
            crate::http::json_response(StatusCode::OK, &wire(&record))
        }
        Ok(XmpCreateOutcome::Conflict) => error(
            StatusCode::CONFLICT,
            "export_conflict",
            "The request identity was already used with a different payload",
        ),
        Ok(XmpCreateOutcome::Stale) => error(
            StatusCode::CONFLICT,
            "stale_edit",
            "The expected edit state is no longer current",
        ),
        Ok(XmpCreateOutcome::Expired) => error(
            StatusCode::GONE,
            "export_expired",
            "The request identity expired and cannot start new work",
        ),
        Ok(XmpCreateOutcome::NotFound) => error(
            StatusCode::NOT_FOUND,
            "unknown_photo",
            "The Photo is not part of the published Library",
        ),
        Ok(XmpCreateOutcome::MissingRecipe) => error(
            StatusCode::CONFLICT,
            "stale_edit",
            "The Photo has no confirmed edit recipe",
        ),
        Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "outcome_unknown",
            "The request outcome is unconfirmed",
        ),
    }
}
pub(crate) async fn list(
    State(state): State<HttpState>,
    Path(photo_id): Path<String>,
) -> Response<Body> {
    match state.application.library.photo_xmp_exports(&photo_id).await {
        Ok(Some(records)) => crate::http::json_response(
            StatusCode::OK,
            &serde_json::json!({"exports": records.iter().map(wire).collect::<Vec<_>>()}),
        ),
        Ok(None) => error(
            StatusCode::NOT_FOUND,
            "unknown_photo",
            "The Photo is not part of the published Library",
        ),
        Err(_) => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "resource_unavailable",
            "The XMP export state could not be read",
        ),
    }
}
pub(crate) async fn artifact(
    State(state): State<HttpState>,
    Path((photo_id, export_id)): Path<(String, String)>,
) -> Response<Body> {
    let record = match state.application.library.xmp_export(&export_id).await {
        Ok(Some(record)) if record.photo_id == photo_id => record,
        Ok(_) => {
            return error(
                StatusCode::NOT_FOUND,
                "unknown_export",
                "The XMP export identity is unknown",
            );
        }
        Err(_) => {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "resource_unavailable",
                "The XMP export state could not be read",
            );
        }
    };
    if record.expires_at <= now() {
        return error(
            StatusCode::GONE,
            "export_expired",
            "The XMP export artifact has expired",
        );
    }
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, XMP_CONTENT_TYPE)
        .header(header::CONTENT_LENGTH, record.byte_length.to_string())
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", record.filename),
        )
        .header(header::CACHE_CONTROL, "no-store")
        .header("x-content-type-options", "nosniff")
        .header("slipstream-artifact-export-id", &record.export_id)
        .header("slipstream-artifact-target", "edit-state-xmp")
        .header("slipstream-artifact-content-type", XMP_CONTENT_TYPE)
        .header("slipstream-artifact-filename", &record.filename)
        .header(
            "slipstream-artifact-byte-length",
            record.byte_length.to_string(),
        )
        .header("slipstream-artifact-sha256", &record.sha256)
        .header(
            "slipstream-artifact-created-at",
            timestamp(record.created_at),
        )
        .header(
            "slipstream-artifact-expires-at",
            timestamp(record.expires_at),
        )
        .body(Body::from(record.document))
        .expect("valid XMP artifact response")
}
