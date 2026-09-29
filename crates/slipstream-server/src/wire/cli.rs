// CLI contract wire shapes and export responses.
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
pub struct CapabilityLimitsWire {
    pub list_page_maximum: usize,
    pub mutation_photo_ids_maximum: usize,
    pub removal_photo_ids_maximum: usize,
    pub album_reorder_members_maximum: usize,
    pub retained_query_ids_maximum: usize,
    pub retained_query_idle_seconds: u64,
    /// The largest reviewed recovery page one request returns.
    pub recovery_page_maximum: usize,
    /// The largest Folder-prefix recovery scope the service evaluates.
    pub recovery_mappings_maximum: usize,
    /// The largest reviewed recovery batch one apply commits.
    pub recovery_apply_maximum: usize,
    /// The idle interval after which a recovery continuation expires.
    pub recovery_review_idle_seconds: u64,
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
    pub updated_at: Option<u64>,
    pub publication: Option<String>,
    pub completed: Option<usize>,
    pub total: Option<usize>,
    pub updated_ms: u64,
    pub last_recovery: Option<ScanRecoveryWire>,
    pub fingerprints: Option<FingerprintProgressWire>,
}

impl From<ScanStatusWire> for CliScanStatusWire {
    fn from(value: ScanStatusWire) -> Self {
        Self {
            state: value.state,
            updated_at: value.updated_at,
            publication: value.publication,
            completed: value.completed,
            total: value.total,
            updated_ms: value.updated_ms,
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
    Present(Box<CliPhotoItemWire>),
    Missing(MissingItemWire),
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CliPhotoItemWire {
    pub id: String,
    /// The Library identity of the associated Original File.
    pub original_id: String,
    /// The current Library-relative Location of that Original File.
    pub location: String,
    pub filename: String,
    pub original_kind: &'static str,
    pub original_available: bool,
    pub selection_state: &'static str,
    pub rating: u8,
    pub decision_version: String,
    pub removed_at_ms: Option<i64>,
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
            original_id: value.original_id,
            location: value.original_location,
            filename: value.filename,
            original_kind: match value.original_kind {
                slipstream_core::OriginalKind::Raw => "raw",
                slipstream_core::OriginalKind::Jpeg => "jpeg",
            },
            original_available: value.original_available,
            selection_state: selection_state(value.selection_state),
            rating: value.rating,
            decision_version: value.decision_version,
            removed_at_ms: value.removed_at_ms,
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

fn export_format(workload: &str) -> (&'static str, &'static str, &'static str) {
    match workload {
        "development-tiff" => ("development-tiff", "develop", "image/tiff"),
        "film-jpeg" => ("film-jpeg", "film", "image/jpeg"),
        _ => unreachable!("validated Export workload"),
    }
}

/// The closed artifact object, or `null` when no validated artifact is
/// retained.
pub(crate) fn export_artifact_object(
    record: &slipstream_core::ExportRecord,
) -> Option<ExportArtifactWire> {
    let artifact = record.artifact.as_ref()?;
    let (target, stage, content_type) = export_format(&record.snapshot.workload);
    Some(ExportArtifactWire {
        export_id: record.id.clone(),
        target,
        stage,
        content_type,
        width: artifact.width,
        height: artifact.height,
        profile_identity: artifact.profile_identity.clone(),
        byte_length: artifact.size,
        sha256: artifact.sha256.clone(),
        expires_at: export_time(artifact.expires_at),
    })
}

pub(crate) fn export_submit(record: &slipstream_core::ExportRecord) -> ExportSubmitWire {
    let (target, _, _) = export_format(&record.snapshot.workload);
    ExportSubmitWire {
        export_id: record.id.clone(),
        state: record.state.name(),
        target,
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
    let (target, _, _) = export_format(&record.snapshot.workload);
    ExportSummaryWire {
        export_id: record.id.clone(),
        state: record.state.name(),
        target,
    }
}

pub(crate) fn export_inspect(record: &slipstream_core::ExportRecord) -> ExportInspectWire {
    let (target, _, _) = export_format(&record.snapshot.workload);
    ExportInspectWire {
        export_id: record.id.clone(),
        photo_id: record.snapshot.photo_id.clone(),
        state: record.state.name(),
        target,
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
