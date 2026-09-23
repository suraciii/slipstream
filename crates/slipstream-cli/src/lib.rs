use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use reqwest::{Client, Method, StatusCode, header::RETRY_AFTER};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    env,
    ffi::OsString,
    fmt,
    fs::OpenOptions,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};
use url::Url;

const CLI_CONTRACT_VERSION: u16 = 1;
const DEFAULT_TIMEOUT_SECONDS: u64 = 30;
const DEFAULT_LIST_PAGE: usize = 50;
const MAXIMUM_LIST_PAGE: usize = 60;
const CONTRACT_HEADER: &str = "Slipstream-CLI-Contract";
const MAXIMUM_JSON_RESPONSE_BYTES: usize = 1024 * 1024;
const MAXIMUM_INPUT_BYTES: usize = 64 * 1024;
const MAXIMUM_MUTATION_PHOTO_IDS: usize = 100;

#[derive(Debug, Parser)]
#[command(
    name = "slipstream",
    version,
    about = "Query a Slipstream Photo Library",
    disable_help_subcommand = true,
    infer_long_args = false
)]
pub struct Cli {
    /// HTTPS Slipstream service origin. Overrides SLIPSTREAM_SERVER_URL.
    #[arg(long, value_name = "URL")]
    pub server: Option<String>,

    /// Private file containing the instance Access Token. Overrides SLIPSTREAM_ACCESS_TOKEN_FILE.
    #[arg(long, value_name = "FILE")]
    pub token_file: Option<PathBuf>,

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
    /// Create an empty Album with a name unique in the Library.
    Create {
        /// Album name; at most 120 characters, not only whitespace.
        #[arg(long, value_name = "NAME", value_parser = album_name)]
        name: String,
    },
    /// Rename one Album against its observed Album version.
    Rename {
        #[arg(value_name = "ALBUM_ID", value_parser = nonempty)]
        album_id: String,
        /// New Album name; at most 120 characters, not only whitespace.
        #[arg(long, value_name = "NAME", value_parser = album_name)]
        name: String,
        /// Album version observed by a prior read.
        #[arg(long, value_name = "VERSION", value_parser = nonempty)]
        if_version: String,
    },
    /// Delete one Album. Original Files are never changed.
    Delete {
        #[arg(value_name = "ALBUM_ID", value_parser = nonempty)]
        album_id: String,
        /// Album version observed by a prior read.
        #[arg(long, value_name = "VERSION", value_parser = nonempty)]
        if_version: String,
    },
    /// Append new Photos in submitted order against the observed version.
    Add(AlbumMembershipArgs),
    /// Remove Photos and report the resulting saved position.
    Remove(AlbumMembershipArgs),
    /// Replace the complete membership order of one Album.
    Reorder(AlbumMembershipArgs),
}

#[derive(Debug, Args)]
pub struct AlbumMembershipArgs {
    #[arg(value_name = "ALBUM_ID", value_parser = nonempty)]
    album_id: String,
    /// UTF-8 JSON file holding one ordered `photoIds` array of at most 100
    /// distinct Photo IDs; `-` reads the document from stdin.
    #[arg(long, value_name = "FILE", value_parser = nonempty)]
    input: String,
    /// Album version observed by a prior read.
    #[arg(long, value_name = "VERSION", value_parser = nonempty)]
    if_version: String,
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

fn album_name(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err("must not be empty or only whitespace".to_owned())
    } else if trimmed.chars().count() > 120 {
        Err("must be at most 120 characters".to_owned())
    } else {
        Ok(value.to_owned())
    }
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
    AlbumsCreate,
    AlbumsRename,
    AlbumsDelete,
    AlbumsAdd,
    AlbumsRemove,
    AlbumsReorder,
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
            Self::AlbumsCreate => "albums-create",
            Self::AlbumsRename => "albums-rename",
            Self::AlbumsDelete => "albums-delete",
            Self::AlbumsAdd => "albums-add",
            Self::AlbumsRemove => "albums-remove",
            Self::AlbumsReorder => "albums-reorder",
        }
    }
}

fn command_operation(command: &Command) -> Operation {
    match command {
        Command::Status => Operation::Status,
        Command::Folders { .. } => Operation::FoldersList,
        Command::Albums { command } => match command {
            AlbumCommand::List(_) => Operation::AlbumsList,
            AlbumCommand::Get { .. } => Operation::AlbumsGet,
            AlbumCommand::Create { .. } => Operation::AlbumsCreate,
            AlbumCommand::Rename { .. } => Operation::AlbumsRename,
            AlbumCommand::Delete { .. } => Operation::AlbumsDelete,
            AlbumCommand::Add(_) => Operation::AlbumsAdd,
            AlbumCommand::Remove(_) => Operation::AlbumsRemove,
            AlbumCommand::Reorder(_) => Operation::AlbumsReorder,
        },
        Command::Photos { command } => match command {
            PhotoCommand::List(_) => Operation::PhotosList,
            PhotoCommand::Get { .. } => Operation::PhotosGet,
        },
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MembershipKind {
    Add,
    Remove,
    Reorder,
}

impl MembershipKind {
    fn operation(self) -> Operation {
        match self {
            Self::Add => Operation::AlbumsAdd,
            Self::Remove => Operation::AlbumsRemove,
            Self::Reorder => Operation::AlbumsReorder,
        }
    }

    fn wire(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Remove => "remove",
            Self::Reorder => "reorder",
        }
    }

    fn limit_name(self) -> &'static str {
        match self {
            Self::Reorder => "albumReorderMembersMaximum",
            Self::Add | Self::Remove => "mutationPhotoIdsMaximum",
        }
    }
}

