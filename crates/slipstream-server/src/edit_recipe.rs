//! Edit Recipe HTTP surface: one read, one guarded save, and one explicit
//! rebind per Photo. The routes share the same operations for Web and CLI
//! clients, validate the CLI contract header when it is present, and map
//! `EditRecipeWriteOutcome` onto the closed outcome and error-code sets of
//! the Photo Development Surface contract.

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use slipstream_core::{
    CaptureReviewMetadata, EditRecipe, EditRecipeRead, EditRecipeSettings, EditRecipeWriteOutcome,
    OriginalKind, PhotoRead, RebindEditRecipe, SaveEditRecipe, WhiteBalanceIntent,
};
use slipstream_processing::photo_profile::{
    self, APPROVED_EXPOSURE_MILLI_EV_MAX, APPROVED_EXPOSURE_MILLI_EV_MIN,
};

use crate::{
    ProcessingConfig,
    http::{
        CLI_CONTRACT_HEADER, HttpState, cli_error, invalid_cli, read_cli_json_body, read_json_body,
        require_cli_contract, require_published, valid_id,
    },
};

/// The approved white-balance mode of the first workload. The core state
/// layer only stores this mode, so every other wire value is invalid input.
const APPROVED_WHITE_BALANCE: &str = "as-shot";

/// Mirrors the core request-identity bound; the persistence owner enforces
/// the same limit again before admitting a write.
const MAXIMUM_REQUEST_ID_BYTES: usize = 128;

// ---------------------------------------------------------------- support state

/// The support state of one Photo's source class against the approved
/// profiles. States are the closed stage values of the contract: `ready`,
/// `unavailable`, or `unsupported`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SupportWire {
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    profile_id: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
}

