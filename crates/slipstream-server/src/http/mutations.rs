// Album, Photo decision, and removal mutation handlers.
use axum::{
    body::Body,
    extract::State,
    http::{Request, Response, StatusCode},
    response::IntoResponse,
};
use serde::Deserialize;
use serde_json::Value;
use slipstream_core::{LibraryError, SelectionState};

use super::{
    ALBUM_PHOTO_IDS_MAX, ApiError, CLI_CONTRACT_HEADER, HttpState, api_error, cli_error,
    has_exact_keys, invalid_cli, json_response, mutate_album_route, read_cli_json_body,
    read_json_body, require_cli_contract, selection_state, valid_batch_selection, valid_id,
    valid_ids, valid_name, valid_rating, valid_selection,
};
use crate::{
    folders::valid_folder_location,
    wire::{CliAlbumChangeWire, CliAlbumCreationWire, CliPhotoDecisionResultWire},
};
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
    ) -> super::cli::CliBoundaryResult<slipstream_core::CheckedAlbumMutation> {
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
) -> super::cli::CliBoundaryResult<slipstream_core::CheckedAlbumMutation> {
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

/// One bounded, version-checked Photo decision batch from a CLI client. One
/// field and value apply to every requested Photo; each item names the
/// observed decision version the caller intends to write against.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CliPhotoDecisionBody {
    field: String,
    value: Value,
    photos: Vec<CliPhotoDecisionPhotoBody>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CliPhotoDecisionPhotoBody {
    photo_id: String,
    if_version: String,
}

impl CliPhotoDecisionBody {
    fn into_mutation(
        self,
    ) -> super::cli::CliBoundaryResult<slipstream_core::CheckedPhotoDecisionMutation> {
        let field = match self.field.as_str() {
            "selectionState" => slipstream_core::PhotoStateField::SelectionState,
            "rating" => slipstream_core::PhotoStateField::Rating,
            _ => {
                return Err(Box::new(invalid_cli(
                    "field",
                    "The decision field must be selectionState or rating.",
                )));
            }
        };
        let value = match field {
            slipstream_core::PhotoStateField::SelectionState => self
                .value
                .as_str()
                .and_then(|value| match value {
                    "undecided" => Some(SelectionState::Undecided),
                    "selected" => Some(SelectionState::Selected),
                    "rejected" => Some(SelectionState::Rejected),
                    _ => None,
                })
                .map(slipstream_core::PhotoStateValue::Selection),
            slipstream_core::PhotoStateField::Rating => self
                .value
                .as_u64()
                .filter(|rating| *rating <= slipstream_core::MAXIMUM_PHOTO_RATING as u64)
                .map(|rating| slipstream_core::PhotoStateValue::Rating(rating as u8)),
        };
        let Some(value) = value else {
            return Err(Box::new(invalid_cli(
                "value",
                "The decision value must match the field's type and range.",
            )));
        };
        if self.photos.len() > slipstream_core::PHOTO_STATE_BATCH_MAX {
            return Err(Box::new(cli_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "limit_exceeded",
                "Reduce the Photo ID list and try again.",
                serde_json::json!({
                    "limitName": "photoIds",
                    "limit": slipstream_core::PHOTO_STATE_BATCH_MAX,
                    "actual": self.photos.len()
                }),
            )));
        }
        if self.photos.is_empty()
            || self
                .photos
                .iter()
                .any(|photo| !valid_id(&photo.photo_id) || photo.if_version.is_empty())
            || self
                .photos
                .iter()
                .map(|photo| photo.photo_id.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != self.photos.len()
        {
            return Err(Box::new(invalid_cli(
                "photos",
                "Photo items must be a nonempty ordered list of distinct valid Photo IDs with nonempty versions.",
            )));
        }
        Ok(slipstream_core::CheckedPhotoDecisionMutation {
            field,
            value,
            photos: self
                .photos
                .into_iter()
                .map(|photo| slipstream_core::CheckedPhotoDecisionItem {
                    photo_id: photo.photo_id,
                    expected_version: photo.if_version,
                })
                .collect(),
        })
    }
}

/// Applies one checked, atomic Photo decision batch for a negotiated CLI
/// request. Per-Photo conflicts and missing records are domain results in the
/// response body; only validation and storage failures are errors.
pub(crate) async fn mutate_photo_decision(
    State(state): State<HttpState>,
    request: Request<Body>,
) -> Response<Body> {
    if let Err(response) = require_cli_contract(&request) {
        return *response;
    }
    let body: CliPhotoDecisionBody = match read_cli_json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let mutation = match body.into_mutation() {
        Ok(mutation) => mutation,
        Err(response) => return *response,
    };
    match state
        .application
        .mutate_photo_decision_checked(mutation)
        .await
    {
        Ok(result) => json_response(StatusCode::OK, &CliPhotoDecisionResultWire::from(result)),
        Err(error) => map_photo_decision_write_error(error),
    }
}

fn map_photo_decision_write_error(error: LibraryError) -> Response<Body> {
    match error {
        LibraryError::PhotoDecisionWrite(slipstream_core::PhotoDecisionWriteError::Invalid) => {
            invalid_cli("body", "The Photo decision batch is invalid.")
        }
        LibraryError::PhotoDecisionWrite(
            slipstream_core::PhotoDecisionWriteError::LimitExceeded { limit, actual },
        ) => cli_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "limit_exceeded",
            "Reduce the Photo ID list and try again.",
            serde_json::json!({"limitName": "photoIds", "limit": limit, "actual": actual}),
        ),
        _ => cli_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage_failed",
            "Inspect server health and the current Photo decisions before trying again.",
            serde_json::json!({"operation": "photos-set"}),
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
