//! Edit Recipe reads and guarded writes: one strict `--input` reader per
//! write, one service read with semantic validation, and one mutation per
//! command. The caller owns every guard — request identity, expected recipe
//! version, and expected or newly observed source revision — and this module
//! never refreshes a guard behind the caller's back or retries a write.

use super::{
    AdmissionState, CommandFailure, MutationIdentity, Operation, ServiceClient, read_input_bytes,
    valid_request_identity, web_url,
};
use reqwest::Method;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use url::Url;

const CAPABILITY_OPERATION: Operation = Operation::ProcessingCapability;
const GET_OPERATION: Operation = Operation::PhotosRecipeGet;
const SAVE_OPERATION: Operation = Operation::PhotosRecipeSave;
const REBIND_OPERATION: Operation = Operation::PhotosRecipeRebind;

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

fn unusable_input() -> CommandFailure {
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

/// Validates one capability report against the closed wire contract and
/// returns the preserved report. A response outside the contract is a
/// transport failure, not a claimed condition.
fn validated_capability_report(report: CapabilityReportWire) -> Result<Value, CommandFailure> {
    let invalid = || CommandFailure::transport(CAPABILITY_OPERATION);
    let state_valid = matches!(
        report.state.as_str(),
        "disabled"
            | "launcher-unavailable"
            | "bundle-unavailable"
            | "source-unsupported"
            | "resource-unavailable"
            | "ready"
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
    let identities_valid = match (&report.bundle_id, &report.incarnation) {
        (Some(bundle), Some(incarnation)) => {
            report.state != "disabled" && lower_hex(bundle, 64) && lower_hex(incarnation, 32)
        }
        (None, None) => matches!(report.state.as_str(), "disabled" | "launcher-unavailable"),
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
        || !recipe_valid
        || !controls_valid
        || (read.processing_available && (read.source_support != "supported" || !recipe_admitted))
    {
        return Err(invalid());
    }
    let web_url = web_url(origin, &format!("/?photoId={photo_id}")).map_err(|()| invalid())?;
    let mut value = serde_json::to_value(&read).map_err(|_| invalid())?;
    value["webUrl"] = Value::String(web_url);
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn save_document(guard: Value, exposure: Value, white_balance: Value) -> Vec<u8> {
        json!({
            "requestId": "edit-001",
            "expectedRecipeVersion": guard,
            "expectedSourceRevision": "observed-source-revision",
            "settings": {
                "exposureEv": exposure,
                "whiteBalance": white_balance,
            },
        })
        .to_string()
        .into_bytes()
    }

    fn failure_argument(failure: &CommandFailure) -> String {
        failure.payload.details["argument"]
            .as_str()
            .expect("input failures name an argument")
            .to_owned()
    }

    fn refuses(failure: CommandFailure, argument: &str) {
        assert_eq!(failure.exit_code, 2, "for {failure:?}");
        assert_eq!(failure.payload.code, "invalid_input");
        assert_eq!(failure_argument(&failure), argument);
    }

    fn origin() -> Url {
        Url::parse("https://slipstream.example").expect("origin parses")
    }

    fn read_wire(document: Value) -> RecipeReadWire {
        serde_json::from_value(document).expect("fixture decodes")
    }

    fn capability_wire(document: Value) -> CapabilityReportWire {
        serde_json::from_value(document).expect("fixture decodes")
    }

    fn identity(operation: Operation) -> MutationIdentity {
        MutationIdentity {
            operation,
            photo_ids: vec!["p1".to_owned()],
            album_id: None,
            album_name: None,
            mappings: Vec::new(),
        }
    }

    // ------------------------------------------------------------ save input

    #[test]
    fn save_input_accepts_the_documented_shape_with_an_explicit_null_guard() {
        let body = parse_save(save_document(
            Value::Null,
            json!(1.0),
            json!({ "mode": "as-shot" }),
        ))
        .expect("documented save input");
        assert_eq!(
            body,
            json!({
                "requestId": "edit-001",
                "expectedRecipeVersion": Value::Null,
                "expectedSourceRevision": "observed-source-revision",
                "settings": {
                    "exposureEv": 1.0,
                    "whiteBalance": { "mode": "as-shot" },
                },
            })
        );
    }

    #[test]
    fn save_input_accepts_an_explicit_string_guard_and_payload_bounds() {
        let body = parse_save(
            json!({
                "requestId": "edit-002",
                "expectedRecipeVersion": "recipe-7",
                "expectedSourceRevision": "source-3",
                "settings": {
                    "exposureEv": 0.25,
                    "whiteBalance": {
                        "mode": "temperature-tint",
                        "temperatureKelvin": 40_000,
                        "tintMilli": -150_000,
                    },
                },
            })
            .to_string()
            .into_bytes(),
        )
        .expect("explicit guard save input");
        assert_eq!(body["expectedRecipeVersion"], "recipe-7");
        assert_eq!(
            body["settings"]["whiteBalance"],
            json!({
                "mode": "temperature-tint",
                "temperatureKelvin": 40_000,
                "tintMilli": -150_000,
            })
        );
        assert_eq!(body.as_object().expect("object").len(), 4);
    }

    #[test]
    fn save_input_refuses_an_omitted_nullable_guard_but_not_the_explicit_null() {
        let omitted = br#"{
            "requestId": "edit-001",
            "expectedSourceRevision": "observed-source-revision",
            "settings": {"exposureEv": 0.0, "whiteBalance": {"mode": "as-shot"}}
        }"#;
        refuses(parse_save(omitted.to_vec()).unwrap_err(), "input");
        parse_save(save_document(
            Value::Null,
            json!(0.0),
            json!({ "mode": "as-shot" }),
        ))
        .expect("the explicit null guard is valid");
    }

    #[test]
    fn save_input_refuses_malformed_documents() {
        for invalid in [
            Vec::new(),
            b"{".to_vec(),
            b"[]".to_vec(),
            br#""edit-001""#.to_vec(),
            br#"{"requestId":"a","expectedRecipeVersion":null,"expectedSourceRevision":"s"}"#
                .to_vec(),
            br#"{"requestId":"a","expectedRecipeVersion":null,"expectedSourceRevision":"s",
                 "settings":{"exposureEv":1.0,"whiteBalance":{"mode":"as-shot"}}} trailing"#
                .to_vec(),
            br#"{"requestId":"a","requestId":"b","expectedRecipeVersion":null,
                 "expectedSourceRevision":"s","settings":{"exposureEv":1.0,
                 "whiteBalance":{"mode":"as-shot"}}}"#
                .to_vec(),
            br#"{"requestId":"a","expectedRecipeVersion":null,"expectedSourceRevision":"s",
                 "extra":1,"settings":{"exposureEv":1.0,"whiteBalance":{"mode":"as-shot"}}}"#
                .to_vec(),
            br#"{"requestId":"a","expectedRecipeVersion":null,"expectedSourceRevision":"s",
                 "settings":{"exposureEv":1.0,"whiteBalance":{"mode":"as-shot"},"extra":1}}"#
                .to_vec(),
            br#"{"requestId":"a","expectedRecipeVersion":null,"expectedSourceRevision":5,
                 "settings":{"exposureEv":1.0,"whiteBalance":{"mode":"as-shot"}}}"#
                .to_vec(),
        ] {
            refuses(parse_save(invalid).unwrap_err(), "input");
        }
    }

    #[test]
    fn save_input_refuses_empty_or_illformed_guards_in_admission_order() {
        let empty_request_id = save_document(Value::Null, json!(1.0), json!({ "mode": "as-shot" }));
        let mut document: serde_json::Value =
            serde_json::from_slice(&empty_request_id).expect("fixture decodes");
        document["requestId"] = json!("edit 001");
        refuses(
            parse_save(document.to_string().into_bytes()).unwrap_err(),
            "requestId",
        );

        for (argument, guard, source) in [
            ("expectedRecipeVersion", json!(""), json!("source-3")),
            ("expectedSourceRevision", json!("recipe-7"), json!("")),
        ] {
            let document = json!({
                "requestId": "edit-001",
                "expectedRecipeVersion": guard,
                "expectedSourceRevision": source,
                "settings": {"exposureEv": 1.0, "whiteBalance": {"mode": "as-shot"}},
            });
            refuses(
                parse_save(document.to_string().into_bytes()).unwrap_err(),
                argument,
            );
        }
    }

    #[test]
    fn save_input_refuses_nonfinite_exposure_without_hardcoding_a_range() {
        // serde_json itself refuses numbers outside the f64 range; the
        // module's finite guard covers any number a decoder admits.
        let overflowing = br#"{
            "requestId": "edit-001",
            "expectedRecipeVersion": null,
            "expectedSourceRevision": "observed-source-revision",
            "settings": {"exposureEv": 1e400, "whiteBalance": {"mode": "as-shot"}}
        }"#;
        refuses(parse_save(overflowing.to_vec()).unwrap_err(), "input");
    }

    #[test]
    fn save_input_refuses_white_balance_payloads_outside_the_closed_shapes() {
        for (argument, white_balance) in [
            // In-range integers outside the published payload bounds.
            (
                "whiteBalance",
                json!({"mode": "temperature-tint", "temperatureKelvin": 999, "tintMilli": 0}),
            ),
            (
                "whiteBalance",
                json!({"mode": "temperature-tint", "temperatureKelvin": 40_001, "tintMilli": 0}),
            ),
            (
                "whiteBalance",
                json!({"mode": "temperature-tint", "temperatureKelvin": 6_500, "tintMilli": 150_001}),
            ),
            (
                "whiteBalance",
                json!({"mode": "temperature-tint", "temperatureKelvin": 6_500, "tintMilli": -150_001}),
            ),
            // A mode that requires fields the document omits or nulls out.
            (
                "whiteBalance",
                json!({"mode": "temperature-tint", "temperatureKelvin": 6_500}),
            ),
            // A mode that requires no fields must not carry them.
            (
                "whiteBalance",
                json!({"mode": "as-shot", "temperatureKelvin": 6_500}),
            ),
            ("whiteBalance", json!({"mode": "as-shot", "tintMilli": 0})),
        ] {
            refuses(
                parse_save(save_document(Value::Null, json!(1.0), white_balance)).unwrap_err(),
                argument,
            );
        }
        // Wrong types and unknown modes fail at the decoder.
        for white_balance in [
            json!({"mode": "temperature-tint", "temperatureKelvin": 6.5, "tintMilli": 0}),
            json!({"mode": "custom"}),
            json!("as-shot"),
        ] {
            refuses(
                parse_save(save_document(Value::Null, json!(1.0), white_balance)).unwrap_err(),
                "input",
            );
        }
        // A duplicated key inside whiteBalance is refused by the decoder;
        // the document must be raw text because the json! macro collapses
        // duplicate keys while building the fixture.
        let duplicated = br#"{
            "requestId": "edit-001",
            "expectedRecipeVersion": null,
            "expectedSourceRevision": "observed-source-revision",
            "settings": {"exposureEv": 1.0, "whiteBalance": {
                "mode": "temperature-tint",
                "temperatureKelvin": 6500,
                "temperatureKelvin": 6501,
                "tintMilli": 0
            }}
        }"#;
        refuses(parse_save(duplicated.to_vec()).unwrap_err(), "input");
    }

    // ---------------------------------------------------------- rebind input

    #[test]
    fn rebind_input_accepts_only_the_exact_three_key_document() {
        let body = parse_rebind(
            br#"{"requestId":"rebind-1","expectedRecipeVersion":"recipe-7",
                 "newSourceRevision":"source-9"}"#
                .to_vec(),
        )
        .expect("documented rebind input");
        assert_eq!(
            body,
            json!({
                "requestId": "rebind-1",
                "expectedRecipeVersion": "recipe-7",
                "newSourceRevision": "source-9",
            })
        );
    }

    #[test]
    fn rebind_input_refuses_incomplete_or_illformed_guards() {
        for (document, argument) in [
            (Vec::new(), "input"),
            (
                br#"{"requestId":"rebind-1","newSourceRevision":"source-9"}"#.to_vec(),
                "input",
            ),
            (
                br#"{"requestId":"rebind-1","expectedRecipeVersion":"r",
                     "expectedRecipeVersion":"r2","newSourceRevision":"s"}"#
                    .to_vec(),
                "input",
            ),
            (
                br#"{"requestId":"rebind-1","expectedRecipeVersion":"",
                     "newSourceRevision":"source-9"}"#
                    .to_vec(),
                "expectedRecipeVersion",
            ),
            (
                br#"{"requestId":"rebind-1","expectedRecipeVersion":"recipe-7",
                     "newSourceRevision":""}"#
                    .to_vec(),
                "newSourceRevision",
            ),
            (
                br#"{"requestId":"rebind 1","expectedRecipeVersion":"recipe-7",
                     "newSourceRevision":"source-9"}"#
                    .to_vec(),
                "requestId",
            ),
            (
                br#"{"requestId":"rebind-1","expectedRecipeVersion":"recipe-7",
                     "newSourceRevision":"source-9","extra":1}"#
                    .to_vec(),
                "input",
            ),
        ] {
            refuses(parse_rebind(document).unwrap_err(), argument);
        }
    }

    // ------------------------------------------------------------ read shape

    fn read_fixture() -> Value {
        json!({
            "photoId": "p1",
            "sourceRevision": "source-3",
            "recipe": Value::Null,
            "sourceSupport": "supported",
            "supportReason": Value::Null,
            "processingAvailable": false,
            "controls": {
                "exposure": {"minimumEv": -4.0, "maximumEv": 4.0, "stepEv": 0.001},
                "whiteBalanceModes": ["as-shot"],
            },
        })
    }

    #[test]
    fn recipe_read_renders_the_documented_facts_with_the_photo_destination() {
        let value = validated_recipe_read(read_wire(read_fixture()), "p1", &origin())
            .expect("documented read");
        assert_eq!(value["photoId"], "p1");
        assert_eq!(value["sourceRevision"], "source-3");
        assert_eq!(value["recipe"], Value::Null);
        assert_eq!(value["sourceSupport"], "supported");
        assert_eq!(value["supportReason"], Value::Null);
        assert_eq!(value["processingAvailable"], false);
        assert_eq!(
            value["controls"],
            json!({
                "exposure": {"minimumEv": -4.0, "maximumEv": 4.0, "stepEv": 0.001},
                "whiteBalanceModes": ["as-shot"],
            })
        );
        assert_eq!(value["webUrl"], "https://slipstream.example/?photoId=p1");
    }

    #[test]
    fn recipe_read_accepts_an_unavailable_source_with_its_closed_reason() {
        // The confirmed outcomes and the retryable waits are all closed
        // reasons this client believes without downgrading the read.
        for reason in [
            "original-missing",
            "original-unreadable",
            "read-pending",
            "resource-unavailable",
        ] {
            let document = json!({
                "photoId": "p1",
                "sourceRevision": Value::Null,
                "recipe": Value::Null,
                "sourceSupport": "unavailable",
                "supportReason": reason,
                "processingAvailable": false,
                "controls": {
                    "exposure": {"minimumEv": -4.0, "maximumEv": 4.0, "stepEv": 0.001},
                    "whiteBalanceModes": ["as-shot"],
                },
            });
            let value = validated_recipe_read(read_wire(document), "p1", &origin())
                .unwrap_or_else(|failure| panic!("{reason} is a closed reason: {failure:?}"));
            assert_eq!(value["sourceSupport"], "unavailable");
            assert_eq!(value["supportReason"], json!(reason));
            assert_eq!(value["sourceRevision"], Value::Null);
        }
    }

    #[test]
    fn retained_unadmitted_settings_cannot_claim_processing_available() {
        for (exposure, white_balance) in [
            (
                1.0,
                json!({"mode":"temperature-tint", "temperatureKelvin":6500,"tintMilli":0}),
            ),
            (5.0, json!({"mode":"as-shot"})),
            (0.0005, json!({"mode":"as-shot"})),
        ] {
            let mut document = read_fixture();
            document["recipe"] = json!({"recipeVersion":"retained", "exposureEv":exposure, "whiteBalance":white_balance});
            let retained =
                validated_recipe_read(read_wire(document.clone()), "p1", &origin()).unwrap();
            assert_eq!(retained["recipe"], document["recipe"]);
            assert_eq!(retained["processingAvailable"], false);
            document["processingAvailable"] = json!(true);
            assert_eq!(
                validated_recipe_read(read_wire(document), "p1", &origin())
                    .unwrap_err()
                    .payload
                    .code,
                "transport_failed"
            );
        }
    }

    #[test]
    fn recipe_read_accepts_retained_temperature_tint_inside_the_payload_bounds() {
        let mut document = read_fixture();
        document["recipe"] = json!({
            "recipeVersion": "recipe-7",
            "exposureEv": 1.5,
            "whiteBalance": {
                "mode": "temperature-tint",
                "temperatureKelvin": 40_000,
                "tintMilli": 150_000,
            },
        });
        let value =
            validated_recipe_read(read_wire(document), "p1", &origin()).expect("retained intent");
        assert_eq!(
            value["recipe"]["whiteBalance"],
            json!({
                "mode": "temperature-tint",
                "temperatureKelvin": 40_000,
                "tintMilli": 150_000,
            })
        );
    }

    #[test]
    fn recipe_read_refuses_responses_outside_the_closed_contract() {
        let invalid = |document: Value| {
            let failure = validated_recipe_read(read_wire(document), "p1", &origin()).unwrap_err();
            assert_eq!(failure.exit_code, 6, "for {failure:?}");
            assert_eq!(failure.payload.code, "transport_failed");
            assert_eq!(failure.payload.details["operation"], "photos-recipe-get");
        };
        // A read must answer for exactly the requested Photo.
        let mut document = read_fixture();
        document["photoId"] = json!("p2");
        invalid(document);
        // The support state is closed.
        let mut document = read_fixture();
        document["sourceSupport"] = json!("missing");
        invalid(document);
        // sourceRevision is null exactly when sourceSupport is unavailable.
        let mut document = read_fixture();
        document["sourceRevision"] = Value::Null;
        invalid(document);
        let mut document = read_fixture();
        document["sourceRevision"] = json!("");
        invalid(document);
        let mut document = read_fixture();
        document["sourceSupport"] = json!("unavailable");
        invalid(document);
        // supportReason is non-null only with unavailable and carries a
        // closed reason.
        let mut document = read_fixture();
        document["supportReason"] = json!("original-missing");
        invalid(document);
        let mut document = read_fixture();
        document["supportReason"] = json!("read-pending");
        invalid(document);
        let mut document = read_fixture();
        document["sourceSupport"] = json!("unavailable");
        document["sourceRevision"] = Value::Null;
        document["supportReason"] = json!("original-rotated");
        invalid(document);
        let mut document = read_fixture();
        document["sourceSupport"] = json!("unavailable");
        document["sourceRevision"] = Value::Null;
        invalid(document);
        // A retained recipe keeps the shared field shapes.
        let mut document = read_fixture();
        document["recipe"] = json!({
            "recipeVersion": "",
            "exposureEv": 1.0,
            "whiteBalance": { "mode": "as-shot" },
        });
        invalid(document);
        let mut document = read_fixture();
        document["recipe"] = json!({
            "recipeVersion": "recipe-7",
            "exposureEv": 1.0,
            "whiteBalance": {
                "mode": "temperature-tint",
                "temperatureKelvin": 999,
                "tintMilli": 0,
            },
        });
        invalid(document);
        // The controls carry a sane closed range.
        let mut document = read_fixture();
        document["controls"]["exposure"]["stepEv"] = json!(0.0);
        invalid(document);
        let mut document = read_fixture();
        document["controls"]["exposure"] =
            json!({"minimumEv": 4.0, "maximumEv": -4.0, "stepEv": 0.001});
        invalid(document);
        let mut document = read_fixture();
        document["controls"]["whiteBalanceModes"] = json!([]);
        invalid(document);
    }

    // --------------------------------------------------- write confirmation

    #[test]
    fn write_confirmation_renders_the_documented_result_for_matching_outcomes() {
        for outcome in ["saved", "unchanged"] {
            let result = RecipeWriteWire {
                outcome: outcome.to_owned(),
                recipe_version: "recipe-8".to_owned(),
                source_revision: "source-3".to_owned(),
            };
            let value = confirmed_recipe_write(
                &identity(SAVE_OPERATION),
                "p1",
                "edit-001",
                "source-3",
                "https://slipstream.example/?photoId=p1".to_owned(),
                result,
            )
            .expect("matching confirmation");
            assert_eq!(
                value,
                json!({
                    "photoId": "p1",
                    "requestId": "edit-001",
                    "outcome": outcome,
                    "recipeVersion": "recipe-8",
                    "sourceRevision": "source-3",
                    "webUrl": "https://slipstream.example/?photoId=p1",
                })
            );
        }
    }

    #[test]
    fn write_confirmation_treats_an_unusable_response_as_an_unknown_outcome() {
        let unknown = |result: RecipeWriteWire, submitted: &str| {
            let failure = confirmed_recipe_write(
                &identity(SAVE_OPERATION),
                "p1",
                "edit-001",
                submitted,
                "https://slipstream.example/?photoId=p1".to_owned(),
                result,
            )
            .unwrap_err();
            assert_eq!(failure.exit_code, 7, "for {failure:?}");
            assert_eq!(failure.payload.code, "outcome_unknown");
            assert_eq!(failure.payload.effect, "unknown");
            assert_eq!(failure.payload.details["operation"], "photos-recipe-save");
            assert_eq!(failure.payload.details["photoIds"], json!(["p1"]));
        };
        unknown(
            RecipeWriteWire {
                outcome: "conflicted".to_owned(),
                recipe_version: "recipe-8".to_owned(),
                source_revision: "source-3".to_owned(),
            },
            "source-3",
        );
        unknown(
            RecipeWriteWire {
                outcome: "saved".to_owned(),
                recipe_version: "".to_owned(),
                source_revision: "source-3".to_owned(),
            },
            "source-3",
        );
        // The returned source must be exactly the submitted guard.
        unknown(
            RecipeWriteWire {
                outcome: "saved".to_owned(),
                recipe_version: "recipe-8".to_owned(),
                source_revision: "source-4".to_owned(),
            },
            "source-3",
        );
    }

    // ------------------------------------------------------------ capability

    fn ready_capability() -> Value {
        json!({
            "state": "ready",
            "bundleId": "c".repeat(64),
            "incarnation": "a".repeat(32),
            "exposure": {"minimumEv": -4.0, "maximumEv": 4.0, "stepEv": 0.001},
            "profiles": [
                {
                    "profileId": "sony-a7iv",
                    "whiteBalanceModes": ["as-shot"],
                    "whiteBalanceRanges": Value::Null,
                },
            ],
            "stages": {"develop": "ready", "film": "ready"},
        })
    }

    #[test]
    fn capability_preserves_the_reported_profiles_and_ranges() {
        let value =
            validated_capability_report(capability_wire(ready_capability())).expect("ready report");
        assert_eq!(value, ready_capability());
        // The disabled condition carries no observed launcher identities.
        let mut disabled = ready_capability();
        disabled["state"] = json!("disabled");
        disabled["bundleId"] = Value::Null;
        disabled["incarnation"] = Value::Null;
        disabled["stages"] = json!({"develop": "unavailable", "film": "unavailable"});
        validated_capability_report(capability_wire(disabled)).expect("disabled report");
        // A source-unsupported deployment reports an empty profile list.
        let mut unsupported = ready_capability();
        unsupported["state"] = json!("source-unsupported");
        unsupported["profiles"] = json!([]);
        unsupported["stages"] = json!({"develop": "unsupported", "film": "unsupported"});
        let value = validated_capability_report(capability_wire(unsupported))
            .expect("source-unsupported report");
        assert_eq!(value["profiles"], json!([]));
    }

    #[test]
    fn capability_refuses_reports_outside_the_closed_contract() {
        let invalid = |document: Value| {
            let failure = validated_capability_report(capability_wire(document)).unwrap_err();
            assert_eq!(failure.exit_code, 6, "for {failure:?}");
            assert_eq!(failure.payload.code, "transport_failed");
            assert_eq!(
                failure.payload.details["operation"],
                "processing-capability"
            );
        };
        let mut document = ready_capability();
        document["state"] = json!("offline");
        invalid(document);
        let mut document = ready_capability();
        document["stages"]["film"] = json!("queued");
        invalid(document);
        let mut document = ready_capability();
        document["exposure"]["stepEv"] = json!(0.0);
        invalid(document);
        let mut document = ready_capability();
        document["exposure"] = json!({"minimumEv": 4.0, "maximumEv": -4.0, "stepEv": 0.001});
        invalid(document);
        let mut document = ready_capability();
        document["bundleId"] = json!("");
        invalid(document);
        let mut document = ready_capability();
        document["profiles"][0]["profileId"] = json!("");
        invalid(document);
        let mut document = ready_capability();
        document["profiles"][0]["whiteBalanceModes"] = json!([]);
        invalid(document);
        let mut document = ready_capability();
        document["profiles"][0]["whiteBalanceModes"] = json!([""]);
        invalid(document);
        let mut document = ready_capability();
        document["bundleId"] = Value::Null;
        invalid(document);
        let mut document = ready_capability();
        document["stages"]["develop"] = json!("unsupported");
        invalid(document);
        let mut document = ready_capability();
        document["profiles"][0]["whiteBalanceRanges"] = json!(42);
        invalid(document);
        for state in [
            "disabled",
            "launcher-unavailable",
            "bundle-unavailable",
            "resource-unavailable",
        ] {
            let mut document = ready_capability();
            document["state"] = json!(state);
            document["stages"] = json!({"develop": "unavailable", "film": "unavailable"});
            document["profiles"] = json!([]);
            if state == "disabled" {
                document["bundleId"] = Value::Null;
                document["incarnation"] = Value::Null;
            }
            invalid(document);
        }
    }

    #[test]
    fn missing_nullable_facts_and_null_white_balance_fields_are_refused() {
        for key in ["sourceRevision", "recipe", "supportReason"] {
            let mut value = read_fixture();
            value.as_object_mut().unwrap().remove(key);
            assert!(serde_json::from_value::<RecipeReadWire>(value).is_err());
        }
        for key in ["temperatureKelvin", "tintMilli"] {
            let mut white_balance = json!({"mode":"as-shot"});
            white_balance[key] = Value::Null;
            assert!(parse_save(save_document(Value::Null, json!(1), white_balance)).is_err());
        }
    }
}
