//! Edit Recipe reads and guarded writes: one strict `--input` reader per
//! write, one service read with semantic validation, and one mutation per
//! command. The caller owns every guard — request identity, expected recipe
//! version, and expected or newly observed source revision — and this module
//! never refreshes a guard behind the caller's back or retries a write.

use super::{
    AdmissionState, CommandFailure, MutationIdentity, Operation, ServiceClient, read_input_bytes,
    valid_request_identity, valid_sha256, web_url,
};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;
use url::Url;

const CAPABILITY_OPERATION: Operation = Operation::ProcessingCapability;
const MODULES_OPERATION: Operation = Operation::ProcessingModules;
const GET_OPERATION: Operation = Operation::PhotosRecipeGet;
const SAVE_OPERATION: Operation = Operation::PhotosRecipeSave;
const REBIND_OPERATION: Operation = Operation::PhotosRecipeRebind;
const PROCESSING_GET_OPERATION: Operation = Operation::PhotosProcessingRecipeGet;
const PROCESSING_SAVE_OPERATION: Operation = Operation::PhotosProcessingRecipeSave;
const PROCESSING_EXPORT_OPERATION: Operation = Operation::PhotosProcessingExport;
const ARTIFACT_OPERATION: Operation = Operation::ProcessingArtifact;
const PROCESSING_EXPORT_STATUS_OPERATION: Operation = Operation::PhotosProcessingExportStatus;
const PROCESSING_EXPORT_CANCEL_OPERATION: Operation = Operation::PhotosProcessingExportCancel;

/// The shared white-balance payload bounds of the Photo Development Surface
/// wire contract. They are closed and published independent of admission, so
/// a conforming client can always construct a wire-valid request even when
/// the deployment admits no adjustable mode for execution.
const TEMPERATURE_KELVIN_BOUNDS: std::ops::RangeInclusive<i32> = 1_000..=40_000;
const TINT_MILLI_BOUNDS: std::ops::RangeInclusive<i32> = -150_000..=150_000;

// ---------------------------------------------------------------- commands

/// The `photos recipe` subcommands. Every write carries its complete guard
/// set in one `--input` document; the command line never supplies partial
/// guards that this module could silently complete.
#[derive(Debug, clap::Subcommand)]
pub enum RecipeCommand {
    /// Read one Photo's current Edit Recipe and source facts.
    Get {
        /// One Photo ID.
        #[arg(value_name = "PHOTO_ID", value_parser = super::nonempty)]
        photo_id: String,
    },
    /// Guarded save of development settings with explicit revisions.
    Save(RecipeWriteArgs),
    /// Explicit adoption of a newly observed source revision.
    Rebind(RecipeWriteArgs),
}

#[derive(Debug, clap::Args)]
pub struct RecipeWriteArgs {
    #[arg(value_name = "PHOTO_ID", value_parser = super::nonempty)]
    pub photo_id: String,
    /// UTF-8 JSON file holding the complete guarded write body; `-` reads
    /// standard input.
    #[arg(long, value_name = "FILE", value_parser = super::nonempty)]
    pub input: String,
}

/// Reads and validates the write body before any network access. A `Get`
/// needs no input; each write reads its complete document once and returns
/// the exact serialized body the mutation later submits.
pub(super) async fn prepare(command: &RecipeCommand) -> Result<Option<Value>, CommandFailure> {
    match command {
        RecipeCommand::Get { .. } => Ok(None),
        RecipeCommand::Save(args) => Ok(Some(parse_save(read_input_bytes(&args.input).await?)?)),
        RecipeCommand::Rebind(args) => {
            Ok(Some(parse_rebind(read_input_bytes(&args.input).await?)?))
        }
    }
}

// ---------------------------------------------------------------- input shapes

/// The one accepted `photos recipe save --input` document shape. Derived
/// decoding rejects unknown keys, duplicate keys, trailing content, and
/// non-object documents, including inside `settings` and `whiteBalance`.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveRecipeInput {
    request_id: String,
    /// A required but nullable key: the explicit JSON null guards "no recipe
    /// exists", while an omitted key is refused. With `deserialize_with` and
    /// no `default`, serde reports a missing field without calling the
    /// function, so an omitted guard never collapses into the null guard.
    #[serde(deserialize_with = "required_nullable_string")]
    expected_recipe_version: Option<String>,
    expected_source_revision: String,
    settings: SettingsWire,
}

/// The one accepted `photos recipe rebind --input` document shape. Every key
/// is a required nonempty string; the rebind has no nullable guard.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RebindRecipeInput {
    request_id: String,
    expected_recipe_version: String,
    new_source_revision: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingsWire {
    exposure_ev: f64,
    white_balance: WhiteBalanceWire,
}

