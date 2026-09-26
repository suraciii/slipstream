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

mod preview_download;

const CLI_CONTRACT_VERSION: u16 = 1;
const DEFAULT_TIMEOUT_SECONDS: u64 = 30;
const DEFAULT_LIST_PAGE: usize = 50;
const MAXIMUM_LIST_PAGE: usize = 60;
const CONTRACT_HEADER: &str = "Slipstream-CLI-Contract";
const MAXIMUM_JSON_RESPONSE_BYTES: usize = 1024 * 1024;
const MAXIMUM_INPUT_BYTES: usize = 64 * 1024;
const MAXIMUM_MUTATION_PHOTO_IDS: usize = 100;
const MAXIMUM_TRASH_IDS: usize = 5_000;

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
    /// Request one Library check through the service-owned scan cycle.
    Library {
        #[command(subcommand)]
        command: LibraryCommand,
    },
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
    /// Inspect and permanently delete files from the persistent Trash.
    Trash {
        #[command(subcommand)]
        command: TrashCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum TrashCommand {
    /// List Trash items in removal order.
    List(TrashListArgs),
    /// Capture a fixed Trash review before confirmation and deletion.
    Review(TrashReviewArgs),
    /// Permanently delete the reviewed operation and report every outcome.
    Delete {
        #[arg(value_parser = nonempty)]
        operation_id: String,
    },
    /// Reopen a durable Permanent Deletion result.
    Operation {
        #[arg(value_parser = nonempty)]
        operation_id: String,
    },
}

#[derive(Debug, Args)]
pub struct TrashListArgs {
    /// Number of items to skip.
    #[arg(long, default_value_t = 0)]
    pub start: usize,
    /// Maximum items in this page (1 through 60).
    #[arg(long, value_name = "N", value_parser = page_limit, default_value_t = 60)]
    pub limit: u8,
}

#[derive(Debug, Args)]
pub struct TrashReviewArgs {
    #[arg(value_name = "OPERATION_ID", value_parser = nonempty)]
    pub operation_id: String,
    /// Review every current Trash item except IDs in --exclude-input.
    #[arg(long, conflicts_with = "input")]
    pub all: bool,
    /// UTF-8 JSON file holding {"photoIds":[...]} or a bare ID array.
    #[arg(long, value_name = "FILE", value_parser = nonempty, conflicts_with = "all")]
    pub input: Option<String>,
    /// UTF-8 JSON file holding {"photoIds":[...]} or a bare ID array to exclude.
    #[arg(long, value_name = "FILE", value_parser = nonempty, requires = "all")]
    pub exclude_input: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum LibraryCommand {
    /// Check the Library using the service-owned scan cycle.
    Check,
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
    /// Change Selection State or Rating for identified Photos against the
    /// decision versions observed by a prior read. One command changes one
    /// field and reports changed, unchanged, conflict, or missing per Photo.
    Set(PhotoDecisionArgs),
    /// Remove exactly the reviewed Photo IDs in one bounded operation. The
    /// input must contain current rejected decision and removal evidence.
    Remove(PhotoRemovalArgs),
    /// Read the durable result of one explicit removal operation.
    RemovalOperation {
        #[arg(value_parser = nonempty)]
        operation_id: String,
    },
    /// Restore one observed removal operation or explicit reviewed markers.
    Restore(PhotoRestoreArgs),
    /// Read the durable result of one explicit Restore attempt.
    RestoreOperation {
        #[arg(value_parser = nonempty)]
        operation_id: String,
    },
    /// Download one current Photo Preview to a new local JPEG file (at most 64 MiB).
    Preview {
        #[arg(value_parser = nonempty)]
        photo_id: String,
        /// New local JPEG path; an existing file or symbolic link is never replaced.
        #[arg(long, value_name = "PATH", required = true)]
        file: PathBuf,
        /// Requested Preview target.
        #[arg(long, value_enum, default_value_t = PreviewSize::Review)]
        size: PreviewSize,
    },
}

#[derive(Debug, Args)]
pub struct PhotoRemovalArgs {
    #[arg(value_name = "OPERATION_ID", value_parser = nonempty)]
    pub operation_id: String,
    /// UTF-8 JSON file holding one complete explicit removal document; `-`
    /// reads it from stdin.
    #[arg(long, value_name = "FILE", value_parser = nonempty)]
    pub input: String,
}
#[derive(Debug, Args)]
pub struct PhotoRestoreArgs {
    #[arg(value_name = "OPERATION_ID", value_parser = nonempty)]
    pub operation_id: String,
    /// UTF-8 JSON file holding {"photos":[{"photoId","removedAtMs"}]}.
    /// Omit it to restore the complete named removal operation.
    #[arg(long, value_name = "FILE", value_parser = nonempty)]
    pub input: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum PreviewSize {
    Thumbnail,
    Review,
}

#[derive(Debug, Args)]
pub struct PhotoDecisionArgs {
    /// One Photo ID for the single-Photo forms.
    #[arg(value_name = "PHOTO_ID", value_parser = nonempty, conflicts_with = "input")]
    pub photo_id: Option<String>,
    /// Selection State to apply to the named Photo.
    #[arg(long, value_enum, conflicts_with_all = ["rating", "input"])]
    pub selection: Option<SetSelectionArg>,
    /// Rating from 0 through 5 to apply to the named Photo. Zero clears it.
    #[arg(long, value_name = "N", value_parser = rating, conflicts_with_all = ["selection", "input"])]
    pub rating: Option<u8>,
    /// UTF-8 JSON file holding one complete decision batch of at most 100
    /// distinct Photo IDs, each with its observed version; `-` reads the
    /// document from stdin.
    #[arg(long, value_name = "FILE", value_parser = nonempty, conflicts_with_all = ["photo_id", "selection", "rating", "if_version"])]
    pub input: Option<String>,
    /// Decision version observed by a prior read of the named Photo.
    #[arg(long, value_name = "VERSION", value_parser = nonempty, conflicts_with = "input", requires = "photo_id")]
    pub if_version: Option<String>,
}

#[derive(Clone, Copy, Debug, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum SetSelectionArg {
    Undecided,
    Selected,
    Rejected,
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
    pub committed_preview_path: Option<String>,
}

#[derive(Clone, Copy, Debug)]
enum Operation {
    Status,
    LibraryCheck,
    FoldersList,
    AlbumsList,
    AlbumsGet,
    PhotosList,
    PhotosGet,
    PhotosPreview,
    PhotosSet,
    PhotosRemove,
    PhotosRemovalInspect,
    PhotosRestore,
    PhotosRestoreInspect,
    AlbumsCreate,
    AlbumsRename,
    AlbumsDelete,
    AlbumsAdd,
    AlbumsRemove,
    AlbumsReorder,
    TrashList,
    TrashReview,
    TrashDelete,
    TrashRead,
}

impl Operation {
    fn wire(self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::LibraryCheck => "library-check",
            Self::FoldersList => "folders-list",
            Self::AlbumsList => "albums-list",
            Self::AlbumsGet => "albums-get",
            Self::PhotosList => "photos-list",
            Self::PhotosGet => "photos-get",
            Self::PhotosPreview => "photos-preview",
            Self::PhotosSet => "photos-set",
            Self::PhotosRemove => "photos-remove",
            Self::PhotosRemovalInspect => "photos-removal-operation",
            Self::PhotosRestore => "photos-restore",
            Self::PhotosRestoreInspect => "photos-restore-operation",
            Self::AlbumsCreate => "albums-create",
            Self::AlbumsRename => "albums-rename",
            Self::AlbumsDelete => "albums-delete",
            Self::AlbumsAdd => "albums-add",
            Self::AlbumsRemove => "albums-remove",
            Self::AlbumsReorder => "albums-reorder",
            Self::TrashList => "trash-list",
            Self::TrashReview => "trash-review",
            Self::TrashDelete => "trash-delete",
            Self::TrashRead => "trash-operation",
        }
    }
}

fn command_operation(command: &Command) -> Operation {
    match command {
        Command::Status => Operation::Status,
        Command::Library { .. } => Operation::LibraryCheck,
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
            PhotoCommand::Preview { .. } => Operation::PhotosPreview,
            PhotoCommand::Set(_) => Operation::PhotosSet,
            PhotoCommand::Remove(_) => Operation::PhotosRemove,
            PhotoCommand::RemovalOperation { .. } => Operation::PhotosRemovalInspect,
            PhotoCommand::Restore(_) => Operation::PhotosRestore,
            PhotoCommand::RestoreOperation { .. } => Operation::PhotosRestoreInspect,
        },
        Command::Trash { command } => match command {
            TrashCommand::List(_) => Operation::TrashList,
            TrashCommand::Review(_) => Operation::TrashReview,
            TrashCommand::Delete { .. } => Operation::TrashDelete,
            TrashCommand::Operation { .. } => Operation::TrashRead,
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
#[derive(Debug)]
struct PendingTrashReview {
    photo_ids: Vec<String>,
    exclude_photo_ids: Vec<String>,
}

/// Records the point where a mutation request was handed to the transport.
/// Any later timeout, interruption, or unusable response is an unknown
/// outcome instead of a claimed refusal.
#[derive(Debug, Default)]
struct AdmissionState {
    identity: std::sync::Mutex<Option<MutationIdentity>>,
}

#[derive(Debug, Default)]
struct PublicationState {
    committed: std::sync::Mutex<Option<Value>>,
}

impl PublicationState {
    fn record(&self, data: Value) {
        *self
            .committed
            .lock()
            .expect("publication state is lockable") = Some(data);
    }

    fn committed(&self) -> Option<Value> {
        self.committed
            .lock()
            .expect("publication state is lockable")
            .clone()
    }
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

    /// A mixed Photo batch keeps its confirmed results in `data` beside the
    /// `partial_result` error.
    fn partial(data: Value, error: ErrorPayload) -> Self {
        Self {
            schema_version: 1,
            status: "partial",
            data: Some(data),
            error: Some(error),
        }
    }

    /// An all-unsuccessful Photo batch keeps its complete result array in
    /// `data` beside its partition error.
    fn error_with_data(data: Value, error: ErrorPayload) -> Self {
        Self {
            schema_version: 1,
            status: "error",
            data: Some(data),
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
    /// Confirmed per-Photo results for a mixed or all-unsuccessful Photo
    /// batch; the complete result array stays in `data` for those failures.
    data: Option<Box<Value>>,
}

impl CommandFailure {
    fn from_payload(exit_code: u8, payload: ErrorPayload) -> Self {
        Self {
            exit_code,
            payload,
            data: None,
        }
    }

    fn invalid(argument: &str, reason: impl Into<String>) -> Self {
        Self::from_payload(
            2,
            ErrorPayload {
                code: "invalid_input".to_owned(),
                message: "Correct the request and try again.".to_owned(),
                effect: "none".to_owned(),
                details: json!({ "argument": argument, "reason": reason.into() }),
            },
        )
    }

    fn transport(operation: Operation) -> Self {
        Self::from_payload(
            6,
            ErrorPayload {
                code: "transport_failed".to_owned(),
                message: "Check the service connection and try again.".to_owned(),
                effect: "none".to_owned(),
                details: json!({ "operation": operation.wire() }),
            },
        )
    }

    fn incompatible(supported: Vec<u16>) -> Self {
        Self::from_payload(
            6,
            ErrorPayload {
                code: "incompatible_server".to_owned(),
                message: "Use a compatible Slipstream client and service.".to_owned(),
                effect: "none".to_owned(),
                details: json!({
                    "requestedContractVersion": CLI_CONTRACT_VERSION,
                    "supportedContractVersions": supported,
                }),
            },
        )
    }

    fn limit_exceeded(limit_name: &str, limit: usize, actual: usize) -> Self {
        Self::from_payload(
            2,
            ErrorPayload {
                code: "limit_exceeded".to_owned(),
                message: "Reduce the request and try again.".to_owned(),
                effect: "none".to_owned(),
                details: json!({ "limitName": limit_name, "limit": limit, "actual": actual }),
            },
        )
    }

    fn local_input(path: Option<&str>) -> Self {
        Self::from_payload(
            6,
            ErrorPayload {
                code: "local_io_failed".to_owned(),
                message: "Check the local input file and try again.".to_owned(),
                effect: "none".to_owned(),
                details: json!({
                    "operation": "read-input",
                    "path": path,
                    "fileCommitted": false,
                }),
            },
        )
    }

    fn published_preview(data: Value, interrupted: bool) -> Self {
        Self::from_payload(
            if interrupted { 130 } else { 6 },
            ErrorPayload {
                code: "local_io_failed".to_owned(),
                message: "The Preview file was published; inspect it before trying again."
                    .to_owned(),
                effect: "partial".to_owned(),
                details: json!({
                    "operation": "write-output",
                    "path": data["path"],
                    "fileCommitted": true,
                }),
            },
        )
        .with_data(data)
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
            data: None,
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
            data: None,
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
            data: None,
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
            data: None,
        }
    }

    fn unknown(identity: &MutationIdentity) -> Self {
        Self::from_payload(
            7,
            ErrorPayload {
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
        )
    }

    fn interrupted_unknown(identity: &MutationIdentity) -> Self {
        Self::from_payload(
            130,
            ErrorPayload {
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
        )
    }

    /// A mixed Photo batch: at least one sibling decision committed while
    /// at least one requested Photo conflicted or was missing.
    fn photo_batch_partial(counts: &Value) -> Self {
        Self::from_payload(
            5,
            ErrorPayload {
                code: "partial_result".to_owned(),
                message: "Read the confirmed sibling outcomes, then re-check each conflicting or missing Photo with a fresh version.".to_owned(),
                effect: "partial".to_owned(),
                details: json!({ "counts": counts }),
            },
        )
    }

    /// A batch whose every item conflicted; the details identify the first
    /// conflicting Photo in request order.
    fn photo_batch_conflict(reference: &str, current_version: &str) -> Self {
        Self::from_payload(
            4,
            ErrorPayload {
                code: "conflict".to_owned(),
                message: "Read the current decisions and retry with their fresh versions."
                    .to_owned(),
                effect: "none".to_owned(),
                details: json!({
                    "resource": "photo",
                    "reference": reference,
                    "currentVersion": current_version,
                }),
            },
        )
    }

    /// A batch whose every item was missing; the details identify the first
    /// missing Photo in request order.
    fn photo_batch_missing(reference: &str) -> Self {
        Self::from_payload(
            3,
            ErrorPayload {
                code: "not_found".to_owned(),
                message: "Check the requested Photo IDs against a fresh query.".to_owned(),
                effect: "none".to_owned(),
                details: json!({ "resource": "photo", "reference": reference }),
            },
        )
    }

    fn with_data(mut self, data: Value) -> Self {
        self.data = Some(Box::new(data));
        self
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
    removal_photo_ids_maximum: u64,
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
    removed_at_ms: Option<i64>,
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
    removed_at_ms: Option<i64>,
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
struct RemovalInput {
    photos: Vec<RemovalInputPhoto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemovalInputPhoto {
    photo_id: String,
    selection_state: String,
    decision_version: String,
    removed_at_ms: Value,
}

#[derive(Debug)]
struct PreparedRemoval {
    photos: Vec<RemovalTarget>,
}

#[derive(Clone, Debug)]
struct RemovalTarget {
    photo_id: String,
    selection_state: String,
    decision_version: String,
    removed_at_ms: Option<i64>,
}

impl PreparedRemoval {
    fn body(&self, operation_id: &str) -> Value {
        json!({
            "operationId": operation_id,
            "photos": self.photos.iter().map(|photo| json!({
                "photoId": photo.photo_id,
                "selectionState": photo.selection_state,
                "decisionVersion": photo.decision_version,
                "removedAtMs": photo.removed_at_ms,
            })).collect::<Vec<_>>(),
        })
    }
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RestoreInput {
    photos: Vec<RestoreInputPhoto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RestoreInputPhoto {
    photo_id: String,
    removed_at_ms: i64,
}

#[derive(Debug)]
struct PreparedRestore {
    markers: Vec<RestoreMarker>,
}

#[derive(Clone, Debug)]
struct RestoreMarker {
    photo_id: String,
    removed_at_ms: i64,
}

impl PreparedRestore {
    fn body(&self, operation_id: &str) -> Value {
        json!({
            "operationId": operation_id,
            "photos": self.markers.iter().map(|marker| json!({
                "photoId": marker.photo_id,
                "removedAtMs": marker.removed_at_ms,
            })).collect::<Vec<_>>(),
        })
    }
}

/// One identified Photo together with the decision version the caller
/// observed before intending to write.
#[derive(Clone, Debug)]
struct DecisionTarget {
    photo_id: String,
    if_version: String,
}

#[derive(Clone, Copy, Debug)]
enum DecisionField {
    SelectionState,
    Rating,
}

impl DecisionField {
    fn wire(self) -> &'static str {
        match self {
            Self::SelectionState => "selectionState",
            Self::Rating => "rating",
        }
    }
}

/// One validated decision request shared by the single-Photo and batch forms.
/// A single-Photo command is a one-item batch.
#[derive(Debug)]
struct PreparedDecision {
    field: DecisionField,
    value: Value,
    photos: Vec<DecisionTarget>,
}

impl PreparedDecision {
    fn body(&self) -> Value {
        json!({
            "field": self.field.wire(),
            "value": self.value,
            "photos": self.photos.iter().map(|photo| json!({
                "photoId": photo.photo_id,
                "ifVersion": photo.if_version,
            })).collect::<Vec<_>>(),
        })
    }
}

/// The one accepted `photos set --input` document shape. Deserializing it
/// rejects unknown keys, duplicate keys, trailing content, and non-object
/// documents, including inside each Photo item.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DecisionInput {
    field: String,
    value: Value,
    photos: Vec<DecisionInputPhoto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DecisionInputPhoto {
    photo_id: String,
    if_version: String,
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

/// One confirmed checked Photo decision batch from the service.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PhotoDecisionWire {
    results: Vec<PhotoDecisionItemWire>,
    counts: PhotoDecisionCountsWire,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PhotoDecisionCountsWire {
    changed: usize,
    unchanged: usize,
    conflict: usize,
    missing: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PhotoDecisionItemWire {
    photo_id: String,
    outcome: String,
    prior: Option<PhotoDecisionFactsWire>,
    current: Option<PhotoDecisionSnapshotWire>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PhotoDecisionFactsWire {
    selection_state: SelectionState,
    rating: u8,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PhotoDecisionSnapshotWire {
    selection_state: SelectionState,
    rating: u8,
    decision_version: String,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PhotoRemovalWire {
    operation_id: String,
    counts: PhotoRemovalCountsWire,
    results: Vec<PhotoRemovalItemWire>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PhotoRemovalCountsWire {
    removed: usize,
    changed_elsewhere: usize,
    missing: usize,
    already_removed: usize,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PhotoRemovalItemWire {
    photo_id: String,
    outcome: String,
    removed_at_ms: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PhotoRestoreWire {
    operation_id: String,
    counts: PhotoRestoreCountsWire,
    results: Vec<PhotoRestoreItemWire>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PhotoRestoreCountsWire {
    restored: usize,
    already_active: usize,
    changed_elsewhere: usize,
    missing: usize,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PhotoRestoreItemWire {
    photo_id: String,
    outcome: String,
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
            && capabilities.limits.removal_photo_ids_maximum > 0
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
        "preview_unavailable" => {
            required_keys(&["photoId", "state"])
                && string("photoId").is_some()
                && string("state").is_some_and(|value| {
                    matches!(value, "inspection-pending" | "failed" | "unavailable")
                })
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
    Some(CommandFailure::from_payload(exit_code, error))
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
async fn read_trash_ids(input: &str) -> Result<Vec<String>, CommandFailure> {
    let bytes = read_input_bytes(input).await?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
        CommandFailure::invalid(
            "input",
            "The input must be a JSON object with photoIds or a bare ID array.",
        )
    })?;
    let photo_ids = match value {
        Value::Array(values) => values
            .into_iter()
            .map(|value| value.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>(),
        Value::Object(mut object) => {
            let value = object.remove("photoIds");
            if !object.is_empty() {
                None
            } else {
                value.and_then(|value| {
                    value
                        .as_array()?
                        .iter()
                        .map(|value| value.as_str().map(str::to_owned))
                        .collect::<Option<Vec<_>>>()
                })
            }
        }
        _ => None,
    }
    .ok_or_else(|| {
        CommandFailure::invalid(
            "input",
            "The Trash ID input must contain only a photoIds string array.",
        )
    })?;
    if photo_ids.len() > MAXIMUM_TRASH_IDS {
        return Err(CommandFailure::limit_exceeded(
            "permanentDeletionPhotoIdsMaximum",
            MAXIMUM_TRASH_IDS,
            photo_ids.len(),
        ));
    }
    if photo_ids.iter().any(String::is_empty)
        || photo_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != photo_ids.len()
    {
        return Err(CommandFailure::invalid(
            "input",
            "Trash Photo IDs must be nonempty and distinct.",
        ));
    }
    Ok(photo_ids)
}

/// Validates the complete decision document before any write is attempted.
/// The refusal order mirrors the service's own admission order, so the same
/// request is refused for the same reason whichever boundary sees it first.
fn parse_decision_input(bytes: Vec<u8>) -> Result<PreparedDecision, CommandFailure> {
    let document: DecisionInput = serde_json::from_slice(&bytes).map_err(|_| {
        CommandFailure::invalid(
            "input",
            "The input must be one JSON object with exactly field, value, and photos.",
        )
    })?;
    let field = match document.field.as_str() {
        "selectionState" => DecisionField::SelectionState,
        "rating" => DecisionField::Rating,
        _ => {
            return Err(CommandFailure::invalid(
                "field",
                "The decision field must be selectionState or rating.",
            ));
        }
    };

    let value_matches_field = match field {
        DecisionField::SelectionState => document
            .value
            .as_str()
            .is_some_and(|value| matches!(value, "undecided" | "selected" | "rejected")),
        DecisionField::Rating => document.value.as_u64().is_some_and(|rating| rating <= 5),
    };
    if !value_matches_field {
        return Err(CommandFailure::invalid(
            "value",
            "The decision value must match the field's type and range.",
        ));
    }
    if document.photos.len() > MAXIMUM_MUTATION_PHOTO_IDS {
        return Err(CommandFailure::limit_exceeded(
            "photoIds",
            MAXIMUM_MUTATION_PHOTO_IDS,
            document.photos.len(),
        ));
    }
    if document.photos.is_empty()
        || document
            .photos
            .iter()
            .any(|photo| photo.photo_id.is_empty() || photo.if_version.is_empty())
        || document
            .photos
            .iter()
            .map(|photo| photo.photo_id.as_str())
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != document.photos.len()
    {
        return Err(CommandFailure::invalid(
            "photos",
            "Photo items must be a nonempty ordered list of distinct Photo IDs with nonempty versions.",
        ));
    }
    Ok(PreparedDecision {
        field,
        value: document.value,
        photos: document
            .photos
            .into_iter()
            .map(|photo| DecisionTarget {
                photo_id: photo.photo_id,
                if_version: photo.if_version,
            })
            .collect(),
    })
}

async fn read_decision_input(input: &str) -> Result<PreparedDecision, CommandFailure> {
    parse_decision_input(read_input_bytes(input).await?)
}
fn parse_removal_input(bytes: Vec<u8>) -> Result<PreparedRemoval, CommandFailure> {
    let document: RemovalInput = serde_json::from_slice(&bytes).map_err(|_| {
        CommandFailure::invalid(
            "input",
            "The input must be one object with a distinct photos evidence array.",
        )
    })?;
    if document.photos.is_empty() {
        return Err(CommandFailure::invalid(
            "photos",
            "The explicit removal target list must not be empty.",
        ));
    }
    if document.photos.len() > MAXIMUM_MUTATION_PHOTO_IDS {
        return Err(CommandFailure::limit_exceeded(
            "removalPhotoIdsMaximum",
            MAXIMUM_MUTATION_PHOTO_IDS,
            document.photos.len(),
        ));
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut photos = Vec::with_capacity(document.photos.len());
    for photo in document.photos {
        if photo.photo_id.is_empty()
            || !ids.insert(photo.photo_id.clone())
            || photo.selection_state != "rejected"
            || photo.decision_version.is_empty()
        {
            return Err(CommandFailure::invalid(
                "photos",
                "Each target must have a distinct ID, rejected Selection State, and nonempty decision version.",
            ));
        }
        let removed_at_ms = match photo.removed_at_ms {
            Value::Null => None,
            Value::Number(value) => match value.as_i64().filter(|value| *value >= 0) {
                Some(value) => Some(value),
                None => {
                    return Err(CommandFailure::invalid(
                        "removedAtMs",
                        "The removal marker must be null or nonnegative.",
                    ));
                }
            },
            _ => {
                return Err(CommandFailure::invalid(
                    "removedAtMs",
                    "The removal marker must be null or nonnegative.",
                ));
            }
        };
        photos.push(RemovalTarget {
            photo_id: photo.photo_id,
            selection_state: photo.selection_state,
            decision_version: photo.decision_version,
            removed_at_ms,
        });
    }
    Ok(PreparedRemoval { photos })
}

async fn read_removal_input(input: &str) -> Result<PreparedRemoval, CommandFailure> {
    parse_removal_input(read_input_bytes(input).await?)
}
fn parse_restore_input(bytes: Vec<u8>) -> Result<PreparedRestore, CommandFailure> {
    let document: RestoreInput = serde_json::from_slice(&bytes).map_err(|_| {
        CommandFailure::invalid(
            "input",
            "The input must be one object with a distinct photos marker array.",
        )
    })?;
    if document.photos.is_empty() || document.photos.len() > MAXIMUM_MUTATION_PHOTO_IDS {
        return Err(if document.photos.is_empty() {
            CommandFailure::invalid(
                "photos",
                "The explicit Restore target list must not be empty.",
            )
        } else {
            CommandFailure::limit_exceeded(
                "removalPhotoIdsMaximum",
                MAXIMUM_MUTATION_PHOTO_IDS,
                document.photos.len(),
            )
        });
    }
    let mut ids = std::collections::BTreeSet::new();
    let mut markers = Vec::with_capacity(document.photos.len());
    for photo in document.photos {
        if photo.photo_id.is_empty()
            || !ids.insert(photo.photo_id.clone())
            || photo.removed_at_ms < 0
        {
            return Err(CommandFailure::invalid(
                "photos",
                "Restore markers must have distinct IDs and nonnegative removal identities.",
            ));
        }
        markers.push(RestoreMarker {
            photo_id: photo.photo_id,
            removed_at_ms: photo.removed_at_ms,
        });
    }
    Ok(PreparedRestore { markers })
}

async fn read_restore_input(input: &str) -> Result<PreparedRestore, CommandFailure> {
    parse_restore_input(read_input_bytes(input).await?)
}

/// Builds the one-item batch shared by the single-Photo forms. The command
/// was semantically validated before any network access, so each required
/// piece is present.
fn single_photo_decision(args: &PhotoDecisionArgs) -> PreparedDecision {
    let (field, value) = if let Some(selection) = args.selection {
        (
            DecisionField::SelectionState,
            serde_json::to_value(selection).expect("selection values serialize"),
        )
    } else {
        (
            DecisionField::Rating,
            json!(args.rating.expect("one decision field is present")),
        )
    };
    PreparedDecision {
        field,
        value,
        photos: vec![DecisionTarget {
            photo_id: args.photo_id.clone().expect("a Photo ID is present"),
            if_version: args.if_version.clone().expect("a version is present"),
        }],
    }
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

/// Validates one confirmed decision batch against the submitted request and
/// derives the CLI reference data shape, partition, and exit code. Every
/// result must match its requested Photo and outcome-specific key set, the
/// reported counts must count those outcomes, and every changed or
/// unchanged result must repeat the requested decision value in its
/// current snapshot; anything else is an unknown outcome rather than a
/// claimed partition.
fn confirmed_decision_result(
    identity: &MutationIdentity,
    prepared: &PreparedDecision,
    result: PhotoDecisionWire,
) -> Result<Value, CommandFailure> {
    let unknown = || CommandFailure::unknown(identity);
    if result.results.len() != prepared.photos.len() {
        return Err(unknown());
    }
    let mut counted = [0_usize; 4];
    let mut first_conflict: Option<(String, String)> = None;
    let mut first_missing: Option<String> = None;
    let mut items = Vec::with_capacity(result.results.len());
    // A changed or unchanged result reports the requested decision value,
    // so any other current value is an untrustworthy response.
    let echoed_request = |current: &PhotoDecisionSnapshotWire| match prepared.field {
        DecisionField::SelectionState => serde_json::to_value(&current.selection_state)
            .is_ok_and(|snapshot| snapshot == prepared.value),
        DecisionField::Rating => prepared.value.as_u64() == Some(u64::from(current.rating)),
    };
    for (item, submitted_photo) in result.results.into_iter().zip(&prepared.photos) {
        if item.photo_id != submitted_photo.photo_id {
            return Err(unknown());
        }
        let mut value = json!({
            "photoId": item.photo_id,
            "outcome": item.outcome,
        });
        let current_valid = |current: &PhotoDecisionSnapshotWire| {
            !current.decision_version.is_empty() && current.rating <= 5
        };
        match item.outcome.as_str() {
            "changed" => {
                let (Some(prior), Some(current)) = (&item.prior, &item.current) else {
                    return Err(unknown());
                };
                if prior.rating > 5 || !current_valid(current) || !echoed_request(current) {
                    return Err(unknown());
                }
                counted[0] += 1;
                value["prior"] = json!({
                    "selectionState": prior.selection_state,
                    "rating": prior.rating,
                });
                value["current"] = json!({
                    "selectionState": current.selection_state,
                    "rating": current.rating,
                    "decisionVersion": current.decision_version,
                });
            }
            "unchanged" | "conflict" => {
                if item.prior.is_some() {
                    return Err(unknown());
                }
                let Some(current) = &item.current else {
                    return Err(unknown());
                };
                if !current_valid(current) {
                    return Err(unknown());
                }
                if item.outcome == "unchanged" {
                    if !echoed_request(current) {
                        return Err(unknown());
                    }
                    counted[1] += 1;
                } else {
                    counted[2] += 1;
                    if first_conflict.is_none() {
                        first_conflict =
                            Some((item.photo_id.clone(), current.decision_version.clone()));
                    }
                }
                value["current"] = json!({
                    "selectionState": current.selection_state,
                    "rating": current.rating,
                    "decisionVersion": current.decision_version,
                });
            }
            "missing" => {
                if item.prior.is_some() || item.current.is_some() {
                    return Err(unknown());
                }
                counted[3] += 1;
                if first_missing.is_none() {
                    first_missing = Some(item.photo_id.clone());
                }
            }
            _ => return Err(unknown()),
        }
        items.push(value);
    }
    let [changed, unchanged, conflict, missing] = counted;
    if (changed, unchanged, conflict, missing)
        != (
            result.counts.changed,
            result.counts.unchanged,
            result.counts.conflict,
            result.counts.missing,
        )
    {
        return Err(unknown());
    }
    let counts = json!({
        "changed": changed,
        "unchanged": unchanged,
        "conflict": conflict,
        "missing": missing,
    });
    let data = json!({ "results": items, "counts": counts });
    if conflict + missing == 0 {
        return Ok(data);
    }
    if changed + unchanged > 0 {
        return Err(CommandFailure::photo_batch_partial(&counts).with_data(data));
    }
    if conflict > 0 {
        let (reference, current_version) =
            first_conflict.expect("a conflicting result was counted");
        return Err(
            CommandFailure::photo_batch_conflict(&reference, &current_version).with_data(data),
        );
    }
    let reference = first_missing.expect("a missing result was counted");
    Err(CommandFailure::photo_batch_missing(&reference).with_data(data))
}

fn confirmed_removal_result(
    identity: &MutationIdentity,
    operation_id: &str,
    prepared: &PreparedRemoval,
    result: PhotoRemovalWire,
) -> Result<Value, CommandFailure> {
    if result.operation_id != operation_id || result.results.len() != prepared.photos.len() {
        return Err(CommandFailure::unknown(identity));
    }
    let submitted = prepared
        .photos
        .iter()
        .map(|photo| photo.photo_id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let mut counts = PhotoRemovalCountsWire {
        removed: 0,
        changed_elsewhere: 0,
        missing: 0,
        already_removed: 0,
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut items = Vec::with_capacity(result.results.len());
    for item in result.results {
        if !submitted.contains(item.photo_id.as_str()) || !seen.insert(item.photo_id.clone()) {
            return Err(CommandFailure::unknown(identity));
        }
        match item.outcome.as_str() {
            "removed" => {
                if item.removed_at_ms.is_none_or(|value| value < 0) {
                    return Err(CommandFailure::unknown(identity));
                }
                counts.removed += 1;
            }
            "changed-elsewhere" => {
                if item.removed_at_ms.is_some() {
                    return Err(CommandFailure::unknown(identity));
                }
                counts.changed_elsewhere += 1;
            }
            "unavailable" => {
                if item.removed_at_ms.is_some() {
                    return Err(CommandFailure::unknown(identity));
                }
                counts.missing += 1;
            }
            "already-removed" => {
                if item.removed_at_ms.is_some() {
                    return Err(CommandFailure::unknown(identity));
                }
                counts.already_removed += 1;
            }
            _ => return Err(CommandFailure::unknown(identity)),
        };
        items.push(json!({
            "photoId": item.photo_id,
            "outcome": item.outcome,
            "removedAtMs": item.removed_at_ms,
        }));
    }
    if seen.len() != submitted.len()
        || counts.removed != result.counts.removed
        || counts.changed_elsewhere != result.counts.changed_elsewhere
        || counts.missing != result.counts.missing
        || counts.already_removed != result.counts.already_removed
    {
        return Err(CommandFailure::unknown(identity));
    }
    let data = json!({
        "operationId": result.operation_id,
        "counts": {
            "removed": counts.removed,
            "changedElsewhere": counts.changed_elsewhere,
            "missing": counts.missing,
            "alreadyRemoved": counts.already_removed,
        },
        "results": items,
    });
    if counts.changed_elsewhere == 0 && counts.missing == 0 {
        return Ok(data);
    }
    let code = if counts.changed_elsewhere > 0 {
        "conflict"
    } else {
        "not_found"
    };
    let message = if code == "conflict" {
        "Read current Photo evidence before submitting a replacement removal."
    } else {
        "Query Photos and use current Photo IDs before submitting a replacement removal."
    };
    let effect = if counts.removed + counts.already_removed > 0 {
        "partial"
    } else {
        "none"
    };
    Err(CommandFailure::from_payload(
        if code == "conflict" { 4 } else { 3 },
        ErrorPayload {
            code: code.to_owned(),
            message: message.to_owned(),
            effect: effect.to_owned(),
            details: json!({"operationId": operation_id}),
        },
    )
    .with_data(data))
}

fn confirmed_restore_result(
    identity: &MutationIdentity,
    operation_id: &str,
    prepared: &PreparedRestore,
    result: PhotoRestoreWire,
) -> Result<Value, CommandFailure> {
    if result.operation_id != operation_id || result.results.len() != prepared.markers.len() {
        return Err(CommandFailure::unknown(identity));
    }
    let submitted = prepared
        .markers
        .iter()
        .map(|marker| marker.photo_id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let mut counts = PhotoRestoreCountsWire {
        restored: 0,
        already_active: 0,
        changed_elsewhere: 0,
        missing: 0,
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut items = Vec::with_capacity(result.results.len());
    for item in result.results {
        if !submitted.contains(item.photo_id.as_str()) || !seen.insert(item.photo_id.clone()) {
            return Err(CommandFailure::unknown(identity));
        }
        match item.outcome.as_str() {
            "restored" => counts.restored += 1,
            "already-active" => counts.already_active += 1,
            "changed-elsewhere" => counts.changed_elsewhere += 1,
            "unavailable" => counts.missing += 1,
            _ => return Err(CommandFailure::unknown(identity)),
        }
        items.push(json!({
            "photoId": item.photo_id,
            "outcome": item.outcome,
        }));
    }
    if seen.len() != submitted.len()
        || counts.restored != result.counts.restored
        || counts.already_active != result.counts.already_active
        || counts.changed_elsewhere != result.counts.changed_elsewhere
        || counts.missing != result.counts.missing
    {
        return Err(CommandFailure::unknown(identity));
    }
    let data = json!({
        "operationId": result.operation_id,
        "counts": {
            "restored": counts.restored,
            "alreadyActive": counts.already_active,
            "changedElsewhere": counts.changed_elsewhere,
            "missing": counts.missing,
        },
        "results": items,
    });
    if counts.changed_elsewhere == 0 && counts.missing == 0 {
        return Ok(data);
    }
    let code = if counts.changed_elsewhere > 0 {
        "conflict"
    } else {
        "not_found"
    };
    let message = if code == "conflict" {
        "Read current Trash evidence before submitting a replacement Restore."
    } else {
        "Query Trash and use current Photo IDs before submitting a replacement Restore."
    };
    let effect = if counts.restored + counts.already_active > 0 {
        "partial"
    } else {
        "none"
    };
    Err(CommandFailure::from_payload(
        if code == "conflict" { 4 } else { 3 },
        ErrorPayload {
            code: code.to_owned(),
            message: message.to_owned(),
            effect: effect.to_owned(),
            details: json!({"operationId": operation_id}),
        },
    )
    .with_data(data))
}

fn restore_wire_value(
    result: PhotoRestoreWire,
    operation: Operation,
) -> Result<Value, CommandFailure> {
    let total = result
        .counts
        .restored
        .checked_add(result.counts.already_active)
        .and_then(|value| value.checked_add(result.counts.changed_elsewhere))
        .and_then(|value| value.checked_add(result.counts.missing));
    if result.operation_id.is_empty() || total != Some(result.results.len()) {
        return Err(CommandFailure::transport(operation));
    }
    let mut ids = std::collections::BTreeSet::new();
    for item in &result.results {
        if item.photo_id.is_empty() || !ids.insert(item.photo_id.as_str()) {
            return Err(CommandFailure::transport(operation));
        }
        if !matches!(
            item.outcome.as_str(),
            "restored" | "already-active" | "changed-elsewhere" | "unavailable"
        ) {
            return Err(CommandFailure::transport(operation));
        }
    }
    serde_json::to_value(result).map_err(|_| CommandFailure::transport(operation))
}
fn removal_wire_value(
    result: PhotoRemovalWire,
    operation: Operation,
) -> Result<Value, CommandFailure> {
    if result.operation_id.is_empty()
        || result.counts.removed
            + result.counts.changed_elsewhere
            + result.counts.missing
            + result.counts.already_removed
            != result.results.len()
    {
        return Err(CommandFailure::transport(operation));
    }
    let mut ids = std::collections::BTreeSet::new();
    for item in &result.results {
        if item.photo_id.is_empty() || !ids.insert(item.photo_id.as_str()) {
            return Err(CommandFailure::transport(operation));
        }
        match item.outcome.as_str() {
            "removed" if item.removed_at_ms.is_some_and(|value| value >= 0) => {}
            "changed-elsewhere" | "unavailable" | "already-removed"
                if item.removed_at_ms.is_none() => {}
            _ => return Err(CommandFailure::transport(operation)),
        }
    }
    serde_json::to_value(result).map_err(|_| CommandFailure::transport(operation))
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
    publication: &PublicationState,
) -> Result<Value, CommandFailure> {
    let operation = command_operation(&cli.command);
    validate_command(&cli.command)?;
    let preview_destination = match &cli.command {
        Command::Photos {
            command: PhotoCommand::Preview { file, .. },
        } => Some(preview_download::Destination::preflight(file)?),
        _ => None,
    };
    let origin = service_origin(cli, environment)?;
    let token_path = access_token_path(cli)?;
    let token = read_access_token(token_path).await?;
    // The complete membership and decision documents validate before any
    // network access, so a local input failure can never depend on service
    // reachability.
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
    let pending_decision = match &cli.command {
        Command::Photos {
            command: PhotoCommand::Set(args),
        } => match &args.input {
            Some(input) => Some(read_decision_input(input).await?),
            None => None,
        },
        _ => None,
    };
    let pending_removal = match &cli.command {
        Command::Photos {
            command: PhotoCommand::Remove(args),
        } => Some(read_removal_input(&args.input).await?),
        _ => None,
    };
    let pending_restore = match &cli.command {
        Command::Photos {
            command: PhotoCommand::Restore(args),
        } => match &args.input {
            Some(input) => Some(read_restore_input(input).await?),
            None => None,
        },
        _ => None,
    };
    let pending_trash_review = match &cli.command {
        Command::Trash {
            command: TrashCommand::Review(args),
        } => {
            let photo_ids = match (&args.all, &args.input) {
                (true, None) => Vec::new(),
                (false, Some(input)) => read_trash_ids(input).await?,
                _ => {
                    return Err(CommandFailure::invalid(
                        "input",
                        "Trash review requires --all or --input, but not both.",
                    ));
                }
            };
            let exclude_photo_ids = match &args.exclude_input {
                Some(input) => read_trash_ids(input).await?,
                None => Vec::new(),
            };
            Some(PendingTrashReview {
                photo_ids,
                exclude_photo_ids,
            })
        }
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
            Command::Library {
                command: LibraryCommand::Check,
            } => {
                let identity = MutationIdentity {
                    operation,
                    photo_ids: Vec::new(),
                    album_id: None,
                    album_name: None,
                };
                let scan: ScanStatus = match client
                    .mutation(
                        &identity,
                        admission,
                        client.endpoint(&["api", "scan"]),
                        json!({}),
                    )
                    .await
                {
                    Ok(scan) => scan,
                    Err(failure) if failure.payload.code == "library_unavailable" => {
                        let data = json!({ "scan": failure.payload.details["scan"] });
                        return Err(failure.with_data(data));
                    }
                    Err(failure) => return Err(failure),
                };
                Ok(json!({ "scan": scan }))
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
                    removed_at_ms,
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
                    removed_at_ms,
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
            Command::Photos {
                command: PhotoCommand::Preview { photo_id, size, .. },
            } => {
                preview_download::download(
                    &client,
                    photo_id,
                    *size,
                    preview_destination.expect("Preview destination was checked"),
                    publication,
                )
                .await
            }
            Command::Photos {
                command: PhotoCommand::Set(args),
            } => {
                // The decision document was validated before connecting; the
                // single-Photo forms are a one-item batch of the same shape.
                let prepared = match pending_decision {
                    Some(prepared) => prepared,
                    None => single_photo_decision(args),
                };
                let identity = MutationIdentity {
                    operation,
                    photo_ids: prepared
                        .photos
                        .iter()
                        .map(|photo| photo.photo_id.clone())
                        .collect(),
                    album_id: None,
                    album_name: None,
                };
                let result: PhotoDecisionWire = client
                    .mutation(
                        &identity,
                        admission,
                        client.endpoint(&["api", "photo-decisions"]),
                        prepared.body(),
                    )
                    .await?;
                confirmed_decision_result(&identity, &prepared, result)
            }
            Command::Photos {
                command: PhotoCommand::Remove(args),
            } => {
                let prepared = pending_removal
                    .as_ref()
                    .expect("removal input was prepared");
                let identity = MutationIdentity {
                    operation,
                    photo_ids: prepared
                        .photos
                        .iter()
                        .map(|photo| photo.photo_id.clone())
                        .collect(),
                    album_id: None,
                    album_name: None,
                };
                let result: PhotoRemovalWire = client
                    .mutation(
                        &identity,
                        admission,
                        client.endpoint(&["api", "photos", "remove-explicit"]),
                        prepared.body(&args.operation_id),
                    )
                    .await?;
                confirmed_removal_result(&identity, &args.operation_id, prepared, result)
            }
            Command::Photos {
                command: PhotoCommand::RemovalOperation { operation_id },
            } => {
                let result: PhotoRemovalWire = client
                    .json(
                        operation,
                        Method::GET,
                        client.endpoint(&["api", "photos", "removal-operations", operation_id]),
                        None,
                    )
                    .await?;
                removal_wire_value(result, operation)
            }
            Command::Photos {
                command: PhotoCommand::RestoreOperation { operation_id },
            } => {
                let result: PhotoRestoreWire = client
                    .json(
                        operation,
                        Method::GET,
                        client.endpoint(&["api", "photos", "restore-operations", operation_id]),
                        None,
                    )
                    .await?;
                restore_wire_value(result, operation)
            }
            Command::Photos {
                command: PhotoCommand::Restore(args),
            } => {
                let photo_ids = pending_restore
                    .as_ref()
                    .map(|prepared| {
                        prepared
                            .markers
                            .iter()
                            .map(|marker| marker.photo_id.clone())
                            .collect()
                    })
                    .unwrap_or_default();
                let identity = MutationIdentity {
                    operation,
                    photo_ids,
                    album_id: None,
                    album_name: None,
                };
                match pending_restore.as_ref() {
                    Some(prepared) => {
                        let result: PhotoRestoreWire = client
                            .mutation(
                                &identity,
                                admission,
                                client.endpoint(&["api", "photos", "restore-explicit"]),
                                prepared.body(&args.operation_id),
                            )
                            .await?;
                        confirmed_restore_result(&identity, &args.operation_id, prepared, result)
                    }
                    None => {
                        let data: Value = client
                            .mutation(
                                &identity,
                                admission,
                                client.endpoint(&["api", "photos", "restore"]),
                                json!({ "operation": args.operation_id }),
                            )
                            .await?;
                        if !data.is_object() {
                            return Err(CommandFailure::unknown(&identity));
                        }
                        Ok(data)
                    }
                }
            }
            Command::Trash {
                command: TrashCommand::List(args),
            } => {
                let mut url = client.endpoint(&["api", "trash"]);
                url.query_pairs_mut()
                    .append_pair("start", &args.start.to_string())
                    .append_pair("limit", &args.limit.to_string());
                let data: Value = client.json(operation, Method::GET, url, None).await?;
                if !data.is_object() {
                    return Err(CommandFailure::transport(operation));
                }
                Ok(data)
            }
            Command::Trash {
                command: TrashCommand::Review(args),
            } => {
                let pending = pending_trash_review
                    .as_ref()
                    .expect("Trash review input was prepared");
                let data: Value = client
                    .json(
                        operation,
                        Method::POST,
                        client.endpoint(&["api", "trash", "review"]),
                        Some(json!({
                            "operationId": args.operation_id,
                            "all": args.all,
                            "photoIds": pending.photo_ids,
                            "excludePhotoIds": pending.exclude_photo_ids,
                        })),
                    )
                    .await?;
                if !data.is_object() {
                    return Err(CommandFailure::transport(operation));
                }
                Ok(data)
            }
            Command::Trash {
                command: TrashCommand::Delete { operation_id },
            } => {
                let identity = MutationIdentity {
                    operation,
                    photo_ids: Vec::new(),
                    album_id: None,
                    album_name: None,
                };
                client
                    .mutation(
                        &identity,
                        admission,
                        client.endpoint(&["api", "trash", "delete"]),
                        json!({ "operationId": operation_id }),
                    )
                    .await
            }
            Command::Trash {
                command: TrashCommand::Operation { operation_id },
            } => {
                let data: Value = client
                    .json(
                        operation,
                        Method::GET,
                        client.endpoint(&["api", "trash", "operations", operation_id]),
                        None,
                    )
                    .await?;
                if !data.is_object() {
                    return Err(CommandFailure::transport(operation));
                }
                Ok(data)
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
            // Attached failure data is server-controlled on the same
            // untrusted path as the payload.
            if let Some(data) = failure.data.as_mut() {
                redact_value(data, &client.token);
            }
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
        Command::Photos {
            command: PhotoCommand::Set(args),
        } if args.input.is_none() => {
            if args.photo_id.is_none() {
                return Err(CommandFailure::invalid(
                    "arguments",
                    "photos set needs PHOTO_ID with --selection or --rating and --if-version, or --input FILE.",
                ));
            }
            if args.selection.is_none() && args.rating.is_none() {
                return Err(CommandFailure::invalid(
                    "arguments",
                    "The single-Photo forms need exactly one of --selection or --rating.",
                ));
            }
            if args.if_version.is_none() {
                return Err(CommandFailure::invalid(
                    "if-version",
                    "The single-Photo forms need the decision version observed by a prior read.",
                ));
            }
            Ok(())
        }
        Command::Trash {
            command: TrashCommand::Review(args),
        } => {
            if args.all == args.input.is_some() {
                return Err(CommandFailure::invalid(
                    "arguments",
                    "trash review needs exactly one of --all or --input.",
                ));
            }
            if args.exclude_input.is_some() && !args.all {
                return Err(CommandFailure::invalid(
                    "exclude-input",
                    "--exclude-input requires --all.",
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
    let publication = PublicationState::default();
    let command = tokio::time::timeout_at(
        deadline,
        execute(&cli, environment, &admission, &publication),
    );
    tokio::pin!(command);
    let (exit_code, envelope) = tokio::select! {
        result = &mut command => match result {
            Ok(Ok(data)) => (0, Envelope::success(data)),
            Ok(Err(failure)) => {
                let envelope = match failure.data {
                    Some(data) if failure.payload.effect == "partial" => {
                        Envelope::partial(*data, failure.payload)
                    }
                    Some(data) => Envelope::error_with_data(*data, failure.payload),
                    None => Envelope::error(failure.payload),
                };
                (failure.exit_code, envelope)
            }
            Err(_) => {
                let failure = match publication.committed() {
                    Some(data) => CommandFailure::published_preview(data, false),
                    None => match admission.admitted() {
                        Some(identity) => CommandFailure::unknown(&identity),
                        None => CommandFailure::transport(operation),
                    },
                };
                let envelope = match failure.data {
                    Some(data) => Envelope::partial(*data, failure.payload),
                    None => Envelope::error(failure.payload),
                };
                (failure.exit_code, envelope)
            }
        },
        _ = tokio::signal::ctrl_c() => {
            if let Some(data) = publication.committed() {
                let failure = CommandFailure::published_preview(data, true);
                (130, Envelope::partial(*failure.data.unwrap(), failure.payload))
            } else {
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
        }
    };
    render_invocation(
        output,
        exit_code,
        &envelope,
        publication
            .committed()
            .and_then(|value| value["path"].as_str().map(str::to_owned)),
    )
}

pub fn invalid_invocation(output: OutputFormat, reason: impl Into<String>) -> InvocationResult {
    let failure = CommandFailure::invalid("arguments", reason);
    render_invocation(
        output,
        failure.exit_code,
        &Envelope::error(failure.payload),
        None,
    )
}

fn render_invocation(
    output: OutputFormat,
    exit_code: u8,
    envelope: &Envelope,
    committed_preview_path: Option<String>,
) -> InvocationResult {
    let stdout = match output {
        OutputFormat::Json => format!(
            "{}\n",
            serde_json::to_string(envelope).expect("envelope serialization is infallible")
        ),
        OutputFormat::Text => render_text(envelope),
    };
    InvocationResult {
        exit_code,
        stdout,
        committed_preview_path,
    }
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
        assert!(Cli::try_parse_from(["slipstream", "library", "check"]).is_ok());
        assert!(Cli::try_parse_from(["slipstream", "library", "check", "extra"]).is_err());
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
            b"".as_slice().to_vec(),
            b"{".as_slice().to_vec(),
            b"[]".as_slice().to_vec(),
            b"\"photoIds\"".as_slice().to_vec(),
            b"{\"photoIds\": []}".as_slice().to_vec(),
            b"{\"photoIds\": [\"\"]}".as_slice().to_vec(),
            b"{\"photoIds\": [\"a\", \"a\"]}".as_slice().to_vec(),
            b"{\"photoIds\": [\"a\"], \"photoIds\": [\"b\"]}"
                .as_slice()
                .to_vec(),
            b"{\"photoIds\": [\"a\"], \"extra\": 1}".as_slice().to_vec(),
            b"{\"photoIds\": [\"a\"]} trailing".as_slice().to_vec(),
            b"{\"photoIds\": [1]}".as_slice().to_vec(),
            b"\xff\xfe{\"photoIds\": [\"a\"]}".as_slice().to_vec(),
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

    #[test]
    fn photo_decision_parser_requires_one_complete_form() {
        let single = |arguments: &[&str]| {
            let mut invocation = vec!["slipstream", "photos", "set"];
            invocation.extend(arguments);
            Cli::try_parse_from(invocation)
        };
        assert!(single(&["p1", "--selection", "selected", "--if-version", "v"]).is_ok());
        assert!(single(&["p1", "--selection", "undecided", "--if-version", "v"]).is_ok());
        assert!(single(&["p1", "--rating", "3", "--if-version", "v"]).is_ok());
        assert!(single(&["p1", "--rating", "0", "--if-version", "v"]).is_ok());
        assert!(single(&["--input", "decisions.json"]).is_ok());
        assert!(single(&["--input", "-"]).is_ok());
        // Selection and Rating cannot change in the same command, and the
        // batch form cannot be mixed with the single-Photo options.
        assert!(
            single(&[
                "p1",
                "--selection",
                "selected",
                "--rating",
                "3",
                "--if-version",
                "v"
            ])
            .is_err()
        );
        assert!(single(&["--input", "d.json", "--if-version", "v"]).is_err());
        assert!(single(&["--input", "d.json", "p1"]).is_err());
        assert!(single(&["--input", "d.json", "--selection", "selected"]).is_err());
        // Unknown values and out-of-range ratings are parser errors.
        assert!(single(&["p1", "--selection", "all", "--if-version", "v"]).is_err());
        assert!(single(&["p1", "--rating", "6", "--if-version", "v"]).is_err());
        assert!(single(&["p1", "--rating", "-1", "--if-version", "v"]).is_err());
        assert!(single(&["--if-version", "v"]).is_err());
        // The single-Photo forms must be complete before any network access.
        let missing_field = single(&["p1", "--if-version", "v"]).expect("the shape parses");
        assert!(validate_command(&missing_field.command).is_err());
        let missing_version = single(&["p1", "--selection", "selected"]).expect("the shape parses");
        assert!(validate_command(&missing_version.command).is_err());
    }

    #[test]
    fn decision_input_validates_the_complete_document() {
        let id = |index: usize| format!("00000000-0000-4000-8000-{index:012x}");
        let document = |field: Value, value: Value, photos: Value| {
            serde_json::to_vec(&json!({ "field": field, "value": value, "photos": photos }))
                .unwrap()
        };
        let items = |count: usize| {
            (0..count)
                .map(|index| json!({ "photoId": id(index), "ifVersion": "v1" }))
                .collect::<Vec<_>>()
        };
        let parsed = |bytes: Vec<u8>| parse_decision_input(bytes).map(|prepared| prepared.body());
        assert_eq!(
            parsed(document(
                "selectionState".into(),
                "rejected".into(),
                json!(items(2))
            ))
            .unwrap(),
            json!({
                "field": "selectionState",
                "value": "rejected",
                "photos": [
                    { "photoId": id(0), "ifVersion": "v1" },
                    { "photoId": id(1), "ifVersion": "v1" }
                ]
            })
        );
        assert!(parsed(document("rating".into(), 4.into(), json!(items(1)))).is_ok());
        assert!(parsed(document("rating".into(), 0.into(), json!(items(1)))).is_ok());

        let invalid = |bytes: Vec<u8>, argument: &str| {
            let failure = parse_decision_input(bytes).unwrap_err();
            assert_eq!(failure.exit_code, 2);
            assert_eq!(failure.payload.code, "invalid_input");
            assert_eq!(failure.payload.details["argument"], argument);
        };
        for (bytes, argument) in [
            (b"".as_slice().to_vec(), "input"),
            (b"{".as_slice().to_vec(), "input"),
            (b"[]".as_slice().to_vec(), "input"),
            (b"\"x\"".as_slice().to_vec(), "input"),
            (b"\xff\xfe{}".as_slice().to_vec(), "input"),
            (b"{\"field\": \"rating\"}".as_slice().to_vec(), "input"),
            (
                b"{\"field\": \"rating\", \"value\": 4, \"photos\": [], \"extra\": 1}".as_slice().to_vec(),
                "input",
            ),
            (
                b"{\"field\": \"rating\", \"field\": \"rating\", \"value\": 4, \"photos\": []}".as_slice().to_vec(),
                "input",
            ),
            (
                b"{\"field\": \"rating\", \"value\": 4, \"photos\": [{\"photoId\": \"a\", \"ifVersion\": \"v\", \"ifVersion\": \"w\"}]}".as_slice().to_vec(),
                "input",
            ),
            (
                b"{\"field\": \"rating\", \"value\": 4, \"photos\": [{\"photoId\": 1, \"ifVersion\": \"v\"}]}".as_slice().to_vec(),
                "input",
            ),
            (
                b"{\"field\": \"rating\", \"value\": 4, \"photos\": []} trailing".as_slice().to_vec(),
                "input",
            ),
            (document("selection".into(), "selected".into(), json!(items(1))), "field"),
            (document(5.into(), "selected".into(), json!(items(1))), "input"),
            (document("rating".into(), "4".into(), json!(items(1))), "value"),
            (document("rating".into(), 4.5.into(), json!(items(1))), "value"),
            (document("rating".into(), 6.into(), json!(items(1))), "value"),
            (document("rating".into(), (-1).into(), json!(items(1))), "value"),
            (
                document("selectionState".into(), 1.into(), json!(items(1))),
                "value",
            ),
            (
                document("selectionState".into(), "picked".into(), json!(items(1))),
                "value",
            ),
            (
                document("rating".into(), 4.into(), serde_json::json!([])),
                "photos",
            ),
            (
                document(
                    "rating".into(),
                    4.into(),
                    json!([
                        { "photoId": id(0), "ifVersion": "v1" },
                        { "photoId": id(0), "ifVersion": "v2" }
                    ])
                ),
                "photos",
            ),
            (
                document("rating".into(), 4.into(), json!([{ "photoId": "", "ifVersion": "v1" }])),
                "photos",
            ),
            (
                document("rating".into(), 4.into(), json!([{ "photoId": id(0), "ifVersion": "" }])),
                "photos",
            ),
        ] {
            invalid(bytes.to_vec(), argument);
        }

        let over_limit = parse_decision_input(document(
            "rating".into(),
            4.into(),
            json!(items(MAXIMUM_MUTATION_PHOTO_IDS + 1)),
        ))
        .unwrap_err();
        assert_eq!(over_limit.exit_code, 2);
        assert_eq!(over_limit.payload.code, "limit_exceeded");
        assert_eq!(
            over_limit.payload.details,
            json!({
                "limitName": "photoIds",
                "limit": MAXIMUM_MUTATION_PHOTO_IDS,
                "actual": MAXIMUM_MUTATION_PHOTO_IDS + 1,
            })
        );
    }

    #[test]
    fn decision_batches_partition_from_validated_results_only() {
        let identity = MutationIdentity {
            operation: Operation::PhotosSet,
            photo_ids: vec!["a".to_owned(), "b".to_owned()],
            album_id: None,
            album_name: None,
        };
        let submitted = |ids: &[&str]| {
            ids.iter()
                .map(|id| DecisionTarget {
                    photo_id: (*id).to_owned(),
                    if_version: "v1".to_owned(),
                })
                .collect::<Vec<_>>()
        };
        let request = |ids: &[&str]| PreparedDecision {
            field: DecisionField::Rating,
            value: json!(4),
            photos: submitted(ids),
        };
        let facts = |selection: &str, rating: u8| PhotoDecisionFactsWire {
            selection_state: serde_json::from_value::<SelectionState>(json!(selection)).unwrap(),
            rating,
        };
        let snapshot = |selection: &str, rating: u8, version: &str| PhotoDecisionSnapshotWire {
            selection_state: serde_json::from_value::<SelectionState>(json!(selection)).unwrap(),
            rating,
            decision_version: version.to_owned(),
        };
        let changed = |id: &str, version: &str| PhotoDecisionItemWire {
            photo_id: id.to_owned(),
            outcome: "changed".to_owned(),
            prior: Some(facts("undecided", 0)),
            current: Some(snapshot("selected", 4, version)),
        };
        let unchanged = |id: &str, version: &str| PhotoDecisionItemWire {
            photo_id: id.to_owned(),
            outcome: "unchanged".to_owned(),
            prior: None,
            current: Some(snapshot("undecided", 4, version)),
        };
        let conflict = |id: &str, version: &str| PhotoDecisionItemWire {
            photo_id: id.to_owned(),
            outcome: "conflict".to_owned(),
            prior: None,
            current: Some(snapshot("rejected", 2, version)),
        };
        let missing = |id: &str| PhotoDecisionItemWire {
            photo_id: id.to_owned(),
            outcome: "missing".to_owned(),
            prior: None,
            current: None,
        };
        let wire = |results: Vec<PhotoDecisionItemWire>, counts: [usize; 4]| PhotoDecisionWire {
            results,
            counts: PhotoDecisionCountsWire {
                changed: counts[0],
                unchanged: counts[1],
                conflict: counts[2],
                missing: counts[3],
            },
        };

        // Only changed or unchanged results keep status ok and exit 0.
        let confirmed = confirmed_decision_result(
            &identity,
            &request(&["a", "b"]),
            wire(vec![changed("a", "v2"), conflict("b", "v9")], [1, 0, 1, 0]),
        )
        .unwrap_err();
        assert_eq!(confirmed.exit_code, 5);
        assert_eq!(confirmed.payload.code, "partial_result");
        assert_eq!(confirmed.payload.effect, "partial");
        assert_eq!(
            confirmed.payload.details,
            json!({ "counts": { "changed": 1, "unchanged": 0, "conflict": 1, "missing": 0 } })
        );
        assert_eq!(
            confirmed.data.as_ref().unwrap()["results"]
                .as_array()
                .unwrap()
                .len(),
            2
        );

        let (code, exit, reference) = {
            let failure = confirmed_decision_result(
                &identity,
                &request(&["a", "b"]),
                wire(vec![conflict("a", "v8"), conflict("b", "v9")], [0, 0, 2, 0]),
            )
            .unwrap_err();
            assert_eq!(failure.exit_code, 4);
            assert_eq!(failure.payload.effect, "none");
            assert_eq!(
                failure.payload.details,
                json!({ "resource": "photo", "reference": "a", "currentVersion": "v8" })
            );
            assert_eq!(failure.data.as_ref().unwrap()["counts"]["conflict"], 2);
            (
                failure.payload.code.clone(),
                failure.exit_code,
                failure.payload.details["reference"].clone(),
            )
        };
        assert_eq!(
            (code.as_str(), exit, reference.as_str().unwrap()),
            ("conflict", 4, "a")
        );

        let missing_failure = confirmed_decision_result(
            &identity,
            &request(&["a", "b"]),
            wire(vec![missing("a"), missing("b")], [0, 0, 0, 2]),
        )
        .unwrap_err();
        assert_eq!(missing_failure.exit_code, 3);
        assert_eq!(missing_failure.payload.code, "not_found");
        assert_eq!(
            missing_failure.payload.details,
            json!({ "resource": "photo", "reference": "a" })
        );

        let ok = confirmed_decision_result(
            &identity,
            &request(&["a", "b"]),
            wire(vec![changed("a", "v2"), changed("b", "v3")], [2, 0, 0, 0]),
        )
        .unwrap();
        assert_eq!(
            ok["counts"],
            json!({ "changed": 2, "unchanged": 0, "conflict": 0, "missing": 0 })
        );
        assert_eq!(
            ok["results"][0]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            ["current", "outcome", "photoId", "prior"]
        );

        let ok_mixed = confirmed_decision_result(
            &identity,
            &request(&["a", "b"]),
            wire(vec![changed("a", "v2"), unchanged("b", "v2")], [1, 1, 0, 0]),
        )
        .unwrap();
        assert_eq!(
            ok_mixed["counts"],
            json!({ "changed": 1, "unchanged": 1, "conflict": 0, "missing": 0 })
        );

        for (label, wire_result) in [
            (
                "reordered results",
                wire(vec![changed("b", "v2"), changed("a", "v3")], [2, 0, 0, 0]),
            ),
            (
                "counts do not match outcomes",
                wire(vec![changed("a", "v2"), conflict("b", "v9")], [2, 0, 0, 0]),
            ),
            (
                "short result array",
                wire(vec![changed("a", "v2")], [1, 0, 0, 0]),
            ),
            (
                "changed without prior",
                PhotoDecisionWire {
                    results: vec![PhotoDecisionItemWire {
                        photo_id: "a".to_owned(),
                        outcome: "changed".to_owned(),
                        prior: None,
                        current: Some(snapshot("selected", 3, "v2")),
                    }],
                    counts: PhotoDecisionCountsWire {
                        changed: 1,
                        unchanged: 0,
                        conflict: 0,
                        missing: 0,
                    },
                },
            ),
            (
                "changed current does not echo the requested value",
                wire(
                    vec![PhotoDecisionItemWire {
                        photo_id: "a".to_owned(),
                        outcome: "changed".to_owned(),
                        prior: Some(facts("undecided", 0)),
                        current: Some(snapshot("selected", 3, "v2")),
                    }],
                    [1, 0, 0, 0],
                ),
            ),
            (
                "unchanged current does not echo the requested value",
                wire(
                    vec![PhotoDecisionItemWire {
                        photo_id: "a".to_owned(),
                        outcome: "unchanged".to_owned(),
                        prior: None,
                        current: Some(snapshot("undecided", 2, "v2")),
                    }],
                    [0, 1, 0, 0],
                ),
            ),
        ] {
            let failure = confirmed_decision_result(&identity, &request(&["a", "b"]), wire_result)
                .unwrap_err();
            assert_eq!(failure.exit_code, 7, "for {label}");
            assert_eq!(failure.payload.code, "outcome_unknown", "for {label}");
            assert_eq!(failure.payload.details["operation"], "photos-set");
        }
    }
}
