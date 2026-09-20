use super::*;
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryOverviewResponse {
    pub published: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publication: Option<String>,
    pub photo_count: usize,
    pub scan: ScanStatusWire,
    pub albums: Vec<AlbumSummaryWire>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanStatusWire {
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub publication: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<usize>,
    /// The committed recovery result of the most recent completed scan, once
    /// one has completed since the server started.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_recovery: Option<ScanRecoveryWire>,
    /// Truthful background fingerprint enrollment counters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprints: Option<FingerprintProgressWire>,
}

/// Recovery facts from the most recent committed scan.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanRecoveryWire {
    pub relocated_photos: usize,
    pub fingerprinted_originals: usize,
    pub unavailable_photos: usize,
}

/// Background fingerprint enrollment counters.
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FingerprintProgressWire {
    pub enrolled: usize,
    pub pending: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumSummaryWire {
    pub id: String,
    pub name: String,
    pub photo_count: usize,
    pub has_saved_position: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseOpenResponse {
    pub token: String,
    pub total: usize,
    pub position: usize,
    pub selection_counts: SelectionCountsWire,
}

/// Bounded per-state Selection counts for one Browse Snapshot's source.
/// The counts describe the source order before any Selection State filter is
/// applied, so they stay meaningful inside a filtered view.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SelectionCountsWire {
    pub selected: usize,
    pub rejected: usize,
    pub undecided: usize,
}

impl SelectionCountsWire {
    pub(crate) fn from_selection_states(states: impl Iterator<Item = SelectionState>) -> Self {
        let mut counts = Self {
            selected: 0,
            rejected: 0,
            undecided: 0,
        };
        for state in states {
            match state {
                SelectionState::Selected => counts.selected += 1,
                SelectionState::Rejected => counts.rejected += 1,
                SelectionState::Undecided => counts.undecided += 1,
            }
        }
        counts
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowsePositionResponse {
    pub position: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowseWindowResponse {
    pub start: usize,
    pub total: usize,
    pub photos: Vec<PhotoSummary>,
}

#[derive(Clone, Debug)]
pub enum BrowseSourceRequest {
    Library,
    Album(String),
    Folder {
        location: String,
        publication: String,
    },
}

/// The explicit view order requested for one Browse Snapshot.
/// `AlbumOrder` is meaningful only for an Album source and means persisted
/// membership position; `CaptureTimeAscending` matches the Published
/// Library's natural order for library and Folder sources.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowseViewOrder {
    AlbumOrder,
    CaptureTimeAscending,
    CaptureTimeDescending,
}

/// The Selection State filter requested for one Browse Snapshot. `All` keeps
/// every Photo of the source order; every other value keeps only Photos whose
/// current Selection State matches, resolved server-side against the source
/// order before the Snapshot is frozen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrowseSelectionFilter {
    All,
    Undecided,
    Selected,
    Rejected,
}

impl BrowseSelectionFilter {
    pub(crate) fn matches(self, state: SelectionState) -> bool {
        match self {
            Self::All => true,
            Self::Undecided => state == SelectionState::Undecided,
            Self::Selected => state == SelectionState::Selected,
            Self::Rejected => state == SelectionState::Rejected,
        }
    }
}

/// Bounded per-Photo Album membership response: Album identities only.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoAlbumsResponse {
    pub albums: Vec<PhotoAlbumMembershipWire>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoAlbumMembershipWire {
    pub id: String,
    pub name: String,
}

/// One bounded File Location window response.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileLocationsResponse {
    pub publication: String,
    pub parent: String,
    pub start: usize,
    pub limit: usize,
    pub total: usize,
    pub children: Vec<FolderChildWire>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderChildWire {
    pub location: String,
    pub name: String,
    pub photo_count: usize,
    pub has_descendant_folders: bool,
}

/// Bounded Album mutation response: the same summaries the Library
/// Overview exposes, never member lists.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumSummaryListResponse {
    pub albums: Vec<AlbumSummaryWire>,
}

/// Identity-bearing result for one bounded Grid Add to Album operation.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumMembershipAddResponse {
    pub album_id: String,
    pub added_photo_ids: Vec<String>,
    pub already_member_photo_ids: Vec<String>,
    pub albums: Vec<AlbumSummaryWire>,
}

/// Identity-bearing result for one bounded Add-to-Album compensation.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumMembershipRemoveResponse {
    pub album_id: String,
    pub removed_photo_ids: Vec<String>,
    pub already_absent_photo_ids: Vec<String>,
    pub albums: Vec<AlbumSummaryWire>,
}

