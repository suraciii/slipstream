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
    APPROVED_WHITE_BALANCE_MODE,
};

use crate::http::{
    CLI_CONTRACT_HEADER, HttpState, cli_error, invalid_cli, read_cli_json_body, read_json_body,
    require_cli_contract, require_published, valid_id,
};

/// Mirrors the core request-identity bound; the persistence owner enforces
/// the same limit again before admitting a write.
const MAXIMUM_REQUEST_ID_BYTES: usize = 128;

// ---------------------------------------------------------------- support state

/// The source support of one Photo, as the merged Photo Development Surface
/// contract defines it. `state` is closed to `supported`, `unavailable`, or
/// `unsupported`; `reason` carries one of the closed `supportReason` values
/// and is non-null only with `unavailable`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SupportClassification {
    pub(crate) state: &'static str,
    pub(crate) reason: Option<&'static str>,
}

/// The reason recorded when the Original is absent from its remembered
/// Location, so no current source fact can be read.
const ORIGINAL_MISSING: &str = "original-missing";

/// The reason recorded when the Original cannot yield the facts that name
/// its source class: unreadable bytes, an unreadable camera identity, or an
/// unrecognized RAW container.
const ORIGINAL_UNREADABLE: &str = "original-unreadable";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExposureRangeWire {
    pub(crate) minimum_ev: f64,
    pub(crate) maximum_ev: f64,
    pub(crate) step_ev: f64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ControlsWire {
    exposure: ExposureRangeWire,
    white_balance_modes: [&'static str; 1],
}

/// The approved control ranges. They are deployment constants of the
/// approved profile set, so they do not depend on one Photo's support state.
pub(crate) fn approved_controls() -> ControlsWire {
    ControlsWire {
        exposure: approved_exposure_range(),
        white_balance_modes: [APPROVED_WHITE_BALANCE_MODE],
    }
}

/// The approved finite exposure range of the qualified workload in EV.
pub(crate) fn approved_exposure_range() -> ExposureRangeWire {
    ExposureRangeWire {
        minimum_ev: APPROVED_EXPOSURE_MILLI_EV_MIN as f64 / 1000.0,
        maximum_ev: APPROVED_EXPOSURE_MILLI_EV_MAX as f64 / 1000.0,
        step_ev: 0.001,
    }
}

/// The source facts one support derivation needs beside the availability.
#[derive(Clone, Copy)]
pub(crate) struct SourceFacts<'a> {
    kind: OriginalKind,
    filename: &'a str,
    make: Option<&'a str>,
    model: Option<&'a str>,
}

/// Classifies one Photo's source class against the approved profiles.
///
/// The state is a fact about the class, independent of the deployment's
/// processing enablement: the capability report owns that boundary. A
/// missing or unreadable Original reports `unavailable` and never
/// `supported` or `unsupported`, because no current source fact can be
/// read. A JPEG source is a known class without an approved profile, so it
/// reports `unsupported`. For a RAW source the class identity needs the
/// filename container and the observed camera make and model; the bounded
/// metadata read deliberately returns defaults when inspection is saturated,
/// the source changed mid-read, or it cannot be opened, so an unobservable
/// identity means the class is unavailable to observe rather than unlisted.
pub(crate) fn derive_support(
    facts: SourceFacts<'_>,
    source_available: bool,
    original_available: bool,
) -> SupportClassification {
    if !source_available {
        // An Original absent from its remembered Location is missing; one
        // that is present but whose published source facts cannot be read
        // is unreadable.
        return SupportClassification {
            state: "unavailable",
            reason: Some(if original_available {
                ORIGINAL_UNREADABLE
            } else {
                ORIGINAL_MISSING
            }),
        };
    }
    if facts.kind != OriginalKind::Raw {
        return SupportClassification {
            state: "unsupported",
            reason: None,
        };
    }
    let Some(container) = photo_profile::container_of_filename(facts.filename) else {
        return SupportClassification {
            state: "unavailable",
            reason: Some(ORIGINAL_UNREADABLE),
        };
    };
    let (Some(make), Some(model)) = (facts.make, facts.model) else {
        return SupportClassification {
            state: "unavailable",
            reason: Some(ORIGINAL_UNREADABLE),
        };
    };
    match photo_profile::classify(make, model, &container) {
        // The identity is observed, so a failed match is a known-unapproved
        // class.
        None => SupportClassification {
            state: "unsupported",
            reason: None,
        },
        Some(_) => SupportClassification {
            state: "supported",
            reason: None,
        },
    }
}

/// True when the enabled execution payload can represent the stored value:
/// a finite multiple of one thousandth of an EV inside the approved range.
fn representable(settings: &EditRecipeSettings) -> bool {
    let milli = settings.exposure_ev * 1000.0;
    let rounded = milli.round();
    milli.is_finite()
        && (milli - rounded).abs() < 1e-6
        && rounded >= APPROVED_EXPOSURE_MILLI_EV_MIN as f64
        && rounded <= APPROVED_EXPOSURE_MILLI_EV_MAX as f64
}

/// Whether the develop execution can process this Photo right now. Only the
/// reconciled `ready` condition admits processing; every other closed
/// condition reports processing as unavailable. A stored recipe whose
/// white-balance mode the capability does not admit, or that leaves the
/// approved range, stays readable but reports the same; so does an
/// unreadable source or an unapproved class.
fn processing_available(
    support: SupportClassification,
    capability_condition: &str,
    source_available: bool,
    recipe: Option<&EditRecipe>,
) -> bool {
    let admitted = recipe.is_none_or(|recipe| {
        matches!(recipe.settings.white_balance, WhiteBalanceIntent::AsShot)
            && representable(&recipe.settings)
    });
    support.state == "supported" && capability_condition == "ready" && source_available && admitted
}

/// A deployment whose launcher exposes no photo-processing capability has no
/// approved source class at all, so a class the compiled list approves still
/// reads as `unsupported`. This keeps every per-Photo report consistent with
/// the capability report's empty profile list in that state.
fn apply_capability_condition(
    support: SupportClassification,
    condition: &str,
) -> SupportClassification {
    if condition == "source-unsupported" && support.state == "supported" {
        SupportClassification {
            state: "unsupported",
            reason: None,
        }
    } else {
        support
    }
}

/// The closed capability condition of this deployment: the operator choice
/// when disabled, otherwise the condition the launcher answer maps onto.
async fn capability_condition(state: &HttpState) -> &'static str {
    match &state.processing {
        Some(config) => crate::processing_capability::capability_condition(config).await,
        None => "disabled",
    }
}

