use super::*;
mod cli;

pub(crate) use cli::*;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliPhotoGetWire {
    #[serde(flatten)]
    pub photo: CliPhotoItemWire,
    pub metadata: CliPhotoMetadataWire,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliPhotoMetadataWire {
    pub state: &'static str,
    pub capture_time: Option<String>,
    pub aperture: Option<String>,
    pub shutter_speed: Option<String>,
    pub focal_length: Option<String>,
    pub iso: Option<u32>,
    pub make: Option<String>,
    pub model: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliFolderListResponse {
    pub items: Vec<FolderChildWire>,
    pub total: usize,
    pub next_cursor: Option<String>,
    pub evaluated_at: String,
    pub expires_at: Option<String>,
    pub publication: String,
    pub parent: String,
}

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

/// One confirmed removal. Removed Photos are reported by count because the
/// operation id — not a Photo list — is what Undo restores; every Photo that
/// was not newly removed is named.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoRemovalResponse {
    pub operation_id: String,
    pub counts: PhotoRemovalCountsWire,
    pub changed_elsewhere: Vec<String>,
    pub missing: Vec<String>,
    pub already_removed: Vec<String>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoRemovalCountsWire {
    pub removed: usize,
    pub changed_elsewhere: usize,
    pub missing: usize,
    pub already_removed: usize,
}
/// Explicit machine-facing removal result. Every requested identity appears
/// once; a successful item carries the exact marker needed for Restore.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplicitPhotoRemovalResponse {
    pub operation_id: String,
    pub counts: PhotoRemovalCountsWire,
    pub results: Vec<PhotoRemovalItemWire>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoRemovalItemWire {
    pub photo_id: String,
    pub outcome: &'static str,
    pub removed_at_ms: Option<i64>,
}

/// Explicit machine-facing Restore result. Every requested identity appears
/// once with the outcome of comparing its observed removal marker.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplicitPhotoRestoreResponse {
    pub operation_id: String,
    pub counts: ExplicitPhotoRestoreCountsWire,
    pub results: Vec<PhotoRestoreItemWire>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplicitPhotoRestoreCountsWire {
    pub restored: usize,
    pub already_active: usize,
    pub changed_elsewhere: usize,
    pub missing: usize,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoRestoreItemWire {
    pub photo_id: String,
    pub outcome: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoRestorationResponse {
    pub counts: PhotoRestorationCountsWire,
    pub changed_elsewhere: Vec<String>,
    pub missing: Vec<String>,
    pub operations: Vec<PhotoOperationRemainderWire>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoRestorationCountsWire {
    pub restored: usize,
    pub changed_elsewhere: usize,
    pub missing: usize,
}

/// How many Photos one touched removal operation still owns. An operation with
/// nothing left is reported with zero, so the surface offering its Undo stops
/// claiming a count the Library no longer holds.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotoOperationRemainderWire {
    pub operation_id: String,
    pub removed: usize,
}

/// One bounded page of removed Photos, newest removal first, with the same
/// Photo facts Grid View uses to present one.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemovedPhotosResponse {
    pub start: usize,
    pub limit: usize,
    pub total: usize,
    /// The most Photos one Permanent Deletion review may capture. A surface
    /// that selects every Trash result must not exceed it.
    pub review_maximum: usize,
    pub operation: Option<PhotoOperationRemainderWire>,
    pub photos: Vec<RemovedPhotoWire>,
}

/// One removed Photo with the exact removal marker the Photographer reviewed,
/// so a restore can compare and set against it instead of clearing whatever
/// removal the Photo carries now.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemovedPhotoWire {
    pub removed_at_ms: i64,
    pub original_location: String,
    pub original_kind: &'static str,
    pub original_size: Option<u64>,
    /// The retained Permanent Deletion operation whose outcome for this Photo
    /// is still unresolved. While it is present, Restore and another
    /// destructive confirmation are unavailable and the surface can reopen
    /// that operation.
    pub pending_verification_operation_id: Option<String>,
    pub photo: PhotoSummary,
}

/// Fixed review facts and explicit refusals for one Permanent Deletion.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermanentDeletionReviewResponse {
    pub operation_id: String,
    pub items: Vec<PermanentDeletionReviewItemWire>,
    pub rejected: Vec<PermanentDeletionRejectionWire>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermanentDeletionReviewItemWire {
    pub photo_id: String,
    pub removed_at_ms: i64,
    pub original_id: String,
    pub original_location: String,
    pub original_kind: String,
    pub size: u64,
    pub albums: Vec<PhotoAlbumMembershipWire>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermanentDeletionRejectionWire {
    pub photo_id: String,
    pub reason: &'static str,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermanentDeletionResponse {
    pub operation_id: String,
    pub reviewed: usize,
    pub logical_bytes_deleted: u64,
    pub items: Vec<PermanentDeletionItemWire>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermanentDeletionItemWire {
    pub photo_id: String,
    pub original_location: String,
    pub original_kind: &'static str,
    pub state: &'static str,
    pub size: Option<u64>,
    pub message: Option<String>,
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
    pub has_saved_edits: bool,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub make: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl From<slipstream_core::CaptureReviewMetadata> for PhotoMetadataWire {
    fn from(value: slipstream_core::CaptureReviewMetadata) -> Self {
        Self {
            capture_time: value.capture_time,
            aperture: value.aperture,
            iso: value.iso,
            shutter_speed: value.shutter_speed,
            focal_length: value.focal_length,
            make: value.make,
            model: value.model,
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
    let source = match photo.preview_state {
        slipstream_core::PreviewState::Ready => single
            .filter(|original| original.available && original.error_category.is_none())
            .map(|original| preview_source(original.kind.preview_source())),
        _ => None,
    };
    let state = preview_state(photo.preview_state);
    PhotoSummary {
        id: photo.id.clone(),
        available: photo.available,
        original,
        original_filename,
        selection_state: selection_state(photo.selection_state),
        rating: photo.rating,
        has_saved_edits: photo.has_saved_edits,
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
        DerivativeTarget::DevelopmentPreview1224 => "preview",
    }
}

/// The canonical browser Destination for one Photo, shared by every wire that
/// hands a Photo to a client.
pub(crate) fn photo_web_path(photo_id: &str) -> String {
    format!("/?photoId={photo_id}")
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
                "/api/private/derivatives/{}/{}/{}.jpg",
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
    /// The repeated typed facts for a CLI download. The Web derivative route
    /// leaves this empty, so its response stays unchanged.
    pub(crate) cli_facts: Option<CliDerivativeFacts>,
}

/// One reviewed unavailable Photo. `state` reports the current state of the
/// reviewed identity, so a Photo that was recovered, trashed, or that no
/// longer exists keeps its reviewed position instead of shifting the items
/// around it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryItemWire {
    pub state: &'static str,
    pub original_id: String,
    pub photo_id: String,
    pub location: String,
    pub kind: &'static str,
    pub rating: u8,
    pub selection_state: &'static str,
    pub fingerprint_enrolled: bool,
    pub album_count: u64,
    pub web_url: String,
}

impl RecoveryItemWire {
    /// The item one Current Original read reports, in the given state.
    pub(crate) fn from_record(
        record: &slipstream_core::RecoveryRecord,
        state: &'static str,
    ) -> Self {
        Self {
            state,
            original_id: record.original_id.clone(),
            photo_id: record.photo_id.clone(),
            location: record.relative_path.clone(),
            kind: original_kind(&record.kind),
            rating: record.rating,
            selection_state: selection_state(record.selection_state),
            fingerprint_enrolled: record.fingerprint.is_some(),
            album_count: record.album_count,
            web_url: photo_web_path(&record.photo_id),
        }
    }

    /// The same reviewed identity after its record vanished: the last
    /// evaluated facts are the only facts left to report.
    pub(crate) fn missing(retained: &Self) -> Self {
        Self {
            state: "missing",
            ..retained.clone()
        }
    }
}

/// The current state of one reviewed identity. The review reports exactly
/// one of these per retained item.
pub(crate) fn recovery_item_state(record: &slipstream_core::RecoveryRecord) -> &'static str {
    if record.removed {
        "removed"
    } else if record.available {
        "available"
    } else {
        "unavailable"
    }
}

pub(crate) fn original_kind(kind: &slipstream_core::OriginalKind) -> &'static str {
    match kind {
        slipstream_core::OriginalKind::Raw => "raw",
        slipstream_core::OriginalKind::Jpeg => "jpeg",
    }
}

/// The occupying record an explicit retire-and-bind may replace.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetireCandidateWire {
    pub photo_id: String,
    pub original_id: String,
    pub location: String,
}

/// One reviewed proposed mapping for an unavailable Original. `mappingId`
/// identifies exactly the evaluated mapping; applying repeats it so a
/// proposal that changed between review and apply is refused instead of
/// silently committed.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryMappingWire {
    pub mapping_id: String,
    pub original_id: String,
    pub photo_id: String,
    pub from_location: String,
    pub to_location: String,
    pub kind: &'static str,
    pub outcome: &'static str,
    /// True when a persisted fingerprint matched the candidate digest; false
    /// means historical content could not be verified and the confirmation
    /// must say so.
    pub verified: bool,
    pub blocked_reason: Option<&'static str>,
    pub retire: Option<RetireCandidateWire>,
}

impl RecoveryMappingWire {
    /// One reviewed mapping. `mapping_id` and `blocked_reason` come from the
    /// evaluation itself, so a later confirmation binds to exactly the facts
    /// the Photographer reviewed.
    pub(crate) fn from_proposal(proposal: &slipstream_core::ManualProposal) -> Self {
        let outcome = proposal.outcome.code();
        let retire = match &proposal.outcome {
            slipstream_core::ManualOutcome::Occupied {
                retire: Some(retire),
            } => Some(RetireCandidateWire {
                photo_id: retire.photo_id.clone(),
                original_id: retire.original_id.clone(),
                location: retire.location.clone(),
            }),
            _ => None,
        };
        Self {
            mapping_id: proposal.mapping_id.clone(),
            original_id: proposal.original_id.clone(),
            photo_id: proposal.photo_id.clone(),
            from_location: proposal.from_location.clone(),
            to_location: proposal.to_location.clone(),
            kind: original_kind(&proposal.kind),
            outcome,
            verified: proposal.verified,
            blocked_reason: proposal.blocked.map(slipstream_core::MappingBlock::code),
            retire,
        }
    }
}

/// One bounded page of a retained review.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryListResponse<T: Serialize> {
    pub items: Vec<T>,
    pub total: usize,
    pub next_cursor: Option<String>,
    pub evaluated_at: String,
    pub expires_at: Option<String>,
}

/// One committed mapping: where its Original now lives and which occupying
/// Photo it replaced when the Photographer chose that explicit retire.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryAppliedWire {
    pub original_id: String,
    pub photo_id: String,
    pub from_location: String,
    pub to_location: String,
    pub web_url: String,
    pub retired: Option<RetireCandidateWire>,
}

/// Committed result of one reviewed relocation batch.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryApplyResponseWire {
    pub applied_mappings: u64,
    pub refused_mappings: u64,
    pub unavailable_photos: u64,
    pub mappings: Vec<RecoveryAppliedWire>,
}

/// One per-mapping rejection for a refused batch.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryRejectionWire {
    pub original_id: String,
    pub reason: &'static str,
}

/// Structured 409 body for a rejected recovery batch: one primary message
/// plus per-mapping reasons. The whole batch is refused without partial
/// association.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryRejectionResponseWire {
    pub message: &'static str,
    pub rejections: Vec<RecoveryRejectionWire>,
    pub applied_mappings: u64,
    pub refused_mappings: u64,
}

/// One submitted mapping identity an unknown-outcome report names, so the
/// caller can reconcile exactly the correspondences that may have committed.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoverySubmittedMappingWire {
    pub original_id: String,
    pub new_location: String,
    pub mapping_id: String,
}