/// Whether the develop execution can process this Photo right now. A stored
/// recipe outside the approved range stays readable, but reports processing
/// as unavailable instead of being rewritten.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProcessingWire {
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExposureRangeWire {
    pub(crate) min_ev: f64,
    pub(crate) max_ev: f64,
    pub(crate) step_ev: f64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ControlsWire {
    exposure: ExposureRangeWire,
    white_balance: [&'static str; 1],
}

/// The approved control ranges. They are deployment constants of the
/// approved profile set, so they do not depend on one Photo's support state.
pub(crate) fn approved_controls() -> ControlsWire {
    ControlsWire {
        exposure: approved_exposure_range(),
        white_balance: [APPROVED_WHITE_BALANCE],
    }
}

/// The approved finite exposure range of the qualified workload in EV.
pub(crate) fn approved_exposure_range() -> ExposureRangeWire {
    ExposureRangeWire {
        min_ev: APPROVED_EXPOSURE_MILLI_EV_MIN as f64 / 1000.0,
        max_ev: APPROVED_EXPOSURE_MILLI_EV_MAX as f64 / 1000.0,
        step_ev: 0.001,
    }
}

/// The source facts one support derivation needs beside the availability.
#[derive(Clone, Copy)]
struct SourceFacts<'a> {
    kind: OriginalKind,
    filename: &'a str,
    make: Option<&'a str>,
    model: Option<&'a str>,
    processing: Option<&'a ProcessingConfig>,
}

fn derive_support(facts: SourceFacts<'_>, source_available: bool) -> SupportWire {
    // A JPEG source is known-unapproved by its kind. For a RAW source the
    // class identity needs the filename container and the observed camera
    // make and model. The metadata read deliberately returns defaults when
    // inspection is saturated, the source changed mid-read, or it cannot be
    // opened, so a missing identity means the class is unavailable to
    // observe rather than unlisted.
    let Some(container) = (facts.kind == OriginalKind::Raw)
        .then(|| photo_profile::container_of_filename(facts.filename))
        .flatten()
    else {
        return SupportWire {
            state: "unavailable",
            profile_id: None,
            reason: Some("source-class-unobservable"),
        };
    };
    let (Some(make), Some(model)) = (facts.make, facts.model) else {
        return SupportWire {
            state: "unavailable",
            profile_id: None,
            reason: Some("camera-identity-unavailable"),
        };
    };
    match photo_profile::classify(make, model, &container) {
        // The identity is observed, so a failed match is a known-unapproved
        // class.
        None => SupportWire {
            state: "unsupported",
            profile_id: None,
            reason: None,
        },
        Some(profile) if facts.processing.is_none() => SupportWire {
            state: "unavailable",
            profile_id: Some(profile.profile_id),
            reason: Some("operator-disabled"),
        },
        Some(profile) if !source_available => SupportWire {
            state: "unavailable",
            profile_id: Some(profile.profile_id),
            reason: Some("source-unavailable"),
        },
        Some(profile) => SupportWire {
            state: "ready",
            profile_id: Some(profile.profile_id),
            reason: None,
        },
    }
}

// True when the enabled execution payload can represent the stored value:
/// a finite multiple of one thousandth of an EV inside the approved range.
fn representable(settings: &EditRecipeSettings) -> bool {
    let milli = settings.exposure_ev * 1000.0;
    let rounded = milli.round();
    milli.is_finite()
        && (milli - rounded).abs() < 1e-6
        && rounded >= APPROVED_EXPOSURE_MILLI_EV_MIN as f64
        && rounded <= APPROVED_EXPOSURE_MILLI_EV_MAX as f64
}

fn processing_state(support: &SupportWire, recipe: Option<&EditRecipe>) -> ProcessingWire {
    match support.state {
        "ready" => match recipe {
            Some(recipe) if !representable(&recipe.settings) => ProcessingWire {
                state: "unavailable",
                reason: Some("recipe-not-representable"),
            },
            _ => ProcessingWire {
                state: "ready",
                reason: None,
            },
        },
        "unsupported" => ProcessingWire {
            state: "unsupported",
            reason: None,
        },
        _ => ProcessingWire {
            state: "unavailable",
            reason: support.reason,
        },
    }
}

// ---------------------------------------------------------------- wire shapes

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EditRecipeWire {
    revision: String,
    source_revision: String,
    settings: SettingsWire,
}

impl From<EditRecipe> for EditRecipeWire {
    fn from(recipe: EditRecipe) -> Self {
        Self {
            revision: recipe.revision,
            source_revision: recipe.source_revision,
            settings: SettingsWire {
                exposure_ev: recipe.settings.exposure_ev,
                white_balance: white_balance_name(recipe.settings.white_balance),
            },
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SettingsWire {
    exposure_ev: f64,
    white_balance: &'static str,
}

fn white_balance_name(intent: WhiteBalanceIntent) -> &'static str {
    match intent {
        WhiteBalanceIntent::AsShot => APPROVED_WHITE_BALANCE,
    }
}

/// One recipe read: the current recipe or its absence, the observed source
/// revision, the source support state, and the approved control ranges.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EditRecipeResponse {
    photo_id: String,
    recipe: Option<EditRecipeWire>,
    source_revision: String,
    support: SupportWire,
    processing: ProcessingWire,
    controls: ControlsWire,
}

/// One guarded write result. `outcome` is the closed contract outcome; the
/// remaining facts let the client continue without a second read.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EditRecipeWriteResponse {
    outcome: &'static str,
    recipe: EditRecipeWire,
    source_revision: String,
    support: SupportWire,
    processing: ProcessingWire,
    controls: ControlsWire,
}

// ---------------------------------------------------------------- request bodies

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveEditRecipeBody {
    request_id: String,
    expected_recipe_revision: Option<String>,
    expected_source_revision: String,
    settings: SettingsBody,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingsBody {
    exposure_ev: f64,
    white_balance: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RebindEditRecipeBody {
    expected_recipe_revision: String,
    expected_source_revision: String,
}

/// Reads one typed body. CLI requests use the CLI reader; Web requests use
/// the plain reader. Every body failure on these routes maps onto the closed
/// error-code set: an oversize body is `limit_exceeded` and every other
/// shape refusal is `invalid_settings`, so a client cannot confuse a
/// malformed payload with out-of-range settings.
async fn read_recipe_body<T: serde::de::DeserializeOwned>(
    request: Request<Body>,
) -> Result<T, Response<Body>> {
    let cli = request.headers().contains_key(CLI_CONTRACT_HEADER);
    let parsed = if cli {
        read_cli_json_body(request).await
    } else {
        let value = read_json_body(request)
            .await
            .map_err(|response| closed_body_error(response.status()))?;
        serde_json::from_value(value).map_err(|_| body_shape_error())
    };
    parsed.map_err(|response| closed_body_error(response.status()))
}

/// Maps one body-read failure onto the closed error-code set: an oversize
/// body is `limit_exceeded` and every other refusal is `invalid_settings`.
fn closed_body_error(status: StatusCode) -> Response<Body> {
    if status == StatusCode::PAYLOAD_TOO_LARGE {
        cli_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "limit_exceeded",
            "Reduce the request body and try again.",
            serde_json::json!({
                "limitName": "requestBodyBytesMaximum",
                "limit": crate::MAXIMUM_MUTATION_BODY_BYTES,
                "actual": crate::MAXIMUM_MUTATION_BODY_BYTES + 1
            }),
        )
    } else {
        body_shape_error()
    }
}

fn body_shape_error() -> Response<Body> {
    settings_error("body", "The body is malformed or contains unknown fields.")
}

// ---------------------------------------------------------------- error mapping

fn settings_error(argument: &'static str, reason: &'static str) -> Response<Body> {
    cli_error(
        StatusCode::BAD_REQUEST,
        "invalid_settings",
        "Correct the settings against the approved ranges and shape.",
        serde_json::json!({"argument": argument, "reason": reason}),
    )
}

fn unknown_photo(photo_id: &str) -> Response<Body> {
    cli_error(
        StatusCode::NOT_FOUND,
        "unknown_photo",
        "Query Photos and use a current Photo ID.",
        serde_json::json!({"resource": "photo", "reference": photo_id}),
    )
}

/// Persistence and read failures are service-availability failures of these
/// routes, so they carry the closed `processing_unavailable` code with the
/// operation named in the details.
fn storage_error(operation: &'static str) -> Response<Body> {
    cli_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "processing_unavailable",
        "The service cannot read or write the current facts; inspect server health before trying again.",
        serde_json::json!({"operation": operation}),
    )
}

fn unsupported_photo(photo_id: &str, support: &SupportWire) -> Response<Body> {
    cli_error(
        StatusCode::CONFLICT,
        "unsupported_photo",
        "This Photo's source class has no approved profile.",
        serde_json::json!({"photoId": photo_id, "support": support}),
    )
}

/// Current facts for one conflict-family outcome, so the client can recover
/// without a second read.
fn conflict_details(
    photo_id: &str,
    read: &EditRecipeRead,
    support: &SupportWire,
) -> serde_json::Value {
    serde_json::json!({
        "photoId": photo_id,
        "recipeRevision": read.recipe.as_ref().map(|recipe| recipe.revision.clone()),
        "sourceRevision": read.current_source_revision,
        "support": support,
    })
}

fn conflict_response(
    code: &'static str,
    message: &'static str,
    photo_id: &str,
    read: EditRecipeRead,
    support: SupportWire,
) -> Response<Body> {
    cli_error(
        StatusCode::CONFLICT,
        code,
        message,
        conflict_details(photo_id, &read, &support),
    )
}

// ---------------------------------------------------------------- shared reads

/// One serialized read of the recipe facts, Photo facts, and the bounded
/// capture metadata. The recipe read owns the combined source availability
/// and the unknown-Photo refusal.
async fn load_facts(
    state: &HttpState,
    photo_id: &str,
) -> Result<(PhotoRead, CaptureReviewMetadata, EditRecipeRead), Response<Body>> {
    let read = match state.application.library.edit_recipe(photo_id).await {
        Ok(Some(read)) => read,
        Ok(None) => return Err(unknown_photo(photo_id)),
        Err(_) => return Err(storage_error("edit-recipe-read")),
    };
    let photo = match state.application.library.photo(photo_id).await {
        Ok(Some(photo)) => photo,
        Ok(None) => return Err(unknown_photo(photo_id)),
        Err(_) => return Err(storage_error("edit-recipe-read")),
    };
    let metadata = state
        .application
        .photo_metadata(photo_id)
        .await
        .map_err(|error| crate::http::ApiError::from(error).into_response())?;
    Ok((photo, metadata, read))
}

fn source_facts<'a>(
    photo: &'a PhotoRead,
    metadata: &'a CaptureReviewMetadata,
    state: &'a HttpState,
) -> SourceFacts<'a> {
    SourceFacts {
        kind: photo.original_kind,
        filename: &photo.filename,
        make: metadata.make.as_deref(),
        model: metadata.model.as_deref(),
        processing: state.processing.as_ref(),
    }
}