// ---------------------------------------------------------------- wire shapes

/// One stored recipe as the read model renders it. A stored value the
/// execution payload cannot represent stays readable here, including a
/// temperature-tint intent no capability admits.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecipeWire {
    recipe_version: String,
    exposure_ev: f64,
    white_balance: serde_json::Value,
}

impl From<EditRecipe> for RecipeWire {
    fn from(recipe: EditRecipe) -> Self {
        Self {
            recipe_version: recipe.revision,
            exposure_ev: recipe.settings.exposure_ev,
            white_balance: white_balance_wire(recipe.settings.white_balance),
        }
    }
}

/// The stored white-balance intent in the shared field shape: exactly the
/// mode plus the fields that mode requires.
fn white_balance_wire(intent: WhiteBalanceIntent) -> serde_json::Value {
    match intent {
        WhiteBalanceIntent::AsShot => serde_json::json!({ "mode": APPROVED_WHITE_BALANCE_MODE }),
        WhiteBalanceIntent::TemperatureTint {
            temperature_kelvin,
            tint_milli,
        } => serde_json::json!({
            "mode": "temperature-tint",
            "temperatureKelvin": temperature_kelvin,
            "tintMilli": tint_milli,
        }),
    }
}

/// One recipe read: the current recipe or its absence, the observed source
/// revision, the source support state with its closed reason, the
/// processing availability, and the approved control ranges. `sourceRevision`
/// is null exactly when `sourceSupport` is `unavailable`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EditRecipeResponse {
    photo_id: String,
    source_revision: Option<String>,
    recipe: Option<RecipeWire>,
    source_support: &'static str,
    support_reason: Option<&'static str>,
    processing_available: bool,
    controls: ControlsWire,
}