/// Result of adding every Photo projected into one Original Folder to an
/// Album. The Photo IDs stay server-side; the response only reports bounded
/// operation facts and refreshed Album summaries.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderAlbumMutationResponse {
    pub album_id: String,
    pub folder_path: String,
    pub matched_count: usize,
    pub added_count: usize,
    pub already_member_count: usize,
    pub albums: Vec<AlbumSummaryWire>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoSummary {
    pub id: String,
    pub available: bool,
    pub original: OriginalWire,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_filename: Option<String>,
    pub selection_state: &'static str,
    pub rating: u8,
    pub preview: PreviewWire,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OriginalWire {
    pub kind: &'static str,
    pub available: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewWire {
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limited_detail: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<&'static str>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoMetadataWire {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aperture: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iso: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shutter_speed: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focal_length: Option<String>,
}

impl From<slipstream_core::CaptureReviewMetadata> for PhotoMetadataWire {
    fn from(value: slipstream_core::CaptureReviewMetadata) -> Self {
        Self {
            capture_time: value.capture_time,
            aperture: value.aperture,
            iso: value.iso,
            shutter_speed: value.shutter_speed,
            focal_length: value.focal_length,
        }
    }
}

pub(crate) fn photo_summary_indexed_with_url(
    photo: &slipstream_core::PhotoRecord,
    originals: &[slipstream_core::OriginalRecord],
    originals_by_id: &std::collections::HashMap<String, usize>,
    preview_url: Option<String>,
    thumbnail_url: Option<String>,
) -> PhotoSummary {
    let single = originals_by_id
        .get(&photo.original_id)
        .and_then(|position| originals.get(*position));
    let original = single.map(|original| OriginalWire {
        kind: match original.kind {
            slipstream_core::OriginalKind::Raw => "raw",
            slipstream_core::OriginalKind::Jpeg => "jpeg",
        },
        available: original.available,
    });
    let Some(original) = original else {
        // A Photo without its Original record cannot be summarized; callers
        // resolve unknown Photos before reaching this point.
        unreachable!("summarized Photo has no Original record")
    };
    let original_filename = ordering_original_filename(photo, originals, originals_by_id);
    let source = single
        .filter(|original| original.available && original.error_category.is_none())
        .map(|original| preview_source(original.kind.preview_source()));
    let state = preview_state(photo.preview_state);
    PhotoSummary {
        id: photo.id.clone(),
        available: photo.available,
        original,
        original_filename,
        selection_state: selection_state(photo.selection_state),
        rating: photo.rating,
        preview: PreviewWire {
            state,
            source,
            width: photo.preview_width,
            height: photo.preview_height,
            limited_detail: photo
                .preview_width
                .zip(photo.preview_height)
                .map(|(width, height)| width.max(height) < 2560),
            url: preview_url,
            thumbnail_url,
            message: (!photo.available).then_some("Original File is unavailable"),
        },
    }
}

/// The filename of the Photo's ordering Original Location: the RAW Original
/// when the Photo contains RAW, otherwise the JPEG Original. Only the final
/// path component crosses the boundary; the relative Location itself never
/// does.
fn ordering_original_filename(
    photo: &slipstream_core::PhotoRecord,
    originals: &[slipstream_core::OriginalRecord],
    originals_by_id: &std::collections::HashMap<String, usize>,
) -> Option<String> {
    originals_by_id
        .get(&photo.original_id)
        .and_then(|position| originals.get(*position))
        .and_then(|original| {
            let path = original.relative_path.as_str();
            let filename = path.rsplit('/').next().unwrap_or(path);
            (!filename.is_empty()).then(|| filename.to_owned())
        })
}

pub(crate) fn album_summary(summary: slipstream_core::AlbumSummary) -> AlbumSummaryWire {
    AlbumSummaryWire {
        id: summary.id,
        name: summary.name,
        photo_count: summary.photo_count,
        has_saved_position: summary.has_saved_position,
    }
}

pub(crate) fn selection_state(state: SelectionState) -> &'static str {
    match state {
        SelectionState::Undecided => "undecided",
        SelectionState::Selected => "selected",
        SelectionState::Rejected => "rejected",
    }
}

pub(crate) fn preview_state(state: PreviewState) -> &'static str {
    match state {
        PreviewState::InspectionPending => "inspection-pending",
        PreviewState::Ready => "ready",
        PreviewState::Failed => "failed",
        PreviewState::Unavailable => "unavailable",
    }
}

pub(crate) fn preview_source(source: PreviewSource) -> &'static str {
    source.wire_name()
}

pub(crate) fn derivative_target_name(target: DerivativeTarget) -> &'static str {
    match target {
        DerivativeTarget::Thumbnail512 => "thumbnail",
        DerivativeTarget::Review2560 => "review",
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewResponse {
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limited_detail: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<&'static str>,
}

impl PreviewResponse {
    pub(crate) fn ready(
        photo_id: &str,
        ready: &slipstream_core::PreviewReady,
        stale: bool,
    ) -> Self {
        Self {
            state: "ready",
            source: Some(ready.source.wire_name()),
            stale: Some(stale),
            width: Some(ready.width),
            height: Some(ready.height),
            limited_detail: Some(ready.width.max(ready.height) < 2560),
            url: Some(format!(
                "/api/derivatives/{}/{}/{}.jpg",
                photo_id,
                derivative_target_name(ready.target),
                ready.cache_key
            )),
            message: stale.then_some("Showing a stale Preview because current generation failed"),
        }
    }

    pub(crate) fn unavailable(message: &'static str) -> Self {
        Self {
            state: "unavailable",
            source: None,
            stale: None,
            width: None,
            height: None,
            limited_detail: None,
            url: None,
            message: Some(message),
        }
    }

    pub(crate) fn failed(message: &'static str) -> Self {
        Self {
            state: "failed",
            source: None,
            stale: None,
            width: None,
            height: None,
            limited_detail: None,
            url: None,
            message: Some(message),
        }
    }
}

#[derive(Clone, Debug)]
pub struct DerivativeDelivery {
    pub cache_key: String,
    pub bytes: Vec<u8>,
}
