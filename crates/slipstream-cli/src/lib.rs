use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use reqwest::{Client, Method, StatusCode};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{env, ffi::OsString, fmt, time::Duration};
use url::Url;

const CLI_CONTRACT_VERSION: u16 = 1;
const DEFAULT_SERVER: &str = "http://127.0.0.1:3000";
const DEFAULT_TIMEOUT_SECONDS: u64 = 30;
const DEFAULT_LIST_PAGE: usize = 50;
const MAXIMUM_LIST_PAGE: usize = 60;
const CONTRACT_HEADER: &str = "Slipstream-CLI-Contract";
const MAXIMUM_JSON_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Debug, Parser)]
#[command(
    name = "slipstream",
    version,
    about = "Query a Slipstream Photo Library",
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Slipstream service origin. Overrides SLIPSTREAM_SERVER_URL.
    #[arg(long, value_name = "URL")]
    pub server: Option<String>,

    /// Output format for operational commands.
    #[arg(long, value_enum, default_value_t = OutputFormat::Json)]
    pub output: OutputFormat,

    /// Whole-command deadline in seconds.
    #[arg(long, value_name = "SECONDS", default_value_t = DEFAULT_TIMEOUT_SECONDS, value_parser = clap::value_parser!(u64).range(1..=300))]
    pub timeout: u64,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum OutputFormat {
    Json,
    Text,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParseErrorPreferences {
    pub output: OutputFormat,
    pub timeout_seconds: u64,
}

pub fn parse_error_preferences(arguments: &[OsString]) -> ParseErrorPreferences {
    let matches = Cli::command()
        .ignore_errors(true)
        .try_get_matches_from(arguments)
        .ok();
    ParseErrorPreferences {
        output: matches
            .as_ref()
            .and_then(|matches| matches.get_one::<OutputFormat>("output"))
            .copied()
            .unwrap_or(OutputFormat::Json),
        timeout_seconds: matches
            .as_ref()
            .and_then(|matches| matches.get_one::<u64>("timeout"))
            .copied()
            .unwrap_or(DEFAULT_TIMEOUT_SECONDS),
    }
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Inspect service compatibility and current Library status.
    Status,
    /// Discover read-only Original Folders.
    Folders {
        #[command(subcommand)]
        command: FolderCommand,
    },
    /// Discover or inspect Albums.
    Albums {
        #[command(subcommand)]
        command: AlbumCommand,
    },
    /// Query or inspect Photos.
    Photos {
        #[command(subcommand)]
        command: PhotoCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum FolderCommand {
    /// List direct child Folders. Photo counts include descendants.
    List(FolderListArgs),
}

#[derive(Debug, Args)]
pub struct FolderListArgs {
    /// Library-relative parent Location. Empty means the Library Folder.
    #[arg(long, value_name = "LOCATION", conflicts_with = "cursor")]
    pub parent: Option<String>,
    /// Maximum items in this page (1 through 60).
    #[arg(long, value_name = "N", value_parser = page_limit, conflicts_with = "cursor")]
    pub limit: Option<u8>,
    /// Opaque continuation from the preceding Folder page.
    #[arg(long, value_name = "CURSOR", value_parser = nonempty, conflicts_with_all = ["parent", "limit"])]
    pub cursor: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum AlbumCommand {
    /// List current Album summaries.
    List(AlbumListArgs),
    /// Get one current Album summary without its members.
    Get {
        #[arg(value_parser = nonempty)]
        album_id: String,
    },
}

#[derive(Debug, Args)]
pub struct AlbumListArgs {
    /// Match one Album name using the Library naming comparison.
    #[arg(long, value_name = "NAME", conflicts_with_all = ["photo", "cursor"])]
    pub name: Option<String>,
    /// List Albums that contain this opaque Photo ID.
    #[arg(long, value_name = "PHOTO_ID", conflicts_with_all = ["name", "cursor"])]
    pub photo: Option<String>,
    /// Maximum items in this page (1 through 60).
    #[arg(long, value_name = "N", value_parser = page_limit, conflicts_with = "cursor")]
    pub limit: Option<u8>,
    /// Opaque continuation from the preceding Album page.
    #[arg(long, value_name = "CURSOR", value_parser = nonempty, conflicts_with_all = ["name", "photo", "limit"])]
    pub cursor: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum PhotoCommand {
    /// Query a bounded page of Photos. Filters are combined with AND.
    List(PhotoListArgs),
    /// Get one Photo's current facts and bounded metadata.
    Get {
        #[arg(value_parser = nonempty)]
        photo_id: String,
    },
}

#[derive(Debug, Args)]
pub struct PhotoListArgs {
    /// Query one Album by opaque ID.
    #[arg(long, value_name = "ALBUM_ID", conflicts_with_all = ["folder", "cursor"])]
    pub album: Option<String>,
    /// Query one recursive Library-relative Original Folder.
    #[arg(long, value_name = "LOCATION", conflicts_with_all = ["album", "cursor"])]
    pub folder: Option<String>,
    /// Match one Selection State; `all` imposes no Selection State filter.
    #[arg(long, value_enum, conflicts_with = "cursor")]
    pub selection: Option<SelectionArg>,
    /// Inclusive minimum Rating from 0 through 5.
    #[arg(long, value_name = "N", value_parser = rating, conflicts_with = "cursor")]
    pub rating_min: Option<u8>,
    /// Inclusive maximum Rating from 0 through 5.
    #[arg(long, value_name = "N", value_parser = rating, conflicts_with = "cursor")]
    pub rating_max: Option<u8>,
    /// Match one Original kind.
    #[arg(long, value_enum, conflicts_with = "cursor")]
    pub kind: Option<OriginalKindArg>,
    /// Match Original File availability exactly.
    #[arg(long, value_name = "true|false", value_parser = exact_bool, conflicts_with = "cursor")]
    pub available: Option<bool>,
    /// Inclusive camera-local lower bound in YYYY-MM-DDTHH:MM:SS form.
    #[arg(long, value_name = "LOCAL_TIME", value_parser = local_time, conflicts_with = "cursor")]
    pub captured_from: Option<String>,
    /// Exclusive camera-local upper bound in YYYY-MM-DDTHH:MM:SS form.
    #[arg(long, value_name = "LOCAL_TIME", value_parser = local_time, conflicts_with = "cursor")]
    pub captured_before: Option<String>,
    /// Result order. Album order requires an Album source.
    #[arg(long, value_enum, conflicts_with = "cursor")]
    pub order: Option<OrderArg>,
    /// Maximum items in this page (1 through 60).
    #[arg(long, value_name = "N", value_parser = page_limit, conflicts_with = "cursor")]
    pub limit: Option<u8>,
    /// Opaque continuation from the preceding Photo page.
    #[arg(long, value_name = "CURSOR", value_parser = nonempty, conflicts_with_all = ["album", "folder", "selection", "rating_min", "rating_max", "kind", "available", "captured_from", "captured_before", "order", "limit"])]
    pub cursor: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum SelectionArg {
    All,
    Undecided,
    Selected,
    Rejected,
}

#[derive(Clone, Copy, Debug, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum OriginalKindArg {
    Raw,
    Jpeg,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum OrderArg {
    CaptureTimeAsc,
    CaptureTimeDesc,
    AlbumOrder,
}

fn nonempty(value: &str) -> Result<String, String> {
    (!value.is_empty())
        .then(|| value.to_owned())
        .ok_or_else(|| "must not be empty".to_owned())
}

fn page_limit(value: &str) -> Result<u8, String> {
    value
        .parse::<u8>()
        .ok()
        .filter(|value| (1..=60).contains(value))
        .ok_or_else(|| "must be an integer from 1 through 60".to_owned())
}

fn rating(value: &str) -> Result<u8, String> {
    value
        .parse::<u8>()
        .ok()
        .filter(|value| *value <= 5)
        .ok_or_else(|| "must be an integer from 0 through 5".to_owned())
}

fn exact_bool(value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err("must be true or false".to_owned()),
    }
}

fn local_time(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let shape = bytes.len() == 19
        && [4, 7].into_iter().all(|index| bytes[index] == b'-')
        && bytes[10] == b'T'
        && [13, 16].into_iter().all(|index| bytes[index] == b':')
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7 | 10 | 13 | 16) || byte.is_ascii_digit());
    if !shape {
        return Err("must have exact form YYYY-MM-DDTHH:MM:SS".to_owned());
    }
    let number = |range: std::ops::Range<usize>| {
        std::str::from_utf8(&bytes[range])
            .unwrap()
            .parse::<u32>()
            .unwrap()
    };
    let year = number(0..4);
    let month = number(5..7);
    let day = number(8..10);
    let hour = number(11..13);
    let minute = number(14..16);
    let second = number(17..19);
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let maximum_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => 0,
    };
    if year == 0 || day == 0 || day > maximum_day || hour > 23 || minute > 59 || second > 59 {
        return Err("must be a valid camera-local date and time".to_owned());
    }
    Ok(value.to_owned())
}

#[derive(Debug)]
pub struct InvocationResult {
    pub exit_code: u8,
    pub stdout: String,
}

#[derive(Clone, Copy, Debug)]
enum Operation {
    Status,
    FoldersList,
    AlbumsList,
    AlbumsGet,
    PhotosList,
    PhotosGet,
}

impl Operation {
    fn wire(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::FoldersList => "folders-list",
            Self::AlbumsList => "albums-list",
            Self::AlbumsGet => "albums-get",
            Self::PhotosList => "photos-list",
            Self::PhotosGet => "photos-get",
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    schema_version: u8,
    status: &'static str,
    data: Option<Value>,
    error: Option<ErrorPayload>,
}

impl Envelope {
    fn success(data: Value) -> Self {
        Self {
            schema_version: 1,
            status: "ok",
            data: Some(data),
            error: None,
        }
    }

    fn error(error: ErrorPayload) -> Self {
        Self {
            schema_version: 1,
            status: "error",
            data: None,
            error: Some(error),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ErrorPayload {
    code: String,
    message: String,
    effect: String,
    details: Value,
}

#[derive(Debug)]
struct CommandFailure {
    exit_code: u8,
    payload: ErrorPayload,
}

impl CommandFailure {
    fn invalid(argument: &str, reason: impl Into<String>) -> Self {
        Self {
            exit_code: 2,
            payload: ErrorPayload {
                code: "invalid_input".to_owned(),
                message: "Correct the request and try again.".to_owned(),
                effect: "none".to_owned(),
                details: json!({ "argument": argument, "reason": reason.into() }),
            },
        }
    }

    fn transport(operation: Operation) -> Self {
        Self {
            exit_code: 6,
            payload: ErrorPayload {
                code: "transport_failed".to_owned(),
                message: "Check the service connection and try again.".to_owned(),
                effect: "none".to_owned(),
                details: json!({ "operation": operation.wire() }),
            },
        }
    }

    fn incompatible(supported: Vec<u16>) -> Self {
        Self {
            exit_code: 6,
            payload: ErrorPayload {
                code: "incompatible_server".to_owned(),
                message: "Use a compatible Slipstream client and service.".to_owned(),
                effect: "none".to_owned(),
                details: json!({
                    "requestedContractVersion": CLI_CONTRACT_VERSION,
                    "supportedContractVersions": supported,
                }),
            },
        }
    }
}

#[derive(Debug, Deserialize)]
struct ErrorResponse {
    error: ErrorPayload,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Capabilities {
    server_version: String,
    supported_cli_contract_versions: Vec<u16>,
    limits: CapabilityLimits,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CapabilityLimits {
    list_page_maximum: u64,
    mutation_photo_ids_maximum: u64,
    album_reorder_members_maximum: u64,
    retained_query_ids_maximum: u64,
    retained_query_idle_seconds: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusData {
    #[serde(skip_deserializing, default = "client_version")]
    client_version: String,
    server_version: String,
    cli_contract_version: u16,
    published: bool,
    publication: Option<String>,
    photo_count: u64,
    scan: ScanStatus,
}

fn client_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanStatus {
    state: ScanState,
    publication: Option<String>,
    completed: Option<u64>,
    total: Option<u64>,
    last_recovery: Option<RecoveryCounts>,
    fingerprints: Option<FingerprintCounts>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum ScanState {
    Initializing,
    Discovering,
    Inspecting,
    Recovering,
    Applying,
    Idle,
    Failed,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecoveryCounts {
    relocated_photos: u64,
    fingerprinted_originals: u64,
    unavailable_photos: u64,
}

#[derive(Debug, Deserialize, Serialize)]
struct FingerprintCounts {
    enrolled: u64,
    pending: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct FolderListData {
    items: Vec<FolderItem>,
    total: u64,
    next_cursor: Option<String>,
    evaluated_at: String,
    expires_at: Option<String>,
    publication: String,
    parent: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct FolderItem {
    location: String,
    name: String,
    photo_count: u64,
    has_descendant_folders: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ListData<T> {
    items: Vec<T>,
    total: u64,
    next_cursor: Option<String>,
    evaluated_at: String,
    expires_at: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum AlbumListItem {
    Missing(MissingItem),
    Present(AlbumSummary),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct MissingItem {
    id: String,
    state: MissingState,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum MissingState {
    Missing,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct AlbumSummary {
    id: String,
    name: String,
    photo_count: u64,
    has_saved_position: bool,
    album_version: String,
    #[serde(rename = "webPath")]
    web_path: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum PhotoListItem {
    Missing(MissingItem),
    Present(PhotoItem),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PhotoItem {
    id: String,
    filename: String,
    original_kind: OriginalKind,
    original_available: bool,
    selection_state: SelectionState,
    rating: u8,
    decision_version: String,
    capture_time: Option<String>,
    preview: PreviewFacts,
    #[serde(rename = "webPath")]
    web_path: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum OriginalKind {
    Raw,
    Jpeg,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum SelectionState {
    Undecided,
    Selected,
    Rejected,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum PreviewState {
    InspectionPending,
    Ready,
    Failed,
    Unavailable,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewFacts {
    state: PreviewState,
    source: Option<PreviewSource>,
    source_revision: Option<String>,
    width: Option<u64>,
    height: Option<u64>,
    detail_limited: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum PreviewSource {
    JpegOriginal,
    RawEmbeddedJpeg,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PhotoGet {
    id: String,
    filename: String,
    original_kind: OriginalKind,
    original_available: bool,
    selection_state: SelectionState,
    rating: u8,
    decision_version: String,
    capture_time: Option<String>,
    preview: PreviewFacts,
    #[serde(rename = "webPath")]
    web_path: String,
    metadata: PhotoMetadata,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct PhotoMetadata {
    state: MetadataState,
    capture_time: Option<String>,
    aperture: Option<String>,
    shutter_speed: Option<String>,
    focal_length: Option<String>,
    iso: Option<u64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum MetadataState {
    Pending,
    Known,
    Missing,
    Invalid,
    Failed,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PhotoQueryRequest<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<PhotoSource<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selection: Option<SelectionArg>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rating_minimum: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rating_maximum: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<OriginalKindArg>,
    #[serde(skip_serializing_if = "Option::is_none")]
    available: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    captured_from: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    captured_before: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    order: Option<OrderArg>,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<u8>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum PhotoSource<'a> {
    Album {
        #[serde(rename = "albumId")]
        album_id: &'a str,
    },
    Folder {
        location: &'a str,
    },
}

struct ServiceClient {
    origin: Url,
    client: Client,
}

impl fmt::Debug for ServiceClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ServiceClient")
            .field("origin", &self.origin)
            .finish()
    }
}

impl ServiceClient {
    fn new(origin: Url) -> Result<Self, CommandFailure> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| CommandFailure::transport(Operation::Status))?;
        Ok(Self { origin, client })
    }

    fn endpoint(&self, segments: &[&str]) -> Url {
        let mut url = self.origin.clone();
        {
            let mut path = url
                .path_segments_mut()
                .expect("HTTP origins can hold paths");
            path.clear();
            for segment in segments {
                path.push(segment);
            }
        }
        url
    }

    async fn capabilities(&self, operation: Operation) -> Result<Capabilities, CommandFailure> {
        let url = self.endpoint(&["api", "capabilities"]);
        let response = self
            .client
            .get(url)
            .header(CONTRACT_HEADER, CLI_CONTRACT_VERSION)
            .send()
            .await
            .map_err(|_| CommandFailure::transport(operation))?;
        let status = response.status();
        let bytes = response_bytes(response, operation).await?;
        if status != StatusCode::OK {
            if let Ok(response) = serde_json::from_slice::<ErrorResponse>(&bytes)
                && response.error.code == "incompatible_server"
            {
                return Err(validate_route_error(response.error, operation)?);
            }
            return Err(CommandFailure::incompatible(Vec::new()));
        }
        let capabilities: Capabilities =
            serde_json::from_slice(&bytes).map_err(|_| CommandFailure::incompatible(Vec::new()))?;
        let valid_limits = capabilities.limits.list_page_maximum == 60
            && capabilities.limits.mutation_photo_ids_maximum > 0
            && capabilities.limits.album_reorder_members_maximum > 0
            && capabilities.limits.retained_query_ids_maximum > 0
            && capabilities.limits.retained_query_idle_seconds > 0;
        if capabilities.server_version.is_empty() || !valid_limits {
            return Err(CommandFailure::incompatible(
                capabilities.supported_cli_contract_versions,
            ));
        }
        if !capabilities
            .supported_cli_contract_versions
            .contains(&CLI_CONTRACT_VERSION)
        {
            return Err(CommandFailure::incompatible(
                capabilities.supported_cli_contract_versions,
            ));
        }
        Ok(capabilities)
    }

    async fn json<T: DeserializeOwned>(
        &self,
        operation: Operation,
        method: Method,
        url: Url,
        body: Option<Value>,
    ) -> Result<T, CommandFailure> {
        let mut request = self
            .client
            .request(method, url)
            .header(CONTRACT_HEADER, CLI_CONTRACT_VERSION);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|_| CommandFailure::transport(operation))?;
        let status = response.status();
        if status.is_redirection() {
            return Err(CommandFailure::transport(operation));
        }
        let bytes = response_bytes(response, operation).await?;
        if status != StatusCode::OK {
            let error = serde_json::from_slice::<ErrorResponse>(&bytes)
                .map_err(|_| CommandFailure::transport(operation))?
                .error;
            return Err(validate_route_error(error, operation)?);
        }
        serde_json::from_slice(&bytes).map_err(|_| CommandFailure::transport(operation))
    }
}

async fn response_bytes(
    mut response: reqwest::Response,
    operation: Operation,
) -> Result<Vec<u8>, CommandFailure> {
    if response
        .content_length()
        .is_some_and(|length| length > MAXIMUM_JSON_RESPONSE_BYTES as u64)
    {
        return Err(CommandFailure::transport(operation));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| CommandFailure::transport(operation))?
    {
        if bytes.len().saturating_add(chunk.len()) > MAXIMUM_JSON_RESPONSE_BYTES {
            return Err(CommandFailure::transport(operation));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn validate_route_error(
    error: ErrorPayload,
    operation: Operation,
) -> Result<CommandFailure, CommandFailure> {
    if error.effect != "none" || error.message.is_empty() {
        return Err(CommandFailure::transport(operation));
    }
    let details = error
        .details
        .as_object()
        .ok_or_else(|| CommandFailure::transport(operation))?;
    let string = |name: &str| {
        details
            .get(name)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
    };
    let required_keys = |expected: &[&str]| expected.iter().all(|key| details.contains_key(*key));
    let valid = match error.code.as_str() {
        "invalid_input" => {
            required_keys(&["argument", "reason"])
                && string("argument").is_some()
                && string("reason").is_some()
        }
        "not_found" => {
            required_keys(&["resource", "reference"])
                && string("resource")
                    .is_some_and(|value| matches!(value, "photo" | "album" | "folder"))
                && string("reference").is_some()
        }
        "cursor_expired" => {
            required_keys(&["cursorKind", "reason"])
                && string("cursorKind")
                    .is_some_and(|value| matches!(value, "folder" | "album" | "photo"))
                && string("reason").is_some_and(|value| {
                    matches!(
                        value,
                        "publication_replaced" | "process_restarted" | "idle_or_evicted"
                    )
                })
        }
        "incompatible_server" => {
            required_keys(&["requestedContractVersion", "supportedContractVersions"])
                && details
                    .get("requestedContractVersion")
                    .and_then(Value::as_u64)
                    .is_some()
                && details
                    .get("supportedContractVersions")
                    .and_then(Value::as_array)
                    .is_some_and(|versions| {
                        versions.iter().all(|version| version.as_u64().is_some())
                    })
        }
        "library_unavailable" => {
            required_keys(&["scan"])
                && serde_json::from_value::<ScanStatus>(details["scan"].clone()).is_ok()
        }
        "server_busy" => {
            required_keys(&["operation", "retryAfterSeconds"])
                && string("operation") == Some(operation.wire())
                && (details["retryAfterSeconds"].is_null()
                    || details["retryAfterSeconds"].as_u64().is_some())
        }
        "storage_failed" => {
            required_keys(&["operation"]) && string("operation") == Some(operation.wire())
        }
        _ => return Err(CommandFailure::transport(operation)),
    };
    if !valid {
        return Err(CommandFailure::transport(operation));
    }
    let exit_code = if error.code == "invalid_input" {
        2
    } else if error.code == "not_found" {
        3
    } else {
        6
    };
    Ok(CommandFailure {
        exit_code,
        payload: error,
    })
}

fn service_origin(cli: &Cli, environment: Option<&str>) -> Result<Url, CommandFailure> {
    let value = cli
        .server
        .as_deref()
        .or(environment)
        .unwrap_or(DEFAULT_SERVER);
    if value.is_empty() {
        return Err(CommandFailure::invalid(
            "server",
            "The service URL must not be empty.",
        ));
    }
    let url = Url::parse(value).map_err(|_| {
        CommandFailure::invalid("server", "The service URL must be an HTTP or HTTPS origin.")
    })?;
    let has_userinfo = value
        .split_once("://")
        .map(|(_, rest)| {
            rest.split(['/', '?', '#'])
                .next()
                .unwrap_or(rest)
                .contains('@')
        })
        .unwrap_or(false);
    let valid = matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && !has_userinfo
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none();
    if !valid {
        return Err(CommandFailure::invalid(
            "server",
            "The service URL must be an HTTP or HTTPS origin without credentials, path, query, or fragment.",
        ));
    }
    Ok(url)
}

fn web_url(origin: &Url, path: &str) -> Result<String, ()> {
    if !path.starts_with("/?") || path.contains('#') {
        return Err(());
    }
    let resolved = origin.join(path).map_err(|_| ())?;
    if resolved.scheme() != origin.scheme()
        || resolved.host_str() != origin.host_str()
        || resolved.port_or_known_default() != origin.port_or_known_default()
    {
        return Err(());
    }
    Ok(resolved.to_string())
}

fn validate_nonempty(value: &str) -> Result<(), ()> {
    (!value.is_empty()).then_some(()).ok_or(())
}

fn valid_camera_time(value: &str) -> bool {
    if !value.is_ascii() || value.len() < 19 || local_time(&value[..19]).is_err() {
        return false;
    }
    value.len() == 19
        || value
            .strip_prefix(&value[..19])
            .and_then(|fraction| fraction.strip_prefix('.'))
            .is_some_and(|fraction| {
                (1..=9).contains(&fraction.len())
                    && fraction.bytes().all(|byte| byte.is_ascii_digit())
            })
}

fn valid_utc_time(value: &str) -> bool {
    value.strip_suffix('Z').is_some_and(valid_camera_time)
}

fn missing_value(missing: MissingItem) -> Result<Value, ()> {
    validate_nonempty(&missing.id)?;
    serde_json::to_value(missing).map_err(|_| ())
}

fn album_value(album: AlbumSummary, origin: &Url) -> Result<Value, ()> {
    validate_nonempty(&album.id)?;
    validate_nonempty(&album.album_version)?;
    Ok(json!({
        "id": album.id,
        "name": album.name,
        "photoCount": album.photo_count,
        "hasSavedPosition": album.has_saved_position,
        "albumVersion": album.album_version,
        "webUrl": web_url(origin, &album.web_path)?,
    }))
}

fn preview_valid(preview: &PreviewFacts) -> bool {
    match preview.state {
        PreviewState::Ready => {
            preview.source.is_some()
                && preview
                    .source_revision
                    .as_deref()
                    .is_some_and(|value| !value.is_empty())
                && preview.width.is_some_and(|value| value > 0)
                && preview.height.is_some_and(|value| value > 0)
                && preview.detail_limited.is_some()
        }
        _ => {
            preview.source.is_none()
                && preview.source_revision.is_none()
                && preview.width.is_none()
                && preview.height.is_none()
                && preview.detail_limited.is_none()
        }
    }
}

fn photo_value(photo: PhotoItem, origin: &Url) -> Result<Value, ()> {
    validate_nonempty(&photo.id)?;
    validate_nonempty(&photo.decision_version)?;
    if photo.rating > 5
        || photo
            .capture_time
            .as_deref()
            .is_some_and(|value| !valid_camera_time(value))
        || !preview_valid(&photo.preview)
    {
        return Err(());
    }
    let mut value = serde_json::to_value(&photo).map_err(|_| ())?;
    let object = value.as_object_mut().ok_or(())?;
    object.remove("webPath");
    object.insert(
        "webUrl".to_owned(),
        Value::String(web_url(origin, &photo.web_path)?),
    );
    Ok(value)
}

fn list_expiry_valid<T>(list: &ListData<T>, page_limit: usize) -> bool {
    list.items.len() <= page_limit
        && list.total >= list.items.len() as u64
        && valid_utc_time(&list.evaluated_at)
        && list.next_cursor.is_some() == list.expires_at.is_some()
        && list.expires_at.as_deref().is_none_or(valid_utc_time)
}

async fn execute(cli: &Cli, environment: Option<&str>) -> Result<Value, CommandFailure> {
    let operation = match &cli.command {
        Command::Status => Operation::Status,
        Command::Folders { .. } => Operation::FoldersList,
        Command::Albums {
            command: AlbumCommand::List(_),
        } => Operation::AlbumsList,
        Command::Albums {
            command: AlbumCommand::Get { .. },
        } => Operation::AlbumsGet,
        Command::Photos {
            command: PhotoCommand::List(_),
        } => Operation::PhotosList,
        Command::Photos {
            command: PhotoCommand::Get { .. },
        } => Operation::PhotosGet,
    };
    validate_command(&cli.command)?;
    let client = ServiceClient::new(service_origin(cli, environment)?)?;
    client.capabilities(operation).await?;

    match &cli.command {
        Command::Status => {
            let data: StatusData = client
                .json(
                    operation,
                    Method::GET,
                    client.endpoint(&["api", "status"]),
                    None,
                )
                .await?;
            if data.server_version.is_empty()
                || data.cli_contract_version != CLI_CONTRACT_VERSION
                || data.publication.as_deref().is_some_and(str::is_empty)
            {
                return Err(CommandFailure::transport(operation));
            }
            serde_json::to_value(data).map_err(|_| CommandFailure::transport(operation))
        }
        Command::Folders {
            command: FolderCommand::List(args),
        } => {
            let mut url = client.endpoint(&["api", "file-locations"]);
            if let Some(cursor) = &args.cursor {
                url.query_pairs_mut().append_pair("cursor", cursor);
            } else {
                if let Some(parent) = &args.parent {
                    url.query_pairs_mut().append_pair("parent", parent);
                }
                if let Some(limit) = args.limit {
                    url.query_pairs_mut()
                        .append_pair("limit", &limit.to_string());
                }
            }
            let data: FolderListData = client.json(operation, Method::GET, url, None).await?;
            let page_limit = if args.cursor.is_some() {
                MAXIMUM_LIST_PAGE
            } else {
                usize::from(args.limit.unwrap_or(DEFAULT_LIST_PAGE as u8))
            };
            if data.expires_at.is_some()
                || data.items.len() > page_limit
                || !valid_utc_time(&data.evaluated_at)
                || data.total < data.items.len() as u64
                || data.publication.is_empty()
                || data.next_cursor.as_deref().is_some_and(str::is_empty)
            {
                return Err(CommandFailure::transport(operation));
            }
            serde_json::to_value(data).map_err(|_| CommandFailure::transport(operation))
        }
        Command::Albums {
            command: AlbumCommand::List(args),
        } => {
            let mut url = client.endpoint(&["api", "album-summaries"]);
            if let Some(cursor) = &args.cursor {
                url.query_pairs_mut().append_pair("cursor", cursor);
            } else {
                if let Some(name) = &args.name {
                    url.query_pairs_mut().append_pair("name", name);
                }
                if let Some(photo) = &args.photo {
                    url.query_pairs_mut().append_pair("photoId", photo);
                }
                if let Some(limit) = args.limit {
                    url.query_pairs_mut()
                        .append_pair("limit", &limit.to_string());
                }
            }
            let data: ListData<AlbumListItem> =
                client.json(operation, Method::GET, url, None).await?;
            let page_limit = if args.cursor.is_some() {
                MAXIMUM_LIST_PAGE
            } else {
                usize::from(args.limit.unwrap_or(DEFAULT_LIST_PAGE as u8))
            };
            if !list_expiry_valid(&data, page_limit)
                || data.next_cursor.as_deref().is_some_and(str::is_empty)
            {
                return Err(CommandFailure::transport(operation));
            }
            let items = data
                .items
                .into_iter()
                .map(|item| match item {
                    AlbumListItem::Missing(missing) => missing_value(missing),
                    AlbumListItem::Present(album) => album_value(album, &client.origin),
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| CommandFailure::transport(operation))?;
            Ok(json!({
                "items": items,
                "total": data.total,
                "nextCursor": data.next_cursor,
                "evaluatedAt": data.evaluated_at,
                "expiresAt": data.expires_at,
            }))
        }
        Command::Albums {
            command: AlbumCommand::Get { album_id },
        } => {
            let data: AlbumSummary = client
                .json(
                    operation,
                    Method::GET,
                    client.endpoint(&["api", "albums", album_id]),
                    None,
                )
                .await?;
            album_value(data, &client.origin).map_err(|_| CommandFailure::transport(operation))
        }
        Command::Photos {
            command: PhotoCommand::List(args),
        } => {
            let data: ListData<PhotoListItem> = if let Some(cursor) = &args.cursor {
                client
                    .json(
                        operation,
                        Method::GET,
                        client.endpoint(&["api", "photo-queries", cursor]),
                        None,
                    )
                    .await?
            } else {
                let source = args
                    .album
                    .as_deref()
                    .map(|album_id| PhotoSource::Album { album_id })
                    .or_else(|| {
                        args.folder
                            .as_deref()
                            .map(|location| PhotoSource::Folder { location })
                    });
                let request = PhotoQueryRequest {
                    source,
                    selection: args.selection,
                    rating_minimum: args.rating_min,
                    rating_maximum: args.rating_max,
                    kind: args.kind,
                    available: args.available,
                    captured_from: args.captured_from.as_deref(),
                    captured_before: args.captured_before.as_deref(),
                    order: args.order,
                    limit: args.limit,
                };
                let body = serde_json::to_value(request)
                    .map_err(|_| CommandFailure::transport(operation))?;
                client
                    .json(
                        operation,
                        Method::POST,
                        client.endpoint(&["api", "photo-queries"]),
                        Some(body),
                    )
                    .await?
            };
            let page_limit = if args.cursor.is_some() {
                MAXIMUM_LIST_PAGE
            } else {
                usize::from(args.limit.unwrap_or(DEFAULT_LIST_PAGE as u8))
            };
            if !list_expiry_valid(&data, page_limit)
                || data.next_cursor.as_deref().is_some_and(str::is_empty)
            {
                return Err(CommandFailure::transport(operation));
            }
            let items = data
                .items
                .into_iter()
                .map(|item| match item {
                    PhotoListItem::Missing(missing) => missing_value(missing),
                    PhotoListItem::Present(photo) => photo_value(photo, &client.origin),
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| CommandFailure::transport(operation))?;
            Ok(json!({
                "items": items,
                "total": data.total,
                "nextCursor": data.next_cursor,
                "evaluatedAt": data.evaluated_at,
                "expiresAt": data.expires_at,
            }))
        }
        Command::Photos {
            command: PhotoCommand::Get { photo_id },
        } => {
            let photo: PhotoGet = client
                .json(
                    operation,
                    Method::GET,
                    client.endpoint(&["api", "photos", photo_id]),
                    None,
                )
                .await?;
            let PhotoGet {
                id,
                filename,
                original_kind,
                original_available,
                selection_state,
                rating,
                decision_version,
                capture_time,
                preview,
                web_path,
                metadata,
            } = photo;
            let metadata_values_absent = metadata.capture_time.is_none()
                && metadata.aperture.is_none()
                && metadata.shutter_speed.is_none()
                && metadata.focal_length.is_none()
                && metadata.iso.is_none();
            if metadata
                .capture_time
                .as_deref()
                .is_some_and(|value| !valid_camera_time(value))
                || (!matches!(metadata.state, MetadataState::Known) && !metadata_values_absent)
            {
                return Err(CommandFailure::transport(operation));
            }
            let item = PhotoItem {
                id,
                filename,
                original_kind,
                original_available,
                selection_state,
                rating,
                decision_version,
                capture_time,
                preview,
                web_path,
            };
            let mut value = photo_value(item, &client.origin)
                .map_err(|_| CommandFailure::transport(operation))?;
            value
                .as_object_mut()
                .ok_or_else(|| CommandFailure::transport(operation))?
                .insert(
                    "metadata".to_owned(),
                    serde_json::to_value(metadata)
                        .map_err(|_| CommandFailure::transport(operation))?,
                );
            Ok(value)
        }
    }
}

fn validate_command(command: &Command) -> Result<(), CommandFailure> {
    match command {
        Command::Photos {
            command: PhotoCommand::List(args),
        } if args.cursor.is_none() => {
            if args
                .rating_min
                .zip(args.rating_max)
                .is_some_and(|(minimum, maximum)| minimum > maximum)
            {
                return Err(CommandFailure::invalid(
                    "rating-min",
                    "The minimum Rating must not exceed the maximum Rating.",
                ));
            }
            if args
                .captured_from
                .as_ref()
                .zip(args.captured_before.as_ref())
                .is_some_and(|(from, before)| from >= before)
            {
                return Err(CommandFailure::invalid(
                    "captured-from",
                    "The lower Capture Time bound must precede the upper bound.",
                ));
            }
            if args.order == Some(OrderArg::AlbumOrder) && args.album.is_none() {
                return Err(CommandFailure::invalid(
                    "order",
                    "album-order requires an Album source.",
                ));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

pub async fn invoke(cli: Cli, environment: Option<&str>) -> InvocationResult {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(cli.timeout);
    invoke_until(cli, environment, deadline).await
}

pub async fn invoke_until(
    cli: Cli,
    environment: Option<&str>,
    deadline: tokio::time::Instant,
) -> InvocationResult {
    let output = cli.output;
    let operation = match &cli.command {
        Command::Status => Operation::Status,
        Command::Folders { .. } => Operation::FoldersList,
        Command::Albums {
            command: AlbumCommand::List(_),
        } => Operation::AlbumsList,
        Command::Albums { .. } => Operation::AlbumsGet,
        Command::Photos {
            command: PhotoCommand::List(_),
        } => Operation::PhotosList,
        Command::Photos { .. } => Operation::PhotosGet,
    };
    let command = tokio::time::timeout_at(deadline, execute(&cli, environment));
    tokio::pin!(command);
    let (exit_code, envelope) = tokio::select! {
        result = &mut command => match result {
            Ok(Ok(data)) => (0, Envelope::success(data)),
            Ok(Err(failure)) => (failure.exit_code, Envelope::error(failure.payload)),
            Err(_) => {
                let failure = CommandFailure::transport(operation);
                (failure.exit_code, Envelope::error(failure.payload))
            }
        },
        _ = tokio::signal::ctrl_c() => {
            let mut failure = CommandFailure::transport(operation);
            failure.payload.message = "The command was interrupted. Inspect status before continuing.".to_owned();
            (130, Envelope::error(failure.payload))
        }
    };
    render_invocation(output, exit_code, &envelope)
}

pub fn invalid_invocation(output: OutputFormat, reason: impl Into<String>) -> InvocationResult {
    let failure = CommandFailure::invalid("arguments", reason);
    render_invocation(output, failure.exit_code, &Envelope::error(failure.payload))
}

fn render_invocation(output: OutputFormat, exit_code: u8, envelope: &Envelope) -> InvocationResult {
    let stdout = match output {
        OutputFormat::Json => format!(
            "{}\n",
            serde_json::to_string(envelope).expect("envelope serialization is infallible")
        ),
        OutputFormat::Text => render_text(envelope),
    };
    InvocationResult { exit_code, stdout }
}

fn render_text(envelope: &Envelope) -> String {
    match (&envelope.data, &envelope.error) {
        (Some(data), None) => format!(
            "Success\n{}\n",
            serde_json::to_string_pretty(data).expect("result serialization is infallible")
        ),
        (_, Some(error)) => format!(
            "Error: {}\n{}\n{}\n",
            error.code,
            error.message,
            serde_json::to_string_pretty(&error.details)
                .expect("error serialization is infallible")
        ),
        _ => "Error: invalid result\n".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn parser_rejects_duplicates_abbreviations_and_cursor_combinations() {
        assert!(Cli::try_parse_from(["slipstream", "--time", "3", "status"]).is_err());
        assert!(
            Cli::try_parse_from(["slipstream", "--timeout", "3", "--timeout", "4", "status"])
                .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "slipstream",
                "albums",
                "list",
                "--cursor",
                "x",
                "--limit",
                "2"
            ])
            .is_err()
        );
        assert!(Cli::try_parse_from(["slipstream", "albums", "list", "--cursor", ""]).is_err());
        assert!(Cli::try_parse_from(["slipstream", "status", "--output", "text"]).is_err());
    }

    #[test]
    fn parser_error_preferences_recover_valid_global_options() {
        let preferences = |arguments: &[&str]| {
            parse_error_preferences(
                &arguments
                    .iter()
                    .map(OsString::from)
                    .collect::<Vec<OsString>>(),
            )
        };
        assert_eq!(
            preferences(&[
                "slipstream",
                "--output=text",
                "--timeout=7",
                "photos",
                "list",
                "--rating-min",
                "7",
            ]),
            ParseErrorPreferences {
                output: OutputFormat::Text,
                timeout_seconds: 7,
            }
        );
        assert_eq!(
            preferences(&[
                "slipstream",
                "--output",
                "text",
                "--timeout",
                "9",
                "photos",
                "list",
                "--rating-min",
                "7",
            ]),
            ParseErrorPreferences {
                output: OutputFormat::Text,
                timeout_seconds: 9,
            }
        );
        assert_eq!(
            preferences(&[
                "slipstream",
                "--output=invalid",
                "--timeout=invalid",
                "status",
            ]),
            ParseErrorPreferences {
                output: OutputFormat::Json,
                timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
            }
        );
    }

    #[test]
    fn parser_validates_exact_values_and_local_times() {
        assert!(
            Cli::try_parse_from(["slipstream", "photos", "list", "--available", "yes"]).is_err()
        );
        assert!(
            Cli::try_parse_from([
                "slipstream",
                "photos",
                "list",
                "--captured-from",
                "2024-02-29T23:59:59"
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "slipstream",
                "photos",
                "list",
                "--captured-from",
                "2023-02-29T00:00:00"
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "slipstream",
                "photos",
                "list",
                "--captured-from",
                "2024-01-01T00:00:00Z"
            ])
            .is_err()
        );
    }

    #[test]
    fn list_validation_enforces_requested_and_global_page_bounds() {
        let list = ListData {
            items: vec![(); 2],
            total: 2,
            next_cursor: None,
            evaluated_at: "2026-01-01T00:00:00Z".to_owned(),
            expires_at: None,
        };
        assert!(!list_expiry_valid(&list, 1));
        assert!(list_expiry_valid(&list, 2));
        let continuation = ListData {
            items: vec![(); MAXIMUM_LIST_PAGE + 1],
            total: (MAXIMUM_LIST_PAGE + 1) as u64,
            next_cursor: Some("cursor".to_owned()),
            evaluated_at: "2026-01-01T00:00:00Z".to_owned(),
            expires_at: Some("2026-01-01T00:15:00Z".to_owned()),
        };
        assert!(!list_expiry_valid(&continuation, MAXIMUM_LIST_PAGE));
    }

    #[test]
    fn connection_requires_a_clean_origin_and_obeys_precedence() {
        let explicit = Cli::try_parse_from([
            "slipstream",
            "--server",
            "https://example.test:8443",
            "status",
        ])
        .unwrap();
        assert_eq!(
            service_origin(&explicit, Some("http://ignored.test"))
                .unwrap()
                .as_str(),
            "https://example.test:8443/"
        );
        let defaulted = Cli::try_parse_from(["slipstream", "status"]).unwrap();
        assert_eq!(
            service_origin(&defaulted, None).unwrap().as_str(),
            DEFAULT_SERVER.to_owned() + "/"
        );
        assert!(service_origin(&defaulted, Some("")).is_err());
        for invalid in [
            "ftp://example.test",
            "http://user@example.test",
            "http://@example.test",
            "http://example.test/path",
            "http://example.test/?x=1",
        ] {
            assert!(
                service_origin(&defaulted, Some(invalid)).is_err(),
                "accepted {invalid}"
            );
        }
    }

    #[test]
    fn envelopes_and_explicit_text_are_deterministic() {
        let envelope = Envelope::success(json!({"value": 1}));
        assert_eq!(
            serde_json::to_string(&envelope).unwrap(),
            r#"{"schemaVersion":1,"status":"ok","data":{"value":1},"error":null}"#
        );
        assert_eq!(render_text(&envelope), "Success\n{\n  \"value\": 1\n}\n");
    }

    #[test]
    fn help_and_version_are_offline_parser_results() {
        assert_eq!(
            Cli::try_parse_from(["slipstream", "--help"])
                .unwrap_err()
                .kind(),
            clap::error::ErrorKind::DisplayHelp
        );
        assert_eq!(
            Cli::try_parse_from(["slipstream", "--version"])
                .unwrap_err()
                .kind(),
            clap::error::ErrorKind::DisplayVersion
        );
        assert_eq!(
            Cli::try_parse_from(["slipstream", "photos", "--help"])
                .unwrap_err()
                .kind(),
            clap::error::ErrorKind::DisplayHelp
        );
    }

    #[test]
    fn semantic_query_validation_precedes_network_access() {
        let reversed = Cli::try_parse_from([
            "slipstream",
            "photos",
            "list",
            "--rating-min",
            "5",
            "--rating-max",
            "4",
        ])
        .unwrap();
        assert!(validate_command(&reversed.command).is_err());
        let album_order =
            Cli::try_parse_from(["slipstream", "photos", "list", "--order", "album-order"])
                .unwrap();
        assert!(validate_command(&album_order.command).is_err());
    }
}