/// One guarded write result. `outcome` is the closed contract outcome; the
/// remaining facts let the client continue without a second read.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct EditRecipeWriteResponse {
    outcome: &'static str,
    recipe_version: String,
    source_revision: String,
}

// ---------------------------------------------------------------- request bodies

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveEditRecipeBody {
    request_id: String,
    expected_recipe_version: Option<String>,
    expected_source_revision: String,
    settings: SettingsBody,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingsBody {
    exposure_ev: f64,
    white_balance: serde_json::Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RebindEditRecipeBody {
    request_id: String,
    expected_recipe_version: String,
    new_source_revision: String,
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
        StatusCode::UNPROCESSABLE_ENTITY,
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
/// routes: the service lacks the facts or resources the operation needs, so
/// they carry the closed `resource_unavailable` code with the operation
/// named in the details.
fn storage_error(operation: &'static str) -> Response<Body> {
    cli_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "resource_unavailable",
        "The service cannot read or write the current facts; inspect server health before trying again.",
        serde_json::json!({"operation": operation}),
    )
}

fn unsupported_photo(photo_id: &str) -> Response<Body> {
    cli_error(
        StatusCode::UNPROCESSABLE_ENTITY,
        "unsupported_photo",
        "This Photo's source class has no approved profile.",
        serde_json::json!({"photoId": photo_id}),
    )
}

/// The source facts that guard a write cannot be read, so no guarded write
/// is possible until a later read observes them again.
fn unavailable_source(photo_id: &str, reason: Option<&'static str>) -> Response<Body> {
    cli_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "resource_unavailable",
        "Current source facts cannot be read, so no guarded write is possible.",
        serde_json::json!({"photoId": photo_id, "supportReason": reason}),
    )
}

/// Current facts for one conflict-family outcome, so the client can recover
/// without a second read.
fn conflict_details(read: &EditRecipeRead) -> serde_json::Value {
    serde_json::json!({
        "currentSourceRevision": read.current_source_revision,
        "currentRecipeVersion": read.recipe.as_ref().map(|recipe| recipe.revision.clone()),
    })
}

fn conflict_response(
    code: &'static str,
    message: &'static str,
    read: EditRecipeRead,
) -> Response<Body> {
    cli_error(StatusCode::CONFLICT, code, message, conflict_details(&read))
}

// ---------------------------------------------------------------- shared reads

/// One serialized read of the recipe facts, Photo facts, and the bounded
/// capture metadata. The recipe read owns the combined source availability
/// and the unknown-Photo refusal.
pub(crate) async fn load_facts(
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

pub(crate) fn source_facts<'a>(
    photo: &'a PhotoRead,
    metadata: &'a CaptureReviewMetadata,
) -> SourceFacts<'a> {
    SourceFacts {
        kind: photo.original_kind,
        filename: &photo.filename,
        make: metadata.make.as_deref(),
        model: metadata.model.as_deref(),
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
    let facts = source_facts(&photo, &metadata);
    let condition = capability_condition(&state).await;
    let support = apply_capability_condition(
        derive_support(facts, read.source_available, photo.original_available),
        condition,
    );
    let processing_available = processing_available(
        support,
        condition,
        read.source_available,
        read.recipe.as_ref(),
    );
    let recipe = read.recipe.map(RecipeWire::from);
    ok_response(&EditRecipeResponse {
        source_revision: (support.state != "unavailable")
            .then(|| read.current_source_revision.clone()),
        recipe,
        source_support: support.state,
        support_reason: support.reason,
        processing_available,
        controls: approved_controls(),
        photo_id,
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
    let facts = source_facts(&photo, &metadata);
    let support = apply_capability_condition(
        derive_support(facts, read.source_available, photo.original_available),
        capability_condition(&state).await,
    );
    match support.state {
        // A known-unapproved class refuses every guarded write.
        "unsupported" => return unsupported_photo(&photo_id),
        // The source facts that guard the write cannot be read.
        "unavailable" => return unavailable_source(&photo_id, support.reason),
        _ => {}
    }
    let mutation = SaveEditRecipe {
        photo_id: photo_id.clone(),
        request_id: body.request_id,
        expected_recipe_version: body.expected_recipe_version,
        expected_source_revision: body.expected_source_revision,
        settings,
    };
    let outcome = match state.application.library.save_edit_recipe(mutation).await {
        Ok(outcome) => outcome,
        Err(_) => return storage_error("edit-recipe-save"),
    };
    map_write_outcome(&photo_id, outcome)
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
    if !valid_request_id(&body.request_id) {
        return settings_error(
            "requestId",
            "The rebind must carry a valid request identity: 1 to 128 characters of ASCII letters, digits, `.`, `_`, or `-`.",
        );
    }
    if body.expected_recipe_version.is_empty() {
        return settings_error(
            "expectedRecipeVersion",
            "The rebind must carry the previously observed recipe version.",
        );
    }
    if body.new_source_revision.is_empty() {
        return settings_error(
            "newSourceRevision",
            "The rebind must carry the newly observed source revision.",
        );
    }
    let (photo, metadata, read) = match load_facts(&state, &photo_id).await {
        Ok(facts) => facts,
        Err(response) => return response,
    };
    let facts = source_facts(&photo, &metadata);
    let support = apply_capability_condition(
        derive_support(facts, read.source_available, photo.original_available),
        capability_condition(&state).await,
    );
    match support.state {
        // A known-unapproved class refuses every guarded write.
        "unsupported" => return unsupported_photo(&photo_id),
        // The source facts that guard the write cannot be read.
        "unavailable" => return unavailable_source(&photo_id, support.reason),
        _ => {}
    }
    let mutation = RebindEditRecipe {
        photo_id: photo_id.clone(),
        request_id: body.request_id,
        expected_recipe_version: body.expected_recipe_version,
        new_source_revision: body.new_source_revision,
    };
    let outcome = match state.application.library.rebind_edit_recipe(mutation).await {
        Ok(outcome) => outcome,
        Err(_) => return storage_error("edit-recipe-rebind"),
    };
    map_write_outcome(&photo_id, outcome)
}

// ---------------------------------------------------------------- validation

/// The shared field shape's request identity: 1..=128 characters of ASCII
/// letters, digits, `.`, `_`, or `-`.
fn valid_request_id(request_id: &str) -> bool {
    !request_id.is_empty()
        && request_id.len() <= MAXIMUM_REQUEST_ID_BYTES
        && request_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn validated_settings(body: &SaveEditRecipeBody) -> Option<EditRecipeSettings> {
    // The shared field shape closes the request identity to 1..=128
    // characters of ASCII letters, digits, `.`, `_`, or `-`.
    if !valid_request_id(&body.request_id) {
        return None;
    }
    if body.expected_source_revision.is_empty()
        || body
            .expected_recipe_version
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
    let white_balance = parse_white_balance_payload(&body.settings.white_balance)?;
    Some(EditRecipeSettings {
        exposure_ev: body.settings.exposure_ev,
        white_balance,
    })
}

/// Parses one shared-field-shape `whiteBalance` value: exactly `mode` plus
/// the fields that mode requires. The payload bounds are closed and
/// published independent of admission, so a temperature-tint intent inside
/// the bounds is wire-valid even though only as-shot is admitted for
/// execution.
fn parse_white_balance_payload(value: &serde_json::Value) -> Option<WhiteBalanceIntent> {
    let object = value.as_object()?;
    let mode = object.get("mode")?.as_str()?;
    match mode {
        APPROVED_WHITE_BALANCE_MODE if object.len() == 1 => Some(WhiteBalanceIntent::AsShot),
        "temperature-tint" if object.len() == 3 => {
            let temperature_kelvin = integer_field(object, "temperatureKelvin")?;
            let tint_milli = integer_field(object, "tintMilli")?;
            let intent = WhiteBalanceIntent::TemperatureTint {
                temperature_kelvin,
                tint_milli,
            };
            intent.within_payload_bounds().then_some(intent)
        }
        _ => None,
    }
}

fn integer_field(object: &serde_json::Map<String, serde_json::Value>, name: &str) -> Option<i32> {
    object
        .get(name)
        .and_then(serde_json::Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
}

fn invalid_settings_response() -> Response<Body> {
    settings_error(
        "settings",
        "The save must carry a nonempty request identity, both expected revisions, and an exposure on the approved thousandth-of-an-EV grid with a white-balance value inside the published payload bounds.",
    )
}

// ---------------------------------------------------------------- outcome mapping

/// Maps one core write outcome onto the closed outcome and error-code sets.
/// The saved-versus-replayed fact is decided by persistence inside the write
/// transaction, so a concurrent write after the commit can never reclassify
/// this response. Conflict-family responses carry the current source
/// revision and the current recipe version.
fn map_write_outcome(photo_id: &str, outcome: EditRecipeWriteOutcome) -> Response<Body> {
    match outcome {
        EditRecipeWriteOutcome::Saved(recipe) => write_response("saved", recipe),
        EditRecipeWriteOutcome::Replayed(recipe) | EditRecipeWriteOutcome::Unchanged(recipe) => {
            write_response("unchanged", recipe)
        }
        EditRecipeWriteOutcome::Conflict(current) => conflict_response(
            "recipe_conflict",
            "The expected recipe revision is no longer current; decide again from the carried facts.",
            current,
        ),
        EditRecipeWriteOutcome::SourceChanged(current) => conflict_response(
            "source_changed",
            "The source revision changed; the saved intent is preserved for an explicit rebind.",
            current,
        ),
        EditRecipeWriteOutcome::RequiresRebind(current) => conflict_response(
            "requires_rebind",
            "The stored binding is stale; only an explicit rebind may adopt the new source.",
            current,
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
        EditRecipeWriteOutcome::UnsupportedPhoto => unsupported_photo(photo_id),
        EditRecipeWriteOutcome::Unavailable => unavailable_source(photo_id, None),
        EditRecipeWriteOutcome::InvalidSettings => invalid_settings_response(),
    }
}

fn write_response(outcome: &'static str, recipe: EditRecipe) -> Response<Body> {
    ok_response(&EditRecipeWriteResponse {
        outcome,
        recipe_version: recipe.revision,
        source_revision: recipe.source_revision,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_unsupported_condition_downgrades_only_supported_classes() {
        let supported = SupportClassification {
            state: "supported",
            reason: None,
        };
        let downgraded = apply_capability_condition(supported, "source-unsupported");
        assert_eq!(downgraded.state, "unsupported");
        assert!(downgraded.reason.is_none());

        // Other conditions keep the class fact; an already-unavailable or
        // unsupported class never becomes supported.
        for condition in [
            "disabled",
            "launcher-unavailable",
            "ready",
            "bundle-unavailable",
            "resource-unavailable",
        ] {
            assert_eq!(
                apply_capability_condition(supported, condition).state,
                "supported"
            );
        }
        let unavailable = SupportClassification {
            state: "unavailable",
            reason: Some(ORIGINAL_MISSING),
        };
        assert_eq!(
            apply_capability_condition(unavailable, "source-unsupported").state,
            "unavailable"
        );
        let unsupported = SupportClassification {
            state: "unsupported",
            reason: None,
        };
        assert_eq!(
            apply_capability_condition(unsupported, "source-unsupported").state,
            "unsupported"
        );
    }

    #[test]
    fn temperature_tint_payload_bounds_close_the_shape() {
        assert_eq!(
            parse_white_balance_payload(&serde_json::json!({"mode": "as-shot"})),
            Some(WhiteBalanceIntent::AsShot)
        );
        assert_eq!(
            parse_white_balance_payload(&serde_json::json!({
                "mode": "temperature-tint", "temperatureKelvin": 6500, "tintMilli": -10
            })),
            Some(WhiteBalanceIntent::TemperatureTint {
                temperature_kelvin: 6500,
                tint_milli: -10,
            })
        );
        // Out of bounds, wrong arity, wrong types, and unknown modes refuse.
        assert_eq!(
            parse_white_balance_payload(&serde_json::json!({
                "mode": "temperature-tint", "temperatureKelvin": 999, "tintMilli": 0
            })),
            None
        );
        assert_eq!(
            parse_white_balance_payload(&serde_json::json!({
                "mode": "temperature-tint", "temperatureKelvin": 6500
            })),
            None
        );
        assert_eq!(
            parse_white_balance_payload(&serde_json::json!({
                "mode": "temperature-tint", "temperatureKelvin": 6.5, "tintMilli": 0
            })),
            None
        );
        assert_eq!(
            parse_white_balance_payload(&serde_json::json!({"mode": "custom"})),
            None
        );
        assert_eq!(
            parse_white_balance_payload(&serde_json::json!("as-shot")),
            None
        );
    }

    #[test]
    fn stored_temperature_tint_reads_but_blocks_processing() {
        let recipe = EditRecipe {
            photo_id: "photo".to_owned(),
            revision: "rev-1".to_owned(),
            source_revision: "source-1".to_owned(),
            settings: EditRecipeSettings {
                exposure_ev: 0.25,
                white_balance: WhiteBalanceIntent::TemperatureTint {
                    temperature_kelvin: 6500,
                    tint_milli: 12,
                },
            },
        };
        let wire = RecipeWire::from(recipe.clone());
        assert_eq!(
            wire.white_balance,
            serde_json::json!({
                "mode": "temperature-tint", "temperatureKelvin": 6500, "tintMilli": 12
            })
        );
        let support = SupportClassification {
            state: "supported",
            reason: None,
        };
        assert!(!processing_available(support, "ready", true, Some(&recipe)));
        // The same exposure as as-shot stays processable only when the
        // reconciled condition is ready.
        let mut as_shot = recipe.clone();
        as_shot.settings.white_balance = WhiteBalanceIntent::AsShot;
        assert!(processing_available(support, "ready", true, Some(&as_shot)));
    }

    #[test]
    fn only_the_ready_condition_admits_processing() {
        let recipe = EditRecipe {
            photo_id: "photo".to_owned(),
            revision: "rev-1".to_owned(),
            source_revision: "source-1".to_owned(),
            settings: EditRecipeSettings {
                exposure_ev: 0.0,
                white_balance: WhiteBalanceIntent::AsShot,
            },
        };
        let support = SupportClassification {
            state: "supported",
            reason: None,
        };
        for condition in [
            "disabled",
            "launcher-unavailable",
            "bundle-unavailable",
            "source-unsupported",
            "resource-unavailable",
        ] {
            assert!(
                !processing_available(support, condition, true, Some(&recipe)),
                "{condition} must not admit processing"
            );
        }
        assert!(processing_available(support, "ready", true, Some(&recipe)));
    }
}