// ---------------------------------------------------------------- handlers

/// `GET /api/photos/{id}/edit-recipe`
pub(crate) async fn get_edit_recipe(
    State(state): State<HttpState>,
    axum::extract::Path(photo_id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    if request.headers().contains_key(CLI_CONTRACT_HEADER)
        && let Err(response) = require_cli_contract(&request)
    {
        return *response;
    }
    if let Err(response) = require_published(&state.application) {
        return *response;
    }
    if !valid_id(&photo_id) {
        return invalid_cli("photoId", "The Photo ID is invalid.");
    }
    // One serialized recipe read owns every response fact, so a rescan
    // between reads cannot mix an old recipe with newer support facts.
    let (photo, metadata, read) = match load_facts(&state, &photo_id).await {
        Ok(facts) => facts,
        Err(response) => return response,
    };
    let support = derive_support(
        source_facts(&photo, &metadata, &state),
        read.source_available,
    );
    let processing = processing_state(&support, read.recipe.as_ref());
    ok_response(&EditRecipeResponse {
        photo_id,
        recipe: read.recipe.map(EditRecipeWire::from),
        source_revision: read.current_source_revision,
        support,
        processing,
        controls: approved_controls(),
    })
}

/// `POST /api/photos/{id}/edit-recipe`: one guarded save carrying a stable
/// request identity and both expected revisions.
pub(crate) async fn post_edit_recipe(
    State(state): State<HttpState>,
    axum::extract::Path(photo_id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    if request.headers().contains_key(CLI_CONTRACT_HEADER)
        && let Err(response) = require_cli_contract(&request)
    {
        return *response;
    }
    if let Err(response) = require_published(&state.application) {
        return *response;
    }
    if !valid_id(&photo_id) {
        return invalid_cli("photoId", "The Photo ID is invalid.");
    }
    let body = match read_recipe_body::<SaveEditRecipeBody>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let settings = match validated_settings(&body) {
        Some(settings) => settings,
        None => return invalid_settings_response(),
    };
    let (photo, metadata, read) = match load_facts(&state, &photo_id).await {
        Ok(facts) => facts,
        Err(response) => return response,
    };
    let facts = source_facts(&photo, &metadata, &state);
    let support = derive_support(facts, read.source_available);
    if support.state == "unsupported" {
        return unsupported_photo(&photo_id, &support);
    }
    let mutation = SaveEditRecipe {
        photo_id: photo_id.clone(),
        request_id: body.request_id,
        expected_recipe_revision: body.expected_recipe_revision,
        expected_source_revision: body.expected_source_revision,
        settings,
    };
    let outcome = match state.application.library.save_edit_recipe(mutation).await {
        Ok(outcome) => outcome,
        Err(_) => return storage_error("edit-recipe-save"),
    };
    map_write_outcome(&state, &photo_id, facts, read, outcome).await
}

/// `POST /api/photos/{id}/edit-recipe/rebind`: explicit rebinding of saved
/// intent to a newly observed source revision.
pub(crate) async fn post_edit_recipe_rebind(
    State(state): State<HttpState>,
    axum::extract::Path(photo_id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    if request.headers().contains_key(CLI_CONTRACT_HEADER)
        && let Err(response) = require_cli_contract(&request)
    {
        return *response;
    }
    if let Err(response) = require_published(&state.application) {
        return *response;
    }
    if !valid_id(&photo_id) {
        return invalid_cli("photoId", "The Photo ID is invalid.");
    }
    let body = match read_recipe_body::<RebindEditRecipeBody>(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    if body.expected_recipe_revision.is_empty() {
        return settings_error(
            "expectedRecipeRevision",
            "The rebind must carry the observed recipe revision.",
        );
    }
    if body.expected_source_revision.is_empty() {
        return settings_error(
            "expectedSourceRevision",
            "The rebind must carry the newly observed source revision.",
        );
    }
    let (photo, metadata, read) = match load_facts(&state, &photo_id).await {
        Ok(facts) => facts,
        Err(response) => return response,
    };
    let facts = source_facts(&photo, &metadata, &state);
    let support = derive_support(facts, read.source_available);
    if support.state == "unsupported" {
        return unsupported_photo(&photo_id, &support);
    }
    let mutation = RebindEditRecipe {
        photo_id: photo_id.clone(),
        expected_recipe_revision: body.expected_recipe_revision,
        expected_source_revision: body.expected_source_revision,
    };
    let outcome = match state.application.library.rebind_edit_recipe(mutation).await {
        Ok(outcome) => outcome,
        Err(_) => return storage_error("edit-recipe-rebind"),
    };
    map_write_outcome(&state, &photo_id, facts, read, outcome).await
}

// ---------------------------------------------------------------- validation

fn validated_settings(body: &SaveEditRecipeBody) -> Option<EditRecipeSettings> {
    if body.request_id.is_empty()
        || body.request_id.len() > MAXIMUM_REQUEST_ID_BYTES
        || body.request_id.chars().any(char::is_control)
    {
        return None;
    }
    if body.expected_source_revision.is_empty()
        || body
            .expected_recipe_revision
            .as_deref()
            .is_some_and(str::is_empty)
    {
        return None;
    }
    let milli = body.settings.exposure_ev * 1000.0;
    let rounded = milli.round();
    if !milli.is_finite() || (milli - rounded).abs() >= 1e-6 {
        return None;
    }
    let milli = rounded as i64;
    if !(APPROVED_EXPOSURE_MILLI_EV_MIN..=APPROVED_EXPOSURE_MILLI_EV_MAX).contains(&milli) {
        return None;
    }
    if body.settings.white_balance != APPROVED_WHITE_BALANCE {
        return None;
    }
    Some(EditRecipeSettings {
        exposure_ev: body.settings.exposure_ev,
        white_balance: WhiteBalanceIntent::AsShot,
    })
}

fn invalid_settings_response() -> Response<Body> {
    settings_error(
        "settings",
        "The save must carry a nonempty request identity, both expected revisions, and an exposure on the approved thousandth-of-an-EV grid with as-shot white balance.",
    )
}

// ---------------------------------------------------------------- outcome mapping

/// Maps one core write outcome onto the closed outcome and error-code sets.
/// Conflict-family responses carry the current recipe revision, source
/// revision, and support state.
async fn map_write_outcome(
    state: &HttpState,
    photo_id: &str,
    facts: SourceFacts<'_>,
    read: EditRecipeRead,
    outcome: EditRecipeWriteOutcome,
) -> Response<Body> {
    let source_available = read.source_available;
    let pre_write_revision = read.recipe.map(|recipe| recipe.revision);
    match outcome {
        EditRecipeWriteOutcome::Saved(recipe) => {
            // `Saved` covers a fresh commit and a receipt replay, and a
            // replay must report `unchanged` because no write occurred. One
            // post-write read separates them: a fresh commit installs a
            // revision that was not current before, while a replay leaves
            // either the superseded receipt revision or the pre-write
            // revision in place.
            let current_revision = state
                .application
                .library
                .edit_recipe(photo_id)
                .await
                .ok()
                .flatten()
                .and_then(|current| current.recipe.map(|recipe| recipe.revision));
            let installed = current_revision.as_deref() == Some(recipe.revision.as_str());
            let replayed =
                !installed || pre_write_revision.as_deref() == Some(recipe.revision.as_str());
            let outcome_name = if replayed { "unchanged" } else { "saved" };
            write_response(outcome_name, recipe, facts, source_available)
        }
        EditRecipeWriteOutcome::Unchanged(recipe) => {
            write_response("unchanged", recipe, facts, source_available)
        }
        EditRecipeWriteOutcome::Conflict(current) => conflict_response(
            "recipe_conflict",
            "The expected recipe revision is no longer current; decide again from the carried facts.",
            photo_id,
            current,
            derive_support(facts, source_available),
        ),
        EditRecipeWriteOutcome::SourceChanged(current) => conflict_response(
            "source_changed",
            "The source revision changed; the saved intent is preserved for an explicit rebind.",
            photo_id,
            current,
            derive_support(facts, source_available),
        ),
        EditRecipeWriteOutcome::RequiresRebind(current) => conflict_response(
            "requires_rebind",
            "The stored binding is stale; only an explicit rebind may adopt the new source.",
            photo_id,
            current,
            derive_support(facts, source_available),
        ),
        EditRecipeWriteOutcome::RequestConflict => cli_error(
            StatusCode::CONFLICT,
            "request_conflict",
            "This request identity was already used with a different payload.",
            serde_json::json!({"photoId": photo_id}),
        ),
        EditRecipeWriteOutcome::MissingPhoto => unknown_photo(photo_id),
        EditRecipeWriteOutcome::MissingRecipe => cli_error(
            StatusCode::NOT_FOUND,
            "missing_recipe",
            "A write requiring an existing recipe found none.",
            serde_json::json!({"photoId": photo_id}),
        ),
        EditRecipeWriteOutcome::UnsupportedPhoto => {
            unsupported_photo(photo_id, &derive_support(facts, source_available))
        }
        EditRecipeWriteOutcome::Unavailable => cli_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "processing_unavailable",
            "Current source facts cannot be read, so no guarded write is possible.",
            serde_json::json!({"photoId": photo_id}),
        ),
        EditRecipeWriteOutcome::InvalidSettings => invalid_settings_response(),
    }
}

fn write_response(
    outcome: &'static str,
    recipe: EditRecipe,
    facts: SourceFacts<'_>,
    source_available: bool,
) -> Response<Body> {
    let support = derive_support(facts, source_available);
    let processing = processing_state(&support, Some(&recipe));
    ok_response(&EditRecipeWriteResponse {
        outcome,
        source_revision: recipe.source_revision.clone(),
        recipe: EditRecipeWire::from(recipe),
        support,
        processing,
        controls: approved_controls(),
    })
}

fn ok_response<T: Serialize>(value: &T) -> Response<Body> {
    let body = serde_json::to_vec(value).expect("response serializes");
    Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header(axum::http::header::CONTENT_LENGTH, body.len())
        .body(Body::from(body))
        .expect("valid JSON response")
}
