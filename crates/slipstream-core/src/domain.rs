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
