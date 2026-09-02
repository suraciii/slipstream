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

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoSummary {
    pub id: String,
    pub available: bool,
    pub ambiguous: bool,
    pub originals: Vec<OriginalWire>,
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

pub(crate) fn photo_summary_indexed_with_url(
    photo: &slipstream_core::PhotoRecord,
    originals: &[slipstream_core::OriginalRecord],
    originals_by_id: &std::collections::HashMap<String, usize>,
    preview_url: Option<String>,
    thumbnail_url: Option<String>,
) -> PhotoSummary {
    let original = |id: &Option<String>, kind: &'static str| {
        id.as_ref().and_then(|id| {
            originals_by_id
                .get(id)
                .and_then(|position| originals.get(*position))
                .map(|original| OriginalWire {
                    kind,
                    available: original.available,
                })
        })
    };
    let originals = [
        original(&photo.raw_original_id, "raw"),
        original(&photo.jpeg_original_id, "jpeg"),
    ]
    .into_iter()
    .flatten()
    .collect();
    let source = photo.preview_source.map(preview_candidate);
    let state = preview_state(photo.preview_state);
    PhotoSummary {
        id: photo.id.clone(),
        available: photo.available,
        ambiguous: photo.ambiguous,
        originals,
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

pub(crate) fn preview_candidate(candidate: PreviewCandidate) -> &'static str {
    match candidate {
        PreviewCandidate::MatchingJpeg => "matching-jpeg",
        PreviewCandidate::EmbeddedRawJpeg => "embedded-raw-jpeg",
    }
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
            source: Some(preview_candidate(ready.source)),
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
