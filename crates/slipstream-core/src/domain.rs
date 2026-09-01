use crate::capture::CaptureFact;
use std::{fmt, sync::Arc};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OriginalKind {
    Raw,
    Jpeg,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewCandidate {
    MatchingJpeg,
    EmbeddedRawJpeg,
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
    pub raw_original_id: Option<String>,
    pub jpeg_original_id: Option<String>,
    pub ambiguous: bool,
    pub available: bool,
    pub preview_state: PreviewState,
    pub preview_candidate: Option<PreviewCandidate>,
    pub preview_source: Option<PreviewCandidate>,
    pub preview_source_revision: Option<String>,
    pub preview_width: Option<u32>,
    pub preview_height: Option<u32>,
    pub cache_revision: Option<String>,
    pub sort_path: String,
    pub selection_state: SelectionState,
    pub rating: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScanSnapshot {
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

#[derive(Clone, Debug, PartialEq)]
pub struct PreviewSeed {
    pub photo_id: String,
    pub state: PreviewState,
    pub expected_candidate: PreviewCandidate,
    pub expected_source_revision: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub cache_revision: Option<String>,
    pub actual_source: Option<PreviewCandidate>,
    pub actual_source_revision: Option<String>,
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
