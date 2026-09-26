use super::*;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CapabilitiesResponse {
    pub server_version: &'static str,
    pub supported_cli_contract_versions: [u16; 1],
    pub limits: CapabilityLimitsWire,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CapabilityLimitsWire {
    pub list_page_maximum: usize,
    pub mutation_photo_ids_maximum: usize,
    pub album_reorder_members_maximum: usize,
    pub retained_query_ids_maximum: usize,
    pub retained_query_idle_seconds: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliStatusResponse {
    pub server_version: &'static str,
    pub cli_contract_version: u16,
    pub published: bool,
    pub publication: Option<String>,
    pub photo_count: usize,
    pub scan: CliScanStatusWire,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliScanStatusWire {
    pub state: &'static str,
    pub publication: Option<String>,
    pub completed: Option<usize>,
    pub total: Option<usize>,
    pub last_recovery: Option<ScanRecoveryWire>,
    pub fingerprints: Option<FingerprintProgressWire>,
}

impl From<ScanStatusWire> for CliScanStatusWire {
    fn from(value: ScanStatusWire) -> Self {
        Self {
            state: value.state,
            publication: value.publication,
            completed: value.completed,
            total: value.total,
            last_recovery: value.last_recovery,
            fingerprints: value.fingerprints,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliListResponse<T: Serialize> {
    pub items: Vec<T>,
    pub total: usize,
    pub next_cursor: Option<String>,
    pub evaluated_at: String,
    pub expires_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum AlbumListItemWire {
    Present(CliAlbumSummaryWire),
    Missing(MissingItemWire),
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliAlbumSummaryWire {
    pub id: String,
    pub name: String,
    pub photo_count: usize,
    pub has_saved_position: bool,
    pub album_version: String,
    pub web_path: String,
}

impl From<slipstream_core::AlbumSummary> for CliAlbumSummaryWire {
    fn from(value: slipstream_core::AlbumSummary) -> Self {
        let web_path = format!("/?source=album&albumId={}", value.id);
        Self {
            id: value.id,
            name: value.name,
            photo_count: value.photo_count,
            has_saved_position: value.has_saved_position,
            album_version: value.album_version,
            web_path,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CliAlbumCreationWire {
    pub album: CliAlbumSummaryWire,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliAlbumRenameWire {
    pub album: CliAlbumSummaryWire,
    pub renamed: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliAlbumDeleteWire {
    pub album_id: String,
    pub deleted: bool,
    pub original_files_changed: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliAlbumAddWire {
    pub album: CliAlbumSummaryWire,
    pub added_photo_ids: Vec<String>,
    pub already_member_photo_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliAlbumRemoveWire {
    pub album: CliAlbumSummaryWire,
    pub removed_photo_ids: Vec<String>,
    pub already_absent_photo_ids: Vec<String>,
    pub saved_photo_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliAlbumReorderWire {
    pub album: CliAlbumSummaryWire,
    pub ordered_photo_ids: Vec<String>,
    pub reordered: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum CliAlbumChangeWire {
    Rename(CliAlbumRenameWire),
    Delete(CliAlbumDeleteWire),
    Add(CliAlbumAddWire),
    Remove(CliAlbumRemoveWire),
    Reorder(CliAlbumReorderWire),
}

impl From<slipstream_core::CheckedAlbumMutationResult> for CliAlbumChangeWire {
    fn from(value: slipstream_core::CheckedAlbumMutationResult) -> Self {
        match value {
            slipstream_core::CheckedAlbumMutationResult::Renamed { album, renamed } => {
                Self::Rename(CliAlbumRenameWire {
                    album: album.into(),
                    renamed,
                })
            }
            slipstream_core::CheckedAlbumMutationResult::Deleted { album_id } => {
                Self::Delete(CliAlbumDeleteWire {
                    album_id,
                    deleted: true,
                    original_files_changed: false,
                })
            }
            slipstream_core::CheckedAlbumMutationResult::Added {
                album,
                added_photo_ids,
                already_member_photo_ids,
            } => Self::Add(CliAlbumAddWire {
                album: album.into(),
                added_photo_ids,
                already_member_photo_ids,
            }),
            slipstream_core::CheckedAlbumMutationResult::Removed {
                album,
                removed_photo_ids,
                already_absent_photo_ids,
                saved_photo_id,
            } => Self::Remove(CliAlbumRemoveWire {
                album: album.into(),
                removed_photo_ids,
                already_absent_photo_ids,
                saved_photo_id,
            }),
            slipstream_core::CheckedAlbumMutationResult::Reordered {
                album,
                ordered_photo_ids,
                reordered,
            } => Self::Reorder(CliAlbumReorderWire {
                album: album.into(),
                ordered_photo_ids,
                reordered,
            }),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct MissingItemWire {
    pub id: String,
    pub state: &'static str,
}

/// One confirmed checked Photo decision batch. Results retain request order
/// with exactly one outcome per requested Photo, and the counts count those
/// outcomes.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliPhotoDecisionResultWire {
    pub results: Vec<CliPhotoDecisionItemWire>,
    pub counts: CliPhotoDecisionCountsWire,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliPhotoDecisionCountsWire {
    pub changed: usize,
    pub unchanged: usize,
    pub conflict: usize,
    pub missing: usize,
}

/// One requested Photo's outcome. `prior` appears only for a confirmed
/// change and `current` never appears for a missing Photo, so each outcome
/// reports exactly the keys the CLI reference defines for it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliPhotoDecisionItemWire {
    pub photo_id: String,
    pub outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prior: Option<CliPhotoDecisionFactsWire>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<CliPhotoDecisionSnapshotWire>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliPhotoDecisionFactsWire {
    pub selection_state: &'static str,
    pub rating: u8,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliPhotoDecisionSnapshotWire {
    pub selection_state: &'static str,
    pub rating: u8,
    pub decision_version: String,
}

impl From<slipstream_core::CheckedPhotoDecisionResult> for CliPhotoDecisionResultWire {
    fn from(value: slipstream_core::CheckedPhotoDecisionResult) -> Self {
        Self {
            results: value
                .results
                .into_iter()
                .map(|item| {
                    let (outcome, prior, current) = match item.outcome {
                        slipstream_core::CheckedPhotoDecisionOutcome::Changed {
                            prior,
                            current,
                        } => (
                            "changed",
                            Some(CliPhotoDecisionFactsWire {
                                selection_state: selection_state(prior.selection_state),
                                rating: prior.rating,
                            }),
                            Some(photo_decision_snapshot_wire(current)),
                        ),
                        slipstream_core::CheckedPhotoDecisionOutcome::Unchanged { current } => (
                            "unchanged",
                            None,
                            Some(photo_decision_snapshot_wire(current)),
                        ),
                        slipstream_core::CheckedPhotoDecisionOutcome::Conflict { current } => (
                            "conflict",
                            None,
                            Some(photo_decision_snapshot_wire(current)),
                        ),
                        slipstream_core::CheckedPhotoDecisionOutcome::Missing => {
                            ("missing", None, None)
                        }
                    };
                    CliPhotoDecisionItemWire {
                        photo_id: item.photo_id,
                        outcome,
                        prior,
                        current,
                    }
                })
                .collect(),
            counts: CliPhotoDecisionCountsWire {
                changed: value.counts.changed,
                unchanged: value.counts.unchanged,
                conflict: value.counts.conflict,
                missing: value.counts.missing,
            },
        }
    }
}

fn photo_decision_snapshot_wire(
    snapshot: slipstream_core::PhotoDecisionSnapshot,
) -> CliPhotoDecisionSnapshotWire {
    CliPhotoDecisionSnapshotWire {
        selection_state: selection_state(snapshot.selection_state),
        rating: snapshot.rating,
        decision_version: snapshot.decision_version,
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum PhotoListItemWire {
    Present(CliPhotoItemWire),
    Missing(MissingItemWire),
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliPhotoItemWire {
    pub id: String,
    pub filename: String,
    pub original_kind: &'static str,
    pub original_available: bool,
    pub selection_state: &'static str,
    pub rating: u8,
    pub decision_version: String,
    pub has_saved_edits: bool,
    pub capture_time: Option<String>,
    pub preview: CliPreviewFactsWire,
    pub web_path: String,
}

impl From<slipstream_core::PhotoRead> for CliPhotoItemWire {
    fn from(value: slipstream_core::PhotoRead) -> Self {
        let ready = value.preview_state == PreviewState::Ready;
        let detail_limited = value
            .preview_width
            .zip(value.preview_height)
            .map(|(width, height)| width.max(height) < 2560)
            .filter(|_| ready);
        let capture_time = (value.capture.state == slipstream_core::CaptureMetadataState::Known)
            .then_some(value.capture.order_key)
            .flatten();
        let web_path = photo_web_path(&value.id);
        Self {
            id: value.id,
            filename: value.filename,
            original_kind: match value.original_kind {
                slipstream_core::OriginalKind::Raw => "raw",
                slipstream_core::OriginalKind::Jpeg => "jpeg",
            },
            original_available: value.original_available,
            selection_state: selection_state(value.selection_state),
            rating: value.rating,
            decision_version: value.decision_version,
            has_saved_edits: value.has_saved_edits,
            capture_time,
            preview: CliPreviewFactsWire {
                state: preview_state(value.preview_state),
                source: ready
                    .then(|| value.preview_source.map(preview_source))
                    .flatten(),
                source_revision: ready.then_some(value.preview_source_revision).flatten(),
                width: ready.then_some(value.preview_width).flatten(),
                height: ready.then_some(value.preview_height).flatten(),
                detail_limited,
            },
            web_path,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliPreviewFactsWire {
    pub state: &'static str,
    pub source: Option<&'static str>,
    pub source_revision: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub detail_limited: Option<bool>,
}

/// The current supported derivative one admitted CLI Preview request may
/// download. Every fact belongs to the bytes the caller is about to read.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliPreviewResponse {
    pub photo_id: String,
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_revision: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail_limited: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub web_path: String,
}

/// The typed facts a CLI derivative download repeats so the caller can compare
/// them with the metadata it was admitted with. The response body is JPEG bytes,
/// so these travel as custom headers.
#[derive(Clone, Debug)]
pub(crate) struct CliDerivativeFacts {
    pub photo_id: String,
    pub source: &'static str,
    pub source_revision: String,
    pub width: u32,
    pub height: u32,
}

impl CliDerivativeFacts {
    /// The repeated facts as header name and value pairs. `source_revision`
    /// separates its fields with NUL, which is not a legal header value, so the
    /// revision is hexadecimal.
    pub(crate) fn headers(&self) -> [(&'static str, String); 5] {
        [
            (PREVIEW_PHOTO_HEADER, self.photo_id.clone()),
            (PREVIEW_SOURCE_HEADER, self.source.to_owned()),
            (
                PREVIEW_REVISION_HEADER,
                crate::queries::hex_encode(self.source_revision.as_bytes()),
            ),
            (PREVIEW_WIDTH_HEADER, self.width.to_string()),
            (PREVIEW_HEIGHT_HEADER, self.height.to_string()),
        ]
    }
}

pub(crate) const PREVIEW_PHOTO_HEADER: &str = "slipstream-preview-photo";
pub(crate) const PREVIEW_SOURCE_HEADER: &str = "slipstream-preview-source";
pub(crate) const PREVIEW_REVISION_HEADER: &str = "slipstream-preview-revision";
pub(crate) const PREVIEW_WIDTH_HEADER: &str = "slipstream-preview-width";
pub(crate) const PREVIEW_HEIGHT_HEADER: &str = "slipstream-preview-height";

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

/// One unavailable Photo listed by the bounded recovery review entry.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnavailableOriginalWire {
    pub original_id: String,
    pub photo_id: String,
    pub location: String,
    pub kind: &'static str,
    pub rating: u8,
    pub selection_state: &'static str,
    pub fingerprint_enrolled: bool,
    pub album_count: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoverySurveyWire {
    pub unavailable: Vec<UnavailableOriginalWire>,
}

impl From<slipstream_core::RecoverySurvey> for RecoverySurveyWire {
    fn from(survey: slipstream_core::RecoverySurvey) -> Self {
        Self {
            unavailable: survey
                .unavailable
                .into_iter()
                .map(|record| UnavailableOriginalWire {
                    original_id: record.original_id,
                    photo_id: record.photo_id,
                    location: record.relative_path,
                    kind: match record.kind {
                        slipstream_core::OriginalKind::Raw => "raw",
                        slipstream_core::OriginalKind::Jpeg => "jpeg",
                    },
                    rating: record.rating,
                    selection_state: match record.selection_state {
                        slipstream_core::SelectionState::Undecided => "undecided",
                        slipstream_core::SelectionState::Selected => "selected",
                        slipstream_core::SelectionState::Rejected => "rejected",
                    },
                    fingerprint_enrolled: record.fingerprint.is_some(),
                    album_count: record.album_count,
                })
                .collect(),
        }
    }
}

/// The occupying record an explicit retire-and-bind may replace.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetireCandidateWire {
    pub photo_id: String,
    pub original_id: String,
    pub location: String,
}

/// One inspectable proposed mapping for an unavailable Original.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryProposalWire {
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retire: Option<RetireCandidateWire>,
}

impl From<slipstream_core::ManualProposal> for RecoveryProposalWire {
    fn from(proposal: slipstream_core::ManualProposal) -> Self {
        let outcome = match &proposal.outcome {
            slipstream_core::ManualOutcome::Matched => "matched",
            slipstream_core::ManualOutcome::ContentMismatch => "content-mismatch",
            slipstream_core::ManualOutcome::Missing => "missing",
            slipstream_core::ManualOutcome::KindMismatch => "kind-mismatch",
            slipstream_core::ManualOutcome::Unreadable => "unreadable",
            slipstream_core::ManualOutcome::Occupied { .. } => "occupied",
            slipstream_core::ManualOutcome::Colliding => "colliding",
        };
        let retire = match proposal.outcome {
            slipstream_core::ManualOutcome::Occupied {
                retire: Some(retire),
            } => Some(RetireCandidateWire {
                photo_id: retire.photo_id,
                original_id: retire.original_id,
                location: retire.location,
            }),
            _ => None,
        };
        Self {
            original_id: proposal.original_id,
            photo_id: proposal.photo_id,
            from_location: proposal.from_location,
            to_location: proposal.to_location,
            kind: match proposal.kind {
                slipstream_core::OriginalKind::Raw => "raw",
                slipstream_core::OriginalKind::Jpeg => "jpeg",
            },
            outcome,
            verified: proposal.verified,
            retire,
        }
    }
}

/// Committed result of one manual relocation batch.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryApplyResponseWire {
    pub relocated_photos: u64,
    pub unavailable_photos: u64,
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
}

/// The closed artifact metadata object. Its fields are exactly the download
/// response headers, so a client validates a download field for field.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportArtifactWire {
    pub(crate) export_id: String,
    pub(crate) target: &'static str,
    pub(crate) stage: &'static str,
    pub(crate) content_type: &'static str,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) profile_identity: String,
    pub(crate) byte_length: u64,
    pub(crate) sha256: String,
    pub(crate) expires_at: String,
}

/// The body of one accepted Export submission or replay.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportSubmitWire {
    pub(crate) export_id: String,
    pub(crate) state: &'static str,
    pub(crate) target: &'static str,
    pub(crate) recipe_version: String,
    pub(crate) source_revision: String,
    pub(crate) receipt_expires_at: Option<String>,
    pub(crate) artifact_expires_at: Option<String>,
}

/// One bounded list entry of a Photo's retained Exports.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportSummaryWire {
    pub(crate) export_id: String,
    pub(crate) state: &'static str,
    pub(crate) target: &'static str,
}

/// Bounded list of one Photo's retained Exports.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportListWire {
    pub(crate) exports: Vec<ExportSummaryWire>,
}

/// One Export's full inspectable state with the closed artifact object.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportInspectWire {
    pub(crate) export_id: String,
    pub(crate) photo_id: String,
    pub(crate) state: &'static str,
    pub(crate) target: &'static str,
    pub(crate) recipe_version: String,
    pub(crate) source_revision: String,
    pub(crate) bundle_id: String,
    pub(crate) terminal_outcome: Option<&'static str>,
    pub(crate) failure_reason: Option<String>,
    pub(crate) receipt_expires_at: Option<String>,
    pub(crate) artifact: Option<ExportArtifactWire>,
}

/// The settled Export one cancellation returns.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportCancelWire {
    pub(crate) export_id: String,
    pub(crate) state: &'static str,
    pub(crate) terminal_outcome: Option<&'static str>,
}

/// The admitted retry attempt.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExportRetryWire {
    pub(crate) export_id: String,
    pub(crate) state: &'static str,
}

fn export_time(seconds: u64) -> String {
    crate::queries::format_time(std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds))
}

fn terminal_outcome(state: slipstream_core::ExportState) -> Option<&'static str> {
    match state {
        slipstream_core::ExportState::Succeeded => Some("succeeded"),
        slipstream_core::ExportState::Failed => Some("failed"),
        slipstream_core::ExportState::Cancelled => Some("cancelled"),
        slipstream_core::ExportState::Queued | slipstream_core::ExportState::Running => None,
    }
}

/// The receipt expiry with the submit response meaning: `null` while the
/// Export is active, terminal settlement plus the reconciliation period for
/// every terminal state.
fn receipt_expires_at(record: &slipstream_core::ExportRecord) -> Option<String> {
    record.retain_until.map(export_time)
}

/// The closed artifact object, or `null` when no validated artifact is
/// retained.
pub(crate) fn export_artifact_object(
    record: &slipstream_core::ExportRecord,
) -> Option<ExportArtifactWire> {
    let artifact = record.artifact.as_ref()?;
    Some(ExportArtifactWire {
        export_id: record.id.clone(),
        target: "development-tiff",
        stage: "develop",
        content_type: "image/tiff",
        width: artifact.width,
        height: artifact.height,
        profile_identity: artifact.profile_identity.clone(),
        byte_length: artifact.size,
        sha256: artifact.sha256.clone(),
        expires_at: export_time(artifact.expires_at),
    })
}

pub(crate) fn export_submit(record: &slipstream_core::ExportRecord) -> ExportSubmitWire {
    ExportSubmitWire {
        export_id: record.id.clone(),
        state: record.state.name(),
        target: "development-tiff",
        recipe_version: record.snapshot.recipe_revision.clone(),
        source_revision: record.snapshot.source_revision.clone(),
        receipt_expires_at: receipt_expires_at(record),
        artifact_expires_at: record
            .artifact
            .as_ref()
            .map(|artifact| export_time(artifact.expires_at)),
    }
}

pub(crate) fn export_summary(record: &slipstream_core::ExportRecord) -> ExportSummaryWire {
    ExportSummaryWire {
        export_id: record.id.clone(),
        state: record.state.name(),
        target: "development-tiff",
    }
}

pub(crate) fn export_inspect(record: &slipstream_core::ExportRecord) -> ExportInspectWire {
    ExportInspectWire {
        export_id: record.id.clone(),
        photo_id: record.snapshot.photo_id.clone(),
        state: record.state.name(),
        target: "development-tiff",
        recipe_version: record.snapshot.recipe_revision.clone(),
        source_revision: record.snapshot.source_revision.clone(),
        bundle_id: record.snapshot.bundle_id.clone(),
        terminal_outcome: terminal_outcome(record.state),
        failure_reason: record.outcome.clone(),
        receipt_expires_at: receipt_expires_at(record),
        artifact: export_artifact_object(record),
    }
}

pub(crate) fn export_cancel(record: &slipstream_core::ExportRecord) -> ExportCancelWire {
    ExportCancelWire {
        export_id: record.id.clone(),
        state: record.state.name(),
        terminal_outcome: terminal_outcome(record.state),
    }
}

pub(crate) fn export_retry(record: &slipstream_core::ExportRecord) -> ExportRetryWire {
    ExportRetryWire {
        export_id: record.id.clone(),
        state: record.state.name(),
    }
}
