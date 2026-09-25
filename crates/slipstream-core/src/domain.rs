use crate::capture::CaptureFact;
use std::{fmt, sync::Arc};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OriginalKind {
    Raw,
    Jpeg,
}

impl OriginalKind {
    /// The Preview Source this Original kind always uses. A JPEG Original is
    /// its own Preview; a RAW Original uses its largest usable embedded JPEG.
    pub fn preview_source(self) -> PreviewSource {
        match self {
            Self::Raw => PreviewSource::RawEmbeddedJpeg,
            Self::Jpeg => PreviewSource::JpegOriginal,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewSource {
    JpegOriginal,
    RawEmbeddedJpeg,
}

impl PreviewSource {
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::JpegOriginal => "jpeg-original",
            Self::RawEmbeddedJpeg => "raw-embedded-jpeg",
        }
    }

    pub fn parse_wire_name(value: &str) -> Option<Self> {
        match value {
            "jpeg-original" => Some(Self::JpegOriginal),
            "raw-embedded-jpeg" => Some(Self::RawEmbeddedJpeg),
            _ => None,
        }
    }

    /// The historical v5 persistence name for this source, used only when
    /// reading legacy rows during the v5-to-v6 migration.
    pub fn legacy_database_name(self) -> &'static str {
        match self {
            Self::JpegOriginal => "matching-jpeg",
            Self::RawEmbeddedJpeg => "embedded-raw-jpeg",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewState {
    InspectionPending,
    Ready,
    Failed,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SelectionState {
    Undecided,
    Selected,
    Rejected,
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RelativeOriginalPath(Arc<str>);

impl RelativeOriginalPath {
    pub fn parse(value: impl Into<String>) -> Result<Self, PathError> {
        let value = value.into();
        if value.is_empty()
            || value.starts_with('/')
            || value.contains('\0')
            || value
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(PathError);
        }
        Ok(Self(Arc::from(value)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RelativeOriginalPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OriginalFacts {
    pub size: u64,
    pub mtime_ms: f64,
    pub device: u64,
    pub inode: u64,
}

impl OriginalFacts {
    pub const UNREADABLE: Self = Self {
        size: 0,
        mtime_ms: 0.0,
        device: 0,
        inode: 0,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OriginalErrorCategory {
    Unreadable,
    Changed,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiscoveredOriginal {
    pub path: RelativeOriginalPath,
    pub kind: OriginalKind,
    pub facts: OriginalFacts,
    pub error_category: Option<OriginalErrorCategory>,
    pub error_message: Option<String>,
    pub capture: CaptureFact,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OriginalScanError {
    pub path: RelativeOriginalPath,
    pub kind: OriginalKind,
    pub category: OriginalErrorCategory,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScanResult {
    pub originals: Vec<DiscoveredOriginal>,
    pub errors: Vec<OriginalScanError>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OriginalRecord {
    pub id: String,
    pub relative_path: RelativeOriginalPath,
    pub kind: OriginalKind,
    pub facts: OriginalFacts,
    pub available: bool,
    pub error_category: Option<OriginalErrorCategory>,
    pub error_message: Option<String>,
    pub capture: CaptureFact,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PhotoRecord {
    pub id: String,
    pub original_id: String,
    pub available: bool,
    pub preview_state: PreviewState,
    pub preview_source_revision: Option<String>,
    pub preview_width: Option<u32>,
    pub preview_height: Option<u32>,
    pub cache_revision: Option<String>,
    pub sort_path: String,
    pub selection_state: SelectionState,
    pub rating: u8,
    pub has_saved_edits: bool,
}

/// One persisted content fingerprint for an Original File at one observed
/// revision. A fingerprint is evidence for Location recovery; it is not
/// Original File or Photo identity, and equal digests at multiple Locations
/// remain independent Originals.
#[derive(Clone, Debug, PartialEq)]
pub struct OriginalFingerprint {
    pub original_id: String,
    pub digest: String,
    pub size: u64,
    pub mtime_ms: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScanSnapshot {
    pub published: bool,
    pub originals: Vec<OriginalRecord>,
    pub photos: Vec<PhotoRecord>,
    pub errors: Vec<OriginalScanError>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AlbumMember {
    pub photo_id: String,
    pub position: u32,
    pub available: bool,
    pub selection_state: SelectionState,
    pub rating: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AlbumRecord {
    pub id: String,
    pub name: String,
    pub last_reviewed_photo_id: Option<String>,
    pub members: Vec<AlbumMember>,
}

/// Bounded Album summary: per-Album facts without member materialization.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlbumSummary {
    pub id: String,
    pub name: String,
    pub photo_count: usize,
    pub has_saved_position: bool,
    /// Process-epoch mutation guard for the Album name and ordered membership.
    pub album_version: String,
}

/// Current persisted Photo facts returned with their decision mutation guard.
/// The guard and the guarded Selection State and Rating are read by one
/// serialized persistence-owner command.
#[derive(Clone, Debug, PartialEq)]
pub struct PhotoRead {
    pub id: String,
    pub filename: String,
    pub original_kind: OriginalKind,
    pub original_available: bool,
    pub selection_state: SelectionState,
    pub rating: u8,
    pub decision_version: String,
    pub capture: CaptureFact,
    pub preview_state: PreviewState,
    pub preview_source: Option<PreviewSource>,
    pub preview_source_revision: Option<String>,
    pub preview_width: Option<u32>,
    pub preview_height: Option<u32>,
    pub has_saved_edits: bool,
}

/// White-balance intent stored by the engine-independent state layer. The
/// closed payload bounds are published independent of admission: `as-shot`
/// is the only mode the qualified capability admits for execution, while a
/// stored `temperature-tint` value is retained editing intent that reads
/// back and renders but is never executed until the capability report
/// admits the mode again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WhiteBalanceIntent {
    AsShot,
    TemperatureTint {
        /// 1,000 through 40,000 Kelvin.
        temperature_kelvin: i32,
        /// -150,000 through 150,000 thousandths of the green–magenta unit.
        tint_milli: i32,
    },
}

impl WhiteBalanceIntent {
    /// The shared wire mode name of this intent.
    pub fn mode_name(self) -> &'static str {
        match self {
            Self::AsShot => "as-shot",
            Self::TemperatureTint { .. } => "temperature-tint",
        }
    }

    /// True while the intent stays within the closed payload bounds, so a
    /// persisted value can never leave the published shape.
    pub fn within_payload_bounds(self) -> bool {
        match self {
            Self::AsShot => true,
            Self::TemperatureTint {
                temperature_kelvin,
                tint_milli,
            } => {
                (1_000..=40_000).contains(&temperature_kelvin)
                    && (-150_000..=150_000).contains(&tint_milli)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditRecipeSettings {
    pub exposure_ev: f64,
    pub white_balance: WhiteBalanceIntent,
}

/// The one saved recipe for a Photo and the Library source revision to which
/// it is currently bound.
#[derive(Clone, Debug, PartialEq)]
pub struct EditRecipe {
    pub photo_id: String,
    pub revision: String,
    pub source_revision: String,
    pub settings: EditRecipeSettings,
}

/// Current recipe and source facts from one serialized persistence read.
#[derive(Clone, Debug, PartialEq)]
pub struct EditRecipeRead {
    pub recipe: Option<EditRecipe>,
    pub current_source_revision: String,
    pub source_available: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SaveEditRecipe {
    pub photo_id: String,
    /// Stable caller-owned identity used to resolve a retry after a lost
    /// response. It is not the recipe version and must not be regenerated
    /// while retrying one save.
    pub request_id: String,
    pub expected_recipe_version: Option<String>,
    pub expected_source_revision: String,
    pub settings: EditRecipeSettings,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RebindEditRecipe {
    pub photo_id: String,
    /// The caller-owned identity that makes one rebind idempotent, exactly
    /// like a save identity.
    pub request_id: String,
    pub expected_recipe_version: String,
    pub new_source_revision: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EditRecipeWriteOutcome {
    /// A fresh commit installed a new recipe version inside the write
    /// transaction.
    Saved(EditRecipe),
    /// A receipt replay of a committed write: nothing was written, and the
    /// carried recipe is the committed receipt, whatever advanced since.
    Replayed(EditRecipe),
    Unchanged(EditRecipe),
    Conflict(EditRecipeRead),
    SourceChanged(EditRecipeRead),
    RequiresRebind(EditRecipeRead),
    /// The request identity was already used with a different payload.
    RequestConflict,
    MissingPhoto,
    MissingRecipe,
    UnsupportedPhoto,
    Unavailable,
    InvalidSettings,
}

/// A validated camera-local query boundary. It deliberately contains no
/// timezone: persisted Capture Time ordering compares camera-local values.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureTimeBound(String);

impl CaptureTimeBound {
    pub fn parse(value: impl Into<String>) -> Result<Self, PhotoQueryError> {
        let value = value.into();
        let bytes = value.as_bytes();
        let shape = bytes.len() == 19
            && bytes[4] == b'-'
            && bytes[7] == b'-'
            && bytes[10] == b'T'
            && bytes[13] == b':'
            && bytes[16] == b':'
            && bytes.iter().enumerate().all(|(index, byte)| {
                matches!(index, 4 | 7 | 10 | 13 | 16) || byte.is_ascii_digit()
            });
        if !shape {
            return Err(PhotoQueryError::Invalid);
        }
        let number = |range: std::ops::Range<usize>| {
            value[range]
                .parse::<u32>()
                .map_err(|_| PhotoQueryError::Invalid)
        };
        let year = number(0..4)?;
        let month = number(5..7)?;
        let day = number(8..10)?;
        let hour = number(11..13)?;
        let minute = number(14..16)?;
        let second = number(17..19)?;
        let leap =
            year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
        let days = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if leap => 29,
            2 => 28,
            _ => return Err(PhotoQueryError::Invalid),
        };
        if year == 0 || day == 0 || day > days || hour > 23 || minute > 59 || second > 59 {
            return Err(PhotoQueryError::Invalid);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PhotoQuerySource {
    AllPhotos,
    Album(String),
    /// A Library-relative Original Folder Location. The empty string denotes
    /// the Library Folder; descendants are selected component-wise.
    Folder(String),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhotoQueryOrder {
    CaptureTimeAscending,
    CaptureTimeDescending,
    AlbumOrder,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhotoQuery {
    pub source: PhotoQuerySource,
    pub selection_state: Option<SelectionState>,
    pub rating_minimum: Option<u8>,
    pub rating_maximum: Option<u8>,
    pub original_kind: Option<OriginalKind>,
    pub original_available: Option<bool>,
    pub captured_from: Option<CaptureTimeBound>,
    pub captured_before: Option<CaptureTimeBound>,
    pub order: PhotoQueryOrder,
}

/// Scan-owned query facts from one Published Library. The server shares one
/// immutable projection per publication; the persistence owner combines these
/// facts with current decisions and current Album membership.
#[derive(Clone, Debug, PartialEq)]
pub struct PhotoQueryCandidate {
    pub photo_id: String,
    pub relative_path: String,
    pub sort_path: String,
    pub original_kind: OriginalKind,
    pub original_available: bool,
    pub capture: CaptureFact,
    pub preview_state: PreviewState,
    pub preview_source_revision: Option<String>,
    pub preview_width: Option<u32>,
    pub preview_height: Option<u32>,
}

impl PhotoQueryCandidate {
    pub fn capture_order_key(&self) -> Option<&str> {
        self.capture.order_key.as_deref()
    }
}

#[derive(Debug)]
pub struct PhotoQueryProjection {
    capture_time_ascending: Vec<PhotoQueryCandidate>,
    capture_time_descending: Vec<usize>,
    by_id: std::collections::HashMap<String, usize>,
}

impl PhotoQueryProjection {
    pub fn new(
        capture_time_ascending: Vec<PhotoQueryCandidate>,
        capture_time_descending: Vec<usize>,
    ) -> Option<Self> {
        if capture_time_descending.len() != capture_time_ascending.len()
            || capture_time_descending
                .iter()
                .any(|index| *index >= capture_time_ascending.len())
        {
            return None;
        }
        let by_id = capture_time_ascending
            .iter()
            .enumerate()
            .map(|(index, candidate)| (candidate.photo_id.clone(), index))
            .collect::<std::collections::HashMap<_, _>>();
        if by_id.len() != capture_time_ascending.len()
            || capture_time_descending
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != capture_time_ascending.len()
        {
            return None;
        }
        Some(Self {
            capture_time_ascending,
            capture_time_descending,
            by_id,
        })
    }

    pub fn ascending(&self) -> &[PhotoQueryCandidate] {
        &self.capture_time_ascending
    }

    pub fn descending(&self) -> impl Iterator<Item = &PhotoQueryCandidate> {
        self.capture_time_descending
            .iter()
            .map(|index| &self.capture_time_ascending[*index])
    }

    pub fn get(&self, photo_id: &str) -> Option<&PhotoQueryCandidate> {
        self.by_id
            .get(photo_id)
            .map(|index| &self.capture_time_ascending[*index])
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AlbumQueryFilter {
    All,
    ExactName(String),
    ContainsPhoto(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PhotoQueryError {
    Invalid,
    SourceNotFound,
    ResultLimitExceeded { limit: usize },
    Storage,
}

impl fmt::Display for PhotoQueryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid => formatter.write_str("Photo query is not valid"),
            Self::SourceNotFound => formatter.write_str("Photo query source was not found"),
            Self::ResultLimitExceeded { limit } => {
                write!(
                    formatter,
                    "Photo query exceeds the retained-ID limit of {limit}"
                )
            }
            Self::Storage => formatter.write_str("Photo query could not read persisted state"),
        }
    }
}

impl std::error::Error for PhotoQueryError {}

/// One Album that contains a Photo, for the bounded per-Photo membership
/// query. It carries Album identity only, never member lists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhotoAlbumMembership {
    pub album_id: String,
    pub album_name: String,
}

/// Ordered Album membership identity for Browse Snapshot construction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlbumBrowseMember {
    pub photo_id: String,
    pub available: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlbumBrowseTarget {
    pub members: Vec<AlbumBrowseMember>,
    pub saved_photo_id: Option<String>,
}

/// Maximum number of Photos admitted by one server-resolved Folder add.
/// Keeping the operation bounded prevents one request from monopolizing the
/// SQLite owner while still covering a substantial ordinary shoot folder.
pub const MAXIMUM_FOLDER_ALBUM_PHOTOS: usize = 100_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AlbumMutation {
    Create {
        name: String,
    },
    Rename {
        album_id: String,
        name: String,
    },
    Delete {
        album_id: String,
    },
    AddMembers {
        album_id: String,
        photo_ids: Vec<String>,
    },
    /// Adds the server-resolved Photos for one validated Original Folder.
    ///
    /// The HTTP layer resolves the Folder against one Published Library and
    /// supplies the resulting ordered identities. Keeping this as a distinct
    /// mutation preserves the small 100-member limit on the ordinary API
    /// while allowing one atomic folder operation.
    AddFolderMembers {
        album_id: String,
        photo_ids: Vec<String>,
    },
    RemoveMember {
        album_id: String,
        photo_id: String,
    },
    Reorder {
        album_id: String,
        photo_ids: Vec<String>,
    },
    SetProgress {
        album_id: String,
        photo_id: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlbumMutationResult {
    pub album_id: String,
    pub added_count: usize,
    pub already_member_count: usize,
}

/// A version-checked Album change for machine clients. Existing browser
/// mutations remain separate so they cannot accidentally bypass this guard.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CheckedAlbumMutation {
    Rename {
        album_id: String,
        name: String,
        expected_version: String,
    },
    Delete {
        album_id: String,
        expected_version: String,
    },
    AddMembers {
        album_id: String,
        photo_ids: Vec<String>,
        expected_version: String,
    },
    RemoveMembers {
        album_id: String,
        photo_ids: Vec<String>,
        expected_version: String,
    },
    Reorder {
        album_id: String,
        photo_ids: Vec<String>,
        expected_version: String,
    },
}

/// Confirmed facts from one checked Album commit. Every summary and version is
/// from the same serialized owner operation as the mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CheckedAlbumMutationResult {
    Renamed {
        album: AlbumSummary,
        renamed: bool,
    },
    Deleted {
        album_id: String,
    },
    Added {
        album: AlbumSummary,
        added_photo_ids: Vec<String>,
        already_member_photo_ids: Vec<String>,
    },
    Removed {
        album: AlbumSummary,
        removed_photo_ids: Vec<String>,
        already_absent_photo_ids: Vec<String>,
        saved_photo_id: Option<String>,
    },
    Reordered {
        album: AlbumSummary,
        ordered_photo_ids: Vec<String>,
        reordered: bool,
    },
}

/// Confirmed creation result, including the first process-epoch Album version.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlbumCreationResult {
    pub album: AlbumSummary,
}

/// The largest number of Photo identities admitted by one ordinary Album
/// membership batch. The browser uses the same bound for its compensation
/// record, so a retry cannot grow into an unbounded SQLite transaction.
pub const ALBUM_MEMBERSHIP_BATCH_MAX: usize = 100;

/// A bounded, identity-bearing Album membership operation. The add and
/// compensation remove paths are separate from the generic Album mutation
/// result because the browser must know exactly which Photos it may remove.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AlbumMembershipMutation {
    Add {
        album_id: String,
        photo_ids: Vec<String>,
    },
    RemoveAdded {
        album_id: String,
        photo_ids: Vec<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlbumMembershipResult {
    pub album_id: String,
    pub added_photo_ids: Vec<String>,
    pub already_member_photo_ids: Vec<String>,
    pub removed_photo_ids: Vec<String>,
    pub already_absent_photo_ids: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhotoStateField {
    SelectionState,
    Rating,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhotoStateValue {
    Selection(SelectionState),
    Rating(u8),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhotoStateMutation {
    pub photo_id: String,
    pub field: PhotoStateField,
    pub value: PhotoStateValue,
    pub expected_current: Option<PhotoStateValue>,
    pub album_id: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhotoStateUndo {
    pub photo_id: String,
    pub field: PhotoStateField,
    pub prior_value: PhotoStateValue,
    pub expected_current: PhotoStateValue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhotoStateMutationResult {
    pub photo_id: String,
    pub undo: PhotoStateUndo,
}

/// The largest number of Photos one bounded batch Selection State write may
/// address. The Grid's multi-selection names the Photos; the server applies
/// one state to all of them in one transaction.
pub const PHOTO_STATE_BATCH_MAX: usize = 100;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhotoStateBatchItem {
    pub photo_id: String,
    pub expected_current: SelectionState,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhotoStateBatchMutation {
    pub photos: Vec<PhotoStateBatchItem>,
    pub value: SelectionState,
}

/// One requested Photo that took the batch write, with the Selection State it
/// held before it, so the browser can describe one truthful Undo.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhotoStateBatchApplied {
    pub photo_id: String,
    pub prior_value: SelectionState,
}

/// One requested Photo whose current Selection State did not match the
/// browser's expected value. The Photo is not written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhotoStateBatchChangedElsewhere {
    pub photo_id: String,
    pub current_value: SelectionState,
}

/// One requested Photo that the current Library no longer holds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhotoStateBatchMissing {
    pub photo_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhotoStateBatchResult {
    pub applied: Vec<PhotoStateBatchApplied>,
    pub changed_elsewhere: Vec<PhotoStateBatchChangedElsewhere>,
    pub missing: Vec<PhotoStateBatchMissing>,
}

/// The largest Rating a Photo decision may carry.
pub const MAXIMUM_PHOTO_RATING: u8 = 5;

/// A version-checked Photo decision batch for machine clients. One field and
/// value apply to every requested Photo; each item names the observed decision
/// version the caller intends to write against. Existing browser mutations
/// remain separate so they cannot accidentally bypass this guard.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckedPhotoDecisionMutation {
    pub field: PhotoStateField,
    pub value: PhotoStateValue,
    pub photos: Vec<CheckedPhotoDecisionItem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckedPhotoDecisionItem {
    pub photo_id: String,
    pub expected_version: String,
}

/// The guarded decision facts one Photo held before a checked change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PhotoDecisionFacts {
    pub selection_state: SelectionState,
    pub rating: u8,
}

/// One Photo's current decision facts together with their process-epoch
/// mutation guard, read by the same serialized persistence-owner command as
/// the decision write.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhotoDecisionSnapshot {
    pub selection_state: SelectionState,
    pub rating: u8,
    pub decision_version: String,
}

/// One requested Photo's outcome in a checked decision batch. The version is
/// compared before no-op detection, so a changed-away-and-back decision
/// conflicts instead of reporting `Unchanged`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CheckedPhotoDecisionOutcome {
    Changed {
        prior: PhotoDecisionFacts,
        current: PhotoDecisionSnapshot,
    },
    Unchanged {
        current: PhotoDecisionSnapshot,
    },
    Conflict {
        current: PhotoDecisionSnapshot,
    },
    Missing,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckedPhotoDecisionItemResult {
    pub photo_id: String,
    pub outcome: CheckedPhotoDecisionOutcome,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CheckedPhotoDecisionCounts {
    pub changed: usize,
    pub unchanged: usize,
    pub conflict: usize,
    pub missing: usize,
}

/// Confirmed facts from one checked decision commit. Results retain request
/// order with exactly one outcome per requested Photo, and the counts count
/// those outcomes. Conflicts and missing records are domain results, not
/// transaction failures.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CheckedPhotoDecisionResult {
    pub results: Vec<CheckedPhotoDecisionItemResult>,
    pub counts: CheckedPhotoDecisionCounts,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreviewSeed {
    pub photo_id: String,
    pub state: PreviewState,
    pub source: PreviewSource,
    pub expected_source_revision: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub cache_revision: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewSeedResult {
    Applied,
    StaleIgnored,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PathError;

impl fmt::Display for PathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Original path escapes the Photo Library")
    }
}

impl std::error::Error for PathError {}

/// Retention window shared by every accepted Export receipt and its captured
/// snapshot, and by a published Development TIFF. The Product Spec owns the
/// seven-day period; this constant is its only implementation definition.
pub const EXPORT_RETENTION_SECONDS: u64 = 7 * 24 * 60 * 60;

/// The one closed processing workload of the first Export capability.
pub const EXPORT_DEVELOPMENT_TIFF_WORKLOAD: &str = "development-tiff";

/// The white-balance payload of the closed first execution recipe.
pub const EXPORT_AS_SHOT_WHITE_BALANCE: &str = "as-shot";

/// A stored exposure is representable by the execution payload only inside the
/// approved finite range. The server supplies the bundle's qualified range so
/// the serialized submission validates against one authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExportExposureRange {
    pub minimum_milli_ev: i64,
    pub maximum_milli_ev: i64,
}

/// The execution payload of the captured recipe: thousandths of an EV and the
/// closed as-shot white balance. This is the projection sent to the launcher;
/// it contains no engine-private settings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExportRecipePayload {
    pub exposure_milli_ev: i64,
    pub white_balance_mode: &'static str,
}

impl ExportRecipePayload {
    /// Converts a stored semantic recipe into its closed execution payload.
    /// Thousandths are rounded half away from zero, then checked against the
    /// approved finite range; a non-finite or out-of-range value is not
    /// representable and refuses the Export instead of being clamped.
    pub fn capture(
        settings: &EditRecipeSettings,
        range: ExportExposureRange,
    ) -> Result<Self, ExportSettingsError> {
        if !settings.exposure_ev.is_finite() {
            return Err(ExportSettingsError);
        }
        let scaled = settings.exposure_ev * 1_000.0;
        if !scaled.is_finite() || scaled.abs() >= i64::MAX as f64 {
            return Err(ExportSettingsError);
        }
        let exposure_milli_ev = scaled.round() as i64;
        if exposure_milli_ev < range.minimum_milli_ev
            || exposure_milli_ev > range.maximum_milli_ev
            || settings.white_balance != WhiteBalanceIntent::AsShot
        {
            return Err(ExportSettingsError);
        }
        Ok(Self {
            exposure_milli_ev,
            white_balance_mode: EXPORT_AS_SHOT_WHITE_BALANCE,
        })
    }

    /// The canonical digest of the frozen protocol's execution tuple: compact
    /// JSON `[exposure_milli_ev, white_balance_mode]`. The launcher recomputes
    /// the same digest from the two semantic values it receives and fails
    /// closed on any mismatch; the durable snapshot binds the digest so a
    /// replay can never change the recipe.
    pub fn digest(&self) -> String {
        use sha2::{Digest, Sha256};
        let canonical = format!(
            "[{},\"{}\"]",
            self.exposure_milli_ev, self.white_balance_mode
        );
        format!("{:x}", Sha256::digest(canonical.as_bytes()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExportSettingsError;

impl fmt::Display for ExportSettingsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .write_str("Recipe settings are not representable by the approved execution payload")
    }
}

impl std::error::Error for ExportSettingsError {}

/// The immutable per-Export capture accepted at submission. Later edits to the
/// Photo's recipe or source never mutate these facts.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportSnapshot {
    pub photo_id: String,
    pub recipe_revision: String,
    pub settings: EditRecipeSettings,
    pub source_revision: String,
    pub source_kind: OriginalKind,
    pub source_profile_id: String,
    pub policy_id: String,
    pub bundle_id: String,
    /// The closed workload value; only `development-tiff` exists.
    pub workload: String,
    /// Digest of the captured execution payload.
    pub recipe_digest: String,
}

impl ExportSnapshot {
    /// The launcher-facing execution payload of the captured recipe. The
    /// snapshot was validated at capture, so this conversion cannot fail for
    /// a well-formed row; a corrupt row reports the settings error.
    pub fn recipe_payload(&self) -> Result<ExportRecipePayload, ExportSettingsError> {
        let white_balance = match self.settings.white_balance {
            WhiteBalanceIntent::AsShot => EXPORT_AS_SHOT_WHITE_BALANCE,
            // A temperature-tint value is retained editing intent that no
            // capability admits for execution; a snapshot carrying one can
            // never produce an execution payload.
            WhiteBalanceIntent::TemperatureTint { .. } => {
                return Err(ExportSettingsError);
            }
        };
        if !self.settings.exposure_ev.is_finite() {
            return Err(ExportSettingsError);
        }
        let scaled = self.settings.exposure_ev * 1_000.0;
        if !scaled.is_finite() || scaled.abs() >= i64::MAX as f64 {
            return Err(ExportSettingsError);
        }
        Ok(ExportRecipePayload {
            exposure_milli_ev: scaled.round() as i64,
            white_balance_mode: white_balance,
        })
    }
}

/// Closed Export lifecycle states.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExportState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

impl ExportState {
    pub fn name(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse_name(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

/// The launcher-owned attempt identity persisted with the Export. A retry
/// after a lost response reuses the pair; an explicit retry allocates a new
/// sequence against the same retained snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportAttempt {
    pub incarnation: String,
    pub sequence: u64,
}

/// Facts of one validated published artifact. They exist only on a
/// `succeeded` Export and disclose the retention expiry and the closed
/// download metadata: geometry and the pinned embedded-profile identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportArtifactFacts {
    pub size: u64,
    pub sha256: String,
    pub expires_at: u64,
    /// Declared image width in pixels.
    pub width: u32,
    /// Declared image height in pixels.
    pub height: u32,
    /// SHA-256 of the embedded ICC profile bytes; the profile identity the
    /// download headers and the inspect artifact object disclose.
    pub profile_identity: String,
}

/// The launcher-facing staged-source evidence recorded after the confined
/// copy was verified. Absent until staging completes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportSourceEvidence {
    pub size: u64,
    pub sha256: String,
}

/// One durable Export record: the immutable snapshot, closed state, terminal
/// outcome, attempt identity, and artifact facts.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportRecord {
    pub id: String,
    pub snapshot: ExportSnapshot,
    /// Verified staged source bytes, recorded between acceptance and launch.
    pub source: Option<ExportSourceEvidence>,
    pub state: ExportState,
    /// Bounded actionable terminal reason; absent until settled.
    pub outcome: Option<String>,
    pub attempt: Option<ExportAttempt>,
    pub artifact: Option<ExportArtifactFacts>,
    pub created_at: u64,
    pub settled_at: Option<u64>,
    /// Receipt and snapshot retention deadline; absent while unsettled.
    pub retain_until: Option<u64>,
}

/// The guarded submission accepted by the serialized persistence owner. The
/// semantic settings are captured from the saved recipe inside the same
/// transaction that validates both expected revisions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExportSubmission {
    /// Stable caller-owned identity resolving a retry after a lost response.
    pub request_id: String,
    pub photo_id: String,
    /// The classified source class of the Photo's RAW Original.
    pub source_profile_id: String,
    /// Exact finite policy identity observed from the processing capability.
    pub policy_id: String,
    /// Exact approved bundle identity observed from the processing capability.
    pub bundle_id: String,
    pub expected_recipe_revision: String,
    pub expected_source_revision: String,
    /// The bundle's qualified exposure range, validated inside the
    /// transaction so a representable capture and the snapshot commit
    /// atomically.
    pub exposure_range: ExportExposureRange,
    /// The deployment's finite retained-output allowance. The complete
    /// artifact must be reservable inside it before the Export is accepted.
    pub retained_output_bytes_max: u64,
}

impl ExportSubmission {
    /// The canonical payload digest that scopes a request identity: two
    /// submissions under one identity conflict unless these digests match.
    pub fn payload_digest(&self) -> String {
        export_submission_payload_digest(
            &self.expected_recipe_revision,
            &self.expected_source_revision,
        )
    }
}

/// The digest of the caller-owned export payload. Server-owned placement —
/// policy, deployment bundle, resolved source profile — is deliberately
/// excluded: a legitimate redeploy may change any of them, and an identical
/// caller payload must still replay under its request identity. The Photo
/// scopes the identity at the receipt key, not here.
pub fn export_submission_payload_digest(
    expected_recipe_revision: &str,
    expected_source_revision: &str,
) -> String {
    let payload = serde_json::json!({
        "expected_recipe_revision": expected_recipe_revision,
        "expected_source_revision": expected_source_revision,
    });
    use sha2::{Digest, Sha256};
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&payload)
                .expect("submission serializes")
                .as_slice()
        )
    )
}

/// The pre-admission resolution of a request identity: recorded identities
/// replay, expire, or conflict without any launcher contact.
#[derive(Clone, Debug, PartialEq)]
pub enum ExportSubmissionResolution {
    /// The identity and payload resolve to the existing Export.
    Existing(Box<ExportRecord>),
    /// The identity's receipt survived its Export's retention; it can never
    /// start new work.
    Expired,
    /// The identity is recorded with a different payload.
    Conflict,
}

/// Outcomes of the one serialized submit transaction.
#[derive(Clone, Debug, PartialEq)]
pub enum ExportSubmitOutcome {
    /// A new Export was accepted with the captured snapshot.
    Created(ExportRecord),
    /// The same request identity and payload resolved to the existing
    /// Export; no new attempt was started.
    Existing(ExportRecord),
    /// The identity was already used with a different payload.
    RequestConflict,
    /// The stored recipe is bound to a different source than the current
    /// published revision; only an explicit rebind may adopt the new source.
    RequiresRebind,
    /// The identity's receipt expired; it cannot start new work.
    Expired,
    UnknownPhoto,
    UnsupportedPhoto,
    MissingRecipe,
    RecipeConflict(EditRecipeRead),
    SourceChanged(EditRecipeRead),
    /// The saved recipe is not representable by the approved execution
    /// payload.
    InvalidSettings,
    /// The retained-output allowance cannot admit the complete artifact.
    RetainedOutputFull,
    /// Current source facts cannot be read, so no guarded submission is
    /// possible.
    Unavailable,
}

/// Why a retry input was refused, or the re-armed Export it produced.
#[derive(Clone, Debug, PartialEq)]
pub enum ExportRetryOutcome {
    /// The Export was re-armed against its retained snapshot with a new
    /// attempt identity.
    Retried(Box<ExportRecord>),
    /// An accepted retry identity was replayed; the current record is
    /// returned and no work is started.
    Replayed(Box<ExportRecord>),
    Unknown,
    /// The retry request identity was already used with a different payload.
    RequestConflict,
    /// The Export is unfinished or already succeeded; only a failed or
    /// cancelled Export within its retention window may be retried.
    NotRetriable,
    /// The retained snapshot's retention window has passed.
    Expired,
    /// The captured source or the approved bundle is no longer available, so
    /// the retained snapshot can never execute again.
    OutputUnavailable,
    /// Current source facts cannot be read, so availability cannot be
    /// re-validated for the retained snapshot.
    ResourceUnavailable,
    RetainedOutputFull,
}

/// A terminal settlement applied exactly once by the orchestration layer or
/// by a racing cancellation.
#[derive(Clone, Debug, PartialEq)]
pub enum ExportSettlement {
    Succeeded {
        artifact_size: u64,
        artifact_sha256: String,
        published_at: u64,
        /// Validated Development TIFF geometry disclosed with the artifact.
        artifact_width: u32,
        artifact_height: u32,
        /// SHA-256 identity of the embedded ICC profile.
        artifact_profile_identity: String,
    },
    Failed {
        outcome: String,
        settled_at: u64,
    },
}

/// Result of one expiry sweep. The persistence owner removes expired rows and
/// leases; the caller removes the named artifact files afterwards.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ExportSweepResult {
    /// Succeeded Exports whose artifact retention passed with no active
    /// lease; one artifact file per id may be removed.
    pub artifact_expiry_ids: Vec<String>,
    /// Exports whose receipt and snapshot retention passed; their rows were
    /// deleted and their identities now resolve as expired.
    pub record_expiry_ids: Vec<String>,
}

/// Outcome of a download-lease acquisition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExportLeaseOutcome {
    Acquired {
        lease_id: String,
        artifact: ExportArtifactFacts,
    },
    /// The artifact retention window passed.
    Expired,
    Unknown,
}