/// The submitted target of one Album mutation, kept for any unknown-outcome
/// report after the request may have been admitted.
#[derive(Clone, Debug)]
struct MutationIdentity {
    operation: Operation,
    photo_ids: Vec<String>,
    album_id: Option<String>,
    album_name: Option<String>,
}

/// Records the point where a mutation request was handed to the transport.
/// Any later timeout, interruption, or unusable response is an unknown
/// outcome instead of a claimed refusal.
#[derive(Debug, Default)]
struct AdmissionState {
    identity: std::sync::Mutex<Option<MutationIdentity>>,
}

impl AdmissionState {
    fn admit(&self, identity: MutationIdentity) {
        *self.identity.lock().expect("admission state is lockable") = Some(identity);
    }

    fn admitted(&self) -> Option<MutationIdentity> {
        self.identity
            .lock()
            .expect("admission state is lockable")
            .clone()
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

    fn limit_exceeded(limit_name: &str, limit: usize, actual: usize) -> Self {
        Self {
            exit_code: 2,
            payload: ErrorPayload {
                code: "limit_exceeded".to_owned(),
                message: "Reduce the request and try again.".to_owned(),
                effect: "none".to_owned(),
                details: json!({ "limitName": limit_name, "limit": limit, "actual": actual }),
            },
        }
    }

    fn local_input(path: Option<&str>) -> Self {
        Self {
            exit_code: 6,
            payload: ErrorPayload {
                code: "local_io_failed".to_owned(),
                message: "Check the local input file and try again.".to_owned(),
                effect: "none".to_owned(),
                details: json!({
                    "operation": "read-input",
                    "path": path,
                    "fileCommitted": false,
                }),
            },
        }
    }

    fn local_credential(path: Option<&str>) -> Self {
        Self {
            exit_code: 6,
            payload: ErrorPayload {
                code: "local_io_failed".to_owned(),
                message: "Check the local credential file and try again.".to_owned(),
                effect: "none".to_owned(),
                details: json!({
                    "operation": "read-credential",
                    "path": path,
                    "fileCommitted": false,
                }),
            },
        }
    }

    fn authentication_required(operation: Operation) -> Self {
        Self {
            exit_code: 6,
            payload: ErrorPayload {
                code: "authentication_required".to_owned(),
                message: "Provide a valid Access Token and try again.".to_owned(),
                effect: "none".to_owned(),
                details: json!({ "operation": operation.wire() }),
            },
        }
    }

    fn access_denied(operation: Operation) -> Self {
        Self {
            exit_code: 6,
            payload: ErrorPayload {
                code: "access_denied".to_owned(),
                message: "The service denied access to this operation.".to_owned(),
                effect: "none".to_owned(),
                details: json!({ "operation": operation.wire() }),
            },
        }
    }

    fn server_busy(operation: Operation, retry_after_seconds: Option<u64>) -> Self {
        Self {
            exit_code: 6,
            payload: ErrorPayload {
                code: "server_busy".to_owned(),
                message: "The service is temporarily unavailable. Try again later.".to_owned(),
                effect: "none".to_owned(),
                details: json!({
                    "operation": operation.wire(),
                    "retryAfterSeconds": retry_after_seconds,
                }),
            },
        }
    }

    fn unknown(identity: &MutationIdentity) -> Self {
        Self {
            exit_code: 7,
            payload: ErrorPayload {
                code: "outcome_unknown".to_owned(),
                message: "Inspect the current state with a read command before continuing."
                    .to_owned(),
                effect: "unknown".to_owned(),
                details: json!({
                    "operation": identity.operation.wire(),
                    "photoIds": identity.photo_ids,
                    "albumId": identity.album_id,
                    "albumName": identity.album_name,
                }),
            },
        }
    }

    fn interrupted_unknown(identity: &MutationIdentity) -> Self {
        Self {
            exit_code: 130,
            payload: ErrorPayload {
                code: "outcome_unknown".to_owned(),
                message: "The command was interrupted and the outcome is unknown. Inspect the current state before continuing.".to_owned(),
                effect: "unknown".to_owned(),
                details: json!({
                    "operation": identity.operation.wire(),
                    "photoIds": identity.photo_ids,
                    "albumId": identity.album_id,
                    "albumName": identity.album_name,
                }),
            },
        }
    }
}

#[derive(Debug, Deserialize)]
struct ErrorResponse {
    error: ErrorPayload,
}

fn retry_after_seconds(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get(RETRY_AFTER)?
        .to_str()
        .ok()?
        .parse()
        .ok()
}

/// Maps only the access-boundary statuses whose refusal semantics are part of
/// the CLI contract. A 503 is trusted only for the explicit access errors.
fn access_boundary_failure(
    status: StatusCode,
    retry_after: Option<u64>,
    body: &[u8],
    operation: Operation,
) -> Option<CommandFailure> {
    match status {
        StatusCode::UNAUTHORIZED => Some(CommandFailure::authentication_required(operation)),
        StatusCode::FORBIDDEN => Some(CommandFailure::access_denied(operation)),
        StatusCode::TOO_MANY_REQUESTS => Some(CommandFailure::server_busy(operation, retry_after)),
        StatusCode::SERVICE_UNAVAILABLE if is_access_unavailable(body) => {
            Some(CommandFailure::server_busy(operation, None))
        }
        _ => None,
    }
}

fn is_access_unavailable(body: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    let Some(object) = value.as_object() else {
        return false;
    };
    object.len() == 1
        && matches!(
            object.get("error").and_then(Value::as_str),
            Some("access_unavailable" | "access_unconfigured")
        )
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

/// The one accepted `--input` document shape. Deserializing it rejects
/// unknown keys, duplicate keys, trailing content, and non-object documents.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MembershipInput {
    photo_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AlbumCreationWire {
    album: AlbumSummary,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AlbumRenameWire {
    album: AlbumSummary,
    renamed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AlbumDeleteWire {
    album_id: String,
    deleted: bool,
    original_files_changed: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AlbumAddWire {
    album: AlbumSummary,
    added_photo_ids: Vec<String>,
    already_member_photo_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AlbumRemoveWire {
    album: AlbumSummary,
    removed_photo_ids: Vec<String>,
    already_absent_photo_ids: Vec<String>,
    saved_photo_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AlbumReorderWire {
    album: AlbumSummary,
    ordered_photo_ids: Vec<String>,
    reordered: bool,
}

struct ServiceClient {
    origin: Url,
    client: Client,
    token: String,
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
    fn new(origin: Url, token: String) -> Result<Self, CommandFailure> {
        let client = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| CommandFailure::transport(Operation::Status))?;
        Ok(Self {
            origin,
            client,
            token,
        })
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
            .bearer_auth(&self.token)
            .send()
            .await
            .map_err(|_| CommandFailure::transport(operation))?;
        let status = response.status();
        if status.is_redirection() {
            return Err(CommandFailure::transport(operation));
        }
        let retry_after = retry_after_seconds(&response);
        if let Some(failure) = access_boundary_failure(status, retry_after, &[], operation) {
            return Err(failure);
        }
        let bytes = response_bytes(response, operation).await?;
        if let Some(failure) = access_boundary_failure(status, retry_after, &bytes, operation) {
            return Err(failure);
        }
        if status != StatusCode::OK {
            if let Ok(response) = serde_json::from_slice::<ErrorResponse>(&bytes)
                && response.error.code == "incompatible_server"
            {
                return Err(
                    validated_route_failure(response.error, operation, &self.token)
                        .unwrap_or_else(|| CommandFailure::transport(operation)),
                );
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
            .header(CONTRACT_HEADER, CLI_CONTRACT_VERSION)
            .bearer_auth(&self.token);
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
        let retry_after = retry_after_seconds(&response);
        if let Some(failure) = access_boundary_failure(status, retry_after, &[], operation) {
            return Err(failure);
        }
        let bytes = response_bytes(response, operation).await?;
        if let Some(failure) = access_boundary_failure(status, retry_after, &bytes, operation) {
            return Err(failure);
        }
        if status != StatusCode::OK {
            let error = serde_json::from_slice::<ErrorResponse>(&bytes)
                .map_err(|_| CommandFailure::transport(operation))?
                .error;
            return Err(validated_route_failure(error, operation, &self.token)
                .unwrap_or_else(|| CommandFailure::transport(operation)));
        }
        serde_json::from_slice(&bytes).map_err(|_| CommandFailure::transport(operation))
    }

    /// Performs one Album mutation. Everything after the request is handed to
    /// the transport can only be reported as an unknown outcome, because a
    /// dropped, malformed, or untrustworthy response is not evidence that the
    /// write was refused. A connect-phase failure never sent the request.
    async fn mutation<T: DeserializeOwned>(
        &self,
        identity: &MutationIdentity,
        admission: &AdmissionState,
        url: Url,
        body: Value,
    ) -> Result<T, CommandFailure> {
        let operation = identity.operation;
        let request = self
            .client
            .request(Method::POST, url)
            .header(CONTRACT_HEADER, CLI_CONTRACT_VERSION)
            .bearer_auth(&self.token)
            .json(&body);
        admission.admit(identity.clone());
        let response = request.send().await.map_err(|error| {
            if error.is_connect() {
                CommandFailure::transport(operation)
            } else {
                CommandFailure::unknown(identity)
            }
        })?;
        let status = response.status();
        if status.is_redirection() {
            return Err(CommandFailure::unknown(identity));
        }
        let retry_after = retry_after_seconds(&response);
        if let Some(failure) = access_boundary_failure(status, retry_after, &[], operation) {
            return Err(failure);
        }
        let bytes = response_bytes(response, operation)
            .await
            .map_err(|_| CommandFailure::unknown(identity))?;
        if let Some(failure) = access_boundary_failure(status, retry_after, &bytes, operation) {
            return Err(failure);
        }
        if status != StatusCode::OK {
            let error = serde_json::from_slice::<ErrorResponse>(&bytes)
                .map_err(|_| CommandFailure::unknown(identity))?
                .error;
            return Err(validated_route_failure(error, operation, &self.token)
                .unwrap_or_else(|| CommandFailure::unknown(identity)));
        }
        serde_json::from_slice(&bytes).map_err(|_| CommandFailure::unknown(identity))
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

/// Checks one structured service error against the CLI reference shapes.
/// Returns `None` when the payload cannot be trusted as a confirmed refusal.
fn validated_route_failure(
    mut error: ErrorPayload,
    operation: Operation,
    secret: &str,
) -> Option<CommandFailure> {
    if error.effect != "none" || error.message.is_empty() {
        return None;
    }
    let details = error.details.as_object()?;
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
        "conflict" => {
            required_keys(&["resource", "reference", "currentVersion"])
                && string("resource").is_some_and(|value| matches!(value, "photo" | "album"))
                && string("reference").is_some()
                && string("currentVersion").is_some()
        }
        "name_conflict" => {
            required_keys(&["name", "albumId"])
                && string("name").is_some()
                && string("albumId").is_some()
        }
        "limit_exceeded" => {
            required_keys(&["limitName", "limit", "actual"])
                && string("limitName").is_some()
                && details.get("limit").and_then(Value::as_u64).is_some()
                && details.get("actual").and_then(Value::as_u64).is_some()
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
        _ => return None,
    };
    if !valid {
        return None;
    }
    redact_error(&mut error, secret);
    let exit_code = match error.code.as_str() {
        "invalid_input" | "limit_exceeded" => 2,
        "not_found" => 3,
        "conflict" | "name_conflict" => 4,
        _ => 6,
    };
    Some(CommandFailure {
        exit_code,
        payload: error,
    })
}

fn service_origin(cli: &Cli, environment: Option<&str>) -> Result<Url, CommandFailure> {
    let value = cli.server.as_deref().or(environment).ok_or_else(|| {
        CommandFailure::invalid(
            "server",
            "Set --server or SLIPSTREAM_SERVER_URL to an HTTPS service origin.",
        )
    })?;
    if value.is_empty() {
        return Err(CommandFailure::invalid(
            "server",
            "The service URL must not be empty.",
        ));
    }
    let url = Url::parse(value).map_err(|_| {
        CommandFailure::invalid("server", "The service URL must be an HTTPS origin.")
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
    let valid = url.scheme() == "https"
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
            "The service URL must be an HTTPS origin without credentials, path, query, or fragment.",
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

/// Reads the bounded UTF-8 bytes of one `--input` document. The blocking-pool
/// read is not itself bounded; on deadline or interruption the
/// executable-boundary terminal exit publishes the envelope and abandons a
/// still-blocked read.
async fn read_input_bytes(input: &str) -> Result<Vec<u8>, CommandFailure> {
    let owned = input.to_owned();
    tokio::task::spawn_blocking(move || {
        if owned == "-" {
            let mut stdin = std::io::stdin().lock();
            read_bounded(&mut stdin, None)
        } else {
            let mut file = std::fs::File::open(&owned)
                .map_err(|_| CommandFailure::local_input(Some(&owned)))?;
            read_bounded(&mut file, Some(&owned))
        }
    })
    .await
    .map_err(|_| CommandFailure::local_input(None))?
}

fn read_bounded(
    source: &mut dyn std::io::Read,
    path: Option<&str>,
) -> Result<Vec<u8>, CommandFailure> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    loop {
        let count = source
            .read(&mut buffer)
            .map_err(|_| CommandFailure::local_input(path))?;
        if count == 0 {
            return Ok(bytes);
        }
        if bytes.len().saturating_add(count) > MAXIMUM_INPUT_BYTES {
            return Err(CommandFailure::limit_exceeded(
                "inputBytesMaximum",
                MAXIMUM_INPUT_BYTES,
                MAXIMUM_INPUT_BYTES + 1,
            ));
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
}

/// Validates the complete membership document before any write is attempted.
fn parse_membership_ids(
    bytes: Vec<u8>,
    limit_name: &'static str,
) -> Result<Vec<String>, CommandFailure> {
    let document: MembershipInput = serde_json::from_slice(&bytes).map_err(|_| {
        CommandFailure::invalid(
            "input",
            "The input must be one JSON object with only an ordered photoIds array.",
        )
    })?;
    let photo_ids = document.photo_ids;
    if photo_ids.len() > MAXIMUM_MUTATION_PHOTO_IDS {
        return Err(CommandFailure::limit_exceeded(
            limit_name,
            MAXIMUM_MUTATION_PHOTO_IDS,
            photo_ids.len(),
        ));
    }
    if photo_ids.is_empty() {
        return Err(CommandFailure::invalid(
            "input",
            "The photoIds array must contain at least one Photo ID.",
        ));
    }
    if photo_ids.iter().any(|photo_id| photo_id.is_empty()) {
        return Err(CommandFailure::invalid(
            "input",
            "Each Photo ID must be a nonempty string.",
        ));
    }
    let distinct = photo_ids
        .iter()
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    if distinct != photo_ids.len() {
        return Err(CommandFailure::invalid(
            "input",
            "The photoIds array must not repeat a Photo ID.",
        ));
    }
    Ok(photo_ids)
}

async fn read_membership_ids(
    input: &str,
    limit_name: &'static str,
) -> Result<Vec<String>, CommandFailure> {
    parse_membership_ids(read_input_bytes(input).await?, limit_name)
}

/// Checks that two returned ID arrays are an order-preserving partition of the
/// submitted IDs: no omission, no duplication, and request order preserved.
fn request_order_partition(submitted: &[String], first: &[String], second: &[String]) -> bool {
    let mut assignment = std::collections::HashMap::<&str, usize>::new();
    for (index, ids) in [first, second].into_iter().enumerate() {
        for id in ids {
            if assignment.insert(id.as_str(), index).is_some() {
                return false;
            }
        }
    }
    if assignment.len() != submitted.len() {
        return false;
    }
    let mut consumed = [0_usize, 0_usize];
    for id in submitted {
        let Some(&assigned) = assignment.get(id.as_str()) else {
            return false;
        };
        if [first, second][assigned]
            .get(consumed[assigned])
            .is_none_or(|expected| expected != id)
        {
            return false;
        }
        consumed[assigned] += 1;
    }
    consumed == [first.len(), second.len()]
}

fn confirmed<T>(result: Result<T, ()>, identity: &MutationIdentity) -> Result<T, CommandFailure> {
    result.map_err(|()| CommandFailure::unknown(identity))
}

async fn membership_mutation(
    kind: MembershipKind,
    args: &AlbumMembershipArgs,
    photo_ids: Vec<String>,
    client: &ServiceClient,
    admission: &AdmissionState,
) -> Result<Value, CommandFailure> {
    let identity = MutationIdentity {
        operation: kind.operation(),
        photo_ids: photo_ids.clone(),
        album_id: Some(args.album_id.clone()),
        album_name: None,
    };
    let result: Value = client
        .mutation(
            &identity,
            admission,
            client.endpoint(&["api", "albums", &args.album_id, "changes"]),
            json!({
                "operation": kind.wire(),
                "photoIds": photo_ids,
                "ifVersion": args.if_version,
            }),
        )
        .await?;
    confirmed_membership_result(kind, &identity, result, &client.origin)
}

/// Validates one confirmed membership result against the submitted request
/// and renders the CLI reference data shape.
fn confirmed_membership_result(
    kind: MembershipKind,
    identity: &MutationIdentity,
    result: Value,
    origin: &Url,
) -> Result<Value, CommandFailure> {
    let unknown = || CommandFailure::unknown(identity);
    match kind {
        MembershipKind::Add => {
            let result: AlbumAddWire = serde_json::from_value(result).map_err(|_| unknown())?;
            if !request_order_partition(
                &identity.photo_ids,
                &result.added_photo_ids,
                &result.already_member_photo_ids,
            ) {
                return Err(unknown());
            }
            let album = confirmed(album_value(result.album, origin), identity)?;
            Ok(json!({
                "album": album,
                "addedPhotoIds": result.added_photo_ids,
                "alreadyMemberPhotoIds": result.already_member_photo_ids,
            }))
        }
        MembershipKind::Remove => {
            let result: AlbumRemoveWire = serde_json::from_value(result).map_err(|_| unknown())?;
            if result.saved_photo_id.as_deref().is_some_and(str::is_empty)
                || !request_order_partition(
                    &identity.photo_ids,
                    &result.removed_photo_ids,
                    &result.already_absent_photo_ids,
                )
            {
                return Err(unknown());
            }
            let album = confirmed(album_value(result.album, origin), identity)?;
            Ok(json!({
                "album": album,
                "removedPhotoIds": result.removed_photo_ids,
                "alreadyAbsentPhotoIds": result.already_absent_photo_ids,
                "savedPhotoId": result.saved_photo_id,
            }))
        }
        MembershipKind::Reorder => {
            let result: AlbumReorderWire = serde_json::from_value(result).map_err(|_| unknown())?;
            if result.ordered_photo_ids != identity.photo_ids {
                return Err(unknown());
            }
            let album = confirmed(album_value(result.album, origin), identity)?;
            Ok(json!({
                "album": album,
                "orderedPhotoIds": result.ordered_photo_ids,
                "reordered": result.reordered,
            }))
        }
    }
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

async fn execute(
    cli: &Cli,
    environment: Option<&str>,
    admission: &AdmissionState,
) -> Result<Value, CommandFailure> {
    let operation = command_operation(&cli.command);
    validate_command(&cli.command)?;
    let origin = service_origin(cli, environment)?;
    let token_path = access_token_path(cli)?;
    let token = read_access_token(token_path).await?;
    // The complete membership document validates before any network access,
    // so a local input failure can never depend on service reachability.
    let pending_membership = match &cli.command {
        Command::Albums {
            command: AlbumCommand::Add(args),
        } => Some(read_membership_ids(&args.input, MembershipKind::Add.limit_name()).await?),
        Command::Albums {
            command: AlbumCommand::Remove(args),
        } => Some(read_membership_ids(&args.input, MembershipKind::Remove.limit_name()).await?),
        Command::Albums {
            command: AlbumCommand::Reorder(args),
        } => Some(read_membership_ids(&args.input, MembershipKind::Reorder.limit_name()).await?),
        _ => None,
    };
    let client = ServiceClient::new(origin, token)?;
    client.capabilities(operation).await?;

    let result = async {
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
            Command::Albums {
                command: AlbumCommand::Create { name },
            } => {
                let identity = MutationIdentity {
                    operation,
                    photo_ids: Vec::new(),
                    album_id: None,
                    album_name: Some(name.clone()),
                };
                let result: AlbumCreationWire = client
                    .mutation(
                        &identity,
                        admission,
                        client.endpoint(&["api", "albums"]),
                        json!({ "name": name }),
                    )
                    .await?;
                let album = confirmed(album_value(result.album, &client.origin), &identity)?;
                Ok(json!({ "album": album }))
            }
            Command::Albums {
                command:
                    AlbumCommand::Rename {
                        album_id,
                        name,
                        if_version,
                    },
            } => {
                let identity = MutationIdentity {
                    operation,
                    photo_ids: Vec::new(),
                    album_id: Some(album_id.clone()),
                    album_name: Some(name.clone()),
                };
                let result: AlbumRenameWire = client
                    .mutation(
                        &identity,
                        admission,
                        client.endpoint(&["api", "albums", album_id, "changes"]),
                        json!({ "operation": "rename", "name": name, "ifVersion": if_version }),
                    )
                    .await?;
                let album = confirmed(album_value(result.album, &client.origin), &identity)?;
                Ok(json!({ "album": album, "renamed": result.renamed }))
            }
            Command::Albums {
                command:
                    AlbumCommand::Delete {
                        album_id,
                        if_version,
                    },
            } => {
                let identity = MutationIdentity {
                    operation,
                    photo_ids: Vec::new(),
                    album_id: Some(album_id.clone()),
                    album_name: None,
                };
                let result: AlbumDeleteWire = client
                    .mutation(
                        &identity,
                        admission,
                        client.endpoint(&["api", "albums", album_id, "changes"]),
                        json!({ "operation": "delete", "ifVersion": if_version }),
                    )
                    .await?;
                if !result.deleted || result.original_files_changed || result.album_id.is_empty() {
                    return Err(CommandFailure::unknown(&identity));
                }
                Ok(json!({
                    "albumId": result.album_id,
                    "deleted": true,
                    "originalFilesChanged": false,
                }))
            }
            Command::Albums {
                command: AlbumCommand::Add(args),
            } => {
                // The membership document was validated before connecting.
                let photo_ids = pending_membership.expect("membership input was read");
                membership_mutation(MembershipKind::Add, args, photo_ids, &client, admission).await
            }
            Command::Albums {
                command: AlbumCommand::Remove(args),
            } => {
                let photo_ids = pending_membership.expect("membership input was read");
                membership_mutation(MembershipKind::Remove, args, photo_ids, &client, admission)
                    .await
            }
            Command::Albums {
                command: AlbumCommand::Reorder(args),
            } => {
                let photo_ids = pending_membership.expect("membership input was read");
                membership_mutation(MembershipKind::Reorder, args, photo_ids, &client, admission)
                    .await
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
    .await;
    match result {
        Ok(mut data) => {
            redact_value(&mut data, &client.token);
            Ok(data)
        }
        Err(mut failure) => {
            redact_error(&mut failure.payload, &client.token);
            Err(failure)
        }
    }
}

fn redact_error(error: &mut ErrorPayload, secret: &str) {
    redact_string(&mut error.code, secret);
    redact_string(&mut error.message, secret);
    redact_string(&mut error.effect, secret);
    redact_value(&mut error.details, secret);
}

fn redact_value(value: &mut Value, secret: &str) {
    match value {
        Value::String(string) => redact_string(string, secret),
        Value::Array(items) => {
            for item in items {
                redact_value(item, secret);
            }
        }
        Value::Object(fields) => {
            let previous = std::mem::take(fields);
            for (mut key, mut field) in previous {
                redact_string(&mut key, secret);
                redact_value(&mut field, secret);
                fields.insert(key, field);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn redact_string(value: &mut String, secret: &str) {
    if !secret.is_empty() && value.contains(secret) {
        *value = value.replace(secret, "[redacted]");
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

fn access_token_path(cli: &Cli) -> Result<PathBuf, CommandFailure> {
    let path = cli
        .token_file
        .clone()
        .or_else(|| env::var_os("SLIPSTREAM_ACCESS_TOKEN_FILE").map(PathBuf::from));
    let Some(path) = path else {
        return Err(CommandFailure::invalid(
            "token-file",
            "Set --token-file or SLIPSTREAM_ACCESS_TOKEN_FILE.",
        ));
    };
    if path.as_os_str().is_empty() {
        return Err(CommandFailure::invalid(
            "token-file",
            "The credential file path must not be empty.",
        ));
    }
    Ok(path)
}

async fn read_access_token(path: PathBuf) -> Result<String, CommandFailure> {
    tokio::task::spawn_blocking(move || read_access_token_file(&path))
        .await
        .map_err(|_| CommandFailure::local_credential(None))?
}

#[cfg(unix)]
fn read_access_token_file(path: &Path) -> Result<String, CommandFailure> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    const MAXIMUM_CREDENTIAL_BYTES: usize = 45;
    let path_text = path.to_str();
    let mut options = OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let file = options.open(path).map_err(|error| {
        if error.raw_os_error() == Some(libc::ELOOP) {
            CommandFailure::invalid(
                "token-file",
                "The credential file must be a nonsymlink regular file.",
            )
        } else {
            CommandFailure::local_credential(path_text)
        }
    })?;
    let metadata = file
        .metadata()
        .map_err(|_| CommandFailure::local_credential(path_text))?;
    // Check the file opened above, so a symlink swap cannot change which file
    // is validated and read. Do not disclose which credential rule failed.
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
        || metadata.len() > MAXIMUM_CREDENTIAL_BYTES as u64
    {
        return Err(CommandFailure::invalid(
            "token-file",
            "The credential file must be a private regular file owned by the current user.",
        ));
    }
    let mut bytes = Vec::with_capacity(MAXIMUM_CREDENTIAL_BYTES);
    file.take((MAXIMUM_CREDENTIAL_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| CommandFailure::local_credential(path_text))?;
    if bytes.len() > MAXIMUM_CREDENTIAL_BYTES {
        return Err(CommandFailure::invalid(
            "token-file",
            "The credential file must contain one Access Token and an optional line ending.",
        ));
    }
    let token_bytes = bytes
        .strip_suffix(b"\r\n")
        .or_else(|| bytes.strip_suffix(b"\n"))
        .unwrap_or(&bytes);
    if !canonical_access_token(token_bytes) {
        return Err(CommandFailure::invalid(
            "token-file",
            "The credential file must contain one canonical Access Token and an optional line ending.",
        ));
    }
    // canonical_access_token accepts only ASCII base64url bytes.
    Ok(String::from_utf8(token_bytes.to_vec()).expect("validated token is ASCII"))
}

#[cfg(not(unix))]
fn read_access_token_file(_path: &Path) -> Result<String, CommandFailure> {
    Err(CommandFailure::invalid(
        "token-file",
        "Private credential-file checks are unavailable on this platform.",
    ))
}

fn canonical_access_token(token: &[u8]) -> bool {
    token.len() == 43
        && token
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        && matches!(
            token[42],
            b'A' | b'E'
                | b'I'
                | b'M'
                | b'Q'
                | b'U'
                | b'Y'
                | b'c'
                | b'g'
                | b'k'
                | b'o'
                | b's'
                | b'w'
                | b'0'
                | b'4'
                | b'8'
        )
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
    let operation = command_operation(&cli.command);
    let admission = AdmissionState::default();
    let command = tokio::time::timeout_at(deadline, execute(&cli, environment, &admission));
    tokio::pin!(command);
    let (exit_code, envelope) = tokio::select! {
        result = &mut command => match result {
            Ok(Ok(data)) => (0, Envelope::success(data)),
            Ok(Err(failure)) => (failure.exit_code, Envelope::error(failure.payload)),
            Err(_) => {
                let failure = match admission.admitted() {
                    Some(identity) => CommandFailure::unknown(&identity),
                    None => CommandFailure::transport(operation),
                };
                (failure.exit_code, Envelope::error(failure.payload))
            }
        },
        _ = tokio::signal::ctrl_c() => {
            let failure = match admission.admitted() {
                Some(identity) => CommandFailure::interrupted_unknown(&identity),
                None => {
                    let mut failure = CommandFailure::transport(operation);
                    failure.payload.message = "The command was interrupted. Inspect status before continuing.".to_owned();
                    failure
                }
            };
            // A handled interruption exits 130 whether or not a request may
            // have been admitted; only the envelope distinguishes the cases.
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
        let without_server = Cli::try_parse_from(["slipstream", "status"]).unwrap();
        assert!(service_origin(&without_server, None).is_err());
        assert!(service_origin(&without_server, Some("")).is_err());
        for invalid in [
            "ftp://example.test",
            "http://127.0.0.1:3000",
            "http://user@example.test",
            "http://@example.test",
            "https://example.test/path",
            "https://example.test/?x=1",
        ] {
            assert!(
                service_origin(&without_server, Some(invalid)).is_err(),
                "accepted {invalid}"
            );
        }
    }

    #[test]
    fn token_file_option_is_explicit_and_does_not_accept_abbreviations_or_duplicates() {
        assert!(
            Cli::try_parse_from([
                "slipstream",
                "--token-file",
                "/run/slipstream/token",
                "status"
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "slipstream",
                "--token-file",
                "first",
                "--token-file",
                "second",
                "status"
            ])
            .is_err()
        );
        assert!(Cli::try_parse_from(["slipstream", "--tok", "file", "status"]).is_err());
        assert!(canonical_access_token(b"A".repeat(43).as_slice()));
        assert!(!canonical_access_token(b"A".repeat(42).as_slice()));
        assert!(!canonical_access_token(
            b"A".repeat(42)
                .iter()
                .copied()
                .chain(*b"B")
                .collect::<Vec<_>>()
                .as_slice()
        ));
        assert!(!canonical_access_token(
            b"A".repeat(42)
                .iter()
                .copied()
                .chain(*b"=")
                .collect::<Vec<_>>()
                .as_slice()
        ));
    }

    #[cfg(unix)]
    #[test]
    fn credential_file_reader_rejects_fifos_without_blocking() {
        use std::{
            ffi::CString,
            os::unix::ffi::OsStrExt,
            sync::{
                OnceLock,
                atomic::{AtomicU64, Ordering},
                mpsc,
            },
            thread,
        };

        static NEXT_PATH: OnceLock<AtomicU64> = OnceLock::new();
        let sequence = NEXT_PATH
            .get_or_init(|| AtomicU64::new(0))
            .fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "slipstream-cli-token-fifo-{}-{sequence}",
            std::process::id()
        ));
        let c_path = CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

        let (sender, receiver) = mpsc::channel();
        let reader_path = path.clone();
        thread::spawn(move || {
            let _ = sender.send(read_access_token_file(&reader_path));
        });
        let failure = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("opening a FIFO credential must not block");
        assert_eq!(failure.unwrap_err().payload.code, "invalid_input");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn access_boundary_errors_are_normalized_without_server_diagnostics() {
        let unauthorized = access_boundary_failure(
            StatusCode::UNAUTHORIZED,
            None,
            br#"{"error":"token leaked by a server"}"#,
            Operation::Status,
        )
        .unwrap();
        assert_eq!(unauthorized.payload.code, "authentication_required");
        assert_eq!(unauthorized.payload.details["operation"], "status");
        assert!(!unauthorized.payload.message.contains("token leaked"));

        let limited = access_boundary_failure(
            StatusCode::TOO_MANY_REQUESTS,
            Some(7),
            b"{}",
            Operation::AlbumsCreate,
        )
        .unwrap();
        assert_eq!(limited.payload.code, "server_busy");
        assert_eq!(limited.payload.details["retryAfterSeconds"], 7);

        assert!(
            access_boundary_failure(
                StatusCode::SERVICE_UNAVAILABLE,
                None,
                br#"{"error":"storage_failed"}"#,
                Operation::Status,
            )
            .is_none()
        );
        let unconfigured = access_boundary_failure(
            StatusCode::SERVICE_UNAVAILABLE,
            Some(9),
            br#"{"error":"access_unconfigured"}"#,
            Operation::Status,
        )
        .unwrap();
        assert_eq!(unconfigured.payload.code, "server_busy");
        assert_eq!(
            unconfigured.payload.details["retryAfterSeconds"],
            Value::Null
        );
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

    #[test]
    fn album_mutation_parser_enforces_names_versions_and_input() {
        assert!(Cli::try_parse_from(["slipstream", "albums", "create", "--name", "Picks"]).is_ok());
        assert!(Cli::try_parse_from(["slipstream", "albums", "create", "--name", ""]).is_err());
        assert!(Cli::try_parse_from(["slipstream", "albums", "create", "--name", "  "]).is_err());
        let name_arguments = |name: String| {
            ["slipstream", "albums", "create", "--name"]
                .into_iter()
                .map(str::to_owned)
                .chain([name])
                .collect::<Vec<_>>()
        };
        assert!(Cli::try_parse_from(name_arguments("x".repeat(121))).is_err());
        assert!(Cli::try_parse_from(name_arguments("\u{00e9}".repeat(121))).is_err());
        assert!(Cli::try_parse_from(name_arguments("\u{00e9}".repeat(120))).is_ok());
        assert!(
            Cli::try_parse_from(["slipstream", "albums", "rename", "a", "--name", "N"]).is_err()
        );
        assert!(
            Cli::try_parse_from(["slipstream", "albums", "delete", "a", "--if-version", "",])
                .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "slipstream",
                "albums",
                "add",
                "a",
                "--input",
                "members.json",
                "--if-version",
                "v",
                "--if-version",
                "w",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from(["slipstream", "albums", "remove", "a", "--input", "-"]).is_err()
        );
    }

    #[test]
    fn membership_input_validates_the_complete_document() {
        let ids = |count: usize| {
            (0..count)
                .map(|index| format!("00000000-0000-4000-8000-{index:012x}"))
                .collect::<Vec<_>>()
        };
        let document =
            |photo_ids: &[String]| serde_json::to_vec(&json!({ "photoIds": photo_ids })).unwrap();
        let parsed = parse_membership_ids(document(&ids(2)), "mutationPhotoIdsMaximum").unwrap();
        assert_eq!(parsed, ids(2));
        for invalid in [
            b"".as_slice(),
            b"{".as_slice(),
            b"[]".as_slice(),
            b"\"photoIds\"".as_slice(),
            b"{\"photoIds\": []}".as_slice(),
            b"{\"photoIds\": [\"\"]}".as_slice(),
            b"{\"photoIds\": [\"a\", \"a\"]}".as_slice(),
            b"{\"photoIds\": [\"a\"], \"photoIds\": [\"b\"]}".as_slice(),
            b"{\"photoIds\": [\"a\"], \"extra\": 1}".as_slice(),
            b"{\"photoIds\": [\"a\"]} trailing".as_slice(),
            b"{\"photoIds\": [1]}".as_slice(),
            b"\xff\xfe{\"photoIds\": [\"a\"]}".as_slice(),
        ] {
            let failure =
                parse_membership_ids(invalid.to_vec(), "mutationPhotoIdsMaximum").unwrap_err();
            assert_eq!(failure.exit_code, 2, "for {invalid:?}");
            assert_eq!(failure.payload.code, "invalid_input");
            assert_eq!(failure.payload.details["argument"], "input");
        }
        let over_limit = parse_membership_ids(
            document(&ids(MAXIMUM_MUTATION_PHOTO_IDS + 1)),
            "albumReorderMembersMaximum",
        )
        .unwrap_err();
        assert_eq!(over_limit.exit_code, 2);
        assert_eq!(over_limit.payload.code, "limit_exceeded");
        assert_eq!(
            over_limit.payload.details,
            json!({
                "limitName": "albumReorderMembersMaximum",
                "limit": MAXIMUM_MUTATION_PHOTO_IDS,
                "actual": MAXIMUM_MUTATION_PHOTO_IDS + 1,
            })
        );
    }

    #[test]
    fn request_order_partitions_must_cover_and_preserve_request_order() {
        let submitted = ["a", "b", "c", "d"]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();
        let strings = |values: &[&str]| {
            values
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>()
        };
        assert!(request_order_partition(
            &submitted,
            &strings(&["a", "c"]),
            &strings(&["b", "d"])
        ));
        assert!(request_order_partition(
            &submitted,
            &strings(&[]),
            &strings(&["a", "b", "c", "d"])
        ));
        assert!(!request_order_partition(
            &submitted,
            &strings(&["b", "a"]),
            &strings(&["c", "d"])
        ));
        assert!(!request_order_partition(
            &submitted,
            &strings(&["a", "c"]),
            &strings(&["d"])
        ));
        assert!(!request_order_partition(
            &submitted,
            &strings(&["a", "c"]),
            &strings(&["b", "d", "e"])
        ));
        assert!(!request_order_partition(
            &submitted,
            &strings(&["a", "a"]),
            &strings(&["b", "c", "d"])
        ));
        assert!(!request_order_partition(
            &submitted,
            &strings(&["a", "b", "c"]),
            &strings(&["c", "d"])
        ));
    }
}
