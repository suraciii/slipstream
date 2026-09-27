use super::*;
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
    /// Discover admitted processing stages, source profiles, and controls.
    Processing {
        #[command(subcommand)]
        command: ProcessingCommand,
    },
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
pub enum ProcessingCommand {
    /// Read the service's current processing capability report.
    Capability,
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
    pub(crate) album_id: String,
    /// UTF-8 JSON file holding one ordered `photoIds` array of at most 100
    /// distinct Photo IDs; `-` reads the document from stdin.
    #[arg(long, value_name = "FILE", value_parser = nonempty)]
    pub(crate) input: String,
    /// Album version observed by a prior read.
    #[arg(long, value_name = "VERSION", value_parser = nonempty)]
    pub(crate) if_version: String,
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
    /// Read, save, or explicitly rebind one Photo's Edit Recipe.
    Recipe {
        #[command(subcommand)]
        command: development::RecipeCommand,
    },
    /// Request a Develop Edit Preview or download its ready rendition.
    EditPreview(edit_preview_download::EditPreviewArgs),
    /// Read one Photo's standard metadata with provenance, capture facts,
    /// association status, and the evidence a checked Save requires.
    Metadata {
        #[arg(value_parser = nonempty)]
        photo_id: String,
    },
    /// Save explicit field changes to the Photo's associated XMP Sidecar
    /// against the evidence observed by a prior Read. The input document
    /// cannot name a filesystem path.
    MetadataSave(PhotoMetadataSaveArgs),
    /// Submit, inspect, and download a developed Photo Export.
    Export {
        #[command(subcommand)]
        command: PhotoExportCommand,
    },
}

#[derive(Debug, Args)]
pub struct PhotoMetadataSaveArgs {
    #[arg(value_name = "PHOTO_ID", value_parser = nonempty)]
    pub photo_id: String,
    /// UTF-8 JSON file holding one complete save document
    /// {"evidence":{...},"changes":{...}}; `-` reads it from stdin.
    #[arg(long, value_name = "FILE", value_parser = nonempty)]
    pub input: String,
}

#[derive(Debug, Subcommand)]
pub enum PhotoExportCommand {
    /// Submit one Export against the Edit Recipe and source revision
    /// observed by this command, using the caller's request identity.
    Submit(PhotoExportSubmitArgs),
    /// List a Photo's retained Exports in retention order.
    List {
        #[arg(value_name = "PHOTO_ID", value_parser = nonempty)]
        photo_id: String,
    },
    /// Inspect one Export's current state and retained artifact facts.
    Status {
        #[arg(value_name = "EXPORT_ID", value_parser = nonempty)]
        export_id: String,
    },
    /// Download one settled Export's artifact to a new local file.
    Download(PhotoExportDownloadArgs),
}

#[derive(Debug, Args)]
pub struct PhotoExportSubmitArgs {
    #[arg(value_name = "PHOTO_ID", value_parser = nonempty)]
    pub photo_id: String,
    /// Developed output target of the Export.
    #[arg(long, value_enum)]
    pub target: ExportTargetArg,
    /// Caller-generated exactly-once identity: 1 through 128 characters of
    /// ASCII letters, digits, '.', '_', or '-'.
    #[arg(long, value_name = "REQUEST_ID", value_parser = nonempty)]
    pub request_id: String,
}

#[derive(Debug, Args)]
pub struct PhotoExportDownloadArgs {
    #[arg(value_name = "EXPORT_ID", value_parser = nonempty)]
    pub export_id: String,
    /// New local file path; an existing file or symbolic link is never replaced.
    #[arg(long, value_name = "PATH", required = true)]
    pub file: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum ExportTargetArg {
    /// Full-resolution float32 RGB TIFF of the saved Edit Recipe.
    DevelopmentTiff,
    /// Finished JPEG rendered through the qualified film pipeline.
    FilmJpeg,
}

impl ExportTargetArg {
    pub(crate) fn wire(self) -> &'static str {
        match self {
            Self::DevelopmentTiff => "development-tiff",
            Self::FilmJpeg => "film-jpeg",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "development-tiff" => Some(Self::DevelopmentTiff),
            "film-jpeg" => Some(Self::FilmJpeg),
            _ => None,
        }
    }
}

/// One caller-generated mutation or attempt identity, shared with the
/// service's own closed request-identity rule.
pub(crate) fn valid_request_identity(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
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

pub(crate) fn nonempty(value: &str) -> Result<String, String> {
    (!value.is_empty())
        .then(|| value.to_owned())
        .ok_or_else(|| "must not be empty".to_owned())
}

pub(crate) fn album_name(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        Err("must not be empty or only whitespace".to_owned())
    } else if trimmed.chars().count() > 120 {
        Err("must be at most 120 characters".to_owned())
    } else {
        Ok(value.to_owned())
    }
}

pub(crate) fn page_limit(value: &str) -> Result<u8, String> {
    value
        .parse::<u8>()
        .ok()
        .filter(|value| (1..=60).contains(value))
        .ok_or_else(|| "must be an integer from 1 through 60".to_owned())
}

pub(crate) fn rating(value: &str) -> Result<u8, String> {
    value
        .parse::<u8>()
        .ok()
        .filter(|value| *value <= 5)
        .ok_or_else(|| "must be an integer from 0 through 5".to_owned())
}

pub(crate) fn exact_bool(value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err("must be true or false".to_owned()),
    }
}

pub(crate) fn local_time(value: &str) -> Result<String, String> {
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