/// Decodes an explicitly present nullable string: JSON `null` becomes `None`
/// and a string becomes `Some`. See `SaveRecipeInput::expected_recipe_version`.
fn required_nullable_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer)
}

fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

/// The shared white-balance field shape: exactly `mode` plus the fields that
/// mode requires. Derived decoding rejects duplicate and unknown keys inside
/// the object, which a raw `Value` would silently collapse to last-write-wins.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WhiteBalanceWire {
    mode: WhiteBalanceMode,
    #[serde(
        default,
        deserialize_with = "present_integer",
        skip_serializing_if = "Option::is_none"
    )]
    temperature_kelvin: Option<i32>,
    #[serde(
        default,
        deserialize_with = "present_integer",
        skip_serializing_if = "Option::is_none"
    )]
    tint_milli: Option<i32>,
}

fn present_integer<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<i32>, D::Error> {
    i32::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Copy, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum WhiteBalanceMode {
    AsShot,
    TemperatureTint,
}

impl WhiteBalanceWire {
    /// Whether the value is one closed shared shape: `as-shot` alone, or
    /// `temperature-tint` with both payload integers inside the published
    /// bounds. A temperature-tint intent inside the bounds is wire-valid
    /// even when its mode is not admitted for execution.
    fn closed_shape(&self) -> bool {
        match self.mode {
            WhiteBalanceMode::AsShot => {
                self.temperature_kelvin.is_none() && self.tint_milli.is_none()
            }
            WhiteBalanceMode::TemperatureTint => {
                matches!(self.temperature_kelvin, Some(value) if TEMPERATURE_KELVIN_BOUNDS.contains(&value))
                    && matches!(self.tint_milli, Some(value) if TINT_MILLI_BOUNDS.contains(&value))
            }
        }
    }
}

/// Validates the save body in the service's own admission order: request
/// identity, revision guards, exposure, then white balance. The approved
/// exposure range and step stay the service's decision; this reader only
/// refuses an exposure that is not a finite number.
fn parse_save(bytes: Vec<u8>) -> Result<Value, CommandFailure> {
    let input: SaveRecipeInput = serde_json::from_slice(&bytes).map_err(|_| {
        CommandFailure::invalid(
            "input",
            "The input must be one JSON object with exactly requestId, expectedRecipeVersion, \
             expectedSourceRevision, and settings, carrying the shared wire shapes.",
        )
    })?;
    validate_request_identity(&input.request_id)?;
    validate_nonempty_revision(
        "expectedRecipeVersion",
        input.expected_recipe_version.as_deref(),
    )?;
    validate_nonempty_revision(
        "expectedSourceRevision",
        Some(&input.expected_source_revision),
    )?;
    if !input.settings.exposure_ev.is_finite() {
        return Err(CommandFailure::invalid(
            "settings",
            "The exposureEv must be a finite number of EV on the service's approved grid.",
        ));
    }
    if !input.settings.white_balance.closed_shape() {
        return Err(CommandFailure::invalid(
            "whiteBalance",
            "The whiteBalance must be exactly {\"mode\": \"as-shot\"}, or temperature-tint with \
             a temperatureKelvin from 1,000 through 40,000 and a tintMilli from -150,000 \
             through 150,000 and no other fields.",
        ));
    }
    serde_json::to_value(&input).map_err(|_| unusable_input())
}

fn parse_rebind(bytes: Vec<u8>) -> Result<Value, CommandFailure> {
    let input: RebindRecipeInput = serde_json::from_slice(&bytes).map_err(|_| {
        CommandFailure::invalid(
            "input",
            "The input must be one JSON object with exactly requestId, \
             expectedRecipeVersion, and newSourceRevision.",
        )
    })?;
    validate_request_identity(&input.request_id)?;
    validate_nonempty_revision(
        "expectedRecipeVersion",
        Some(&input.expected_recipe_version),
    )?;
    validate_nonempty_revision("newSourceRevision", Some(&input.new_source_revision))?;
    serde_json::to_value(&input).map_err(|_| unusable_input())
}

fn validate_request_identity(request_id: &str) -> Result<(), CommandFailure> {
    if valid_request_identity(request_id) {
        Ok(())
    } else {
        Err(CommandFailure::invalid(
            "requestId",
            "The requestId must be 1 to 128 characters of ASCII letters, digits, `.`, `_`, or `-`.",
        ))
    }
}

/// Guards are opaque nonempty strings. `None` is the save's explicit null
/// guard — it requires that no recipe exists and is valid; only an empty
/// string guard is refused.
fn validate_nonempty_revision(
    argument: &str,
    revision: Option<&str>,
) -> Result<(), CommandFailure> {
    if revision.is_none_or(|revision| !revision.is_empty()) {
        Ok(())
    } else {
        Err(CommandFailure::invalid(
            argument,
            "The revision guard must be a nonempty string.",
        ))
    }
}

pub(super) fn unusable_input() -> CommandFailure {
    CommandFailure::invalid("input", "The input document could not be rendered.")
}

// ---------------------------------------------------------------- execution

/// Executes one recipe command. Reads report their validated facts plus the
/// Photo Destination `webUrl`; writes submit the exact prepared body once,
/// and anything unusable in a post-admission response stays an unknown
/// outcome. No automatic pre-read replaces the caller's guards and no
/// refused or lost write is retried.
pub(super) async fn execute(
    client: &ServiceClient,
    admission: &AdmissionState,
    command: &RecipeCommand,
    prepared: Option<Value>,
) -> Result<Value, CommandFailure> {
    match command {
        RecipeCommand::Get { photo_id } => recipe_get(client, photo_id).await,
        RecipeCommand::Save(args) => {
            let body = prepared.ok_or_else(unusable_input)?;
            recipe_write(
                client,
                admission,
                args,
                body,
                SAVE_OPERATION,
                "expectedSourceRevision",
                &["api", "photos", &args.photo_id, "edit-recipe"],
            )
            .await
        }
        RecipeCommand::Rebind(args) => {
            let body = prepared.ok_or_else(unusable_input)?;
            recipe_write(
                client,
                admission,
                args,
                body,
                REBIND_OPERATION,
                "newSourceRevision",
                &["api", "photos", &args.photo_id, "edit-recipe", "rebind"],
            )
            .await
        }
    }
}

async fn recipe_get(client: &ServiceClient, photo_id: &str) -> Result<Value, CommandFailure> {
    let read: RecipeReadWire = client
        .json(
            GET_OPERATION,
            Method::GET,
            client.endpoint(&["api", "photos", photo_id, "edit-recipe"]),
            None,
        )
        .await?;
    validated_recipe_read(read, photo_id, &client.origin)
}

/// Submits one guarded write with the exact prepared body and confirms the
/// outcome against the submitted request. A lost, refused-late, or
/// shape-violating response after admission is an unknown outcome rather
/// than a claimed refusal or receipt.
async fn recipe_write(
    client: &ServiceClient,
    admission: &AdmissionState,
    args: &RecipeWriteArgs,
    body: Value,
    operation: Operation,
    source_key: &str,
    path: &[&str],
) -> Result<Value, CommandFailure> {
    let prepared_field = |key: &str| {
        body.get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let Some(request_id) = prepared_field("requestId") else {
        return Err(unusable_input());
    };
    let Some(submitted_source) = prepared_field(source_key) else {
        return Err(unusable_input());
    };
    let identity = MutationIdentity {
        operation,
        photo_ids: vec![args.photo_id.clone()],
        album_id: None,
        album_name: None,
        mappings: Vec::new(),
    };
    let result: RecipeWriteWire = client
        .mutation(&identity, admission, client.endpoint(path), body)
        .await?;
    let web_url = web_url(&client.origin, &format!("/?photoId={}", args.photo_id))
        .map_err(|()| CommandFailure::unknown(&identity))?;
    confirmed_recipe_write(
        &identity,
        &args.photo_id,
        &request_id,
        &submitted_source,
        web_url,
        result,
    )
}

/// Validates one confirmed write response against the submitted request.
/// The outcome must be the closed `saved` or `unchanged`, the recipe
/// revision nonempty, and the returned source revision exactly the one this
/// command submitted; anything else is an unknown outcome.
fn confirmed_recipe_write(
    identity: &MutationIdentity,
    photo_id: &str,
    request_id: &str,
    submitted_source: &str,
    web_url: String,
    result: RecipeWriteWire,
) -> Result<Value, CommandFailure> {
    if !matches!(result.outcome.as_str(), "saved" | "unchanged")
        || result.recipe_version.is_empty()
        || result.source_revision.is_empty()
        || result.source_revision != submitted_source
    {
        return Err(CommandFailure::unknown(identity));
    }
    Ok(json!({
        "photoId": photo_id,
        "requestId": request_id,
        "outcome": result.outcome,
        "recipeVersion": result.recipe_version,
        "sourceRevision": result.source_revision,
        "webUrl": web_url,
    }))
}

mod composable;
pub use composable::ProcessingRecipeCommand;
pub(super) use composable::*;
mod processing_export;
pub use processing_export::ProcessingExportArgs;
pub(crate) use processing_export::processing_work_valid;
pub(super) use processing_export::*;
// Composable wire validation is owned by the composable module.
// ---------------------------------------------------------------- capability

/// Reads the processing capability report. The client validates the closed
/// state, stage, and range shapes and preserves the reported profiles and
/// ranges; it never derives availability from a camera name, a profile
/// list, or a previously successful operation.
pub(super) async fn capability(client: &ServiceClient) -> Result<Value, CommandFailure> {
    let report: CapabilityReportWire = client
        .json(
            CAPABILITY_OPERATION,
            Method::GET,
            client.endpoint(&["api", "processing", "capability"]),
            None,
        )
        .await?;
    validated_capability_report(report)
}

/// Reads the per-module discovery document without flattening or interpreting
/// module-owned parameter schemas.
pub(super) async fn modules(client: &ServiceClient) -> Result<Value, CommandFailure> {
    let report: Value = client
        .json(
            MODULES_OPERATION,
            Method::GET,
            client.endpoint(&["api", "processing", "modules"]),
            None,
        )
        .await?;
    if !report.get("modules").is_some_and(Value::is_array)
        || !report.get("contractVersion").is_some_and(Value::is_string)
    {
        return Err(CommandFailure::transport(MODULES_OPERATION));
    }
    Ok(report)
}

/// Validates one capability report against the closed wire contract and
/// returns the preserved report. A response outside the contract is a
/// transport failure, not a claimed condition.
fn validated_capability_report(report: CapabilityReportWire) -> Result<Value, CommandFailure> {
    let invalid = || CommandFailure::transport(CAPABILITY_OPERATION);
    let state_valid = matches!(
        report.state.as_str(),
        "disabled" | "bundle-unavailable" | "source-unsupported" | "resource-unavailable" | "ready"
    );
    let stages_valid = matches!(
        report.stages.develop.as_str(),
        "ready" | "unavailable" | "unsupported"
    ) && matches!(
        report.stages.film.as_str(),
        "ready" | "unavailable" | "unsupported"
    );
    let exposure_valid = report.exposure.minimum_ev.is_finite()
        && report.exposure.maximum_ev.is_finite()
        && report.exposure.step_ev.is_finite()
        && report.exposure.step_ev > 0.0
        && report.exposure.maximum_ev >= report.exposure.minimum_ev;
    let lower_hex = |value: &str, length| {
        value.len() == length
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    };
    // The observed bundle and incarnation travel exactly with the ready
    // condition: every other condition has verified no engine bundle, so it
    // reports null identities rather than unverified ones.
    let identities_valid = match (&report.bundle_id, &report.incarnation) {
        (Some(bundle), Some(incarnation)) => {
            report.state == "ready" && lower_hex(bundle, 64) && lower_hex(incarnation, 32)
        }
        (None, None) => report.state != "ready",
        _ => false,
    };
    let state_shape_valid = match report.state.as_str() {
        "ready" => report.stages.develop == "ready" && !report.profiles.is_empty(),
        "source-unsupported" => {
            report.profiles.is_empty()
                && report.stages.develop == "unsupported"
                && report.stages.film == "unsupported"
        }
        _ => {
            !report.profiles.is_empty()
                && report.stages.develop == "unavailable"
                && report.stages.film == "unavailable"
        }
    };
    let profiles_valid = report.profiles.iter().all(|profile| {
        !profile.profile_id.is_empty()
            && !profile.white_balance_modes.is_empty()
            && profile
                .white_balance_modes
                .iter()
                .all(|mode| !mode.is_empty())
            && profile
                .white_balance_ranges
                .as_ref()
                .is_none_or(Value::is_object)
    });
    if !state_valid
        || !stages_valid
        || !exposure_valid
        || !identities_valid
        || !profiles_valid
        || !state_shape_valid
    {
        return Err(invalid());
    }
    serde_json::to_value(&report).map_err(|_| invalid())
}

// ---------------------------------------------------------------- read shapes

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecipeReadWire {
    photo_id: String,
    #[serde(deserialize_with = "required_nullable")]
    source_revision: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    recipe: Option<RecipeValueWire>,
    source_support: String,
    #[serde(deserialize_with = "required_nullable")]
    support_reason: Option<String>,
    processing_available: bool,
    controls: ControlsWire,
    /// Optional on older servers; absent means the recipe came from the
    /// Original rather than a Development Proxy.
    #[serde(default)]
    edit_source: Option<String>,
    /// Present exactly when `editSource` is `development-proxy`.
    #[serde(default)]
    edit_source_proxy_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecipeValueWire {
    recipe_version: String,
    exposure_ev: f64,
    white_balance: WhiteBalanceWire,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ControlsWire {
    exposure: ExposureRangeWire,
    white_balance_modes: Vec<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExposureRangeWire {
    minimum_ev: f64,
    maximum_ev: f64,
    step_ev: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecipeWriteWire {
    outcome: String,
    recipe_version: String,
    source_revision: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CapabilityReportWire {
    state: String,
    #[serde(deserialize_with = "required_nullable")]
    bundle_id: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    incarnation: Option<String>,
    exposure: ExposureRangeWire,
    profiles: Vec<ProfileWire>,
    stages: StageStatesWire,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProfileWire {
    profile_id: String,
    white_balance_modes: Vec<String>,
    #[serde(deserialize_with = "required_nullable")]
    white_balance_ranges: Option<Value>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StageStatesWire {
    develop: String,
    film: String,
}

/// Validates one recipe read against the closed wire contract and renders
/// the CLI result with the added Photo Destination `webUrl`. `sourceRevision`
/// is null exactly when `sourceSupport` is `unavailable`, `supportReason` is
/// non-null only with `unavailable` and carries a closed reason — the
/// confirmed `original-missing`/`original-unreadable` outcomes or the
/// retryable `read-pending`/`resource-unavailable` waits — and a retained
/// recipe stays inside the shared field shapes. A response outside the
/// contract is a transport failure, not a claimed state.
fn validated_recipe_read(
    read: RecipeReadWire,
    photo_id: &str,
    origin: &Url,
) -> Result<Value, CommandFailure> {
    let invalid = || CommandFailure::transport(GET_OPERATION);
    let support_valid = matches!(
        read.source_support.as_str(),
        "supported" | "unavailable" | "unsupported"
    );
    let edit_source_valid = match read.edit_source.as_deref() {
        None | Some("original") => read.edit_source_proxy_id.is_none(),
        Some("development-proxy") => read
            .edit_source_proxy_id
            .as_deref()
            .is_some_and(valid_sha256),
        Some(_) => false,
    };
    let reason_valid = match read.support_reason.as_deref() {
        None => read.source_support != "unavailable",
        Some(reason) => {
            read.source_support == "unavailable"
                && matches!(
                    reason,
                    "original-missing"
                        | "original-unreadable"
                        | "read-pending"
                        | "resource-unavailable"
                )
        }
    };
    let revision_valid = match read.source_revision.as_deref() {
        None => read.source_support == "unavailable",
        Some(revision) => read.source_support != "unavailable" && !revision.is_empty(),
    };
    let recipe_valid = read.recipe.as_ref().is_none_or(|recipe| {
        !recipe.recipe_version.is_empty()
            && recipe.exposure_ev.is_finite()
            && recipe.white_balance.closed_shape()
    });
    let controls_valid = read.controls.exposure.minimum_ev.is_finite()
        && read.controls.exposure.maximum_ev.is_finite()
        && read.controls.exposure.maximum_ev >= read.controls.exposure.minimum_ev
        && read.controls.exposure.step_ev.is_finite()
        && read.controls.exposure.step_ev > 0.0
        && !read.controls.white_balance_modes.is_empty()
        && read
            .controls
            .white_balance_modes
            .iter()
            .all(|mode| !mode.is_empty());
    let recipe_admitted = read.recipe.as_ref().is_none_or(|recipe| {
        let mode = match recipe.white_balance.mode {
            WhiteBalanceMode::AsShot => "as-shot",
            WhiteBalanceMode::TemperatureTint => "temperature-tint",
        };
        let exposure = read.controls.exposure;
        let steps = recipe.exposure_ev / exposure.step_ev;
        read.controls
            .white_balance_modes
            .iter()
            .any(|allowed| allowed == mode)
            && recipe.exposure_ev >= exposure.minimum_ev
            && recipe.exposure_ev <= exposure.maximum_ev
            && steps.is_finite()
            && (steps - steps.round()).abs() < 1e-6
    });
    if read.photo_id != photo_id
        || !support_valid
        || !reason_valid
        || !revision_valid
        || !edit_source_valid
        || !recipe_valid
        || !controls_valid
        || (read.processing_available && (read.source_support != "supported" || !recipe_admitted))
    {
        return Err(invalid());
    }
    let web_url = web_url(origin, &format!("/?photoId={photo_id}")).map_err(|()| invalid())?;
    let mut read = read;
    read.edit_source
        .get_or_insert_with(|| "original".to_owned());
    let mut value = serde_json::to_value(&read).map_err(|_| invalid())?;
    value["webUrl"] = Value::String(web_url);
    Ok(value)
}

#[cfg(test)]
mod tests;
