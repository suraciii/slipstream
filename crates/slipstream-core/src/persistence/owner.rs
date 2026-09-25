use super::{
    DatabaseName, SchemaVersion, StateDirectory, StateError, StateFileIdentity,
    admission::StateDatabaseLock, validate_canonical_schema,
};
use crate::{
    ALBUM_MEMBERSHIP_BATCH_MAX, AlbumBrowseMember, AlbumBrowseTarget, AlbumCreationResult,
    AlbumMember, AlbumMembershipMutation, AlbumMembershipResult, AlbumMutation,
    AlbumMutationResult, AlbumQueryFilter, AlbumRecord, AlbumSummary, AppliedRelocations,
    CaptureFact, CaptureMetadataState, CaptureTimeField, CheckedAlbumMutation,
    CheckedAlbumMutationResult, CheckedPhotoDecisionCounts, CheckedPhotoDecisionItemResult,
    CheckedPhotoDecisionMutation, CheckedPhotoDecisionOutcome, CheckedPhotoDecisionResult,
    DiscoveredOriginal, EXPORT_DEVELOPMENT_TIFF_WORKLOAD, EXPORT_RETENTION_SECONDS, EditRecipe,
    EditRecipeRead, EditRecipeSettings, EditRecipeWriteOutcome, ExportArtifactFacts, ExportAttempt,
    ExportExposureRange, ExportLeaseOutcome, ExportRecipePayload, ExportRecord, ExportRetryOutcome,
    ExportSettlement, ExportSnapshot, ExportSourceEvidence, ExportState, ExportSubmission,
    ExportSubmissionResolution, ExportSubmitOutcome, ExportSweepResult, LibraryRoot,
    MAXIMUM_FOLDER_ALBUM_PHOTOS, MAXIMUM_PHOTO_RATING, OriginalErrorCategory, OriginalFacts,
    OriginalFingerprint, OriginalKind, OriginalRecord, OriginalScanError, PhotoAlbumMembership,
    PhotoDecisionFacts, PhotoDecisionSnapshot, PhotoOperationRemainder, PhotoQuery,
    PhotoQueryCandidate, PhotoQueryError, PhotoQueryOrder, PhotoQueryProjection, PhotoQuerySource,
    PhotoRead, PhotoRecord, PhotoRemovalCounts, PhotoRemovalMutation, PhotoRemovalResult,
    PhotoRestoration, PhotoRestorationCounts, PhotoRestorationResult, PhotoStateBatchApplied,
    PhotoStateBatchChangedElsewhere, PhotoStateBatchMissing, PhotoStateBatchMutation,
    PhotoStateBatchResult, PhotoStateField, PhotoStateMutation, PhotoStateMutationResult,
    PhotoStateUndo, PhotoStateValue, PreviewSeed, PreviewSeedResult, PreviewState,
    RebindEditRecipe, RecoverySurvey, RelativeOriginalPath, RemovedPhotoRecord,
    RequestedRelocation, SaveEditRecipe, ScanLimits, ScanSnapshot, SelectionState,
    UnavailablePhotoRecord, WhiteBalanceIntent,
    identity::classify_name,
    reconcile::{preview_should_preserve, reconcile, selected_source},
};
use rusqlite::{
    Connection, ErrorCode, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
    params_from_iter, types::Value,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    num::NonZeroUsize,
    path::Path,
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, SyncSender, TrySendError, sync_channel},
    },
    thread::{self, JoinHandle},
};
use tokio::sync::oneshot;

#[cfg(test)]
const DEFAULT_QUEUE_CAPACITY: usize = 64;
const STATE_OPEN: u8 = 0;
const STATE_CLOSING: u8 = 1;
const STATE_CLOSED: u8 = 2;
const SCHEMA_V1_SQL: &str = include_str!("../../../../compatibility/sqlite/schema-v1.sql");
// Save receipts stay durable at this internal boundary. Before Web or CLI
// exposes request identities, the protocol must add an explicit age, expiry,
// and expired-identity outcome; deleting keys without that contract could
// allow an old identity to be reused for a different save.
const EDIT_RECIPE_RECEIPT_PREFIX: &str = "edit_recipe_receipt:";
const MAXIMUM_EDIT_RECIPE_REQUEST_ID_BYTES: usize = 128;

#[derive(Clone, Debug, Deserialize, Serialize)]
struct EditRecipeReceipt {
    photo_id: String,
    payload_digest: String,
    outcome: EditRecipeReceiptOutcome,
    revision: String,
    source_revision: String,
    exposure_ev: f64,
    white_balance_mode: String,
    #[serde(default)]
    temperature_kelvin: Option<i32>,
    #[serde(default)]
    tint_milli: Option<i32>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
enum EditRecipeReceiptOutcome {
    Saved,
    Unchanged,
}

struct MutationVersions {
    epoch: String,
    photo: HashMap<String, u64>,
    album: HashMap<String, u64>,
}

impl MutationVersions {
    fn new() -> Result<Self, PersistenceError> {
        Ok(Self {
            epoch: random_uuid_v4()?,
            photo: HashMap::new(),
            album: HashMap::new(),
        })
    }

    fn photo(&self, id: &str) -> String {
        self.token("photo", id, *self.photo.get(id).unwrap_or(&0))
    }

    fn album(&self, id: &str) -> String {
        self.token("album", id, *self.album.get(id).unwrap_or(&0))
    }

    fn can_advance_photo(&self, id: &str) -> bool {
        self.photo.get(id).copied().unwrap_or(0) < u64::MAX
    }

    fn can_advance_album(&self, id: &str) -> bool {
        self.album.get(id).copied().unwrap_or(0) < u64::MAX
    }

    fn advance_photo(&mut self, id: &str) -> Result<(), MutationError> {
        advance_counter(&mut self.photo, id)
    }

    fn advance_album(&mut self, id: &str) -> Result<(), MutationError> {
        advance_counter(&mut self.album, id)
    }

    fn token(&self, kind: &str, id: &str, counter: u64) -> String {
        format!("{}:{kind}:{id}:{counter}", self.epoch)
    }
}

fn advance_counter(counters: &mut HashMap<String, u64>, id: &str) -> Result<(), MutationError> {
    let counter = counters.entry(id.to_owned()).or_default();
    *counter = counter.checked_add(1).ok_or(MutationError::Persistence)?;
    Ok(())
}

#[derive(Clone, Debug)]
pub enum PersistenceError {
    Saturated,
    Closed,
    State(StateError),
    RecoveryRequired,
    UnsupportedSchema,
    NewerSchema,
    RootMismatch,
    InvalidLegacyData,
    InvalidExpansion,
    InvalidRecovery,
    InvalidRecoveryMapping {
        original_id: String,
        reason: &'static str,
    },
    IdCollision,
    Storage,
    OwnerStopped,
}

impl fmt::Display for PersistenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Saturated => "SQLite persistence queue is saturated",
            Self::Closed => "SQLite persistence is closed",
            Self::State(error) => return error.fmt(formatter),
            Self::RecoveryRequired => "SQLite state requires operator recovery",
            Self::UnsupportedSchema => "SQLite schema is unsupported",
            Self::NewerSchema => "SQLite schema version is newer than this Slipstream build",
            Self::RootMismatch => "SQLite database belongs to a different Photo Library root",
            Self::InvalidLegacyData => "SQLite legacy data cannot be migrated safely",
            Self::InvalidExpansion => "Photo Library expansion could not be proven safely",
            Self::InvalidRecovery => "Original File recovery could not be proven safely",
            Self::InvalidRecoveryMapping { .. } => {
                "Original File recovery could not be proven safely"
            }
            Self::IdCollision => "SQLite identity allocation collided with existing state",
            Self::Storage => "SQLite persistence failed",
            Self::OwnerStopped => "SQLite persistence owner stopped unexpectedly",
        })
    }
}

impl std::error::Error for PersistenceError {}

impl From<StateError> for PersistenceError {
    fn from(value: StateError) -> Self {
        match value {
            StateError::SidecarPresent => Self::RecoveryRequired,
            other => Self::State(other),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MutationError {
    /// The request itself is malformed: an empty, over-limit, or duplicate
    /// address set. It is refused before any state is read or written.
    Invalid,
    NotFound,
    Conflict,
    Persistence,
    Saturated,
    Closed,
}

impl fmt::Display for MutationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Invalid => "Mutation request is not valid",
            Self::NotFound => "Mutation target not found",
            Self::Conflict => "Mutation conflicts with current state",
            Self::Persistence => "Mutation could not be persisted",
            Self::Saturated => "SQLite persistence queue is saturated",
            Self::Closed => "SQLite persistence is closed",
        })
    }
}

impl std::error::Error for MutationError {}

/// Detailed refusal or storage outcome for checked Album creation and changes.
/// Identity-bearing variants let the HTTP owner map the domain result without
/// parsing display text or issuing a racy follow-up read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AlbumWriteError {
    Invalid,
    AlbumNotFound {
        album_id: String,
    },
    PhotoNotFound {
        photo_id: String,
    },
    VersionConflict {
        album_id: String,
        current_version: String,
    },
    NameConflict {
        name: String,
        album_id: String,
    },
    MembershipConflict {
        album_id: String,
        current_version: String,
    },
    LimitExceeded {
        limit: usize,
        actual: usize,
    },
    Persistence,
    Saturated,
    Closed,
}

impl fmt::Display for AlbumWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid => formatter.write_str("Album write request is not valid"),
            Self::AlbumNotFound { .. } => formatter.write_str("Album was not found"),
            Self::PhotoNotFound { .. } => formatter.write_str("Photo was not found"),
            Self::VersionConflict { .. } => {
                formatter.write_str("Album version conflicts with current state")
            }
            Self::NameConflict { .. } => formatter.write_str("Album name already exists"),
            Self::MembershipConflict { .. } => {
                formatter.write_str("Album order does not match current membership")
            }
            Self::LimitExceeded { .. } => formatter.write_str("Album write exceeds its limit"),
            Self::Persistence => formatter.write_str("Album write could not be persisted"),
            Self::Saturated => formatter.write_str("SQLite persistence queue is saturated"),
            Self::Closed => formatter.write_str("SQLite persistence is closed"),
        }
    }
}

impl std::error::Error for AlbumWriteError {}

fn album_write_error_from_persistence(error: PersistenceError) -> AlbumWriteError {
    match error {
        PersistenceError::Saturated => AlbumWriteError::Saturated,
        PersistenceError::Closed => AlbumWriteError::Closed,
        _ => AlbumWriteError::Persistence,
    }
}

fn album_write_error_from_mutation(error: MutationError) -> AlbumWriteError {
    match error {
        MutationError::Saturated => AlbumWriteError::Saturated,
        MutationError::Closed => AlbumWriteError::Closed,
        _ => AlbumWriteError::Persistence,
    }
}

/// Detailed refusal or storage outcome for checked Photo decision batches.
/// Conflicting and missing Photos are per-item domain results, so only
/// request validation and storage failures appear here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PhotoDecisionWriteError {
    Invalid,
    LimitExceeded { limit: usize, actual: usize },
    Persistence,
    Saturated,
    Closed,
}

impl fmt::Display for PhotoDecisionWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid => formatter.write_str("Photo decision write request is not valid"),
            Self::LimitExceeded { .. } => {
                formatter.write_str("Photo decision write exceeds its limit")
            }
            Self::Persistence => formatter.write_str("Photo decision write could not be persisted"),
            Self::Saturated => formatter.write_str("SQLite persistence queue is saturated"),
            Self::Closed => formatter.write_str("SQLite persistence is closed"),
        }
    }
}

impl std::error::Error for PhotoDecisionWriteError {}

fn photo_decision_write_error_from_persistence(error: PersistenceError) -> PhotoDecisionWriteError {
    match error {
        PersistenceError::Saturated => PhotoDecisionWriteError::Saturated,
        PersistenceError::Closed => PhotoDecisionWriteError::Closed,
        _ => PhotoDecisionWriteError::Persistence,
    }
}

fn photo_decision_write_error_from_mutation(error: MutationError) -> PhotoDecisionWriteError {
    match error {
        MutationError::Saturated => PhotoDecisionWriteError::Saturated,
        MutationError::Closed => PhotoDecisionWriteError::Closed,
        _ => PhotoDecisionWriteError::Persistence,
    }
}

fn mutation_error_from_persistence(error: PersistenceError) -> MutationError {
    match error {
        PersistenceError::Saturated => MutationError::Saturated,
        PersistenceError::Closed => MutationError::Closed,
        PersistenceError::OwnerStopped
        | PersistenceError::State(_)
        | PersistenceError::RecoveryRequired
        | PersistenceError::UnsupportedSchema
        | PersistenceError::NewerSchema
        | PersistenceError::RootMismatch
        | PersistenceError::InvalidLegacyData
        | PersistenceError::InvalidExpansion
        | PersistenceError::InvalidRecovery
        | PersistenceError::InvalidRecoveryMapping { .. }
        | PersistenceError::IdCollision
        | PersistenceError::Storage => MutationError::Persistence,
    }
}

fn normalize_album_mutation(mutation: AlbumMutation) -> Result<AlbumMutation, MutationError> {
    let trim_name = |name: String| {
        let name = name.trim().to_owned();
        if name.is_empty() || name.chars().count() > 120 {
            Err(MutationError::Conflict)
        } else {
            Ok(name)
        }
    };
    match mutation {
        AlbumMutation::Create { name } => Ok(AlbumMutation::Create {
            name: trim_name(name)?,
        }),
        AlbumMutation::Rename { album_id, name } => Ok(AlbumMutation::Rename {
            album_id,
            name: trim_name(name)?,
        }),
        AlbumMutation::AddMembers {
            album_id,
            photo_ids,
        } => {
            if photo_ids.len() > 100
                || photo_ids
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != photo_ids.len()
            {
                return Err(MutationError::Conflict);
            }
            Ok(AlbumMutation::AddMembers {
                album_id,
                photo_ids,
            })
        }
        AlbumMutation::AddFolderMembers {
            album_id,
            photo_ids,
        } => {
            if photo_ids.len() > MAXIMUM_FOLDER_ALBUM_PHOTOS
                || photo_ids
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != photo_ids.len()
            {
                return Err(MutationError::Conflict);
            }
            Ok(AlbumMutation::AddFolderMembers {
                album_id,
                photo_ids,
            })
        }
        AlbumMutation::Delete { .. }
        | AlbumMutation::RemoveMember { .. }
        | AlbumMutation::Reorder { .. }
        | AlbumMutation::SetProgress { .. } => Ok(mutation),
    }
}

fn normalize_album_membership_mutation(
    mutation: AlbumMembershipMutation,
) -> Result<AlbumMembershipMutation, MutationError> {
    let (album_id, photo_ids, kind) = match mutation {
        AlbumMembershipMutation::Add {
            album_id,
            photo_ids,
        } => (album_id, photo_ids, 0_u8),
        AlbumMembershipMutation::RemoveAdded {
            album_id,
            photo_ids,
        } => (album_id, photo_ids, 1_u8),
    };
    if album_id.trim().is_empty()
        || photo_ids.is_empty()
        || photo_ids.len() > ALBUM_MEMBERSHIP_BATCH_MAX
        || photo_ids.iter().any(|photo_id| photo_id.trim().is_empty())
        || photo_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != photo_ids.len()
    {
        return Err(MutationError::Invalid);
    }
    Ok(if kind == 0 {
        AlbumMembershipMutation::Add {
            album_id,
            photo_ids,
        }
    } else {
        AlbumMembershipMutation::RemoveAdded {
            album_id,
            photo_ids,
        }
    })
}

fn normalize_album_name(name: String) -> Result<String, AlbumWriteError> {
    let name = name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 120 {
        Err(AlbumWriteError::Invalid)
    } else {
        Ok(name)
    }
}

fn normalize_checked_album_mutation(
    mutation: CheckedAlbumMutation,
) -> Result<CheckedAlbumMutation, AlbumWriteError> {
    let validate_target = |album_id: &str, expected_version: &str| {
        if album_id.trim().is_empty() || expected_version.is_empty() {
            Err(AlbumWriteError::Invalid)
        } else {
            Ok(())
        }
    };
    let validate_photo_ids = |photo_ids: &[String]| {
        if photo_ids.len() > ALBUM_MEMBERSHIP_BATCH_MAX {
            return Err(AlbumWriteError::LimitExceeded {
                limit: ALBUM_MEMBERSHIP_BATCH_MAX,
                actual: photo_ids.len(),
            });
        }
        if photo_ids.is_empty()
            || photo_ids.iter().any(|photo_id| photo_id.trim().is_empty())
            || photo_ids
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != photo_ids.len()
        {
            Err(AlbumWriteError::Invalid)
        } else {
            Ok(())
        }
    };
    match mutation {
        CheckedAlbumMutation::Rename {
            album_id,
            name,
            expected_version,
        } => {
            validate_target(&album_id, &expected_version)?;
            Ok(CheckedAlbumMutation::Rename {
                album_id,
                name: normalize_album_name(name)?,
                expected_version,
            })
        }
        CheckedAlbumMutation::Delete {
            album_id,
            expected_version,
        } => {
            validate_target(&album_id, &expected_version)?;
            Ok(CheckedAlbumMutation::Delete {
                album_id,
                expected_version,
            })
        }
        CheckedAlbumMutation::AddMembers {
            album_id,
            photo_ids,
            expected_version,
        } => {
            validate_target(&album_id, &expected_version)?;
            validate_photo_ids(&photo_ids)?;
            Ok(CheckedAlbumMutation::AddMembers {
                album_id,
                photo_ids,
                expected_version,
            })
        }
        CheckedAlbumMutation::RemoveMembers {
            album_id,
            photo_ids,
            expected_version,
        } => {
            validate_target(&album_id, &expected_version)?;
            validate_photo_ids(&photo_ids)?;
            Ok(CheckedAlbumMutation::RemoveMembers {
                album_id,
                photo_ids,
                expected_version,
            })
        }
        CheckedAlbumMutation::Reorder {
            album_id,
            photo_ids,
            expected_version,
        } => {
            validate_target(&album_id, &expected_version)?;
            validate_photo_ids(&photo_ids)?;
            Ok(CheckedAlbumMutation::Reorder {
                album_id,
                photo_ids,
                expected_version,
            })
        }
    }
}

fn normalize_checked_photo_decision_mutation(
    mutation: CheckedPhotoDecisionMutation,
) -> Result<CheckedPhotoDecisionMutation, PhotoDecisionWriteError> {
    if std::mem::discriminant(&mutation.value)
        != match mutation.field {
            PhotoStateField::SelectionState => {
                std::mem::discriminant(&PhotoStateValue::Selection(SelectionState::Undecided))
            }
            PhotoStateField::Rating => std::mem::discriminant(&PhotoStateValue::Rating(0)),
        }
        || matches!(mutation.value, PhotoStateValue::Rating(value) if value > MAXIMUM_PHOTO_RATING)
    {
        return Err(PhotoDecisionWriteError::Invalid);
    }
    if mutation.photos.len() > crate::PHOTO_STATE_BATCH_MAX {
        return Err(PhotoDecisionWriteError::LimitExceeded {
            limit: crate::PHOTO_STATE_BATCH_MAX,
            actual: mutation.photos.len(),
        });
    }
    if mutation.photos.is_empty()
        || mutation
            .photos
            .iter()
            .any(|photo| photo.photo_id.trim().is_empty() || photo.expected_version.is_empty())
        || mutation
            .photos
            .iter()
            .map(|photo| &photo.photo_id)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != mutation.photos.len()
    {
        return Err(PhotoDecisionWriteError::Invalid);
    }
    Ok(mutation)
}

fn validate_photo_state_mutation(mutation: &PhotoStateMutation) -> Result<(), MutationError> {
    if mutation.expected_current.is_some_and(|expected| {
        std::mem::discriminant(&expected) != std::mem::discriminant(&mutation.value)
    }) {
        return Err(MutationError::Conflict);
    }
    match (mutation.field, mutation.value) {
        (PhotoStateField::SelectionState, PhotoStateValue::Selection(_))
        | (PhotoStateField::Rating, PhotoStateValue::Rating(_)) => Ok(()),
        _ => Err(MutationError::Conflict),
    }
}

fn validate_photo_state_batch_mutation(
    mutation: &PhotoStateBatchMutation,
) -> Result<(), MutationError> {
    if mutation.value == SelectionState::Undecided
        || mutation.photos.is_empty()
        || mutation.photos.len() > crate::PHOTO_STATE_BATCH_MAX
        || mutation
            .photos
            .iter()
            .map(|photo| &photo.photo_id)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != mutation.photos.len()
        || mutation
            .photos
            .iter()
            .any(|photo| photo.photo_id.is_empty())
    {
        return Err(MutationError::Invalid);
    }
    Ok(())
}

type Reply<T> = oneshot::Sender<Result<T, PersistenceError>>;

/// Bounded per-Photo Album membership query result.
type PhotoAlbums = Result<Option<Vec<PhotoAlbumMembership>>, PersistenceError>;
type AlbumReadWindow = Result<Vec<Option<AlbumSummary>>, PersistenceError>;
type AlbumReadWindowReceiver = oneshot::Receiver<AlbumReadWindow>;
type PhotoReadWindow = Result<Vec<Option<PhotoRead>>, PersistenceError>;
type PhotoReadWindowReceiver = oneshot::Receiver<PhotoReadWindow>;
type PhotoExports = Result<Option<Vec<ExportRecord>>, PersistenceError>;
type PhotoExportsReceiver = oneshot::Receiver<PhotoExports>;
/// One bounded page of removed Photos with the complete removed count.
type RemovedPhotoPage = (Vec<RemovedPhotoRecord>, usize);
type RemovedPhotoPageResult = Result<RemovedPhotoPage, PersistenceError>;
type RemovedPhotoPageReceiver = oneshot::Receiver<RemovedPhotoPageResult>;

enum Command {
    Probe(Reply<u64>),
    Snapshot(Reply<ScanSnapshot>),
    ApplyScan {
        discovered: Vec<DiscoveredOriginal>,
        errors: Vec<OriginalScanError>,
        recovery: ScanRecoveryPlan,
        failure_after_first: bool,
        reply: Reply<ScanApplication>,
    },
    RecoveryFacts {
        original_ids: Vec<String>,
        reply: Reply<Vec<OriginalFingerprint>>,
    },
    NextFingerprintTarget(Reply<Option<FingerprintTarget>>),
    StoreFingerprint(OriginalFingerprint, Reply<()>),
    FingerprintCounts(Reply<FingerprintCounts>),
    RecoverySurvey(Reply<RecoverySurvey>),
    ApplyRelocations {
        relocations: Vec<RequestedRelocation>,
        reply: Reply<AppliedRelocations>,
    },
    Preview(PreviewSeed, Reply<PreviewSeedResult>),
    ListAlbums(Reply<Vec<AlbumRecord>>),
    ListAlbumSummaries(Reply<Vec<AlbumSummary>>),
    ReadAlbum {
        album_id: String,
        reply: Reply<Option<AlbumSummary>>,
    },
    ReadAlbums {
        album_ids: Vec<String>,
        reply: Reply<Vec<Option<AlbumSummary>>>,
    },
    CreateAlbumQuery {
        filter: AlbumQueryFilter,
        maximum_results: usize,
        reply: oneshot::Sender<Result<Vec<String>, PhotoQueryError>>,
    },
    ReadPhoto {
        photo_id: String,
        reply: Reply<Option<PhotoRead>>,
    },
    ReadEditRecipe {
        photo_id: String,
        reply: Reply<Option<EditRecipeRead>>,
    },
    SaveEditRecipe(SaveEditRecipe, Reply<EditRecipeWriteOutcome>),
    RebindEditRecipe(RebindEditRecipe, Reply<EditRecipeWriteOutcome>),
    ReadPhotos {
        photo_ids: Vec<String>,
        projection: Arc<PhotoQueryProjection>,
        reply: Reply<Vec<Option<PhotoRead>>>,
    },
    CreatePhotoQuery {
        query: PhotoQuery,
        projection: Arc<PhotoQueryProjection>,
        maximum_results: usize,
        reply: oneshot::Sender<Result<Vec<String>, PhotoQueryError>>,
    },
    PhotoAlbums {
        photo_id: String,
        reply: Reply<Option<Vec<PhotoAlbumMembership>>>,
    },
    AlbumBrowseTarget {
        album_id: String,
        reply: Reply<Option<AlbumBrowseTarget>>,
    },
    MutateAlbum(
        AlbumMutation,
        oneshot::Sender<Result<AlbumMutationResult, MutationError>>,
    ),
    MutateAlbumMembership(
        AlbumMembershipMutation,
        oneshot::Sender<Result<AlbumMembershipResult, MutationError>>,
    ),
    CreateAlbum(
        String,
        oneshot::Sender<Result<AlbumCreationResult, AlbumWriteError>>,
    ),
    MutateAlbumChecked(
        CheckedAlbumMutation,
        oneshot::Sender<Result<CheckedAlbumMutationResult, AlbumWriteError>>,
    ),
    MutatePhotoDecisionChecked(
        CheckedPhotoDecisionMutation,
        oneshot::Sender<Result<CheckedPhotoDecisionResult, PhotoDecisionWriteError>>,
    ),
    MutatePhotoState(
        PhotoStateMutation,
        oneshot::Sender<Result<PhotoStateMutationResult, MutationError>>,
    ),
    MutatePhotoStateBatch(
        PhotoStateBatchMutation,
        oneshot::Sender<Result<PhotoStateBatchResult, MutationError>>,
    ),
    RemovePhotos(
        PhotoRemovalMutation,
        oneshot::Sender<Result<PhotoRemovalResult, MutationError>>,
    ),
    RestorePhotos(
        PhotoRestoration,
        oneshot::Sender<Result<PhotoRestorationResult, MutationError>>,
    ),
    RemovedPhotos {
        start: usize,
        limit: usize,
        reply: Reply<RemovedPhotoPage>,
    },
    WriteProbe(Reply<()>),
    SubmitExport(ExportSubmission, Reply<ExportSubmitOutcome>),
    ReadExport {
        export_id: String,
        reply: Reply<Option<ExportRecord>>,
    },
    ListPhotoExports {
        photo_id: String,
        reply: Reply<Option<Vec<ExportRecord>>>,
    },
    CancelExport {
        export_id: String,
        reply: Reply<Option<ExportRecord>>,
    },
    SettleExport {
        export_id: String,
        settlement: ExportSettlement,
        reply: Reply<Option<ExportRecord>>,
    },
    BeginExportAttempt {
        export_id: String,
        attempt: ExportAttempt,
        reply: Reply<Option<ExportRecord>>,
    },
    RecordExportSource {
        export_id: String,
        size: u64,
        sha256: String,
        reply: Reply<Option<ExportRecord>>,
    },
    RetryExport {
        export_id: String,
        request_id: String,
        expected_bundle_id: String,
        allowance: u64,
        reply: Reply<ExportRetryOutcome>,
    },
    ResolveExportSubmission {
        photo_id: String,
        request_id: String,
        payload_digest: String,
        reply: Reply<Option<ExportSubmissionResolution>>,
    },
    ClaimExportPublication {
        export_id: String,
        incarnation: String,
        sequence: u64,
        reply: Reply<bool>,
    },
    ExportPublicationClaim {
        export_id: String,
        reply: Reply<Option<(String, u64)>>,
    },
    RenewExportLease {
        lease_id: String,
        now: u64,
        reply: Reply<bool>,
    },
    SweepExportExpiry {
        now: u64,
        reply: Reply<ExportSweepResult>,
    },
    UnfinishedExports(Reply<Vec<ExportRecord>>),
    AcquireExportLease {
        export_id: String,
        now: u64,
        reply: Reply<ExportLeaseOutcome>,
    },
    ReleaseExportLease {
        lease_id: String,
        reply: Reply<bool>,
    },
    #[cfg(test)]
    Configuration(Reply<(String, u8)>),
    #[cfg(test)]
    Block {
        entered: oneshot::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
        reply: Reply<()>,
    },
}

struct Admission {
    state: u8,
    sender: Option<SyncSender<Command>>,
}

struct Inner {
    admission: Mutex<Admission>,
    join: Mutex<Option<JoinHandle<()>>>,
    shutdown: Mutex<Option<Result<(), PersistenceError>>>,
}

#[derive(Clone)]
pub struct Persistence {
    inner: Arc<Inner>,
}

impl Persistence {
    #[cfg(test)]
    pub(crate) fn open(
        state: StateDirectory,
        database_name: DatabaseName,
        canonical_root: String,
    ) -> Result<Self, PersistenceError> {
        Self::open_with_capacity(
            state,
            database_name,
            canonical_root,
            NonZeroUsize::new(DEFAULT_QUEUE_CAPACITY).unwrap(),
        )
    }

    pub(crate) fn open_with_capacity(
        state: StateDirectory,
        database_name: DatabaseName,
        canonical_root: String,
        capacity: NonZeroUsize,
    ) -> Result<Self, PersistenceError> {
        let (identity, created) = state.prepare_database_with_creation(&database_name)?;
        if state.startup_sidecars_present(&database_name)? {
            if created {
                state.remove_created_empty_database(&database_name, identity)?;
            }
            return Err(PersistenceError::RecoveryRequired);
        }
        let database_lock = match state.lock_database(&database_name) {
            Ok(lock) => lock,
            Err(error) => {
                if created {
                    state.remove_created_empty_database(&database_name, identity)?;
                }
                return Err(error.into());
            }
        };
        state.verify_database(&database_name, identity)?;
        let (sender, receiver) = sync_channel(capacity.get());
        let (startup_send, startup_receive) = std::sync::mpsc::channel();
        let join = thread::Builder::new()
            .name("slipstream-sqlite".to_owned())
            .spawn(move || {
                owner_main(
                    state,
                    database_name,
                    identity,
                    database_lock,
                    canonical_root,
                    receiver,
                    startup_send,
                )
            })
            .map_err(|_| PersistenceError::OwnerStopped)?;
        let startup_result = startup_receive
            .recv()
            .unwrap_or(Err(PersistenceError::OwnerStopped));
        if let Err(error) = startup_result {
            if join.join().is_err() {
                return Err(PersistenceError::OwnerStopped);
            }
            return Err(error);
        }
        Ok(Self {
            inner: Arc::new(Inner {
                admission: Mutex::new(Admission {
                    state: STATE_OPEN,
                    sender: Some(sender),
                }),
                join: Mutex::new(Some(join)),
                shutdown: Mutex::new(None),
            }),
        })
    }

    pub async fn probe(&self) -> Result<u64, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::Probe(send))?;
        receive.await.unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub async fn snapshot(&self) -> Result<ScanSnapshot, PersistenceError> {
        let receive = self.snapshot_receiver()?;
        receive.await.unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn snapshot_receiver(
        &self,
    ) -> Result<oneshot::Receiver<Result<ScanSnapshot, PersistenceError>>, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::Snapshot(send))?;
        Ok(receive)
    }

    pub async fn apply_scan(
        &self,
        discovered: Vec<DiscoveredOriginal>,
        errors: Vec<OriginalScanError>,
    ) -> Result<ScanSnapshot, PersistenceError> {
        Ok(self
            .apply_scan_recovered(discovered, errors, ScanRecoveryPlan::default())
            .await?
            .snapshot)
    }

    pub async fn apply_scan_recovered(
        &self,
        discovered: Vec<DiscoveredOriginal>,
        errors: Vec<OriginalScanError>,
        recovery: ScanRecoveryPlan,
    ) -> Result<ScanApplication, PersistenceError> {
        let receive = self.apply_scan_recovered_receiver(discovered, errors, recovery)?;
        receive.await.unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    fn apply_scan_recovered_receiver(
        &self,
        discovered: Vec<DiscoveredOriginal>,
        errors: Vec<OriginalScanError>,
        recovery: ScanRecoveryPlan,
    ) -> Result<oneshot::Receiver<Result<ScanApplication, PersistenceError>>, PersistenceError>
    {
        let (send, receive) = oneshot::channel();
        self.submit(Command::ApplyScan {
            discovered,
            errors,
            recovery,
            failure_after_first: false,
            reply: send,
        })?;
        Ok(receive)
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) async fn apply_scan_failure(
        &self,
        discovered: Vec<DiscoveredOriginal>,
        errors: Vec<OriginalScanError>,
    ) -> Result<ScanApplication, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::ApplyScan {
            discovered,
            errors,
            recovery: ScanRecoveryPlan::default(),
            failure_after_first: true,
            reply: send,
        })?;
        receive.await.unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub async fn seed_preview(
        &self,
        preview: PreviewSeed,
    ) -> Result<PreviewSeedResult, PersistenceError> {
        let receive = self.seed_preview_receiver(preview)?;
        receive.await.unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn seed_preview_receiver(
        &self,
        preview: PreviewSeed,
    ) -> Result<oneshot::Receiver<Result<PreviewSeedResult, PersistenceError>>, PersistenceError>
    {
        let (send, receive) = oneshot::channel();
        self.submit(Command::Preview(preview, send))?;
        Ok(receive)
    }

    #[allow(dead_code)]
    pub(crate) fn snapshot_blocking(&self) -> Result<ScanSnapshot, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::Snapshot(send))?;
        receive
            .blocking_recv()
            .unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    #[allow(dead_code)]
    pub(crate) fn apply_scan_blocking(
        &self,
        discovered: Vec<DiscoveredOriginal>,
        errors: Vec<OriginalScanError>,
    ) -> Result<ScanSnapshot, PersistenceError> {
        Ok(self
            .apply_scan_recovered_blocking(discovered, errors, ScanRecoveryPlan::default())?
            .snapshot)
    }

    pub(crate) fn apply_scan_recovered_blocking(
        &self,
        discovered: Vec<DiscoveredOriginal>,
        errors: Vec<OriginalScanError>,
        recovery: ScanRecoveryPlan,
    ) -> Result<ScanApplication, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::ApplyScan {
            discovered,
            errors,
            recovery,
            failure_after_first: false,
            reply: send,
        })?;
        receive
            .blocking_recv()
            .unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn recovery_facts_blocking(
        &self,
        original_ids: Vec<String>,
    ) -> Result<Vec<OriginalFingerprint>, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::RecoveryFacts {
            original_ids,
            reply: send,
        })?;
        receive
            .blocking_recv()
            .unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn next_fingerprint_target_blocking(
        &self,
    ) -> Result<Option<FingerprintTarget>, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::NextFingerprintTarget(send))?;
        receive
            .blocking_recv()
            .unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn store_fingerprint_blocking(
        &self,
        fingerprint: OriginalFingerprint,
    ) -> Result<(), PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::StoreFingerprint(fingerprint, send))?;
        receive
            .blocking_recv()
            .unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn fingerprint_counts_blocking(
        &self,
    ) -> Result<FingerprintCounts, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::FingerprintCounts(send))?;
        receive
            .blocking_recv()
            .unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn recovery_survey_receiver(
        &self,
    ) -> Result<oneshot::Receiver<Result<RecoverySurvey, PersistenceError>>, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::RecoverySurvey(send))?;
        Ok(receive)
    }

    pub(crate) fn apply_relocations_receiver(
        &self,
        relocations: Vec<RequestedRelocation>,
    ) -> Result<oneshot::Receiver<Result<AppliedRelocations, PersistenceError>>, PersistenceError>
    {
        let (send, receive) = oneshot::channel();
        self.submit(Command::ApplyRelocations {
            relocations,
            reply: send,
        })?;
        Ok(receive)
    }

    #[allow(dead_code)]
    pub(crate) fn seed_preview_blocking(
        &self,
        preview: PreviewSeed,
    ) -> Result<PreviewSeedResult, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::Preview(preview, send))?;
        receive
            .blocking_recv()
            .unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub async fn list_albums(&self) -> Result<Vec<AlbumRecord>, PersistenceError> {
        let receive = self.list_albums_receiver()?;
        receive.await.unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn list_albums_receiver(
        &self,
    ) -> Result<oneshot::Receiver<Result<Vec<AlbumRecord>, PersistenceError>>, PersistenceError>
    {
        let (send, receive) = oneshot::channel();
        self.submit(Command::ListAlbums(send))?;
        Ok(receive)
    }

    /// Bounded summaries for browser routes: counts and saved-position
    /// existence stay in SQL; no member rows are materialized.
    pub(crate) fn list_album_summaries_receiver(
        &self,
    ) -> Result<oneshot::Receiver<Result<Vec<AlbumSummary>, PersistenceError>>, PersistenceError>
    {
        let (send, receive) = oneshot::channel();
        self.submit(Command::ListAlbumSummaries(send))?;
        Ok(receive)
    }

    pub(crate) fn album_receiver(
        &self,
        album_id: &str,
    ) -> Result<oneshot::Receiver<Result<Option<AlbumSummary>, PersistenceError>>, PersistenceError>
    {
        let (send, receive) = oneshot::channel();
        self.submit(Command::ReadAlbum {
            album_id: album_id.to_owned(),
            reply: send,
        })?;
        Ok(receive)
    }

    pub(crate) fn albums_by_id_receiver(
        &self,
        album_ids: Vec<String>,
    ) -> Result<AlbumReadWindowReceiver, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::ReadAlbums {
            album_ids,
            reply: send,
        })?;
        Ok(receive)
    }

    pub(crate) fn create_album_query_receiver(
        &self,
        filter: AlbumQueryFilter,
        maximum_results: usize,
    ) -> Result<oneshot::Receiver<Result<Vec<String>, PhotoQueryError>>, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::CreateAlbumQuery {
            filter,
            maximum_results,
            reply: send,
        })?;
        Ok(receive)
    }

    pub(crate) fn photo_receiver(
        &self,
        photo_id: &str,
    ) -> Result<oneshot::Receiver<Result<Option<PhotoRead>, PersistenceError>>, PersistenceError>
    {
        let (send, receive) = oneshot::channel();
        self.submit(Command::ReadPhoto {
            photo_id: photo_id.to_owned(),
            reply: send,
        })?;
        Ok(receive)
    }

    pub(crate) fn edit_recipe_receiver(
        &self,
        photo_id: &str,
    ) -> Result<oneshot::Receiver<Result<Option<EditRecipeRead>, PersistenceError>>, PersistenceError>
    {
        let (send, receive) = oneshot::channel();
        self.submit(Command::ReadEditRecipe {
            photo_id: photo_id.to_owned(),
            reply: send,
        })?;
        Ok(receive)
    }

    pub(crate) fn save_edit_recipe_receiver(
        &self,
        mutation: SaveEditRecipe,
    ) -> Result<oneshot::Receiver<Result<EditRecipeWriteOutcome, PersistenceError>>, PersistenceError>
    {
        let (send, receive) = oneshot::channel();
        self.submit(Command::SaveEditRecipe(mutation, send))?;
        Ok(receive)
    }

    pub(crate) fn rebind_edit_recipe_receiver(
        &self,
        mutation: RebindEditRecipe,
    ) -> Result<oneshot::Receiver<Result<EditRecipeWriteOutcome, PersistenceError>>, PersistenceError>
    {
        let (send, receive) = oneshot::channel();
        self.submit(Command::RebindEditRecipe(mutation, send))?;
        Ok(receive)
    }

    pub(crate) fn submit_export_receiver(
        &self,
        submission: ExportSubmission,
    ) -> Result<oneshot::Receiver<Result<ExportSubmitOutcome, PersistenceError>>, PersistenceError>
    {
        let (send, receive) = oneshot::channel();
        self.submit(Command::SubmitExport(submission, send))?;
        Ok(receive)
    }

    pub(crate) fn export_receiver(
        &self,
        export_id: &str,
    ) -> Result<oneshot::Receiver<Result<Option<ExportRecord>, PersistenceError>>, PersistenceError>
    {
        let (send, receive) = oneshot::channel();
        self.submit(Command::ReadExport {
            export_id: export_id.to_owned(),
            reply: send,
        })?;
        Ok(receive)
    }

    pub(crate) fn photo_exports_receiver(
        &self,
        photo_id: &str,
    ) -> Result<PhotoExportsReceiver, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::ListPhotoExports {
            photo_id: photo_id.to_owned(),
            reply: send,
        })?;
        Ok(receive)
    }

    pub(crate) fn cancel_export_receiver(
        &self,
        export_id: &str,
    ) -> Result<oneshot::Receiver<Result<Option<ExportRecord>, PersistenceError>>, PersistenceError>
    {
        let (send, receive) = oneshot::channel();
        self.submit(Command::CancelExport {
            export_id: export_id.to_owned(),
            reply: send,
        })?;
        Ok(receive)
    }

    pub(crate) fn settle_export_receiver(
        &self,
        export_id: &str,
        settlement: ExportSettlement,
    ) -> Result<oneshot::Receiver<Result<Option<ExportRecord>, PersistenceError>>, PersistenceError>
    {
        let (send, receive) = oneshot::channel();
        self.submit(Command::SettleExport {
            export_id: export_id.to_owned(),
            settlement,
            reply: send,
        })?;
        Ok(receive)
    }

    pub(crate) fn begin_export_attempt_receiver(
        &self,
        export_id: &str,
        attempt: ExportAttempt,
    ) -> Result<oneshot::Receiver<Result<Option<ExportRecord>, PersistenceError>>, PersistenceError>
    {
        let (send, receive) = oneshot::channel();
        self.submit(Command::BeginExportAttempt {
            export_id: export_id.to_owned(),
            attempt,
            reply: send,
        })?;
        Ok(receive)
    }

    pub(crate) fn record_export_source_receiver(
        &self,
        export_id: &str,
        size: u64,
        sha256: &str,
    ) -> Result<oneshot::Receiver<Result<Option<ExportRecord>, PersistenceError>>, PersistenceError>
    {
        let (send, receive) = oneshot::channel();
        self.submit(Command::RecordExportSource {
            export_id: export_id.to_owned(),
            size,
            sha256: sha256.to_owned(),
            reply: send,
        })?;
        Ok(receive)
    }

    pub(crate) fn retry_export_receiver(
        &self,
        export_id: &str,
        request_id: &str,
        expected_bundle_id: &str,
        allowance: u64,
    ) -> Result<oneshot::Receiver<Result<ExportRetryOutcome, PersistenceError>>, PersistenceError>
    {
        let (send, receive) = oneshot::channel();
        self.submit(Command::RetryExport {
            export_id: export_id.to_owned(),
            request_id: request_id.to_owned(),
            expected_bundle_id: expected_bundle_id.to_owned(),
            allowance,
            reply: send,
        })?;
        Ok(receive)
    }

    /// Resolves a request identity without any admission or state change: a
    /// recorded identity replays, expires, or conflicts before the submit
    /// transaction ever runs.
    pub(crate) fn resolve_export_submission_receiver(
        &self,
        photo_id: &str,
        request_id: &str,
        payload_digest: &str,
    ) -> Result<
        oneshot::Receiver<Result<Option<ExportSubmissionResolution>, PersistenceError>>,
        PersistenceError,
    > {
        let (send, receive) = oneshot::channel();
        self.submit(Command::ResolveExportSubmission {
            photo_id: photo_id.to_owned(),
            request_id: request_id.to_owned(),
            payload_digest: payload_digest.to_owned(),
            reply: send,
        })?;
        Ok(receive)
    }

    /// Durably claims the publication of one attempt before its artifact is
    /// renamed into place; `false` means the claim could not be written.
    pub(crate) fn claim_export_publication_receiver(
        &self,
        export_id: &str,
        incarnation: &str,
        sequence: u64,
    ) -> Result<oneshot::Receiver<Result<bool, PersistenceError>>, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::ClaimExportPublication {
            export_id: export_id.to_owned(),
            incarnation: incarnation.to_owned(),
            sequence,
            reply: send,
        })?;
        Ok(receive)
    }

    /// Reads the durable publication claim of an Export, if any.
    pub(crate) fn export_publication_claim_receiver(
        &self,
        export_id: &str,
    ) -> Result<oneshot::Receiver<ExportPublicationClaimReply>, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::ExportPublicationClaim {
            export_id: export_id.to_owned(),
            reply: send,
        })?;
        Ok(receive)
    }

    /// Refreshes a download lease's liveness anchor while its stream runs.
    pub(crate) fn renew_export_lease_receiver(
        &self,
        lease_id: &str,
        now: u64,
    ) -> Result<oneshot::Receiver<Result<bool, PersistenceError>>, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::RenewExportLease {
            lease_id: lease_id.to_owned(),
            now,
            reply: send,
        })?;
        Ok(receive)
    }

    pub(crate) fn sweep_export_expiry_receiver(
        &self,
        now: u64,
    ) -> Result<oneshot::Receiver<Result<ExportSweepResult, PersistenceError>>, PersistenceError>
    {
        let (send, receive) = oneshot::channel();
        self.submit(Command::SweepExportExpiry { now, reply: send })?;
        Ok(receive)
    }

    pub(crate) fn unfinished_exports_receiver(
        &self,
    ) -> Result<oneshot::Receiver<Result<Vec<ExportRecord>, PersistenceError>>, PersistenceError>
    {
        let (send, receive) = oneshot::channel();
        self.submit(Command::UnfinishedExports(send))?;
        Ok(receive)
    }

    pub(crate) fn acquire_export_lease_receiver(
        &self,
        export_id: &str,
        now: u64,
    ) -> Result<oneshot::Receiver<Result<ExportLeaseOutcome, PersistenceError>>, PersistenceError>
    {
        let (send, receive) = oneshot::channel();
        self.submit(Command::AcquireExportLease {
            export_id: export_id.to_owned(),
            now,
            reply: send,
        })?;
        Ok(receive)
    }

    pub(crate) fn release_export_lease_receiver(
        &self,
        lease_id: &str,
    ) -> Result<oneshot::Receiver<Result<bool, PersistenceError>>, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::ReleaseExportLease {
            lease_id: lease_id.to_owned(),
            reply: send,
        })?;
        Ok(receive)
    }

    pub(crate) fn photos_by_id_receiver(
        &self,
        photo_ids: Vec<String>,
        projection: Arc<PhotoQueryProjection>,
    ) -> Result<PhotoReadWindowReceiver, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::ReadPhotos {
            photo_ids,
            projection,
            reply: send,
        })?;
        Ok(receive)
    }

    pub(crate) fn create_photo_query_receiver(
        &self,
        query: PhotoQuery,
        projection: Arc<PhotoQueryProjection>,
        maximum_results: usize,
    ) -> Result<oneshot::Receiver<Result<Vec<String>, PhotoQueryError>>, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::CreatePhotoQuery {
            query,
            projection,
            maximum_results,
            reply: send,
        })?;
        Ok(receive)
    }

    /// Bounded per-Photo membership: the Albums containing one Photo, in
    /// Album-list order, without materializing any member list.
    pub(crate) fn photo_albums_receiver(
        &self,
        photo_id: &str,
    ) -> Result<oneshot::Receiver<PhotoAlbums>, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::PhotoAlbums {
            photo_id: photo_id.to_owned(),
            reply: send,
        })?;
        Ok(receive)
    }

    /// Ordered membership identity for one Album's Browse Snapshot.
    pub(crate) fn album_browse_target_receiver(
        &self,
        album_id: &str,
    ) -> Result<
        oneshot::Receiver<Result<Option<AlbumBrowseTarget>, PersistenceError>>,
        PersistenceError,
    > {
        let (send, receive) = oneshot::channel();
        self.submit(Command::AlbumBrowseTarget {
            album_id: album_id.to_owned(),
            reply: send,
        })?;
        Ok(receive)
    }

    pub async fn mutate_album(
        &self,
        mutation: AlbumMutation,
    ) -> Result<AlbumMutationResult, MutationError> {
        let receive = self.mutate_album_receiver(mutation)?;
        receive.await.unwrap_or(Err(MutationError::Persistence))
    }

    pub(crate) fn mutate_album_receiver(
        &self,
        mutation: AlbumMutation,
    ) -> Result<oneshot::Receiver<Result<AlbumMutationResult, MutationError>>, MutationError> {
        let mutation = normalize_album_mutation(mutation)?;
        let (send, receive) = oneshot::channel();
        self.submit(Command::MutateAlbum(mutation, send))
            .map_err(mutation_error_from_persistence)?;
        Ok(receive)
    }

    pub async fn mutate_album_membership(
        &self,
        mutation: AlbumMembershipMutation,
    ) -> Result<AlbumMembershipResult, MutationError> {
        let receive = self.mutate_album_membership_receiver(mutation)?;
        receive.await.unwrap_or(Err(MutationError::Persistence))
    }

    pub(crate) fn mutate_album_membership_receiver(
        &self,
        mutation: AlbumMembershipMutation,
    ) -> Result<oneshot::Receiver<Result<AlbumMembershipResult, MutationError>>, MutationError>
    {
        let mutation = normalize_album_membership_mutation(mutation)?;
        let (send, receive) = oneshot::channel();
        self.submit(Command::MutateAlbumMembership(mutation, send))
            .map_err(mutation_error_from_persistence)?;
        Ok(receive)
    }

    pub async fn create_album_checked(
        &self,
        name: String,
    ) -> Result<AlbumCreationResult, AlbumWriteError> {
        let receive = self.create_album_checked_receiver(name)?;
        receive.await.unwrap_or(Err(AlbumWriteError::Persistence))
    }

    pub(crate) fn create_album_checked_receiver(
        &self,
        name: String,
    ) -> Result<oneshot::Receiver<Result<AlbumCreationResult, AlbumWriteError>>, AlbumWriteError>
    {
        let name = normalize_album_name(name)?;
        let (send, receive) = oneshot::channel();
        self.submit(Command::CreateAlbum(name, send))
            .map_err(album_write_error_from_persistence)?;
        Ok(receive)
    }

    pub async fn mutate_album_checked(
        &self,
        mutation: CheckedAlbumMutation,
    ) -> Result<CheckedAlbumMutationResult, AlbumWriteError> {
        let receive = self.mutate_album_checked_receiver(mutation)?;
        receive.await.unwrap_or(Err(AlbumWriteError::Persistence))
    }

    pub(crate) fn mutate_album_checked_receiver(
        &self,
        mutation: CheckedAlbumMutation,
    ) -> Result<
        oneshot::Receiver<Result<CheckedAlbumMutationResult, AlbumWriteError>>,
        AlbumWriteError,
    > {
        let mutation = normalize_checked_album_mutation(mutation)?;
        let (send, receive) = oneshot::channel();
        self.submit(Command::MutateAlbumChecked(mutation, send))
            .map_err(album_write_error_from_persistence)?;
        Ok(receive)
    }

    pub async fn mutate_photo_decision_checked(
        &self,
        mutation: CheckedPhotoDecisionMutation,
    ) -> Result<CheckedPhotoDecisionResult, PhotoDecisionWriteError> {
        let receive = self.mutate_photo_decision_checked_receiver(mutation)?;
        receive
            .await
            .unwrap_or(Err(PhotoDecisionWriteError::Persistence))
    }

    pub(crate) fn mutate_photo_decision_checked_receiver(
        &self,
        mutation: CheckedPhotoDecisionMutation,
    ) -> Result<
        oneshot::Receiver<Result<CheckedPhotoDecisionResult, PhotoDecisionWriteError>>,
        PhotoDecisionWriteError,
    > {
        let mutation = normalize_checked_photo_decision_mutation(mutation)?;
        let (send, receive) = oneshot::channel();
        self.submit(Command::MutatePhotoDecisionChecked(mutation, send))
            .map_err(photo_decision_write_error_from_persistence)?;
        Ok(receive)
    }

    pub async fn mutate_photo_state(
        &self,
        mutation: PhotoStateMutation,
    ) -> Result<PhotoStateMutationResult, MutationError> {
        let receive = self.mutate_photo_state_receiver(mutation)?;
        receive.await.unwrap_or(Err(MutationError::Persistence))
    }

    pub(crate) fn mutate_photo_state_receiver(
        &self,
        mutation: PhotoStateMutation,
    ) -> Result<oneshot::Receiver<Result<PhotoStateMutationResult, MutationError>>, MutationError>
    {
        validate_photo_state_mutation(&mutation)?;
        let (send, receive) = oneshot::channel();
        self.submit(Command::MutatePhotoState(mutation, send))
            .map_err(mutation_error_from_persistence)?;
        Ok(receive)
    }

    pub(crate) fn mutate_photo_state_batch_receiver(
        &self,
        mutation: PhotoStateBatchMutation,
    ) -> Result<oneshot::Receiver<Result<PhotoStateBatchResult, MutationError>>, MutationError>
    {
        validate_photo_state_batch_mutation(&mutation)?;
        let (send, receive) = oneshot::channel();
        self.submit(Command::MutatePhotoStateBatch(mutation, send))
            .map_err(mutation_error_from_persistence)?;
        Ok(receive)
    }

    pub(crate) fn remove_photos_receiver(
        &self,
        mutation: PhotoRemovalMutation,
    ) -> Result<oneshot::Receiver<Result<PhotoRemovalResult, MutationError>>, MutationError> {
        if mutation.photo_ids.is_empty() || mutation.operation_id.is_empty() {
            return Err(MutationError::Invalid);
        }
        let (send, receive) = oneshot::channel();
        self.submit(Command::RemovePhotos(mutation, send))
            .map_err(mutation_error_from_persistence)?;
        Ok(receive)
    }

    pub(crate) fn restore_photos_receiver(
        &self,
        restoration: PhotoRestoration,
    ) -> Result<oneshot::Receiver<Result<PhotoRestorationResult, MutationError>>, MutationError>
    {
        if let PhotoRestoration::Photos(markers) = &restoration
            && markers.is_empty()
        {
            return Err(MutationError::Invalid);
        }
        let (send, receive) = oneshot::channel();
        self.submit(Command::RestorePhotos(restoration, send))
            .map_err(mutation_error_from_persistence)?;
        Ok(receive)
    }

    pub(crate) fn removed_photos_receiver(
        &self,
        start: usize,
        limit: usize,
    ) -> Result<RemovedPhotoPageReceiver, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::RemovedPhotos {
            start,
            limit,
            reply: send,
        })?;
        Ok(receive)
    }

    pub async fn write_probe(&self) -> Result<(), PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::WriteProbe(send))?;
        receive.await.unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    fn submit(&self, command: Command) -> Result<(), PersistenceError> {
        let admission = self.inner.admission.lock().unwrap();
        if admission.state != STATE_OPEN {
            return Err(PersistenceError::Closed);
        }
        let sender = admission.sender.as_ref().ok_or(PersistenceError::Closed)?;
        sender.try_send(command).map_err(|error| match error {
            TrySendError::Full(_) => PersistenceError::Saturated,
            TrySendError::Disconnected(_) => PersistenceError::OwnerStopped,
        })
    }

    pub fn shutdown(&self) -> Result<(), PersistenceError> {
        let mut shutdown = self.inner.shutdown.lock().unwrap();
        if let Some(result) = shutdown.clone() {
            return result;
        }

        // Hold the admission lock while transitioning and dropping the sender.
        // submit() holds the same lock through try_send(), so no command can
        // be accepted after shutdown begins and no accepted command is lost.
        {
            let mut admission = self.inner.admission.lock().unwrap();
            admission.state = STATE_CLOSING;
            admission.sender.take();
        }
        let result = self
            .inner
            .join
            .lock()
            .unwrap()
            .take()
            .map(|join| join.join().map_err(|_| PersistenceError::OwnerStopped))
            .unwrap_or(Ok(()));
        {
            let mut admission = self.inner.admission.lock().unwrap();
            admission.state = STATE_CLOSED;
        }
        *shutdown = Some(result.clone());
        result
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.admission.get_mut().unwrap().sender.take();
        if let Some(join) = self.join.get_mut().unwrap().take() {
            let _ = join.join();
        }
    }
}

fn owner_main(
    state: StateDirectory,
    database_name: DatabaseName,
    identity: StateFileIdentity,
    _database_lock: StateDatabaseLock,
    canonical_root: String,
    receiver: Receiver<Command>,
    startup: std::sync::mpsc::Sender<Result<(), PersistenceError>>,
) {
    let mut connection = match open_connection(&state, &database_name, identity, &canonical_root) {
        Ok(connection) => connection,
        Err(error) => {
            let _ = startup.send(Err(error));
            return;
        }
    };
    let mut versions = match MutationVersions::new() {
        Ok(versions) => versions,
        Err(error) => {
            let _ = startup.send(Err(error));
            return;
        }
    };
    let _ = startup.send(Ok(()));
    let mut sequence = 0;
    for command in receiver {
        sequence += 1;
        match command {
            Command::Probe(reply) => {
                let _ = reply.send(Ok(sequence));
            }
            Command::Snapshot(reply) => {
                let result = snapshot(&connection);
                let _ = reply.send(result);
            }
            Command::ApplyScan {
                discovered,
                errors,
                recovery,
                failure_after_first,
                reply,
            } => {
                let result = apply_scan(
                    &state,
                    &database_name,
                    &mut connection,
                    &discovered,
                    &errors,
                    &recovery,
                    failure_after_first,
                );
                let _ = reply.send(result);
            }
            Command::RecoveryFacts {
                original_ids,
                reply,
            } => {
                let result = recovery_facts(&connection, &original_ids);
                let _ = reply.send(result);
            }
            Command::NextFingerprintTarget(reply) => {
                let result = next_fingerprint_target(&connection);
                let _ = reply.send(result);
            }
            Command::StoreFingerprint(fingerprint, reply) => {
                let result =
                    store_fingerprint(&state, &database_name, &mut connection, fingerprint);
                let _ = reply.send(result);
            }
            Command::FingerprintCounts(reply) => {
                let result = fingerprint_counts(&connection);
                let _ = reply.send(result);
            }
            Command::RecoverySurvey(reply) => {
                let result = recovery_survey(&connection);
                let _ = reply.send(result);
            }
            Command::ApplyRelocations { relocations, reply } => {
                let result =
                    apply_manual_relocations(&state, &database_name, &mut connection, &relocations);
                let _ = reply.send(result);
            }
            Command::Preview(preview, reply) => {
                let result = seed_preview(&state, &database_name, &mut connection, preview);
                let _ = reply.send(result);
            }
            Command::ListAlbums(reply) => {
                let _ = reply.send(list_albums(&connection));
            }
            Command::ListAlbumSummaries(reply) => {
                let _ = reply.send(list_album_summaries(&connection, &versions));
            }
            Command::ReadAlbum { album_id, reply } => {
                let _ = reply.send(read_album(&connection, &versions, &album_id));
            }
            Command::ReadAlbums { album_ids, reply } => {
                let result = album_ids
                    .iter()
                    .map(|album_id| read_album(&connection, &versions, album_id))
                    .collect();
                let _ = reply.send(result);
            }
            Command::CreateAlbumQuery {
                filter,
                maximum_results,
                reply,
            } => {
                let _ = reply.send(create_album_query(&connection, filter, maximum_results));
            }
            Command::ReadPhoto { photo_id, reply } => {
                let _ = reply.send(read_photo(&connection, &versions, &photo_id));
            }
            Command::ReadEditRecipe { photo_id, reply } => {
                let _ = reply.send(read_edit_recipe(&connection, &photo_id));
            }
            Command::SaveEditRecipe(mutation, reply) => {
                let result = save_edit_recipe(&state, &database_name, &mut connection, mutation);
                let _ = reply.send(result);
            }
            Command::RebindEditRecipe(mutation, reply) => {
                let result = rebind_edit_recipe(&state, &database_name, &mut connection, mutation);
                let _ = reply.send(result);
            }
            Command::ReadPhotos {
                photo_ids,
                projection,
                reply,
            } => {
                let result = photo_ids
                    .iter()
                    .map(|photo_id| {
                        read_projected_photo(&connection, &versions, &projection, photo_id)
                    })
                    .collect();
                let _ = reply.send(result);
            }
            Command::CreatePhotoQuery {
                query,
                projection,
                maximum_results,
                reply,
            } => {
                let _ = reply.send(create_photo_query(
                    &connection,
                    query,
                    &projection,
                    maximum_results,
                ));
            }
            Command::PhotoAlbums { photo_id, reply } => {
                let _ = reply.send(photo_albums(&connection, &photo_id));
            }
            Command::AlbumBrowseTarget { album_id, reply } => {
                let _ = reply.send(album_browse_target(&connection, &album_id));
            }
            Command::MutateAlbum(mutation, reply) => {
                let plan = album_version_plan(&connection, &mutation);
                let result = match plan {
                    Ok(plan) if !plan.advance || versions.can_advance_album(&plan.album_id) => {
                        let result =
                            mutate_album(&state, &database_name, &mut connection, mutation);
                        if result.is_ok() {
                            if plan.deleted {
                                versions.album.remove(&plan.album_id);
                            } else if plan.advance {
                                let _ = versions.advance_album(&plan.album_id);
                            }
                        }
                        result
                    }
                    Ok(_) => Err(MutationError::Persistence),
                    Err(error) => Err(error),
                };
                let _ = reply.send(result);
            }
            Command::MutateAlbumMembership(mutation, reply) => {
                let album_id = match &mutation {
                    AlbumMembershipMutation::Add { album_id, .. }
                    | AlbumMembershipMutation::RemoveAdded { album_id, .. } => album_id,
                };
                let result = if versions.can_advance_album(album_id) {
                    mutate_album_membership(&state, &database_name, &mut connection, mutation)
                } else {
                    Err(MutationError::Persistence)
                };
                if let Ok(result) = &result
                    && (!result.added_photo_ids.is_empty() || !result.removed_photo_ids.is_empty())
                {
                    let _ = versions.advance_album(&result.album_id);
                }
                let _ = reply.send(result);
            }
            Command::CreateAlbum(name, reply) => {
                let result =
                    create_album_checked(&state, &database_name, &mut connection, &versions, name);
                let _ = reply.send(result);
            }
            Command::MutateAlbumChecked(mutation, reply) => {
                let result = mutate_album_checked(
                    &state,
                    &database_name,
                    &mut connection,
                    &mut versions,
                    mutation,
                );
                let _ = reply.send(result);
            }
            Command::MutatePhotoDecisionChecked(mutation, reply) => {
                let result = mutate_photo_decision_checked(
                    &state,
                    &database_name,
                    &mut connection,
                    &mut versions,
                    mutation,
                );
                let _ = reply.send(result);
            }
            Command::MutatePhotoState(mutation, reply) => {
                let photo_id = mutation.photo_id.clone();
                let value = mutation.value;
                let result = if versions.can_advance_photo(&photo_id) {
                    mutate_photo_state(&state, &database_name, &mut connection, mutation)
                } else {
                    Err(MutationError::Persistence)
                };
                if result
                    .as_ref()
                    .is_ok_and(|result| result.undo.prior_value != value)
                {
                    let _ = versions.advance_photo(&photo_id);
                }
                let _ = reply.send(result);
            }
            Command::MutatePhotoStateBatch(mutation, reply) => {
                let value = mutation.value;
                let can_advance = mutation
                    .photos
                    .iter()
                    .all(|photo| versions.can_advance_photo(&photo.photo_id));
                let result = if can_advance {
                    mutate_photo_state_batch(&state, &database_name, &mut connection, mutation)
                } else {
                    Err(MutationError::Persistence)
                };
                if let Ok(result) = &result {
                    for applied in &result.applied {
                        if applied.prior_value != value {
                            let _ = versions.advance_photo(&applied.photo_id);
                        }
                    }
                }
                let _ = reply.send(result);
            }
            Command::RemovePhotos(mutation, reply) => {
                let result = remove_photos(&state, &database_name, &mut connection, mutation);
                let _ = reply.send(result);
            }
            Command::RestorePhotos(restoration, reply) => {
                let result = restore_photos(&state, &database_name, &mut connection, restoration);
                let _ = reply.send(result);
            }
            Command::RemovedPhotos {
                start,
                limit,
                reply,
            } => {
                let _ = reply.send(removed_photos(&connection, start, limit));
            }
            Command::WriteProbe(reply) => {
                let result = write_transaction(&state, &database_name, &mut connection, |_| Ok(()));
                let _ = reply.send(result);
            }
            Command::SubmitExport(submission, reply) => {
                let result = submit_export(&state, &database_name, &mut connection, submission);
                let _ = reply.send(result);
            }
            Command::ReadExport { export_id, reply } => {
                let _ = reply.send(export_record(&connection, &export_id));
            }
            Command::ListPhotoExports { photo_id, reply } => {
                let _ = reply.send(list_photo_exports(&connection, &photo_id));
            }
            Command::CancelExport { export_id, reply } => {
                let result = cancel_export(&state, &database_name, &mut connection, &export_id);
                let _ = reply.send(result);
            }
            Command::SettleExport {
                export_id,
                settlement,
                reply,
            } => {
                let result = settle_export(
                    &state,
                    &database_name,
                    &mut connection,
                    &export_id,
                    settlement,
                );
                let _ = reply.send(result);
            }
            Command::BeginExportAttempt {
                export_id,
                attempt,
                reply,
            } => {
                let result = begin_export_attempt(
                    &state,
                    &database_name,
                    &mut connection,
                    &export_id,
                    attempt,
                );
                let _ = reply.send(result);
            }
            Command::RecordExportSource {
                export_id,
                size,
                sha256,
                reply,
            } => {
                let result = record_export_source(
                    &state,
                    &database_name,
                    &mut connection,
                    &export_id,
                    size,
                    &sha256,
                );
                let _ = reply.send(result);
            }
            Command::RetryExport {
                export_id,
                request_id,
                expected_bundle_id,
                allowance,
                reply,
            } => {
                let result = retry_export(
                    &state,
                    &database_name,
                    &mut connection,
                    &export_id,
                    &request_id,
                    &expected_bundle_id,
                    allowance,
                );
                let _ = reply.send(result);
            }
            Command::ResolveExportSubmission {
                photo_id,
                request_id,
                payload_digest,
                reply,
            } => {
                let _ = reply.send(Ok(resolve_export_submission(
                    &connection,
                    &photo_id,
                    &request_id,
                    &payload_digest,
                )));
            }
            Command::ClaimExportPublication {
                export_id,
                incarnation,
                sequence,
                reply,
            } => {
                let _ = reply.send(claim_export_publication(
                    &mut connection,
                    &export_id,
                    &incarnation,
                    sequence,
                ));
            }
            Command::ExportPublicationClaim { export_id, reply } => {
                let _ = reply.send(Ok(export_publication_claim(&connection, &export_id)));
            }
            Command::RenewExportLease {
                lease_id,
                now,
                reply,
            } => {
                let _ = reply.send(Ok(renew_export_lease(&mut connection, &lease_id, now)));
            }
            Command::SweepExportExpiry { now, reply } => {
                let result = sweep_export_expiry(&state, &database_name, &mut connection, now);
                let _ = reply.send(result);
            }
            Command::UnfinishedExports(reply) => {
                let _ = reply.send(unfinished_exports(&connection));
            }
            Command::AcquireExportLease {
                export_id,
                now,
                reply,
            } => {
                let result =
                    acquire_export_lease(&state, &database_name, &mut connection, &export_id, now);
                let _ = reply.send(result);
            }
            Command::ReleaseExportLease { lease_id, reply } => {
                let result =
                    release_export_lease(&state, &database_name, &mut connection, &lease_id);
                let _ = reply.send(result);
            }
            #[cfg(test)]
            Command::Configuration(reply) => {
                let result = (|| {
                    let journal = connection
                        .pragma_query_value(None, "journal_mode", |row| row.get(0))
                        .map_err(|_| PersistenceError::Storage)?;
                    let foreign_keys = connection
                        .pragma_query_value(None, "foreign_keys", |row| row.get(0))
                        .map_err(|_| PersistenceError::Storage)?;
                    Ok((journal, foreign_keys))
                })();
                let _ = reply.send(result);
            }
            #[cfg(test)]
            Command::Block {
                entered,
                release,
                reply,
            } => {
                let _ = entered.send(());
                let result = release.recv().map_err(|_| PersistenceError::OwnerStopped);
                let _ = reply.send(result);
            }
        }
    }
}

fn open_connection(
    state: &StateDirectory,
    database_name: &DatabaseName,
    identity: super::StateFileIdentity,
    canonical_root: &str,
) -> Result<Connection, PersistenceError> {
    state.verify_database(database_name, identity)?;
    if state.startup_sidecars_present(database_name)? {
        return Err(PersistenceError::RecoveryRequired);
    }
    let readonly = Connection::open_with_flags(
        state.sqlite_immutable_uri(database_name),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|_| PersistenceError::Storage)?;
    preflight_schema(&readonly, canonical_root)?;
    drop(readonly);
    state.admit_sidecars(database_name)?;
    let mut connection = Connection::open(state.sqlite_path(database_name))
        .map_err(|_| PersistenceError::Storage)?;
    state.verify_database(database_name, identity)?;
    state.admit_sidecars(database_name)?;
    validate_root_binding(&connection, canonical_root)?;
    connection
        .pragma_update(None, "journal_mode", "DELETE")
        .map_err(|_| PersistenceError::Storage)?;
    state.admit_sidecars(database_name)?;
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(|_| PersistenceError::Storage)?;
    startup_schema(state, database_name, &mut connection, canonical_root)?;
    Ok(connection)
}

fn preflight_schema(connection: &Connection, canonical_root: &str) -> Result<(), PersistenceError> {
    preflight_schema_for_max_version(connection, canonical_root, 9)
}

fn preflight_schema_for_max_version(
    connection: &Connection,
    canonical_root: &str,
    maximum_version: u32,
) -> Result<(), PersistenceError> {
    let version: u32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|_| PersistenceError::Storage)?;
    if version > maximum_version {
        return Err(PersistenceError::NewerSchema);
    }
    validate_root_binding(connection, canonical_root)?;
    match version {
        0 if table_exists(connection, "original_files")? => validate_legacy_v0(connection),
        0 => Ok(()),
        1 => validate_canonical_schema(connection, SchemaVersion::V1)
            .map_err(|_| PersistenceError::UnsupportedSchema),
        2 => validate_canonical_schema(connection, SchemaVersion::V2)
            .map_err(|_| PersistenceError::UnsupportedSchema),
        3 => validate_canonical_schema(connection, SchemaVersion::V3)
            .map_err(|_| PersistenceError::UnsupportedSchema),
        4 => validate_canonical_schema(connection, SchemaVersion::V4)
            .map_err(|_| PersistenceError::UnsupportedSchema),
        5 => validate_canonical_schema(connection, SchemaVersion::V5)
            .map_err(|_| PersistenceError::UnsupportedSchema),
        6 => validate_canonical_schema(connection, SchemaVersion::V6)
            .map_err(|_| PersistenceError::UnsupportedSchema),
        7 => validate_canonical_schema(connection, SchemaVersion::V7)
            .map_err(|_| PersistenceError::UnsupportedSchema),
        8 => validate_canonical_schema(connection, SchemaVersion::V8)
            .map_err(|_| PersistenceError::UnsupportedSchema),
        9 => validate_canonical_schema(connection, SchemaVersion::V9)
            .map_err(|_| PersistenceError::UnsupportedSchema),
        _ => unreachable!(),
    }
}

fn validate_root_binding(
    connection: &Connection,
    canonical_root: &str,
) -> Result<(), PersistenceError> {
    if table_exists(connection, "library_metadata")? {
        let stored: Option<String> = connection
            .query_row(
                "SELECT value FROM library_metadata WHERE key='canonical_root'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| PersistenceError::Storage)?;
        if stored
            .as_deref()
            .is_some_and(|stored| stored != canonical_root)
        {
            return Err(PersistenceError::RootMismatch);
        }
    }
    Ok(())
}

fn startup_schema(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    canonical_root: &str,
) -> Result<(), PersistenceError> {
    let version: u32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|_| PersistenceError::Storage)?;
    if version > 9 {
        return Err(PersistenceError::NewerSchema);
    }
    validate_root_binding(connection, canonical_root)?;
    state.admit_sidecars(database_name)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| PersistenceError::Storage)?;
    match version {
        0 => {
            migrate_v0(&transaction)?;
            migrate_v2(&transaction)?;
            migrate_v3(&transaction)?;
            migrate_v4(&transaction)?;
        }
        1 => {
            validate_canonical_schema(&transaction, SchemaVersion::V1)
                .map_err(|_| PersistenceError::UnsupportedSchema)?;
            migrate_v1(&transaction)?;
            migrate_v2(&transaction)?;
            migrate_v3(&transaction)?;
            migrate_v4(&transaction)?;
        }
        2 => {
            validate_canonical_schema(&transaction, SchemaVersion::V2)
                .map_err(|_| PersistenceError::UnsupportedSchema)?;
            migrate_v2(&transaction)?;
            migrate_v3(&transaction)?;
            migrate_v4(&transaction)?;
        }
        3 => {
            validate_canonical_schema(&transaction, SchemaVersion::V3)
                .map_err(|_| PersistenceError::UnsupportedSchema)?;
            migrate_v3(&transaction)?;
            migrate_v4(&transaction)?;
        }
        4 => {
            validate_canonical_schema(&transaction, SchemaVersion::V4)
                .map_err(|_| PersistenceError::UnsupportedSchema)?;
            migrate_v4(&transaction)?;
        }
        5 => validate_canonical_schema(&transaction, SchemaVersion::V5)
            .map_err(|_| PersistenceError::UnsupportedSchema)?,
        6 => validate_canonical_schema(&transaction, SchemaVersion::V6)
            .map_err(|_| PersistenceError::UnsupportedSchema)?,
        7 => validate_canonical_schema(&transaction, SchemaVersion::V7)
            .map_err(|_| PersistenceError::UnsupportedSchema)?,
        8 => validate_canonical_schema(&transaction, SchemaVersion::V8)
            .map_err(|_| PersistenceError::UnsupportedSchema)?,
        9 => validate_canonical_schema(&transaction, SchemaVersion::V9)
            .map_err(|_| PersistenceError::UnsupportedSchema)?,
        _ => unreachable!(),
    }
    if version < 6 {
        migrate_v5(&transaction)?;
    }
    if version < 7 {
        migrate_v6(&transaction)?;
    }
    if version < 8 {
        migrate_v7(&transaction)?;
    }
    if version < 9 {
        migrate_v8(&transaction)?;
    }
    let stored: Option<String> = transaction
        .query_row(
            "SELECT value FROM library_metadata WHERE key='canonical_root'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    if stored.is_none() {
        transaction
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
                [canonical_root],
            )
            .map_err(|_| PersistenceError::Storage)?;
    }
    validate_database(&transaction)?;
    validate_canonical_schema(&transaction, SchemaVersion::V9)
        .map_err(|_| PersistenceError::UnsupportedSchema)?;
    transaction.commit().map_err(|_| PersistenceError::Storage)
}

fn migrate_v0(transaction: &Transaction<'_>) -> Result<(), PersistenceError> {
    if table_exists(transaction, "original_files")? {
        validate_legacy_v0(transaction)?;
        transaction
            .execute_batch(
                "ALTER TABLE original_files RENAME TO original_files_legacy;
                 ALTER TABLE photos RENAME TO photos_legacy;
                 CREATE TABLE original_files(
                   id TEXT PRIMARY KEY, relative_path TEXT NOT NULL UNIQUE,
                   kind TEXT NOT NULL CHECK(kind IN ('raw','jpeg')),
                   size INTEGER NOT NULL CHECK(size >= 0), mtime_ms REAL NOT NULL CHECK(mtime_ms >= 0),
                   available INTEGER NOT NULL CHECK(available IN (0,1)),
                   error_category TEXT CHECK(error_category IS NULL OR error_category IN ('unreadable','changed')),
                   error_message TEXT CHECK(error_message IS NULL OR length(error_message) <= 120));
                 CREATE TABLE photos(
                   id TEXT PRIMARY KEY, raw_original_id TEXT REFERENCES original_files(id), jpeg_original_id TEXT REFERENCES original_files(id),
                   ambiguous INTEGER NOT NULL CHECK(ambiguous IN (0,1)), available INTEGER NOT NULL CHECK(available IN (0,1)),
                   preview_state TEXT NOT NULL CHECK(preview_state IN ('inspection-pending','ready','failed','unavailable')),
                   preview_candidate TEXT CHECK(preview_candidate IS NULL OR preview_candidate IN ('matching-jpeg','embedded-raw-jpeg')),
                   preview_source TEXT CHECK(preview_source IS NULL OR preview_source IN ('matching-jpeg','embedded-raw-jpeg')),
                   preview_source_revision TEXT, preview_width INTEGER CHECK(preview_width IS NULL OR preview_width > 0),
                   preview_height INTEGER CHECK(preview_height IS NULL OR preview_height > 0), cache_revision TEXT, sort_path TEXT NOT NULL);
                 INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,error_category,error_message)
                   SELECT id,relative_path,kind,size,mtime_ms,available,NULL,NULL FROM original_files_legacy;
                 INSERT INTO photos(id,raw_original_id,jpeg_original_id,ambiguous,available,preview_state,preview_source,sort_path)
                   SELECT id,raw_original_id,jpeg_original_id,ambiguous,available,preview_state,preview_source,sort_path FROM photos_legacy;
                 DROP TABLE photos_legacy; DROP TABLE original_files_legacy;
                 CREATE INDEX photos_raw ON photos(raw_original_id);
                 CREATE INDEX photos_jpeg ON photos(jpeg_original_id);
                 PRAGMA user_version = 1;",
            )
            .map_err(|_| PersistenceError::Storage)?;
    } else {
        transaction
            .execute_batch(SCHEMA_V1_SQL)
            .map_err(|_| PersistenceError::Storage)?;
    }
    validate_canonical_schema(transaction, SchemaVersion::V1)
        .map_err(|_| PersistenceError::UnsupportedSchema)?;
    migrate_v1(transaction)
}

// album-language-legacy:start migrate-v1
fn migrate_v1(transaction: &Transaction<'_>) -> Result<(), PersistenceError> {
    // Creates schema v2 state: the legacy photo-set table names are part of
    // the immutable v2-v4 contracts and are renamed to albums by migrate_v4.
    transaction
        .execute_batch(
            "ALTER TABLE photos ADD COLUMN selection_state TEXT NOT NULL DEFAULT 'undecided'
               CHECK(selection_state IN ('undecided','selected','rejected'));
             ALTER TABLE photos ADD COLUMN rating INTEGER NOT NULL DEFAULT 0
               CHECK(rating BETWEEN 0 AND 5);
             CREATE TABLE photo_sets(
               id TEXT PRIMARY KEY,
               name TEXT NOT NULL UNIQUE COLLATE NOCASE CHECK(length(name) BETWEEN 1 AND 120),
               created_at INTEGER NOT NULL);
             CREATE TABLE photo_set_members(
               photo_set_id TEXT NOT NULL REFERENCES photo_sets(id) ON DELETE CASCADE,
               photo_id TEXT NOT NULL REFERENCES photos(id) ON DELETE RESTRICT,
               position INTEGER NOT NULL CHECK(position >= 0),
               PRIMARY KEY(photo_set_id, photo_id),
               UNIQUE(photo_set_id, position));
             CREATE TABLE review_progress(
               photo_set_id TEXT PRIMARY KEY REFERENCES photo_sets(id) ON DELETE CASCADE,
               photo_id TEXT NOT NULL,
               FOREIGN KEY(photo_set_id, photo_id)
                 REFERENCES photo_set_members(photo_set_id, photo_id) ON DELETE CASCADE);
             CREATE INDEX photo_set_members_photo ON photo_set_members(photo_id);
             PRAGMA user_version = 2;",
        )
        .map_err(|_| PersistenceError::Storage)
}
// album-language-legacy:end migrate-v1

fn migrate_v2(transaction: &Transaction<'_>) -> Result<(), PersistenceError> {
    transaction
        .execute_batch(
            "ALTER TABLE original_files ADD COLUMN capture_metadata_state TEXT NOT NULL DEFAULT 'pending'
               CHECK(capture_metadata_state IN ('pending','known','missing','invalid','failed'));
             ALTER TABLE original_files ADD COLUMN capture_order_key TEXT CHECK(capture_order_key IS NULL OR (
               length(capture_order_key)=29 AND substr(capture_order_key,5,1)='-' AND
               substr(capture_order_key,8,1)='-' AND substr(capture_order_key,11,1)='T' AND
               substr(capture_order_key,14,1)=':' AND substr(capture_order_key,17,1)=':' AND
               substr(capture_order_key,20,1)='.' AND
               replace(replace(replace(replace(capture_order_key,'-',''),':',''),'T',''),'.','')
                 NOT GLOB '*[^0-9]*'
             ));
             ALTER TABLE original_files ADD COLUMN capture_time_field TEXT CHECK(capture_time_field IS NULL OR capture_time_field IN ('date-time-original','date-time-digitized'));
             ALTER TABLE original_files ADD COLUMN capture_offset_minutes INTEGER CHECK(capture_offset_minutes IS NULL OR capture_offset_minutes BETWEEN -840 AND 840);
             ALTER TABLE original_files ADD COLUMN capture_source_revision TEXT;
             PRAGMA user_version = 3;",
        )
        .map_err(|_| PersistenceError::Storage)
}

fn migrate_v3(transaction: &Transaction<'_>) -> Result<(), PersistenceError> {
    transaction
        .execute_batch("PRAGMA user_version = 4;")
        .map_err(|_| PersistenceError::Storage)
}

// album-language-legacy:start migrate-v4
fn migrate_v4(transaction: &Transaction<'_>) -> Result<(), PersistenceError> {
    // Issue #95: rename the legacy v4 photo-set storage to canonical albums in
    // one transaction. The new tables use DDL text identical to
    // compatibility/sqlite/schema-v5.sql so the migrated database satisfies
    // the exact schema-v5 manifest. Every album id, name, creation order,
    // membership position, and saved position is copied unchanged.
    transaction
        .execute_batch(
            "CREATE TABLE albums(
               id TEXT PRIMARY KEY,
               name TEXT NOT NULL UNIQUE COLLATE NOCASE CHECK(length(name) BETWEEN 1 AND 120),
               created_at INTEGER NOT NULL);
             CREATE TABLE album_members(
               album_id TEXT NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
               photo_id TEXT NOT NULL REFERENCES photos(id) ON DELETE RESTRICT,
               position INTEGER NOT NULL CHECK(position >= 0),
               PRIMARY KEY(album_id, photo_id),
               UNIQUE(album_id, position));
             CREATE TABLE album_progress(
               album_id TEXT PRIMARY KEY REFERENCES albums(id) ON DELETE CASCADE,
               photo_id TEXT NOT NULL,
               FOREIGN KEY(album_id, photo_id)
                 REFERENCES album_members(album_id, photo_id) ON DELETE CASCADE);
             CREATE INDEX album_members_photo ON album_members(photo_id);
             INSERT INTO albums(id,name,created_at)
               SELECT id,name,created_at FROM photo_sets;
             INSERT INTO album_members(album_id,photo_id,position)
               SELECT photo_set_id,photo_id,position FROM photo_set_members;
             INSERT INTO album_progress(album_id,photo_id)
               SELECT photo_set_id,photo_id FROM review_progress;
             DROP TABLE review_progress;
             DROP TABLE photo_set_members;
             DROP TABLE photo_sets;
             PRAGMA user_version = 5;",
        )
        .map_err(|_| PersistenceError::Storage)
}
// album-language-legacy:end migrate-v4

// independent-photos-legacy:start migrate-v5
fn migrate_v5(transaction: &Transaction<'_>) -> Result<(), PersistenceError> {
    // Issue #304: one independently managed Original File per Photo. A legacy
    // RAW/JPEG pair keeps its Photo identity, decisions, Album references,
    // and saved position on the RAW Original; its JPEG Original receives a new
    // independent Photo with default decisions. Content fingerprints start
    // empty and are enrolled in the background after migration.
    transaction
        .execute_batch(
            "CREATE TABLE original_fingerprints(
               original_id TEXT PRIMARY KEY REFERENCES original_files(id) ON DELETE CASCADE,
               digest TEXT NOT NULL CHECK(length(digest) = 64),
               size INTEGER NOT NULL CHECK(size >= 0),
               mtime_ms REAL NOT NULL CHECK(mtime_ms >= 0));
             CREATE INDEX original_fingerprints_digest ON original_fingerprints(digest);
             CREATE TABLE photos_v6(
               id TEXT PRIMARY KEY,
               original_id TEXT NOT NULL UNIQUE REFERENCES original_files(id) ON DELETE RESTRICT,
               available INTEGER NOT NULL CHECK(available IN (0,1)),
               preview_state TEXT NOT NULL CHECK(preview_state IN ('inspection-pending','ready','failed','unavailable')),
               preview_source_revision TEXT,
               preview_width INTEGER CHECK(preview_width IS NULL OR preview_width > 0),
               preview_height INTEGER CHECK(preview_height IS NULL OR preview_height > 0),
               cache_revision TEXT,
               sort_path TEXT NOT NULL,
               selection_state TEXT NOT NULL DEFAULT 'undecided' CHECK(selection_state IN ('undecided','selected','rejected')),
               rating INTEGER NOT NULL DEFAULT 0 CHECK(rating BETWEEN 0 AND 5));",
        )
        .map_err(|_| PersistenceError::Storage)?;

    let originals = original_facts_by_id(transaction)?;
    let photos = transaction
        .prepare(
            "SELECT id,raw_original_id,jpeg_original_id,preview_state,preview_source,
                    preview_source_revision,preview_width,preview_height,cache_revision,
                    sort_path,selection_state,rating
             FROM photos ORDER BY id",
        )
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], |row| {
            Ok(LegacyPhotoRow {
                id: row.get(0)?,
                raw_original_id: row.get(1)?,
                jpeg_original_id: row.get(2)?,
                preview_state: row.get(3)?,
                preview_source: row.get(4)?,
                preview_source_revision: row.get(5)?,
                preview_width: row.get(6)?,
                preview_height: row.get(7)?,
                cache_revision: row.get(8)?,
                sort_path: row.get(9)?,
                selection_state: row.get(10)?,
                rating: row.get(11)?,
            })
        })
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::InvalidLegacyData)?;
    let mut insert = transaction
        .prepare(
            "INSERT INTO photos_v6(id,original_id,available,preview_state,preview_source_revision,
               preview_width,preview_height,cache_revision,sort_path,selection_state,rating)
             VALUES(?,?,?,?,?,?,?,?,?,?,?)",
        )
        .map_err(|_| PersistenceError::Storage)?;
    let mut reserved_ids = HashSet::new();
    for photo in photos {
        let kept = photo
            .raw_original_id
            .clone()
            .or_else(|| photo.jpeg_original_id.clone());
        let Some(kept) = kept else {
            // A Photo with no Original is unusable; album references, if any,
            // fail closed through the RESTRICT foreign key.
            transaction
                .execute("DELETE FROM photos WHERE id=?", [&photo.id])
                .map_err(|_| PersistenceError::InvalidLegacyData)?;
            continue;
        };
        let Some((kept_available, kept_path, kept_size, kept_mtime, kept_kind)) =
            originals.get(&kept).cloned()
        else {
            return Err(PersistenceError::InvalidLegacyData);
        };
        let preserved = Some(kept_kind.preview_source().legacy_database_name())
            == photo.preview_source.as_deref()
            && revision_matches(
                photo.preview_source_revision.as_deref(),
                &kept_path,
                kept_size,
                kept_mtime,
            );
        let preview_state = if preserved {
            photo.preview_state
        } else {
            "inspection-pending".to_owned()
        };
        insert
            .execute(params![
                photo.id,
                kept,
                i64::from(kept_available),
                preview_state,
                preserved.then_some(photo.preview_source_revision).flatten(),
                preserved.then_some(photo.preview_width).flatten(),
                preserved.then_some(photo.preview_height).flatten(),
                preserved.then_some(photo.cache_revision).flatten(),
                photo.sort_path,
                photo.selection_state,
                photo.rating,
            ])
            .map_err(|_| PersistenceError::InvalidLegacyData)?;
        if let Some(jpeg_id) = photo.jpeg_original_id {
            if photo.raw_original_id.is_none() {
                continue;
            }
            let Some((jpeg_available, jpeg_path, _size, _mtime, _kind)) =
                originals.get(&jpeg_id).cloned()
            else {
                return Err(PersistenceError::InvalidLegacyData);
            };
            let new_id = allocate_library_id(transaction, &mut reserved_ids)?;
            insert
                .execute(params![
                    new_id,
                    jpeg_id,
                    i64::from(jpeg_available),
                    "inspection-pending",
                    Option::<String>::None,
                    Option::<i64>::None,
                    Option::<i64>::None,
                    Option::<String>::None,
                    jpeg_path,
                    "undecided",
                    0,
                ])
                .map_err(|_| PersistenceError::InvalidLegacyData)?;
        }
    }
    // Rebuild photos without violating the album foreign keys: unload the
    // album tables, replace photos, then recreate them with identical DDL and
    // every original row. One transaction keeps the migration atomic.
    let albums: Vec<(String, String, i64)> = transaction
        .prepare("SELECT id,name,created_at FROM albums ORDER BY created_at,id")
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::InvalidLegacyData)?;
    let members: Vec<(String, String, i64)> = transaction
        .prepare("SELECT album_id,photo_id,position FROM album_members ORDER BY album_id,position")
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::InvalidLegacyData)?;
    let progress: Vec<(String, String)> = transaction
        .prepare("SELECT album_id,photo_id FROM album_progress ORDER BY album_id")
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::InvalidLegacyData)?;
    transaction
        .execute_batch(
            "DROP TABLE album_progress;
             DROP TABLE album_members;
             DROP TABLE albums;
             DROP TABLE photos;
             ALTER TABLE photos_v6 RENAME TO photos;
             CREATE INDEX photos_original ON photos(original_id);
             CREATE TABLE albums(
               id TEXT PRIMARY KEY,
               name TEXT NOT NULL UNIQUE COLLATE NOCASE CHECK(length(name) BETWEEN 1 AND 120),
               created_at INTEGER NOT NULL);
             CREATE TABLE album_members(
               album_id TEXT NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
               photo_id TEXT NOT NULL REFERENCES photos(id) ON DELETE RESTRICT,
               position INTEGER NOT NULL CHECK(position >= 0),
               PRIMARY KEY(album_id, photo_id),
               UNIQUE(album_id, position));
             CREATE TABLE album_progress(
               album_id TEXT PRIMARY KEY REFERENCES albums(id) ON DELETE CASCADE,
               photo_id TEXT NOT NULL,
               FOREIGN KEY(album_id, photo_id) REFERENCES album_members(album_id, photo_id) ON DELETE CASCADE);
             CREATE INDEX album_members_photo ON album_members(photo_id);
             PRAGMA user_version = 6;",
        )
        .map_err(|_| PersistenceError::Storage)?;
    {
        let mut insert_album = transaction
            .prepare("INSERT INTO albums(id,name,created_at) VALUES(?,?,?)")
            .map_err(|_| PersistenceError::Storage)?;
        for (id, name, created_at) in &albums {
            insert_album
                .execute(params![id, name, created_at])
                .map_err(|_| PersistenceError::InvalidLegacyData)?;
        }
        let mut insert_member = transaction
            .prepare("INSERT INTO album_members(album_id,photo_id,position) VALUES(?,?,?)")
            .map_err(|_| PersistenceError::Storage)?;
        for (album_id, photo_id, position) in &members {
            insert_member
                .execute(params![album_id, photo_id, position])
                .map_err(|_| PersistenceError::InvalidLegacyData)?;
        }
        let mut insert_progress = transaction
            .prepare("INSERT INTO album_progress(album_id,photo_id) VALUES(?,?)")
            .map_err(|_| PersistenceError::Storage)?;
        for (album_id, photo_id) in &progress {
            insert_progress
                .execute(params![album_id, photo_id])
                .map_err(|_| PersistenceError::InvalidLegacyData)?;
        }
    }
    validate_canonical_schema(transaction, SchemaVersion::V6)
        .map_err(|_| PersistenceError::UnsupportedSchema)
}

fn migrate_v6(transaction: &Transaction<'_>) -> Result<(), PersistenceError> {
    validate_canonical_schema(transaction, SchemaVersion::V6)
        .map_err(|_| PersistenceError::UnsupportedSchema)?;
    transaction
        .execute_batch(
            "CREATE TABLE edit_recipes(
               photo_id TEXT PRIMARY KEY REFERENCES photos(id) ON DELETE RESTRICT,
               revision TEXT NOT NULL CHECK(length(revision) > 0),
               source_revision TEXT NOT NULL CHECK(length(source_revision) > 0),
               exposure_ev REAL NOT NULL CHECK(exposure_ev BETWEEN -1.7976931348623157e308 AND 1.7976931348623157e308),
               white_balance_mode TEXT NOT NULL CHECK(white_balance_mode = 'as-shot')
             );
             PRAGMA user_version = 7;",
        )
        .map_err(|_| PersistenceError::Storage)?;
    validate_canonical_schema(transaction, SchemaVersion::V7)
        .map_err(|_| PersistenceError::UnsupportedSchema)
}

fn migrate_v7(transaction: &Transaction<'_>) -> Result<(), PersistenceError> {
    validate_canonical_schema(transaction, SchemaVersion::V7)
        .map_err(|_| PersistenceError::UnsupportedSchema)?;
    // The closed white-balance payload bounds are published independent of
    // admission, so a recipe can retain a temperature-tint editing intent
    // that no capability admits for execution. The rebuild widens the mode
    // column and adds the two nullable intent values; existing as-shot rows
    // keep null values.
    transaction
        .execute_batch(
            "ALTER TABLE edit_recipes RENAME TO edit_recipes_v7;
             CREATE TABLE edit_recipes(
               photo_id TEXT PRIMARY KEY REFERENCES photos(id) ON DELETE RESTRICT,
               revision TEXT NOT NULL CHECK(length(revision) > 0),
               source_revision TEXT NOT NULL CHECK(length(source_revision) > 0),
               exposure_ev REAL NOT NULL CHECK(exposure_ev BETWEEN -1.7976931348623157e308 AND 1.7976931348623157e308),
               white_balance_mode TEXT NOT NULL CHECK(white_balance_mode IN ('as-shot','temperature-tint')),
               temperature_kelvin INTEGER CHECK(temperature_kelvin IS NULL OR temperature_kelvin BETWEEN 1000 AND 40000),
               tint_milli INTEGER CHECK(tint_milli IS NULL OR tint_milli BETWEEN -150000 AND 150000)
             );
             INSERT INTO edit_recipes(photo_id,revision,source_revision,exposure_ev,white_balance_mode)
               SELECT photo_id,revision,source_revision,exposure_ev,white_balance_mode FROM edit_recipes_v7;
             DROP TABLE edit_recipes_v7;
             CREATE TABLE exports(
               id TEXT PRIMARY KEY,
               photo_id TEXT NOT NULL REFERENCES photos(id) ON DELETE RESTRICT,
               target TEXT NOT NULL CHECK(target = 'development-tiff'),
               state TEXT NOT NULL CHECK(state IN ('queued','running','succeeded','failed','cancelled')),
               outcome TEXT CHECK(outcome IS NULL OR length(outcome) BETWEEN 1 AND 200),
               recipe_revision TEXT NOT NULL CHECK(length(recipe_revision) > 0),
               exposure_ev REAL NOT NULL,
               white_balance_mode TEXT NOT NULL CHECK(white_balance_mode = 'as-shot'),
               source_revision TEXT NOT NULL CHECK(length(source_revision) > 0),
               source_profile_id TEXT NOT NULL CHECK(length(source_profile_id) BETWEEN 1 AND 64),
               source_kind TEXT NOT NULL CHECK(source_kind = 'raw'),
               source_size INTEGER CHECK(source_size IS NULL OR source_size > 0),
               source_sha256 TEXT CHECK(source_sha256 IS NULL OR length(source_sha256) = 64),
               recipe_digest TEXT NOT NULL CHECK(length(recipe_digest) = 64),
               policy_id TEXT NOT NULL CHECK(length(policy_id) = 64),
               bundle_id TEXT NOT NULL CHECK(length(bundle_id) = 64),
               workload TEXT NOT NULL CHECK(workload = 'development-tiff'),
               attempt_incarnation TEXT CHECK(attempt_incarnation IS NULL OR length(attempt_incarnation) = 32),
               attempt_sequence INTEGER CHECK(attempt_sequence IS NULL OR attempt_sequence > 0),
               artifact_size INTEGER CHECK(artifact_size IS NULL OR artifact_size > 0),
               artifact_sha256 TEXT CHECK(artifact_sha256 IS NULL OR length(artifact_sha256) = 64),
               artifact_expires_at INTEGER CHECK(artifact_expires_at IS NULL OR artifact_expires_at >= 0),
               artifact_width INTEGER CHECK(artifact_width IS NULL OR artifact_width > 0),
               artifact_height INTEGER CHECK(artifact_height IS NULL OR artifact_height > 0),
               artifact_profile_identity TEXT CHECK(artifact_profile_identity IS NULL OR length(artifact_profile_identity) = 64),
               created_at INTEGER NOT NULL CHECK(created_at >= 0),
               settled_at INTEGER CHECK(settled_at IS NULL OR settled_at >= 0),
               retain_until INTEGER CHECK(retain_until IS NULL OR retain_until >= 0)
             );
             CREATE INDEX exports_photo ON exports(photo_id);
             CREATE TABLE export_download_leases(
               id TEXT PRIMARY KEY,
               export_id TEXT NOT NULL REFERENCES exports(id) ON DELETE CASCADE,
               created_at INTEGER NOT NULL CHECK(created_at >= 0)
             );
             PRAGMA user_version = 8;",
        )
        .map_err(|_| PersistenceError::Storage)?;
    validate_canonical_schema(transaction, SchemaVersion::V8)
        .map_err(|_| PersistenceError::UnsupportedSchema)
}

/// Issue #416: a removed Photo keeps its row and every retained fact. The
/// removal marker is application-owned Library state, so it is added to the
/// Photo row instead of a separate recovery record.
fn migrate_v8(transaction: &Transaction<'_>) -> Result<(), PersistenceError> {
    validate_canonical_schema(transaction, SchemaVersion::V8)
        .map_err(|_| PersistenceError::UnsupportedSchema)?;
    transaction
        .execute_batch(
            "ALTER TABLE photos ADD COLUMN removed_at_ms INTEGER
               CHECK(removed_at_ms IS NULL OR removed_at_ms >= 0);
             ALTER TABLE photos ADD COLUMN removed_operation TEXT
               CHECK((removed_at_ms IS NULL) = (removed_operation IS NULL));
             PRAGMA user_version = 9;",
        )
        .map_err(|_| PersistenceError::Storage)?;
    validate_canonical_schema(transaction, SchemaVersion::V9)
        .map_err(|_| PersistenceError::UnsupportedSchema)
}

struct LegacyPhotoRow {
    id: String,
    raw_original_id: Option<String>,
    jpeg_original_id: Option<String>,
    preview_state: String,
    preview_source: Option<String>,
    preview_source_revision: Option<String>,
    preview_width: Option<i64>,
    preview_height: Option<i64>,
    cache_revision: Option<String>,
    sort_path: String,
    selection_state: String,
    rating: i64,
}

type LegacyOriginalFacts = (bool, String, u64, f64, crate::OriginalKind);

fn original_facts_by_id(
    transaction: &Transaction<'_>,
) -> Result<HashMap<String, LegacyOriginalFacts>, PersistenceError> {
    transaction
        .prepare("SELECT id,available,relative_path,size,mtime_ms,kind FROM original_files")
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (
                    row.get::<_, i64>(1)? != 0,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    row.get::<_, f64>(4)?,
                    row.get::<_, String>(5)?,
                ),
            ))
        })
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<HashMap<_, _>, _>>()
        .map_err(|_| PersistenceError::Storage)?
        .into_iter()
        .map(|(id, (available, path, size, mtime_ms, kind))| {
            let kind = match kind.as_str() {
                "raw" => crate::OriginalKind::Raw,
                "jpeg" => crate::OriginalKind::Jpeg,
                _ => return Err(PersistenceError::InvalidLegacyData),
            };
            Ok((id, (available, path, size, mtime_ms, kind)))
        })
        .collect()
}

fn revision_matches(stored: Option<&str>, path: &str, size: u64, mtime_ms: f64) -> bool {
    let Some(stored) = stored else { return false };
    crate::source_revision(path, size, mtime_ms).is_ok_and(|current| current == stored)
}
// independent-photos-legacy:end migrate-v5

fn validate_legacy_v0(connection: &Connection) -> Result<(), PersistenceError> {
    let tables = names(connection, "table")?;
    if tables != ["library_metadata", "original_files", "photos"] {
        return Err(PersistenceError::UnsupportedSchema);
    }
    let expected = [
        ("library_metadata", &["key", "value"][..]),
        (
            "original_files",
            &[
                "id",
                "relative_path",
                "kind",
                "size",
                "mtime_ms",
                "available",
                "inspection_error",
            ][..],
        ),
        (
            "photos",
            &[
                "id",
                "raw_original_id",
                "jpeg_original_id",
                "ambiguous",
                "available",
                "preview_state",
                "preview_source",
                "sort_path",
            ][..],
        ),
    ];
    for (table, columns) in expected {
        if table_columns(connection, table)? != columns {
            return Err(PersistenceError::UnsupportedSchema);
        }
    }
    let invalid_original: Option<u8> = connection
        .query_row(
            "SELECT 1 FROM original_files WHERE
             typeof(id) != 'text' OR id = '' OR typeof(relative_path) != 'text' OR relative_path = '' OR
             kind NOT IN ('raw','jpeg') OR typeof(size) != 'integer' OR size < 0 OR
             typeof(mtime_ms) NOT IN ('integer','real') OR mtime_ms < 0 OR
             typeof(available) != 'integer' OR available NOT IN (0,1) LIMIT 1",
            [], |row| row.get(0),
        ).optional().map_err(|_| PersistenceError::Storage)?;
    let invalid_photo: Option<u8> = connection
        .query_row(
            "SELECT 1 FROM photos WHERE
             typeof(id) != 'text' OR id = '' OR typeof(ambiguous) != 'integer' OR ambiguous NOT IN (0,1) OR
             typeof(available) != 'integer' OR available NOT IN (0,1) OR
             preview_state NOT IN ('inspection-pending','ready','failed','unavailable') OR
             (preview_source IS NOT NULL AND preview_source NOT IN ('matching-jpeg','embedded-raw-jpeg')) OR
             typeof(sort_path) != 'text' OR
             (raw_original_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM original_files o WHERE o.id=photos.raw_original_id)) OR
             (jpeg_original_id IS NOT NULL AND NOT EXISTS(SELECT 1 FROM original_files o WHERE o.id=photos.jpeg_original_id)) LIMIT 1",
            [], |row| row.get(0),
        ).optional().map_err(|_| PersistenceError::Storage)?;
    if invalid_original.is_some() || invalid_photo.is_some() {
        return Err(PersistenceError::InvalidLegacyData);
    }
    Ok(())
}

fn recovery_facts(
    connection: &Connection,
    original_ids: &[String],
) -> Result<Vec<OriginalFingerprint>, PersistenceError> {
    let mut facts = Vec::with_capacity(original_ids.len());
    for original_id in original_ids {
        let row = connection
            .query_row(
                "SELECT digest,size,mtime_ms FROM original_fingerprints WHERE original_id=?",
                [original_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, f64>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|_| PersistenceError::Storage)?;
        if let Some((digest, size, mtime_ms)) = row {
            facts.push(OriginalFingerprint {
                original_id: original_id.clone(),
                digest,
                size: size.try_into().map_err(|_| PersistenceError::Storage)?,
                mtime_ms,
            });
        }
    }
    Ok(facts)
}

fn next_fingerprint_target(
    connection: &Connection,
) -> Result<Option<FingerprintTarget>, PersistenceError> {
    let row = connection
        .query_row(
            "SELECT o.id,o.relative_path,o.kind,o.size,o.mtime_ms
             FROM original_files o
             LEFT JOIN original_fingerprints f ON f.original_id=o.id
             WHERE o.available=1 AND o.error_category IS NULL
               AND (f.original_id IS NULL OR f.size != o.size OR f.mtime_ms != o.mtime_ms)
             ORDER BY o.relative_path COLLATE BINARY
             LIMIT 1",
            [],
            |row| {
                Ok(FingerprintTarget {
                    original_id: row.get(0)?,
                    relative_path: row.get(1)?,
                    kind: match row.get::<_, String>(2)?.as_str() {
                        "raw" => crate::OriginalKind::Raw,
                        "jpeg" => crate::OriginalKind::Jpeg,
                        _ => return Err(rusqlite::Error::InvalidQuery),
                    },
                    size: row
                        .get::<_, i64>(3)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    mtime_ms: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    Ok(row)
}

fn store_fingerprint(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    fingerprint: OriginalFingerprint,
) -> Result<(), PersistenceError> {
    write_transaction(state, database_name, connection, |transaction| {
        // A fingerprint is stored only for the revision the hasher observed;
        // an Original that moved on meanwhile keeps its stale row dropped by
        // the next scan and is re-targeted by enrollment.
        let stored = transaction
            .execute(
                "INSERT INTO original_fingerprints(original_id,digest,size,mtime_ms)
                 SELECT ?,?,?,? FROM original_files
                 WHERE id=? AND available=1 AND error_category IS NULL
                   AND size=? AND mtime_ms=?",
                params![
                    fingerprint.original_id,
                    fingerprint.digest,
                    i64::try_from(fingerprint.size).map_err(|_| PersistenceError::Storage)?,
                    fingerprint.mtime_ms,
                    fingerprint.original_id,
                    i64::try_from(fingerprint.size).map_err(|_| PersistenceError::Storage)?,
                    fingerprint.mtime_ms,
                ],
            )
            .map_err(|_| PersistenceError::Storage)?;
        if stored != 1 {
            return Err(PersistenceError::InvalidRecovery);
        }
        Ok(())
    })
}

/// One consistent read of every unavailable Photo plus the Album
/// memberships, for the bounded manual recovery review entry.
fn recovery_survey(connection: &Connection) -> Result<RecoverySurvey, PersistenceError> {
    let mut unavailable = Vec::new();
    {
        let mut statement = connection
            .prepare(
                "SELECT o.id,o.relative_path,o.kind,p.id,p.rating,p.selection_state,f.digest,
                        (SELECT COUNT(*) FROM album_members m WHERE m.photo_id=p.id)
                 FROM photos p
                 JOIN original_files o ON o.id=p.original_id
                 LEFT JOIN original_fingerprints f ON f.original_id=o.id
                 WHERE p.available=0 AND p.removed_at_ms IS NULL
                 ORDER BY o.relative_path COLLATE BINARY",
            )
            .map_err(|_| PersistenceError::Storage)?;
        let rows = statement
            .query_map([], |row| {
                Ok(UnavailablePhotoRecord {
                    original_id: row.get(0)?,
                    relative_path: row.get(1)?,
                    kind: parse_kind(&row.get::<_, String>(2)?)?,
                    photo_id: row.get(3)?,
                    rating: row
                        .get::<_, i64>(4)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    selection_state: parse_selection_state(&row.get::<_, String>(5)?)?,
                    fingerprint: row.get(6)?,
                    album_count: row
                        .get::<_, i64>(7)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                })
            })
            .map_err(|_| PersistenceError::Storage)?;
        for row in rows {
            unavailable.push(row.map_err(|_| PersistenceError::Storage)?);
        }
    }
    let mut referenced_photo_ids = HashSet::new();
    {
        let mut statement = connection
            .prepare(
                "SELECT DISTINCT photo_id FROM (
                   SELECT photo_id FROM album_members
                   UNION ALL
                   SELECT photo_id FROM edit_recipes
                 )",
            )
            .map_err(|_| PersistenceError::Storage)?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|_| PersistenceError::Storage)?;
        for row in rows {
            referenced_photo_ids.insert(row.map_err(|_| PersistenceError::Storage)?);
        }
    }
    Ok(RecoverySurvey {
        unavailable,
        referenced_photo_ids,
    })
}

/// Revalidates one confirmed manual relocation batch and commits it
/// atomically. Every mapping is rechecked against the persisted state
/// observed inside the transaction: stale confirmations, colliding
/// destinations, occupied Locations without an explicit retire, and
/// occupiers with independent user state reject the whole batch without
/// partial association.
fn apply_manual_relocations(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    relocations: &[RequestedRelocation],
) -> Result<AppliedRelocations, PersistenceError> {
    write_transaction(state, database_name, connection, |transaction| {
        if relocations.is_empty() {
            return Err(PersistenceError::InvalidRecovery);
        }
        let mut destinations = HashSet::with_capacity(relocations.len());
        let mut relocating_ids = HashSet::with_capacity(relocations.len());
        for relocation in relocations {
            // Two mappings for one Original File are a colliding batch: only
            // one Location could win, so the batch is refused as a whole.
            if !relocating_ids.insert(relocation.original_id.clone()) {
                return Err(PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "colliding",
                });
            }
        }
        for relocation in relocations {
            let to = RelativeOriginalPath::parse(relocation.to_location.clone()).map_err(|_| {
                PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "invalid-location",
                }
            })?;
            if !destinations.insert(to.as_str().to_owned()) {
                return Err(PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "colliding",
                });
            }
            let persisted = transaction
                .query_row(
                    "SELECT available,kind FROM original_files WHERE id=?",
                    params![relocation.original_id],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()
                .map_err(|_| PersistenceError::Storage)?;
            let Some((available, kind)) = persisted else {
                return Err(PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "stale",
                });
            };
            if available != 0 {
                return Err(PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "stale",
                });
            }
            let filename = to.as_str().rsplit('/').next().unwrap_or_default();
            let kind = parse_kind(&kind).map_err(|_| PersistenceError::Storage)?;
            if classify_name(filename) != Some(kind) {
                return Err(PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "kind-mismatch",
                });
            }
            // A destination owned by another Original requires either a
            // simultaneous relocation of that owner or an explicit retire of
            // an otherwise unreferenced default-state occupier.
            let owner = transaction
                .query_row(
                    "SELECT id FROM original_files WHERE relative_path=?",
                    params![to.as_str()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|_| PersistenceError::Storage)?;
            if let Some(owner_id) = owner
                && owner_id != relocation.original_id
                && !relocating_ids.contains(&owner_id)
            {
                if !relocation.retire_destination {
                    return Err(PersistenceError::InvalidRecoveryMapping {
                        original_id: relocation.original_id.clone(),
                        reason: "occupied",
                    });
                }
                let occupant = transaction
                    .query_row(
                        "SELECT id,rating,selection_state,
                            EXISTS(SELECT 1 FROM edit_recipes e WHERE e.photo_id=photos.id)
                     FROM photos WHERE original_id=?",
                        params![owner_id],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, i64>(1)?,
                                row.get::<_, String>(2)?,
                                row.get::<_, i64>(3)? != 0,
                            ))
                        },
                    )
                    .optional()
                    .map_err(|_| PersistenceError::Storage)?;
                if let Some((photo_id, rating, selection_state, has_saved_edits)) = occupant {
                    if rating != 0 || selection_state != "undecided" || has_saved_edits {
                        return Err(PersistenceError::InvalidRecoveryMapping {
                            original_id: relocation.original_id.clone(),
                            reason: "occupied",
                        });
                    }
                    let members = transaction
                        .query_row(
                            "SELECT COUNT(*) FROM album_members WHERE photo_id=?",
                            params![photo_id],
                            |row| row.get::<_, i64>(0),
                        )
                        .map_err(|_| PersistenceError::Storage)?;
                    if members != 0 {
                        return Err(PersistenceError::InvalidRecoveryMapping {
                            original_id: relocation.original_id.clone(),
                            reason: "occupied",
                        });
                    }
                    transaction
                        .execute("DELETE FROM photos WHERE id=?", params![photo_id])
                        .map_err(|_| PersistenceError::Storage)?;
                }
                transaction
                    .execute("DELETE FROM original_files WHERE id=?", params![owner_id])
                    .map_err(|_| PersistenceError::Storage)?;
            }
        }
        // Two-phase Location updates keep direct swaps from violating the
        // UNIQUE(relative_path) constraint.
        for relocation in relocations {
            transaction
                .execute(
                    "UPDATE original_files SET relative_path=? WHERE id=?",
                    params![
                        format!("\u{0}manual/{}", relocation.original_id),
                        relocation.original_id
                    ],
                )
                .map_err(|_| PersistenceError::Storage)?;
        }
        for relocation in relocations {
            let to = RelativeOriginalPath::parse(relocation.to_location.clone()).map_err(|_| {
                PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "invalid-location",
                }
            })?;
            let size =
                i64::try_from(relocation.facts.size).map_err(|_| PersistenceError::Storage)?;
            let changed = transaction
                .execute(
                    "UPDATE original_files SET relative_path=?,size=?,mtime_ms=?,available=1,
                       error_category=NULL,error_message=NULL,
                       capture_metadata_state='pending',capture_order_key=NULL,
                       capture_time_field=NULL,capture_offset_minutes=NULL,
                       capture_source_revision=NULL
                     WHERE id=?",
                    params![
                        to.as_str(),
                        size,
                        relocation.facts.mtime_ms,
                        relocation.original_id
                    ],
                )
                .map_err(|_| PersistenceError::Storage)?;
            if changed != 1 {
                return Err(PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "stale",
                });
            }
            let changed = transaction
                .execute(
                    "UPDATE photos SET available=1,preview_state='inspection-pending',
                       preview_source_revision=NULL,preview_width=NULL,preview_height=NULL,
                       cache_revision=NULL,sort_path=? WHERE original_id=?",
                    params![to.as_str(), relocation.original_id],
                )
                .map_err(|_| PersistenceError::Storage)?;
            if changed != 1 {
                return Err(PersistenceError::InvalidRecoveryMapping {
                    original_id: relocation.original_id.clone(),
                    reason: "stale",
                });
            }
            // Fingerprints always describe the current persisted revision:
            // the Application layer verified the digest against the candidate
            // before submitting, so the observed facts replace the stale ones.
            transaction
                .execute(
                    "UPDATE original_fingerprints SET size=?,mtime_ms=? WHERE original_id=?",
                    params![size, relocation.facts.mtime_ms, relocation.original_id],
                )
                .map_err(|_| PersistenceError::Storage)?;
        }
        let unavailable = transaction
            .query_row("SELECT COUNT(*) FROM photos WHERE available=0", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(|_| PersistenceError::Storage)?;
        Ok(AppliedRelocations {
            relocated_photos: relocations.len() as u64,
            unavailable_photos: unavailable
                .try_into()
                .map_err(|_| PersistenceError::Storage)?,
        })
    })
}

fn fingerprint_counts(connection: &Connection) -> Result<FingerprintCounts, PersistenceError> {
    connection
        .query_row(
            "SELECT
               SUM(CASE WHEN f.original_id IS NOT NULL AND f.size=o.size AND f.mtime_ms=o.mtime_ms THEN 1 ELSE 0 END),
               SUM(CASE WHEN o.available=1 AND o.error_category IS NULL AND (f.original_id IS NULL OR f.size != o.size OR f.mtime_ms != o.mtime_ms) THEN 1 ELSE 0 END)
             FROM original_files o
             LEFT JOIN original_fingerprints f ON f.original_id=o.id",
            [],
            |row| {
                Ok(FingerprintCounts {
                    enrolled: row.get::<_, Option<i64>>(0)?.unwrap_or(0).try_into().map_err(|_| rusqlite::Error::InvalidQuery)?,
                    pending: row.get::<_, Option<i64>>(1)?.unwrap_or(0).try_into().map_err(|_| rusqlite::Error::InvalidQuery)?,
                })
            },
        )
        .map_err(|_| PersistenceError::Storage)
}

fn validate_database(connection: &Connection) -> Result<(), PersistenceError> {
    if connection
        .prepare("PRAGMA foreign_key_check")
        .and_then(|mut statement| statement.exists([]))
        .map_err(|_| PersistenceError::Storage)?
    {
        return Err(PersistenceError::Storage);
    }
    let integrity: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|_| PersistenceError::Storage)?;
    if integrity != "ok" {
        return Err(PersistenceError::Storage);
    }
    Ok(())
}

type PreservedOriginal = (
    String,
    String,
    i64,
    f64,
    i64,
    Option<String>,
    Option<String>,
);
type PreservedPhoto = (String, String, i64, String, i64);

#[derive(Debug, PartialEq)]
struct ExpansionProjection {
    originals: Vec<PreservedOriginal>,
    photos: Vec<PreservedPhoto>,
    albums: Vec<(String, String, i64)>,
    members: Vec<(String, String, i64)>,
    progress: Vec<(String, String)>,
}

#[derive(Debug)]
struct ExpansionPlan {
    originals: Vec<(String, String, String)>,
    photo_sort_paths: Vec<(String, String)>,
}

pub(crate) fn expand_library_binding(
    proposed_root: &LibraryRoot,
    state: StateDirectory,
    database_name: DatabaseName,
    limits: ScanLimits,
    fail_after_first_update: bool,
) -> Result<(), PersistenceError> {
    let identity = state.prepare_existing_database(&database_name)?;
    let _database_lock = state.lock_database(&database_name)?;
    state.verify_database(&database_name, identity)?;
    let readonly = Connection::open_with_flags(
        state.sqlite_immutable_uri(&database_name),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|_| PersistenceError::Storage)?;
    // A library written by the previous release is still canonical at V7;
    // the writable pass below brings it to the current schema.
    if validate_canonical_schema(&readonly, SchemaVersion::V8).is_err()
        && validate_canonical_schema(&readonly, SchemaVersion::V7).is_err()
    {
        return Err(PersistenceError::UnsupportedSchema);
    }
    let stored_root = required_root_binding(&readonly)?;
    drop(readonly);

    let prefix = expansion_prefix(proposed_root.canonical_path(), &stored_root)?;
    let old_root =
        LibraryRoot::open(&stored_root).map_err(|_| PersistenceError::InvalidExpansion)?;
    let confined_old = proposed_root
        .descendant(
            RelativeOriginalPath::parse(prefix.clone())
                .map_err(|_| PersistenceError::InvalidExpansion)?,
        )
        .map_err(|_| PersistenceError::InvalidExpansion)?;
    if !old_root
        .identifies_same_directory(&confined_old)
        .map_err(|_| PersistenceError::InvalidExpansion)?
    {
        return Err(PersistenceError::InvalidExpansion);
    }
    proposed_root
        .scan(limits)
        .map_err(|_| PersistenceError::InvalidExpansion)?;
    let confined_after_scan = proposed_root
        .descendant(
            RelativeOriginalPath::parse(prefix.clone())
                .map_err(|_| PersistenceError::InvalidExpansion)?,
        )
        .map_err(|_| PersistenceError::InvalidExpansion)?;
    if !old_root
        .identifies_same_directory(&confined_after_scan)
        .map_err(|_| PersistenceError::InvalidExpansion)?
    {
        return Err(PersistenceError::InvalidExpansion);
    }

    state.verify_database(&database_name, identity)?;
    state.admit_sidecars(&database_name)?;
    let mut connection = Connection::open(state.sqlite_path(&database_name))
        .map_err(|_| PersistenceError::Storage)?;
    state.verify_database(&database_name, identity)?;
    state.admit_sidecars(&database_name)?;
    let journal: String = connection
        .pragma_query_value(None, "journal_mode", |row| row.get(0))
        .map_err(|_| PersistenceError::Storage)?;
    if !journal.eq_ignore_ascii_case("delete") {
        return Err(PersistenceError::UnsupportedSchema);
    }
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(|_| PersistenceError::Storage)?;
    // Bring a previous-release schema up to the current one with the same
    // migration chain startup uses, so the expansion writes against the
    // canonical current tables.
    startup_schema(&state, &database_name, &mut connection, &stored_root)?;
    if required_root_binding(&connection)? != stored_root {
        return Err(PersistenceError::RootMismatch);
    }
    validate_database(&connection)?;
    let plan = expansion_plan(&connection, &prefix)?;
    let preserved = expansion_projection(&connection)?;

    state.admit_sidecars(&database_name)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| PersistenceError::Storage)?;
    validate_canonical_schema(&transaction, SchemaVersion::V9)
        .map_err(|_| PersistenceError::UnsupportedSchema)?;
    if required_root_binding(&transaction)? != stored_root
        || expansion_projection(&transaction)? != preserved
    {
        return Err(PersistenceError::InvalidExpansion);
    }
    for (index, (id, old_path, new_path)) in plan.originals.iter().enumerate() {
        let changed = transaction
            .execute(
                "UPDATE original_files SET relative_path=?,capture_metadata_state='pending',capture_order_key=NULL,capture_time_field=NULL,capture_offset_minutes=NULL,capture_source_revision=NULL WHERE id=? AND relative_path=?",
                params![new_path, id, old_path],
            )
            .map_err(|_| PersistenceError::Storage)?;
        if changed != 1 {
            return Err(PersistenceError::InvalidExpansion);
        }
        if fail_after_first_update && index == 0 {
            return Err(PersistenceError::Storage);
        }
    }
    for (id, sort_path) in &plan.photo_sort_paths {
        let changed = transaction
            .execute(
                "UPDATE photos SET sort_path=?,preview_state='inspection-pending',preview_source_revision=NULL,preview_width=NULL,preview_height=NULL,cache_revision=NULL WHERE id=?",
                params![sort_path, id],
            )
            .map_err(|_| PersistenceError::Storage)?;
        if changed != 1 {
            return Err(PersistenceError::InvalidExpansion);
        }
    }
    if transaction
        .execute(
            "UPDATE library_metadata SET value=? WHERE key='canonical_root' AND value=?",
            params![
                proposed_root
                    .canonical_path()
                    .to_str()
                    .ok_or(PersistenceError::InvalidExpansion)?,
                stored_root
            ],
        )
        .map_err(|_| PersistenceError::Storage)?
        != 1
        || expansion_projection(&transaction)? != preserved
    {
        return Err(PersistenceError::InvalidExpansion);
    }
    validate_database(&transaction)?;
    validate_canonical_schema(&transaction, SchemaVersion::V9)
        .map_err(|_| PersistenceError::UnsupportedSchema)?;
    transaction.commit().map_err(|_| PersistenceError::Storage)
}

fn required_root_binding(connection: &Connection) -> Result<String, PersistenceError> {
    connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key='canonical_root'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?
        .ok_or(PersistenceError::InvalidExpansion)
}

fn expansion_prefix(proposed: &Path, stored: &str) -> Result<String, PersistenceError> {
    let stored = Path::new(stored);
    let relative = stored
        .strip_prefix(proposed)
        .map_err(|_| PersistenceError::InvalidExpansion)?;
    let prefix = relative
        .to_str()
        .ok_or(PersistenceError::InvalidExpansion)?;
    RelativeOriginalPath::parse(prefix.to_owned())
        .map(|path| path.as_str().to_owned())
        .map_err(|_| PersistenceError::InvalidExpansion)
}

fn expansion_plan(
    connection: &Connection,
    prefix: &str,
) -> Result<ExpansionPlan, PersistenceError> {
    let originals = connection
        .prepare("SELECT id,relative_path,kind FROM original_files ORDER BY id")
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    let mut paths_by_id = HashMap::new();
    let mut targets = HashSet::new();
    let mut mapped = Vec::with_capacity(originals.len());
    for (id, old_path, kind) in originals {
        let old = RelativeOriginalPath::parse(old_path.clone())
            .map_err(|_| PersistenceError::InvalidExpansion)?;
        let classified = old
            .as_str()
            .rsplit('/')
            .next()
            .and_then(classify_name)
            .ok_or(PersistenceError::InvalidExpansion)?;
        if (classified == OriginalKind::Raw) != (kind == "raw")
            || !matches!(kind.as_str(), "raw" | "jpeg")
        {
            return Err(PersistenceError::InvalidExpansion);
        }
        let new_path = RelativeOriginalPath::parse(format!("{prefix}/{}", old.as_str()))
            .map_err(|_| PersistenceError::InvalidExpansion)?
            .as_str()
            .to_owned();
        if !targets.insert(new_path.clone()) {
            return Err(PersistenceError::InvalidExpansion);
        }
        paths_by_id.insert(id.clone(), new_path.clone());
        mapped.push((id, old_path, new_path));
    }
    let photos = connection
        .prepare("SELECT id,original_id FROM photos ORDER BY id")
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    let mut photo_sort_paths = Vec::with_capacity(photos.len());
    for (id, original) in photos {
        let sort_path = paths_by_id
            .get(&original)
            .ok_or(PersistenceError::InvalidExpansion)?
            .clone();
        photo_sort_paths.push((id, sort_path));
    }
    Ok(ExpansionPlan {
        originals: mapped,
        photo_sort_paths,
    })
}

fn expansion_projection(connection: &Connection) -> Result<ExpansionProjection, PersistenceError> {
    macro_rules! rows {
        ($sql:literal, $map:expr) => {{
            connection
                .prepare($sql)
                .map_err(|_| PersistenceError::Storage)?
                .query_map([], $map)
                .map_err(|_| PersistenceError::Storage)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| PersistenceError::Storage)?
        }};
    }
    Ok(ExpansionProjection {
        originals: rows!(
            "SELECT id,kind,size,mtime_ms,available,error_category,error_message FROM original_files ORDER BY id",
            |row| Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?
            ))
        ),
        photos: rows!(
            "SELECT id,original_id,available,selection_state,rating FROM photos ORDER BY id",
            |row| Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?
            ))
        ),
        albums: rows!("SELECT id,name,created_at FROM albums ORDER BY id", |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        }),
        members: rows!(
            "SELECT album_id,photo_id,position FROM album_members ORDER BY album_id,photo_id",
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        ),
        progress: rows!(
            "SELECT album_id,photo_id FROM album_progress ORDER BY album_id",
            |row| Ok((row.get(0)?, row.get(1)?))
        ),
    })
}

fn snapshot(connection: &Connection) -> Result<ScanSnapshot, PersistenceError> {
    let originals = connection
        .prepare(
            "SELECT id,relative_path,kind,size,mtime_ms,available,error_category,error_message,
                    capture_metadata_state,capture_order_key,capture_time_field,
                    capture_offset_minutes,capture_source_revision
             FROM original_files ORDER BY relative_path COLLATE BINARY",
        )
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], |row| {
            Ok(OriginalRecord {
                id: row.get(0)?,
                relative_path: crate::RelativeOriginalPath::parse(row.get::<_, String>(1)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                kind: parse_kind(&row.get::<_, String>(2)?)?,
                facts: OriginalFacts {
                    size: row
                        .get::<_, i64>(3)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    mtime_ms: row.get(4)?,
                    device: 0,
                    inode: 0,
                },
                available: row.get::<_, i64>(5)? != 0,
                error_category: parse_error_category(row.get(6)?)?,
                error_message: row.get(7)?,
                capture: parse_capture_fact(
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                    row.get(12)?,
                )?,
            })
        })
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    let photos = connection
        .prepare(
            "SELECT p.id,p.original_id,p.available,p.preview_state,
                    p.preview_source_revision,p.preview_width,p.preview_height,p.cache_revision,
                    p.sort_path,p.selection_state,p.rating,
                    EXISTS(SELECT 1 FROM edit_recipes e WHERE e.photo_id=p.id),
                    p.removed_at_ms
             FROM photos p
             LEFT JOIN original_files o ON o.id=p.original_id
             ORDER BY CASE WHEN o.capture_order_key IS NULL THEN 1 ELSE 0 END,
                      o.capture_order_key COLLATE BINARY,
                      p.sort_path COLLATE BINARY,p.id",
        )
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], |row| {
            Ok(PhotoRecord {
                id: row.get(0)?,
                original_id: row.get(1)?,
                available: row.get::<_, i64>(2)? != 0,
                preview_state: parse_preview_state(&row.get::<_, String>(3)?)?,
                preview_source_revision: row.get(4)?,
                preview_width: parse_dimension(row.get(5)?)?,
                preview_height: parse_dimension(row.get(6)?)?,
                cache_revision: row.get(7)?,
                sort_path: row.get(8)?,
                selection_state: parse_selection_state(&row.get::<_, String>(9)?)?,
                rating: row
                    .get::<_, i64>(10)?
                    .try_into()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                has_saved_edits: row.get::<_, i64>(11)? != 0,
                removed: row.get::<_, Option<i64>>(12)?.is_some(),
            })
        })
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    let published = connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key='published_once'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?
        .is_some_and(|value| value == "1");
    Ok(ScanSnapshot {
        published,
        originals,
        photos,
        errors: Vec::new(),
    })
}

fn parse_kind(value: &str) -> rusqlite::Result<OriginalKind> {
    match value {
        "raw" => Ok(OriginalKind::Raw),
        "jpeg" => Ok(OriginalKind::Jpeg),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_error_category(value: Option<String>) -> rusqlite::Result<Option<OriginalErrorCategory>> {
    value
        .map(|value| match value.as_str() {
            "unreadable" => Ok(OriginalErrorCategory::Unreadable),
            "changed" => Ok(OriginalErrorCategory::Changed),
            _ => Err(rusqlite::Error::InvalidQuery),
        })
        .transpose()
}

fn capture_state_name(state: CaptureMetadataState) -> &'static str {
    match state {
        CaptureMetadataState::Pending => "pending",
        CaptureMetadataState::Known => "known",
        CaptureMetadataState::Missing => "missing",
        CaptureMetadataState::Invalid => "invalid",
        CaptureMetadataState::Failed => "failed",
    }
}

fn parse_capture_fact(
    state: String,
    order_key: Option<String>,
    field: Option<String>,
    offset_minutes: Option<i64>,
    source_revision: Option<String>,
) -> rusqlite::Result<CaptureFact> {
    let state = match state.as_str() {
        "pending" => CaptureMetadataState::Pending,
        "known" => CaptureMetadataState::Known,
        "missing" => CaptureMetadataState::Missing,
        "invalid" => CaptureMetadataState::Invalid,
        "failed" => CaptureMetadataState::Failed,
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let field = match field.as_deref() {
        None => None,
        Some(value) => Some(
            CaptureTimeField::parse_database_name(value).ok_or(rusqlite::Error::InvalidQuery)?,
        ),
    };
    let offset_minutes = offset_minutes
        .map(|value| value.try_into().map_err(|_| rusqlite::Error::InvalidQuery))
        .transpose()?;
    let fact = CaptureFact {
        state,
        order_key,
        field,
        offset_minutes,
        source_revision,
    };
    validate_capture_fact(&fact).map_err(|_| rusqlite::Error::InvalidQuery)?;
    Ok(fact)
}

fn valid_capture_order_key(value: &str) -> bool {
    value.len() == 29
        && value.as_bytes()[4] == b'-'
        && value.as_bytes()[7] == b'-'
        && value.as_bytes()[10] == b'T'
        && value.as_bytes()[13] == b':'
        && value.as_bytes()[16] == b':'
        && value.as_bytes()[19] == b'.'
        && value.bytes().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7 | 10 | 13 | 16 | 19) || byte.is_ascii_digit()
        })
}

fn validate_capture_fact(fact: &CaptureFact) -> Result<(), ()> {
    let source_revision = fact
        .source_revision
        .as_deref()
        .is_some_and(|value| !value.is_empty());
    let known = fact
        .order_key
        .as_deref()
        .is_some_and(valid_capture_order_key)
        && fact.field.is_some()
        && source_revision;
    let no_derived =
        fact.order_key.is_none() && fact.field.is_none() && fact.offset_minutes.is_none();
    match fact.state {
        CaptureMetadataState::Pending => no_derived && fact.source_revision.is_none(),
        CaptureMetadataState::Known => known,
        CaptureMetadataState::Missing | CaptureMetadataState::Invalid => {
            no_derived && source_revision
        }
        CaptureMetadataState::Failed => no_derived,
    }
    .then_some(())
    .ok_or(())
}

fn parse_preview_state(value: &str) -> rusqlite::Result<PreviewState> {
    match value {
        "inspection-pending" => Ok(PreviewState::InspectionPending),
        "ready" => Ok(PreviewState::Ready),
        "failed" => Ok(PreviewState::Failed),
        "unavailable" => Ok(PreviewState::Unavailable),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_selection_state(value: &str) -> rusqlite::Result<SelectionState> {
    match value {
        "undecided" => Ok(SelectionState::Undecided),
        "selected" => Ok(SelectionState::Selected),
        "rejected" => Ok(SelectionState::Rejected),
        _ => Err(rusqlite::Error::InvalidQuery),
    }
}

fn parse_dimension(value: Option<i64>) -> rusqlite::Result<Option<u32>> {
    value
        .map(|value| value.try_into().map_err(|_| rusqlite::Error::InvalidQuery))
        .transpose()
}

fn preview_state_name(state: PreviewState) -> &'static str {
    match state {
        PreviewState::InspectionPending => "inspection-pending",
        PreviewState::Ready => "ready",
        PreviewState::Failed => "failed",
        PreviewState::Unavailable => "unavailable",
    }
}

/// The recovery decisions proven outside the state store and applied inside
/// one scan transaction. Relocations map a discovered Location to the
/// persisted Original File identity whose exact content was found there.
/// Fingerprints are keyed by the discovered Location they were computed at;
/// the transaction resolves each to the Original that Location was assigned.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScanRecoveryPlan {
    pub relocations: HashMap<String, String>,
    pub fingerprints: Vec<DiscoveredFingerprint>,
}

/// One complete-content digest computed for the file observed at one
/// discovered Location during this scan.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredFingerprint {
    pub path: String,
    pub digest: String,
}

/// The committed result of one scan application.
#[derive(Clone, Debug, PartialEq)]
pub struct ScanApplication {
    pub snapshot: ScanSnapshot,
    pub relocated_originals: usize,
    pub fingerprinted_originals: usize,
}

/// One enrollment work item: the persisted Original whose current observed
/// revision still needs a content fingerprint.
#[derive(Clone, Debug, PartialEq)]
pub struct FingerprintTarget {
    pub original_id: String,
    pub relative_path: String,
    pub kind: crate::OriginalKind,
    pub size: u64,
    pub mtime_ms: f64,
}

/// Truthful enrollment counters for status reporting.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FingerprintCounts {
    pub enrolled: usize,
    pub pending: usize,
}

fn apply_scan(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    discovered: &[DiscoveredOriginal],
    errors: &[OriginalScanError],
    recovery: &ScanRecoveryPlan,
    failure_after_first: bool,
) -> Result<ScanApplication, PersistenceError> {
    let before = snapshot(connection)?;
    let previous_originals = before
        .originals
        .iter()
        .map(|original| (original.relative_path.as_str().to_owned(), original.clone()))
        .collect::<std::collections::HashMap<_, _>>();
    write_transaction(state, database_name, connection, |transaction| {
        // Validate every proposed relocation against the persisted state and
        // the complete discovered set before any write.
        let mut persisted_by_id = HashMap::with_capacity(before.originals.len());
        let mut persisted_by_path = HashMap::with_capacity(before.originals.len());
        for original in &before.originals {
            persisted_by_id.insert(original.id.clone(), original.clone());
            persisted_by_path.insert(
                original.relative_path.as_str().to_owned(),
                original.id.clone(),
            );
        }
        let mut discovered_by_path = HashMap::with_capacity(discovered.len());
        for original in discovered {
            discovered_by_path.insert(original.path.as_str().to_owned(), original);
        }
        let mut relocation_by_id = HashMap::with_capacity(recovery.relocations.len());
        for (new_path, original_id) in &recovery.relocations {
            let Some(persisted) = persisted_by_id.get(original_id) else {
                return Err(PersistenceError::InvalidRecovery);
            };
            let Some(discovered_original) = discovered_by_path.get(new_path.as_str()) else {
                return Err(PersistenceError::InvalidRecovery);
            };
            if discovered_original.kind != persisted.kind {
                return Err(PersistenceError::InvalidRecovery);
            }
            if relocation_by_id
                .insert(original_id.clone(), new_path.clone())
                .is_some()
            {
                return Err(PersistenceError::InvalidRecovery);
            }
        }
        // The final Location assignment must stay injective: a relocated
        // Original may land on a Location vacated by another relocation, but
        // never on one still owned by a non-relocating Original.
        let mut final_locations = std::collections::BTreeSet::new();
        for original in &before.originals {
            let final_path = relocation_by_id
                .get(&original.id)
                .map_or_else(|| original.relative_path.as_str().to_owned(), Clone::clone);
            if !final_locations.insert(final_path) {
                return Err(PersistenceError::InvalidRecovery);
            }
        }

        // Move every relocated Original to a temporary unique Location first
        // so direct swaps cannot violate the UNIQUE(relative_path) constraint.
        for original_id in relocation_by_id.keys() {
            transaction
                .execute(
                    "UPDATE original_files SET relative_path=? WHERE id=?",
                    params![format!("\u{0}reloc/{original_id}"), original_id],
                )
                .map_err(|_| PersistenceError::Storage)?;
        }
        for (original_id, new_path) in &relocation_by_id {
            let changed = transaction
                .execute(
                    "UPDATE original_files SET relative_path=?,
                       capture_metadata_state='pending',capture_order_key=NULL,
                       capture_time_field=NULL,capture_offset_minutes=NULL,
                       capture_source_revision=NULL
                     WHERE id=?",
                    params![new_path, original_id],
                )
                .map_err(|_| PersistenceError::Storage)?;
            if changed != 1 {
                return Err(PersistenceError::InvalidRecovery);
            }
        }

        let existing_ids = persisted_by_path;
        let mut reserved_ids = HashSet::new();
        let mut original_ids = HashMap::with_capacity(discovered.len());
        for original in discovered {
            let id = if let Some(id) = recovery.relocations.get(original.path.as_str()) {
                id.clone()
            } else if let Some(id) = existing_ids.get(original.path.as_str()) {
                id.clone()
            } else {
                allocate_library_id(transaction, &mut reserved_ids)?
            };
            original_ids.insert(original.path.as_str().to_owned(), id);
        }
        let reconciled = reconcile(discovered, &before.photos, &original_ids, || {
            allocate_library_id(transaction, &mut reserved_ids)
        })?;
        transaction
            .execute("UPDATE original_files SET available=0", [])
            .map_err(|_| PersistenceError::Storage)?;
        transaction
            .execute("UPDATE photos SET available=0", [])
            .map_err(|_| PersistenceError::Storage)?;
        let mut upsert_original = transaction
            .prepare(
                "INSERT INTO original_files(
                    id,relative_path,kind,size,mtime_ms,available,error_category,error_message,
                    capture_metadata_state,capture_order_key,capture_time_field,
                    capture_offset_minutes,capture_source_revision)
                 VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)
                 ON CONFLICT(relative_path) DO UPDATE SET
                   kind=excluded.kind,size=excluded.size,mtime_ms=excluded.mtime_ms,
                   available=excluded.available,error_category=excluded.error_category,error_message=excluded.error_message,
                   capture_metadata_state=excluded.capture_metadata_state,
                   capture_order_key=excluded.capture_order_key,
                   capture_time_field=excluded.capture_time_field,
                   capture_offset_minutes=excluded.capture_offset_minutes,
                   capture_source_revision=excluded.capture_source_revision",
            )
            .map_err(|_| PersistenceError::Storage)?;
        for (index, original) in discovered.iter().enumerate() {
            validate_capture_fact(&original.capture).map_err(|_| PersistenceError::Storage)?;
            let id = original_ids
                .get(original.path.as_str())
                .expect("assigned Original identity");
            upsert_original
                .execute(params![
                    id,
                    original.path.as_str(),
                    match original.kind {
                        OriginalKind::Raw => "raw",
                        OriginalKind::Jpeg => "jpeg",
                    },
                    i64::try_from(original.facts.size).map_err(|_| PersistenceError::Storage)?,
                    original.facts.mtime_ms,
                    i64::from(original.error_category.is_none()),
                    original
                        .error_category
                        .as_ref()
                        .map(|category| match category {
                            OriginalErrorCategory::Unreadable => "unreadable",
                            OriginalErrorCategory::Changed => "changed",
                        }),
                    original.error_message.as_deref(),
                    capture_state_name(original.capture.state),
                    original.capture.order_key.as_deref(),
                    original.capture.field.map(CaptureTimeField::database_name),
                    original.capture.offset_minutes.map(i64::from),
                    original.capture.source_revision.as_deref(),
                ])
                .map_err(|_| PersistenceError::Storage)?;
            if failure_after_first && index == 0 {
                return Err(PersistenceError::Storage);
            }
        }
        let mut upsert_photo = transaction
            .prepare(
                "INSERT INTO photos(id,original_id,available,
                    preview_state,preview_source_revision,
                    preview_width,preview_height,cache_revision,sort_path)
                 VALUES(?,?,?,?,?,?,?,?,?)
                 ON CONFLICT(id) DO UPDATE SET
                    original_id=excluded.original_id,available=excluded.available,
                    preview_state=excluded.preview_state,
                    preview_source_revision=excluded.preview_source_revision,
                    preview_width=excluded.preview_width,preview_height=excluded.preview_height,
                    cache_revision=excluded.cache_revision,sort_path=excluded.sort_path",
            )
            .map_err(|_| PersistenceError::Storage)?;
        for photo in &reconciled {
            let selected = selected_source(photo);
            let selected_path = selected.map(|(original, _)| original.path.as_str().to_owned());
            let preserve = photo
                .prior
                .as_ref()
                .is_some_and(|prior| preview_should_preserve(prior, selected, &previous_originals));
            let source_revision = if preserve {
                photo
                    .prior
                    .as_ref()
                    .and_then(|prior| prior.preview_source_revision.clone())
            } else {
                None
            };
            let preview_state = if preserve {
                photo.prior.as_ref().unwrap().preview_state
            } else if selected.is_some() {
                PreviewState::InspectionPending
            } else {
                PreviewState::Unavailable
            };
            upsert_photo
                .execute(params![
                    photo.id,
                    photo.original_id,
                    i64::from(
                        photo
                            .original
                            .as_ref()
                            .is_some_and(|original| original.error_category.is_none())
                    ),
                    preview_state_name(preview_state),
                    source_revision,
                    preserve
                        .then(|| photo.prior.as_ref().unwrap().preview_width)
                        .flatten()
                        .map(i64::from),
                    preserve
                        .then(|| photo.prior.as_ref().unwrap().preview_height)
                        .flatten()
                        .map(i64::from),
                    preserve
                        .then(|| photo.prior.as_ref().unwrap().cache_revision.clone())
                        .flatten(),
                    if photo.sort_path.is_empty() {
                        selected_path.unwrap_or_default()
                    } else {
                        photo.sort_path.clone()
                    },
                ])
                .map_err(|_| PersistenceError::Storage)?;
        }
        // Fingerprints always describe the current persisted revision: drop
        // rows whose observed facts no longer match, then record every digest
        // freshly computed for this scan.
        transaction
            .execute(
                "DELETE FROM original_fingerprints WHERE original_id IN (
                     SELECT f.original_id FROM original_fingerprints f
                     JOIN original_files o ON o.id=f.original_id
                     WHERE f.size != o.size OR f.mtime_ms != o.mtime_ms)",
                [],
            )
            .map_err(|_| PersistenceError::Storage)?;
        let mut upsert_fingerprint = transaction
            .prepare(
                "INSERT INTO original_fingerprints(original_id,digest,size,mtime_ms)
                 SELECT o.id,?,o.size,o.mtime_ms FROM original_files o WHERE o.relative_path=?
                 ON CONFLICT(original_id) DO UPDATE SET
                   digest=excluded.digest,size=excluded.size,mtime_ms=excluded.mtime_ms",
            )
            .map_err(|_| PersistenceError::Storage)?;
        for fingerprint in &recovery.fingerprints {
            let changed = upsert_fingerprint
                .execute(params![fingerprint.digest, fingerprint.path])
                .map_err(|_| PersistenceError::Storage)?;
            if changed != 1 {
                return Err(PersistenceError::InvalidRecovery);
            }
        }
        transaction
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('published_once','1')
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                [],
            )
            .map_err(|_| PersistenceError::Storage)?;
        Ok(())
    })?;
    let mut result = snapshot(connection)?;
    result.errors = errors.to_vec();
    Ok(ScanApplication {
        snapshot: result,
        relocated_originals: recovery.relocations.len(),
        fingerprinted_originals: recovery.fingerprints.len(),
    })
}

fn seed_preview(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    preview: PreviewSeed,
) -> Result<PreviewSeedResult, PersistenceError> {
    write_transaction(state, database_name, connection, |transaction| {
        // The compare-and-set anchor is the persisted Original's current
        // revision: any scan that changed the Original between inspection and
        // this seed makes the computed revision differ and the seed stale.
        let row = transaction
            .query_row(
                "SELECT o.relative_path,o.size,o.mtime_ms,o.kind,p.original_id
                 FROM photos p JOIN original_files o ON o.id=p.original_id
                 WHERE p.id=?",
                [&preview.photo_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, f64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(|_| PersistenceError::Storage)?;
        let Some((path, size, mtime_ms, kind, original_id)) = row else {
            return Ok(PreviewSeedResult::StaleIgnored);
        };
        let Ok(size) = u64::try_from(size) else {
            return Ok(PreviewSeedResult::StaleIgnored);
        };
        let parsed_kind = match kind.as_str() {
            "raw" => crate::OriginalKind::Raw,
            "jpeg" => crate::OriginalKind::Jpeg,
            _ => return Ok(PreviewSeedResult::StaleIgnored),
        };
        if parsed_kind.preview_source() != preview.source {
            return Ok(PreviewSeedResult::StaleIgnored);
        }
        let Some(current_revision) = crate::source_revision(&path, size, mtime_ms).ok() else {
            return Ok(PreviewSeedResult::StaleIgnored);
        };
        if current_revision != preview.expected_source_revision {
            return Ok(PreviewSeedResult::StaleIgnored);
        }
        let changed = transaction
            .execute(
                "UPDATE photos SET preview_state=?,preview_source_revision=?,
                        preview_width=?,preview_height=?,cache_revision=?
                 WHERE id=? AND original_id=?",
                params![
                    preview_state_name(preview.state),
                    current_revision,
                    preview.width.map(i64::from),
                    preview.height.map(i64::from),
                    preview.cache_revision,
                    preview.photo_id,
                    original_id,
                ],
            )
            .map_err(|_| PersistenceError::Storage)?;
        Ok(if changed == 1 {
            PreviewSeedResult::Applied
        } else {
            PreviewSeedResult::StaleIgnored
        })
    })
}

fn write_transaction<T>(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    operation: impl FnOnce(&Transaction<'_>) -> Result<T, PersistenceError>,
) -> Result<T, PersistenceError> {
    state.admit_sidecars(database_name)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| PersistenceError::Storage)?;
    let result = operation(&transaction)?;
    transaction
        .commit()
        .map_err(|_| PersistenceError::Storage)?;
    Ok(result)
}

fn read_edit_recipe(
    connection: &Connection,
    photo_id: &str,
) -> Result<Option<EditRecipeRead>, PersistenceError> {
    let row = connection
        .query_row(
            "SELECT o.relative_path,o.size,o.mtime_ms,o.available,p.available,
                    e.revision,e.source_revision,e.exposure_ev,e.white_balance_mode,
                    e.temperature_kelvin,e.tint_milli
             FROM photos p JOIN original_files o ON o.id=p.original_id
             LEFT JOIN edit_recipes e ON e.photo_id=p.id WHERE p.id=?",
            [photo_id],
            |row| {
                let relative_path: String = row.get(0)?;
                let size = u64::try_from(row.get::<_, i64>(1)?)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                let mtime_ms: f64 = row.get(2)?;
                let source_available = row.get::<_, i64>(3)? != 0 && row.get::<_, i64>(4)? != 0;
                let recipe_revision: Option<String> = row.get(5)?;
                let recipe_source_revision: Option<String> = row.get(6)?;
                let exposure_ev: Option<f64> = row.get(7)?;
                let white_balance_mode: Option<String> = row.get(8)?;
                let temperature_kelvin: Option<i32> = row.get(9)?;
                let tint_milli: Option<i32> = row.get(10)?;
                let recipe = match (
                    recipe_revision,
                    recipe_source_revision,
                    exposure_ev,
                    white_balance_mode,
                ) {
                    (None, None, None, None) => None,
                    (Some(revision), Some(source_revision), Some(exposure_ev), Some(mode)) => {
                        Some(EditRecipe {
                            photo_id: photo_id.to_owned(),
                            revision,
                            source_revision,
                            settings: EditRecipeSettings {
                                exposure_ev,
                                white_balance: parse_white_balance_intent(
                                    &mode,
                                    temperature_kelvin,
                                    tint_milli,
                                )?,
                            },
                        })
                    }
                    _ => return Err(rusqlite::Error::InvalidQuery),
                };
                Ok((relative_path, size, mtime_ms, source_available, recipe))
            },
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    let Some((relative_path, size, mtime_ms, source_available, recipe)) = row else {
        return Ok(None);
    };
    let current_source_revision = crate::source_revision(&relative_path, size, mtime_ms)
        .map_err(|_| PersistenceError::Storage)?;
    Ok(Some(EditRecipeRead {
        recipe,
        current_source_revision,
        source_available,
    }))
}

fn validate_edit_recipe_request_id(request_id: &str) -> bool {
    !request_id.is_empty()
        && request_id.len() <= MAXIMUM_EDIT_RECIPE_REQUEST_ID_BYTES
        && !request_id.chars().any(char::is_control)
}

fn edit_recipe_payload_digest(mutation: &SaveEditRecipe) -> Result<String, PersistenceError> {
    // The digest formula is durable, not a wire field: receipts written by
    // earlier releases hold digests over these exact serialized keys, so the
    // internal rename of the recipe-version field must not change them.
    // The as-shot white-balance value stays a bare string for the same
    // reason; a value-carrying intent serializes its values because two
    // different payloads under one request identity must never collide.
    let white_balance = match mutation.settings.white_balance {
        WhiteBalanceIntent::AsShot => serde_json::json!("as-shot"),
        WhiteBalanceIntent::TemperatureTint {
            temperature_kelvin,
            tint_milli,
        } => serde_json::json!({
            "mode": "temperature-tint",
            "temperature_kelvin": temperature_kelvin,
            "tint_milli": tint_milli,
        }),
    };
    let payload = serde_json::json!({
        "photo_id": mutation.photo_id,
        "expected_recipe_revision": mutation.expected_recipe_version,
        "expected_source_revision": mutation.expected_source_revision,
        "exposure_ev": mutation.settings.exposure_ev,
        "white_balance": white_balance,
    });
    let bytes = serde_json::to_vec(&payload).map_err(|_| PersistenceError::Storage)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn edit_recipe_receipt_key(request_id: &str) -> String {
    format!("{EDIT_RECIPE_RECEIPT_PREFIX}{request_id}")
}

fn read_edit_recipe_receipt(
    connection: &Connection,
    request_id: &str,
) -> Result<Option<EditRecipeReceipt>, PersistenceError> {
    let value = connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [edit_recipe_receipt_key(request_id)],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    value
        .map(|value| serde_json::from_str(&value).map_err(|_| PersistenceError::Storage))
        .transpose()
}

fn write_edit_recipe_receipt(
    transaction: &Transaction<'_>,
    request_id: &str,
    receipt: &EditRecipeReceipt,
) -> Result<(), PersistenceError> {
    let value = serde_json::to_string(receipt).map_err(|_| PersistenceError::Storage)?;
    transaction
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES(?,?)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![edit_recipe_receipt_key(request_id), value],
        )
        .map_err(|_| PersistenceError::Storage)?;
    Ok(())
}

fn receipt_recipe(receipt: EditRecipeReceipt) -> Result<EditRecipe, PersistenceError> {
    let white_balance = parse_white_balance_intent(
        &receipt.white_balance_mode,
        receipt.temperature_kelvin,
        receipt.tint_milli,
    )
    .map_err(|_| PersistenceError::Storage)?;
    Ok(EditRecipe {
        photo_id: receipt.photo_id,
        revision: receipt.revision,
        source_revision: receipt.source_revision,
        settings: EditRecipeSettings {
            exposure_ev: receipt.exposure_ev,
            white_balance,
        },
    })
}

fn save_edit_recipe(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    mutation: SaveEditRecipe,
) -> Result<EditRecipeWriteOutcome, PersistenceError> {
    if !mutation.settings.exposure_ev.is_finite()
        || !mutation.settings.white_balance.within_payload_bounds()
        || mutation.expected_source_revision.is_empty()
        || !validate_edit_recipe_request_id(&mutation.request_id)
    {
        return Ok(EditRecipeWriteOutcome::InvalidSettings);
    }
    let payload_digest = edit_recipe_payload_digest(&mutation)?;
    write_transaction(state, database_name, connection, |transaction| {
        if let Some(receipt) = read_edit_recipe_receipt(transaction, &mutation.request_id)? {
            if receipt.photo_id != mutation.photo_id || receipt.payload_digest != payload_digest {
                return Ok(EditRecipeWriteOutcome::RequestConflict);
            }
            let recipe = receipt_recipe(receipt.clone())?;
            return Ok(match receipt.outcome {
                EditRecipeReceiptOutcome::Saved => EditRecipeWriteOutcome::Replayed(recipe),
                EditRecipeReceiptOutcome::Unchanged => EditRecipeWriteOutcome::Unchanged(recipe),
            });
        }
        let Some(current) = read_edit_recipe(transaction, &mutation.photo_id)? else {
            return Ok(EditRecipeWriteOutcome::MissingPhoto);
        };
        let Some((kind, available)) = photo_processing_source(transaction, &mutation.photo_id)?
        else {
            return Ok(EditRecipeWriteOutcome::MissingPhoto);
        };
        if kind != crate::OriginalKind::Raw {
            return Ok(EditRecipeWriteOutcome::UnsupportedPhoto);
        }
        if !available || !current.source_available {
            return Ok(EditRecipeWriteOutcome::Unavailable);
        }
        if current.current_source_revision != mutation.expected_source_revision {
            return Ok(EditRecipeWriteOutcome::SourceChanged(current));
        }
        if current
            .recipe
            .as_ref()
            .map(|recipe| recipe.revision.as_str())
            != mutation.expected_recipe_version.as_deref()
        {
            return Ok(EditRecipeWriteOutcome::Conflict(current));
        }
        if current
            .recipe
            .as_ref()
            .is_some_and(|recipe| recipe.source_revision != mutation.expected_source_revision)
        {
            return Ok(EditRecipeWriteOutcome::RequiresRebind(current));
        }
        if let Some(recipe) = &current.recipe
            && recipe.settings == mutation.settings
        {
            let (temperature_kelvin, tint_milli) =
                white_balance_intent_values(recipe.settings.white_balance);
            write_edit_recipe_receipt(
                transaction,
                &mutation.request_id,
                &EditRecipeReceipt {
                    photo_id: recipe.photo_id.clone(),
                    payload_digest,
                    outcome: EditRecipeReceiptOutcome::Unchanged,
                    revision: recipe.revision.clone(),
                    source_revision: recipe.source_revision.clone(),
                    exposure_ev: recipe.settings.exposure_ev,
                    white_balance_mode: white_balance_intent_name(recipe.settings.white_balance)
                        .to_owned(),
                    temperature_kelvin,
                    tint_milli,
                },
            )?;
            return Ok(EditRecipeWriteOutcome::Unchanged(recipe.clone()));
        }

        let revision = random_uuid_v4()?;
        let (temperature_kelvin, tint_milli) =
            white_balance_intent_values(mutation.settings.white_balance);
        transaction
            .execute(
                "INSERT INTO edit_recipes(photo_id,revision,source_revision,exposure_ev,white_balance_mode,temperature_kelvin,tint_milli)
                 VALUES(?,?,?,?,?,?,?)
                 ON CONFLICT(photo_id) DO UPDATE SET revision=excluded.revision,
                    source_revision=excluded.source_revision,exposure_ev=excluded.exposure_ev,
                    white_balance_mode=excluded.white_balance_mode,
                    temperature_kelvin=excluded.temperature_kelvin,tint_milli=excluded.tint_milli",
                params![
                    mutation.photo_id,
                    revision,
                    mutation.expected_source_revision,
                    mutation.settings.exposure_ev,
                    white_balance_intent_name(mutation.settings.white_balance),
                    temperature_kelvin,
                    tint_milli,
                ],
            )
            .map_err(|_| PersistenceError::Storage)?;
        let recipe = EditRecipe {
            photo_id: mutation.photo_id,
            revision,
            source_revision: mutation.expected_source_revision,
            settings: mutation.settings,
        };
        let (temperature_kelvin, tint_milli) =
            white_balance_intent_values(recipe.settings.white_balance);
        write_edit_recipe_receipt(
            transaction,
            &mutation.request_id,
            &EditRecipeReceipt {
                photo_id: recipe.photo_id.clone(),
                payload_digest,
                outcome: EditRecipeReceiptOutcome::Saved,
                revision: recipe.revision.clone(),
                source_revision: recipe.source_revision.clone(),
                exposure_ev: recipe.settings.exposure_ev,
                white_balance_mode: white_balance_intent_name(recipe.settings.white_balance)
                    .to_owned(),
                temperature_kelvin,
                tint_milli,
            },
        )?;
        Ok(EditRecipeWriteOutcome::Saved(recipe))
    })
}

fn edit_recipe_rebind_payload_digest(
    mutation: &RebindEditRecipe,
) -> Result<String, PersistenceError> {
    let payload = serde_json::json!({
        "kind": "rebind",
        "photo_id": mutation.photo_id,
        "expected_recipe_version": mutation.expected_recipe_version,
        "new_source_revision": mutation.new_source_revision,
    });
    let bytes = serde_json::to_vec(&payload).map_err(|_| PersistenceError::Storage)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn rebind_edit_recipe(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    mutation: RebindEditRecipe,
) -> Result<EditRecipeWriteOutcome, PersistenceError> {
    if mutation.new_source_revision.is_empty()
        || !validate_edit_recipe_request_id(&mutation.request_id)
    {
        return Ok(EditRecipeWriteOutcome::InvalidSettings);
    }
    let payload_digest = edit_recipe_rebind_payload_digest(&mutation)?;
    write_transaction(state, database_name, connection, |transaction| {
        // The rebind identity follows the save rules: the same identity and
        // payload replays the committed receipt, and the same identity with
        // a different payload is refused.
        if let Some(receipt) = read_edit_recipe_receipt(transaction, &mutation.request_id)? {
            if receipt.photo_id != mutation.photo_id || receipt.payload_digest != payload_digest {
                return Ok(EditRecipeWriteOutcome::RequestConflict);
            }
            let recipe = receipt_recipe(receipt.clone())?;
            return Ok(match receipt.outcome {
                EditRecipeReceiptOutcome::Saved => EditRecipeWriteOutcome::Replayed(recipe),
                EditRecipeReceiptOutcome::Unchanged => EditRecipeWriteOutcome::Unchanged(recipe),
            });
        }
        let Some(current) = read_edit_recipe(transaction, &mutation.photo_id)? else {
            return Ok(EditRecipeWriteOutcome::MissingPhoto);
        };
        let Some((kind, available)) = photo_processing_source(transaction, &mutation.photo_id)?
        else {
            return Ok(EditRecipeWriteOutcome::MissingPhoto);
        };
        if kind != crate::OriginalKind::Raw {
            return Ok(EditRecipeWriteOutcome::UnsupportedPhoto);
        }
        if !available || !current.source_available {
            return Ok(EditRecipeWriteOutcome::Unavailable);
        }
        let Some(recipe) = current.recipe.as_ref() else {
            return Ok(EditRecipeWriteOutcome::MissingRecipe);
        };
        if recipe.revision != mutation.expected_recipe_version {
            return Ok(EditRecipeWriteOutcome::Conflict(current));
        }
        if current.current_source_revision != mutation.new_source_revision {
            return Ok(EditRecipeWriteOutcome::SourceChanged(current));
        }
        if recipe.source_revision == mutation.new_source_revision {
            let (temperature_kelvin, tint_milli) =
                white_balance_intent_values(recipe.settings.white_balance);
            write_edit_recipe_receipt(
                transaction,
                &mutation.request_id,
                &EditRecipeReceipt {
                    photo_id: recipe.photo_id.clone(),
                    payload_digest,
                    outcome: EditRecipeReceiptOutcome::Unchanged,
                    revision: recipe.revision.clone(),
                    source_revision: recipe.source_revision.clone(),
                    exposure_ev: recipe.settings.exposure_ev,
                    white_balance_mode: white_balance_intent_name(recipe.settings.white_balance)
                        .to_owned(),
                    temperature_kelvin,
                    tint_milli,
                },
            )?;
            return Ok(EditRecipeWriteOutcome::Unchanged(recipe.clone()));
        }
        let revision = random_uuid_v4()?;
        let changed = transaction
            .execute(
                "UPDATE edit_recipes SET revision=?,source_revision=?
                 WHERE photo_id=? AND revision=?",
                params![
                    revision,
                    mutation.new_source_revision,
                    mutation.photo_id,
                    mutation.expected_recipe_version,
                ],
            )
            .map_err(|_| PersistenceError::Storage)?;
        if changed != 1 {
            return Ok(EditRecipeWriteOutcome::Conflict(current));
        }
        let rebound = EditRecipe {
            photo_id: mutation.photo_id,
            revision,
            source_revision: mutation.new_source_revision,
            settings: recipe.settings,
        };
        let (temperature_kelvin, tint_milli) =
            white_balance_intent_values(rebound.settings.white_balance);
        write_edit_recipe_receipt(
            transaction,
            &mutation.request_id,
            &EditRecipeReceipt {
                photo_id: rebound.photo_id.clone(),
                payload_digest,
                outcome: EditRecipeReceiptOutcome::Saved,
                revision: rebound.revision.clone(),
                source_revision: rebound.source_revision.clone(),
                exposure_ev: rebound.settings.exposure_ev,
                white_balance_mode: white_balance_intent_name(rebound.settings.white_balance)
                    .to_owned(),
                temperature_kelvin,
                tint_milli,
            },
        )?;
        Ok(EditRecipeWriteOutcome::Saved(rebound))
    })
}

// Export lifecycle: durable records, request-identity receipts, exactly-once
// settlement, bounded retention, and download leases. Every write runs in the
// serialized owner so a racing cancel and completion settle exactly once.

const EXPORT_RECEIPT_PREFIX: &str = "export_receipt:";
const MAXIMUM_EXPORT_REQUEST_ID_BYTES: usize = 128;
const MAXIMUM_EXPORT_OUTCOME_BYTES: usize = 200;
/// Bounded per-Photo list returned by the retained-export listing.
const EXPORT_LIST_LIMIT: usize = 60;
/// A lease protects an artifact for the duration of one download stream. This
/// bound only reclaims leases leaked by a crashed process; ordinary downloads
/// release their lease when the stream settles.
const EXPORT_LEASE_STALE_SECONDS: u64 = 24 * 60 * 60;

type ExportPublicationClaimReply = Result<Option<(String, u64)>, PersistenceError>;

#[derive(serde::Serialize, serde::Deserialize)]
struct ExportPublicationClaimRow {
    incarnation: String,
    sequence: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ExportReceipt {
    payload_digest: String,
    export_id: String,
    created_at: u64,
    settled_at: Option<u64>,
}

fn export_unix_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn validate_export_request_id(request_id: &str) -> bool {
    !request_id.is_empty()
        && request_id.len() <= MAXIMUM_EXPORT_REQUEST_ID_BYTES
        && !request_id.chars().any(char::is_control)
}

/// Durably records that `attempt` is about to publish `export_id`'s
/// artifact, before the rename: a restart can then tell a file published by
/// this very attempt from a stale leftover of a superseded one.
fn claim_export_publication(
    connection: &mut Connection,
    export_id: &str,
    incarnation: &str,
    sequence: u64,
) -> Result<bool, PersistenceError> {
    let transaction = connection
        .transaction()
        .map_err(|_| PersistenceError::Storage)?;
    let claim = serde_json::json!({
        "incarnation": incarnation,
        "sequence": sequence,
    });
    transaction
        .execute(
            "INSERT OR REPLACE INTO library_metadata(key,value) VALUES(?1,?2)",
            params![export_publication_claim_key(export_id), claim.to_string()],
        )
        .map_err(|_| PersistenceError::Storage)?;
    transaction
        .commit()
        .map_err(|_| PersistenceError::Storage)?;
    Ok(true)
}

/// The durable publication claim of an Export: the attempt whose validated
/// artifact is (about to be) renamed into place, if any.
fn export_publication_claim(connection: &Connection, export_id: &str) -> Option<(String, u64)> {
    let value = connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [export_publication_claim_key(export_id)],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()?;
    let claim: ExportPublicationClaimRow = serde_json::from_str(&value).ok()?;
    Some((claim.incarnation, claim.sequence))
}

fn export_publication_claim_key(export_id: &str) -> String {
    format!("export_publication:{export_id}")
}

fn export_payload_digest(submission: &ExportSubmission) -> Result<String, PersistenceError> {
    Ok(submission.payload_digest())
}

/// Export request identities are unique per Photo; the receipt key carries
/// the Photo identity next to the caller's request identity.
fn export_receipt_key(photo_id: &str, request_id: &str) -> String {
    format!("{EXPORT_RECEIPT_PREFIX}{photo_id}\0{request_id}")
}

/// A post-retention identity marker: the Export row is gone, but its
/// identity stays expired forever.
fn export_expiry_tombstone_key(export_id: &str) -> String {
    format!("export_expired:{export_id}")
}

fn read_export_expiry_tombstone(
    transaction: &Transaction<'_>,
    export_id: &str,
) -> Result<bool, PersistenceError> {
    transaction
        .query_row(
            "SELECT 1 FROM library_metadata WHERE key=?",
            [export_expiry_tombstone_key(export_id)],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map(|found| found.is_some())
        .map_err(|_| PersistenceError::Storage)
}

fn read_export_receipt(
    transaction: &Transaction<'_>,
    photo_id: &str,
    request_id: &str,
) -> Result<Option<ExportReceipt>, PersistenceError> {
    let value = transaction
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [export_receipt_key(photo_id, request_id)],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    value
        .map(|value| serde_json::from_str(&value).map_err(|_| PersistenceError::Storage))
        .transpose()
}

fn write_export_receipt(
    transaction: &Transaction<'_>,
    photo_id: &str,
    request_id: &str,
    receipt: &ExportReceipt,
) -> Result<(), PersistenceError> {
    let value = serde_json::to_string(receipt).map_err(|_| PersistenceError::Storage)?;
    transaction
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES(?,?)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![export_receipt_key(photo_id, request_id), value],
        )
        .map_err(|_| PersistenceError::Storage)?;
    Ok(())
}

struct ExportRow {
    id: String,
    photo_id: String,
    state: ExportState,
    outcome: Option<String>,
    recipe_revision: String,
    exposure_ev: f64,
    source_revision: String,
    source_profile_id: String,
    source_size: Option<u64>,
    source_sha256: Option<String>,
    recipe_digest: String,
    policy_id: String,
    bundle_id: String,
    attempt_incarnation: Option<String>,
    attempt_sequence: Option<u64>,
    artifact_size: Option<u64>,
    artifact_sha256: Option<String>,
    artifact_expires_at: Option<u64>,
    artifact_width: Option<u32>,
    artifact_height: Option<u32>,
    artifact_profile_identity: Option<String>,
    created_at: u64,
    settled_at: Option<u64>,
    retain_until: Option<u64>,
}

const EXPORT_ROW_COLUMNS: &str = "id,photo_id,state,outcome,recipe_revision,exposure_ev,
    source_revision,source_profile_id,source_size,source_sha256,recipe_digest,policy_id,
    bundle_id,attempt_incarnation,attempt_sequence,artifact_size,artifact_sha256,
    artifact_expires_at,artifact_width,artifact_height,artifact_profile_identity,
    created_at,settled_at,retain_until";

fn export_row(_connection: &Connection, row: &rusqlite::Row<'_>) -> rusqlite::Result<ExportRow> {
    let state_name: String = row.get(2)?;
    let state = ExportState::parse_name(&state_name).ok_or(rusqlite::Error::InvalidQuery)?;
    let source_size = match row.get::<_, Option<i64>>(8)? {
        None => None,
        Some(value) => Some(u64::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery)?),
    };
    let attempt_sequence: Option<u64> = match row.get::<_, Option<i64>>(14)? {
        None => None,
        Some(value) => Some(u64::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery)?),
    };
    let artifact_size = match row.get::<_, Option<i64>>(15)? {
        None => None,
        Some(value) => Some(u64::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery)?),
    };
    let artifact_width = match row.get::<_, Option<i64>>(18)? {
        None => None,
        Some(value) => u32::try_from(value)
            .map(Some)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
    };
    let artifact_height = match row.get::<_, Option<i64>>(19)? {
        None => None,
        Some(value) => u32::try_from(value)
            .map(Some)
            .map_err(|_| rusqlite::Error::InvalidQuery)?,
    };
    let created_at =
        u64::try_from(row.get::<_, i64>(21)?).map_err(|_| rusqlite::Error::InvalidQuery)?;
    let settled_at = match row.get::<_, Option<i64>>(22)? {
        None => None,
        Some(value) => Some(u64::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery)?),
    };
    let retain_until = match row.get::<_, Option<i64>>(23)? {
        None => None,
        Some(value) => Some(u64::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery)?),
    };
    Ok(ExportRow {
        id: row.get(0)?,
        photo_id: row.get(1)?,
        state,
        outcome: row.get(3)?,
        recipe_revision: row.get(4)?,
        exposure_ev: row.get(5)?,
        source_revision: row.get(6)?,
        source_profile_id: row.get(7)?,
        source_size,
        source_sha256: row.get(9)?,
        recipe_digest: row.get(10)?,
        policy_id: row.get(11)?,
        bundle_id: row.get(12)?,
        attempt_incarnation: row.get(13)?,
        attempt_sequence,
        artifact_size,
        artifact_sha256: row.get(16)?,
        artifact_expires_at: row
            .get::<_, Option<i64>>(17)?
            .map(|value| u64::try_from(value).map_err(|_| rusqlite::Error::InvalidQuery))
            .transpose()?,
        artifact_width,
        artifact_height,
        artifact_profile_identity: row.get(20)?,
        created_at,
        settled_at,
        retain_until,
    })
}

fn export_record_from_row(row: ExportRow) -> Result<ExportRecord, PersistenceError> {
    let settings = EditRecipeSettings {
        exposure_ev: row.exposure_ev,
        white_balance: WhiteBalanceIntent::AsShot,
    };
    let attempt = match (&row.attempt_incarnation, row.attempt_sequence) {
        (Some(incarnation), Some(sequence)) => Some(ExportAttempt {
            incarnation: incarnation.clone(),
            sequence,
        }),
        (None, None) => None,
        _ => return Err(PersistenceError::Storage),
    };
    let artifact = match (
        row.artifact_size,
        row.artifact_sha256,
        row.artifact_expires_at,
        row.artifact_width,
        row.artifact_height,
        &row.artifact_profile_identity,
    ) {
        (
            Some(size),
            Some(sha256),
            Some(expires_at),
            Some(width),
            Some(height),
            Some(profile_identity),
        ) => Some(ExportArtifactFacts {
            size,
            sha256,
            expires_at,
            width,
            height,
            profile_identity: profile_identity.clone(),
        }),
        (None, None, None, None, None, None) => None,
        // A partial artifact row can never be served as validated metadata.
        _ => return Err(PersistenceError::Storage),
    };
    let source = match (row.source_size, row.source_sha256) {
        (Some(size), Some(sha256)) => Some(ExportSourceEvidence { size, sha256 }),
        (None, None) => None,
        _ => return Err(PersistenceError::Storage),
    };
    let payload = ExportRecipePayload::capture(
        &settings,
        ExportExposureRange {
            minimum_milli_ev: i64::MIN,
            maximum_milli_ev: i64::MAX,
        },
    )
    .map_err(|_| PersistenceError::Storage)?;
    if payload.digest() != row.recipe_digest {
        return Err(PersistenceError::Storage);
    }
    Ok(ExportRecord {
        id: row.id,
        snapshot: ExportSnapshot {
            photo_id: row.photo_id,
            recipe_revision: row.recipe_revision,
            settings,
            source_revision: row.source_revision,
            source_kind: OriginalKind::Raw,
            source_profile_id: row.source_profile_id,
            policy_id: row.policy_id,
            bundle_id: row.bundle_id,
            workload: EXPORT_DEVELOPMENT_TIFF_WORKLOAD.to_owned(),
            recipe_digest: row.recipe_digest,
        },
        source,
        state: row.state,
        outcome: row.outcome,
        attempt,
        artifact,
        created_at: row.created_at,
        settled_at: row.settled_at,
        retain_until: row.retain_until,
    })
}

fn read_export_row(
    transaction: &Transaction<'_>,
    export_id: &str,
) -> Result<Option<ExportRecord>, PersistenceError> {
    let row = transaction
        .query_row(
            &format!("SELECT {EXPORT_ROW_COLUMNS} FROM exports WHERE id=?"),
            [export_id],
            |row| export_row(transaction, row),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    row.map(export_record_from_row).transpose()
}

/// Bytes of the finite retained-output allowance already committed. Unsettled
/// work reserves the complete bounded artifact; a published artifact counts by
/// its actual size until its disclosed expiry passes and its leases release.
fn reserved_retained_bytes(
    transaction: &Transaction<'_>,
    now: u64,
) -> Result<u64, PersistenceError> {
    let reserved: i64 = transaction
        .query_row(
            "SELECT COALESCE(SUM(reserved),0) FROM (
               SELECT CASE
                 WHEN state IN ('queued','running') THEN ?1
                 WHEN state = 'succeeded'
                      AND EXISTS(SELECT 1 FROM export_download_leases l WHERE l.export_id = exports.id)
                   THEN COALESCE(artifact_size, 0)
                 WHEN state = 'succeeded' AND artifact_expires_at > ?2
                   THEN COALESCE(artifact_size, 0)
                 ELSE 0
               END AS reserved
               FROM exports
             )",
            params![crate::MAXIMUM_EXPORT_BYTES as i64, now as i64],
            |row| row.get(0),
        )
        .map_err(|_| PersistenceError::Storage)?;
    u64::try_from(reserved).map_err(|_| PersistenceError::Storage)
}

fn reservable(
    transaction: &Transaction<'_>,
    now: u64,
    allowance: u64,
) -> Result<bool, PersistenceError> {
    let reserved = reserved_retained_bytes(transaction, now)?;
    let requested = reserved.saturating_add(crate::MAXIMUM_EXPORT_BYTES);
    Ok(requested <= allowance && requested >= crate::MAXIMUM_EXPORT_BYTES)
}

fn submit_export(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    submission: ExportSubmission,
) -> Result<ExportSubmitOutcome, PersistenceError> {
    if !validate_export_request_id(&submission.request_id)
        || submission.source_profile_id.is_empty()
    {
        return Ok(ExportSubmitOutcome::InvalidSettings);
    }
    let payload_digest = export_payload_digest(&submission)?;
    write_transaction(state, database_name, connection, |transaction| {
        if let Some(receipt) =
            read_export_receipt(transaction, &submission.photo_id, &submission.request_id)?
        {
            if receipt.payload_digest != payload_digest {
                return Ok(ExportSubmitOutcome::RequestConflict);
            }
            return Ok(match read_export_row(transaction, &receipt.export_id)? {
                Some(record) => ExportSubmitOutcome::Existing(record),
                // The export row is removed exactly when its retention window
                // passes, so a surviving receipt without a row is expired and
                // can never start new work.
                None => ExportSubmitOutcome::Expired,
            });
        }
        let Some(current) = read_edit_recipe(transaction, &submission.photo_id)? else {
            return Ok(ExportSubmitOutcome::UnknownPhoto);
        };
        let Some((kind, available)) = photo_processing_source(transaction, &submission.photo_id)?
        else {
            return Ok(ExportSubmitOutcome::UnknownPhoto);
        };
        if kind != OriginalKind::Raw {
            return Ok(ExportSubmitOutcome::UnsupportedPhoto);
        }
        if !available || !current.source_available {
            return Ok(ExportSubmitOutcome::Unavailable);
        }
        let Some(recipe) = current.recipe.as_ref() else {
            return Ok(ExportSubmitOutcome::MissingRecipe);
        };
        if current.current_source_revision != submission.expected_source_revision {
            return Ok(ExportSubmitOutcome::SourceChanged(current));
        }
        if recipe.source_revision != submission.expected_source_revision {
            // The stored recipe is bound to a source other than the current
            // published revision; an Export must never execute a payload
            // captured against the old binding. A stale binding outranks a
            // stale expected recipe revision.
            return Ok(ExportSubmitOutcome::RequiresRebind);
        }
        if recipe.revision != submission.expected_recipe_revision {
            return Ok(ExportSubmitOutcome::RecipeConflict(current));
        }
        // A saved recipe outside the approved range is invalid input for the
        // Export, not a storage failure.
        let payload =
            match ExportRecipePayload::capture(&recipe.settings, submission.exposure_range) {
                Ok(payload) => payload,
                Err(_) => return Ok(ExportSubmitOutcome::InvalidSettings),
            };
        let now = export_unix_seconds();
        if !reservable(transaction, now, submission.retained_output_bytes_max)? {
            return Ok(ExportSubmitOutcome::RetainedOutputFull);
        }
        let export_id = format!("exp-{}", random_uuid_v4()?);
        transaction
            .execute(
                "INSERT INTO exports(id,photo_id,target,state,outcome,recipe_revision,
                   exposure_ev,white_balance_mode,source_revision,source_profile_id,
                   source_kind,source_size,source_sha256,recipe_digest,policy_id,bundle_id,
                   workload,created_at)
                 VALUES(?1,?2,'development-tiff','queued',NULL,?3,?4,'as-shot',?5,?6,'raw',
                   NULL,NULL,?7,?8,?9,'development-tiff',?10)",
                params![
                    export_id,
                    submission.photo_id,
                    recipe.revision,
                    recipe.settings.exposure_ev,
                    submission.expected_source_revision,
                    submission.source_profile_id,
                    payload.digest(),
                    submission.policy_id,
                    submission.bundle_id,
                    now as i64,
                ],
            )
            .map_err(|_| PersistenceError::Storage)?;
        write_export_receipt(
            transaction,
            &submission.photo_id,
            &submission.request_id,
            &ExportReceipt {
                payload_digest,
                export_id: export_id.clone(),
                created_at: now,
                settled_at: None,
            },
        )?;
        let record = read_export_row(transaction, &export_id)?.ok_or(PersistenceError::Storage)?;
        Ok(ExportSubmitOutcome::Created(record))
    })
}

fn export_record(
    connection: &Connection,
    export_id: &str,
) -> Result<Option<ExportRecord>, PersistenceError> {
    let row = connection
        .query_row(
            &format!("SELECT {EXPORT_ROW_COLUMNS} FROM exports WHERE id=?"),
            [export_id],
            |row| export_row(connection, row),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    row.map(export_record_from_row).transpose()
}

/// Resolves a request identity from its receipt without any state change.
/// `None` means the identity was never recorded and submission may proceed.
fn resolve_export_submission(
    connection: &Connection,
    photo_id: &str,
    request_id: &str,
    payload_digest: &str,
) -> Option<ExportSubmissionResolution> {
    let value = connection
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [export_receipt_key(photo_id, request_id)],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()?;
    let receipt: ExportReceipt = serde_json::from_str(&value).ok()?;
    if receipt.payload_digest != payload_digest {
        return Some(ExportSubmissionResolution::Conflict);
    }
    match export_record(connection, &receipt.export_id) {
        Ok(Some(record)) => Some(ExportSubmissionResolution::Existing(Box::new(record))),
        // The export row is removed exactly when its retention window
        // passes, so a surviving receipt without a row is expired.
        Ok(None) => Some(ExportSubmissionResolution::Expired),
        Err(_) => None,
    }
}

/// Refreshes one download lease's liveness anchor. `false` means the lease
/// is gone and the stream must stop renewing.
fn renew_export_lease(connection: &mut Connection, lease_id: &str, now: u64) -> bool {
    connection
        .execute(
            "UPDATE export_download_leases SET created_at=?1 WHERE id=?2",
            params![now as i64, lease_id],
        )
        .map(|updated| updated > 0)
        .unwrap_or(false)
}

fn list_photo_exports(
    connection: &Connection,
    photo_id: &str,
) -> Result<Option<Vec<ExportRecord>>, PersistenceError> {
    let known = connection
        .query_row("SELECT 1 FROM photos WHERE id=?", [photo_id], |row| {
            row.get::<_, i64>(0)
        })
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    if known.is_none() {
        return Ok(None);
    }
    let rows = connection
        .prepare(&format!(
            "SELECT {EXPORT_ROW_COLUMNS} FROM exports WHERE photo_id=?
             ORDER BY created_at DESC, id DESC LIMIT {EXPORT_LIST_LIMIT}"
        ))
        .map_err(|_| PersistenceError::Storage)?
        .query_map([photo_id], |row| export_row(connection, row))
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    let records = rows
        .into_iter()
        .map(export_record_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(records))
}

/// Records the verified staged source bytes between acceptance and launch.
/// Only unfinished work accepts them, so a settled Export can never grow
/// source evidence after the fact.
fn record_export_source(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    export_id: &str,
    size: u64,
    sha256: &str,
) -> Result<Option<ExportRecord>, PersistenceError> {
    if size == 0 || size > crate::MAXIMUM_EXPORT_BYTES || sha256.len() != 64 {
        return Err(PersistenceError::Storage);
    }
    write_transaction(state, database_name, connection, |transaction| {
        let Some(current) = read_export_row(transaction, export_id)? else {
            return Ok(None);
        };
        if current.state.is_terminal() {
            return Ok(Some(current));
        }
        transaction
            .execute(
                "UPDATE exports SET source_size=?,source_sha256=? WHERE id=?",
                params![size as i64, sha256, export_id],
            )
            .map_err(|_| PersistenceError::Storage)?;
        read_export_row(transaction, export_id)?.map(Ok).transpose()
    })
}

fn settle_export(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    export_id: &str,
    settlement: ExportSettlement,
) -> Result<Option<ExportRecord>, PersistenceError> {
    let outcome_is_bounded = match &settlement {
        ExportSettlement::Succeeded { .. } => true,
        ExportSettlement::Failed { outcome, .. } => {
            !outcome.is_empty() && outcome.len() <= MAXIMUM_EXPORT_OUTCOME_BYTES
        }
    };
    if !outcome_is_bounded {
        return Err(PersistenceError::Storage);
    }
    write_transaction(state, database_name, connection, |transaction| {
        let Some(current) = read_export_row(transaction, export_id)? else {
            return Ok(None);
        };
        // Exactly-once settlement: a racing cancel or completion already
        // decided the terminal state and is never rewritten here.
        if current.state.is_terminal() {
            return Ok(Some(current));
        }
        match settlement {
            ExportSettlement::Succeeded {
                artifact_size,
                artifact_sha256,
                published_at,
                artifact_width,
                artifact_height,
                artifact_profile_identity,
            } => {
                let expiry = published_at.saturating_add(EXPORT_RETENTION_SECONDS);
                transaction
                    .execute(
                        "UPDATE exports SET state='succeeded',outcome=NULL,
                           artifact_size=?,artifact_sha256=?,artifact_expires_at=?,
                           artifact_width=?,artifact_height=?,artifact_profile_identity=?,
                           settled_at=?,retain_until=? WHERE id=?",
                        params![
                            artifact_size as i64,
                            artifact_sha256,
                            expiry as i64,
                            artifact_width as i64,
                            artifact_height as i64,
                            artifact_profile_identity,
                            published_at as i64,
                            expiry as i64,
                            export_id
                        ],
                    )
                    .map_err(|_| PersistenceError::Storage)?;
            }
            ExportSettlement::Failed {
                outcome,
                settled_at,
            } => {
                let retain = settled_at.saturating_add(EXPORT_RETENTION_SECONDS);
                transaction
                    .execute(
                        "UPDATE exports SET state='failed',outcome=?,artifact_size=NULL,
                           artifact_sha256=NULL,artifact_expires_at=NULL,settled_at=?,
                           retain_until=? WHERE id=?",
                        params![outcome, settled_at as i64, retain as i64, export_id],
                    )
                    .map_err(|_| PersistenceError::Storage)?;
            }
        }
        read_export_row(transaction, export_id)?.map(Ok).transpose()
    })
}

fn cancel_export(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    export_id: &str,
) -> Result<Option<ExportRecord>, PersistenceError> {
    write_transaction(state, database_name, connection, |transaction| {
        let Some(current) = read_export_row(transaction, export_id)? else {
            return Ok(None);
        };
        // Cancellation settles exactly once against the actual completion
        // state and never rewrites or undoes a published artifact.
        if current.state.is_terminal() {
            return Ok(Some(current));
        }
        let now = export_unix_seconds();
        transaction
            .execute(
                "UPDATE exports SET state='cancelled',artifact_size=NULL,
                   artifact_sha256=NULL,artifact_expires_at=NULL,settled_at=?,retain_until=?
                 WHERE id=?",
                params![
                    now as i64,
                    now.saturating_add(EXPORT_RETENTION_SECONDS) as i64,
                    export_id
                ],
            )
            .map_err(|_| PersistenceError::Storage)?;
        read_export_row(transaction, export_id)?.map(Ok).transpose()
    })
}

/// Persists the launcher attempt identity and marks the attempt running. A
/// terminal record is returned untouched so a caller that lost a race with
/// cancellation aborts before any work.
fn begin_export_attempt(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    export_id: &str,
    attempt: ExportAttempt,
) -> Result<Option<ExportRecord>, PersistenceError> {
    // The launcher-owned incarnation is 32 lowercase hex characters, exactly
    // as the production Photo protocol validates it.
    if attempt.sequence == 0
        || attempt.incarnation.len() != 32
        || !attempt
            .incarnation
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(PersistenceError::Storage);
    }
    write_transaction(state, database_name, connection, |transaction| {
        let Some(current) = read_export_row(transaction, export_id)? else {
            return Ok(None);
        };
        if current.state.is_terminal() {
            return Ok(Some(current));
        }
        transaction
            .execute(
                "UPDATE exports SET state='running',attempt_incarnation=?,attempt_sequence=?
                 WHERE id=?",
                params![attempt.incarnation, attempt.sequence as i64, export_id],
            )
            .map_err(|_| PersistenceError::Storage)?;
        read_export_row(transaction, export_id)?.map(Ok).transpose()
    })
}

#[allow(clippy::too_many_arguments)]
fn retry_export(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    export_id: &str,
    request_id: &str,
    expected_bundle_id: &str,
    allowance: u64,
) -> Result<ExportRetryOutcome, PersistenceError> {
    if !validate_export_request_id(request_id) {
        return Err(PersistenceError::Storage);
    }
    let retry_digest = format!(
        "{:x}",
        Sha256::digest(format!("retry\0{export_id}").as_bytes())
    );
    write_transaction(state, database_name, connection, |transaction| {
        let Some(current) = read_export_row(transaction, export_id)? else {
            // A reclaimed record keeps its expired identity: retry reports
            // the explicit expired outcome instead of unknown.
            if read_export_expiry_tombstone(transaction, export_id)? {
                return Ok(ExportRetryOutcome::Expired);
            }
            return Ok(ExportRetryOutcome::Unknown);
        };
        // An accepted retry identity resolves to its Export and starts no
        // work; a different payload under that identity is a conflict.
        if let Some(receipt) =
            read_export_receipt(transaction, &current.snapshot.photo_id, request_id)?
        {
            if receipt.export_id == export_id && receipt.payload_digest == retry_digest {
                return Ok(ExportRetryOutcome::Replayed(Box::new(current)));
            }
            return Ok(ExportRetryOutcome::RequestConflict);
        }
        // Only a settled failure or cancellation carries a retryable
        // snapshot; a succeeded artifact is never silently re-rendered and
        // an active attempt is never replaced.
        if !matches!(current.state, ExportState::Failed | ExportState::Cancelled) {
            return Ok(ExportRetryOutcome::NotRetriable);
        }
        let now = export_unix_seconds();
        let Some(retain_until) = current.retain_until else {
            return Ok(ExportRetryOutcome::Unknown);
        };
        if retain_until <= now {
            return Ok(ExportRetryOutcome::Expired);
        }
        // Availability is validated again against the retained snapshot: a
        // swapped approved bundle or a changed/unreadable source means the
        // captured work can never execute again.
        if current.snapshot.bundle_id != expected_bundle_id {
            return Ok(ExportRetryOutcome::OutputUnavailable);
        }
        let Some(recipe) = read_edit_recipe(transaction, &current.snapshot.photo_id)? else {
            return Ok(ExportRetryOutcome::OutputUnavailable);
        };
        if !recipe.source_available {
            return Ok(ExportRetryOutcome::ResourceUnavailable);
        }
        if recipe.current_source_revision != current.snapshot.source_revision {
            return Ok(ExportRetryOutcome::OutputUnavailable);
        }
        if !reservable(transaction, now, allowance)? {
            return Ok(ExportRetryOutcome::RetainedOutputFull);
        }
        transaction
            .execute(
                "UPDATE exports SET state='queued',outcome=NULL,attempt_incarnation=NULL,
                   attempt_sequence=NULL WHERE id=?",
                [export_id],
            )
            .map_err(|_| PersistenceError::Storage)?;
        write_export_receipt(
            transaction,
            &current.snapshot.photo_id,
            request_id,
            &ExportReceipt {
                payload_digest: retry_digest,
                export_id: export_id.to_owned(),
                created_at: now,
                settled_at: None,
            },
        )?;
        Ok(read_export_row(transaction, export_id)?
            .map(|record| ExportRetryOutcome::Retried(Box::new(record)))
            .unwrap_or(ExportRetryOutcome::Unknown))
    })
}

fn sweep_export_expiry(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    now: u64,
) -> Result<ExportSweepResult, PersistenceError> {
    write_transaction(state, database_name, connection, |transaction| {
        transaction
            .execute(
                "DELETE FROM export_download_leases WHERE created_at < ?1",
                [now.saturating_sub(EXPORT_LEASE_STALE_SECONDS) as i64],
            )
            .map_err(|_| PersistenceError::Storage)?;
        let mut result = ExportSweepResult::default();
        let expired_artifacts = transaction
            .prepare(
                "SELECT id FROM exports WHERE state='succeeded'
                 AND artifact_expires_at IS NOT NULL AND artifact_expires_at <= ?1
                 AND NOT EXISTS(SELECT 1 FROM export_download_leases l WHERE l.export_id=exports.id)",
            )
            .map_err(|_| PersistenceError::Storage)?
            .query_map([now as i64], |row| row.get::<_, String>(0))
            .map_err(|_| PersistenceError::Storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| PersistenceError::Storage)?;
        for export_id in expired_artifacts {
            transaction
                .execute(
                    "UPDATE exports SET artifact_size=NULL,artifact_sha256=NULL,
                       artifact_expires_at=NULL WHERE id=?",
                    [&export_id],
                )
                .map_err(|_| PersistenceError::Storage)?;
            result.artifact_expiry_ids.push(export_id);
        }
        let expired_records = transaction
            .prepare(
                "SELECT id FROM exports WHERE retain_until IS NOT NULL AND retain_until <= ?1
                 AND NOT EXISTS(SELECT 1 FROM export_download_leases l WHERE l.export_id=exports.id)",
            )
            .map_err(|_| PersistenceError::Storage)?
            .query_map([now as i64], |row| row.get::<_, String>(0))
            .map_err(|_| PersistenceError::Storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| PersistenceError::Storage)?;
        for export_id in expired_records {
            transaction
                .execute("DELETE FROM exports WHERE id=?", [&export_id])
                .map_err(|_| PersistenceError::Storage)?;
            // The identity stays expired forever: it can never start new
            // work and retry keeps reporting the explicit expired outcome.
            transaction
                .execute(
                    "INSERT OR REPLACE INTO library_metadata(key,value) VALUES(?1,?2)",
                    params![export_expiry_tombstone_key(&export_id), "expired"],
                )
                .map_err(|_| PersistenceError::Storage)?;
            result.record_expiry_ids.push(export_id);
        }
        Ok(result)
    })
}

fn unfinished_exports(connection: &Connection) -> Result<Vec<ExportRecord>, PersistenceError> {
    let rows = connection
        .prepare(&format!(
            "SELECT {EXPORT_ROW_COLUMNS} FROM exports
             WHERE state IN ('queued','running') ORDER BY created_at, id"
        ))
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], |row| export_row(connection, row))
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    rows.into_iter().map(export_record_from_row).collect()
}

fn acquire_export_lease(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    export_id: &str,
    now: u64,
) -> Result<ExportLeaseOutcome, PersistenceError> {
    write_transaction(state, database_name, connection, |transaction| {
        let Some(current) = read_export_row(transaction, export_id)? else {
            return Ok(ExportLeaseOutcome::Unknown);
        };
        let Some(artifact) = current.artifact.as_ref() else {
            return Ok(ExportLeaseOutcome::Unknown);
        };
        if artifact.expires_at <= now {
            return Ok(ExportLeaseOutcome::Expired);
        }
        let lease_id = format!("lease-{}", random_uuid_v4()?);
        transaction
            .execute(
                "INSERT INTO export_download_leases(id,export_id,created_at) VALUES(?,?,?)",
                params![lease_id, export_id, now as i64],
            )
            .map_err(|_| PersistenceError::Storage)?;
        Ok(ExportLeaseOutcome::Acquired {
            lease_id,
            artifact: artifact.clone(),
        })
    })
}

fn release_export_lease(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    lease_id: &str,
) -> Result<bool, PersistenceError> {
    write_transaction(state, database_name, connection, |transaction| {
        let changed = transaction
            .execute("DELETE FROM export_download_leases WHERE id=?", [lease_id])
            .map_err(|_| PersistenceError::Storage)?;
        Ok(changed == 1)
    })
}

fn photo_processing_source(
    connection: &Connection,
    photo_id: &str,
) -> Result<Option<(crate::OriginalKind, bool)>, PersistenceError> {
    connection
        .query_row(
            "SELECT o.kind,o.available,p.available FROM photos p
             JOIN original_files o ON o.id=p.original_id WHERE p.id=?",
            [photo_id],
            |row| {
                Ok((
                    parse_kind(&row.get::<_, String>(0)?)?,
                    row.get::<_, i64>(1)? != 0 && row.get::<_, i64>(2)? != 0,
                ))
            },
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)
}

fn white_balance_intent_name(intent: WhiteBalanceIntent) -> &'static str {
    intent.mode_name()
}

fn white_balance_intent_values(intent: WhiteBalanceIntent) -> (Option<i32>, Option<i32>) {
    match intent {
        WhiteBalanceIntent::AsShot => (None, None),
        WhiteBalanceIntent::TemperatureTint {
            temperature_kelvin,
            tint_milli,
        } => (Some(temperature_kelvin), Some(tint_milli)),
    }
}

fn parse_white_balance_intent(
    mode: &str,
    temperature_kelvin: Option<i32>,
    tint_milli: Option<i32>,
) -> rusqlite::Result<WhiteBalanceIntent> {
    let intent = match (mode, temperature_kelvin, tint_milli) {
        ("as-shot", None, None) => WhiteBalanceIntent::AsShot,
        ("temperature-tint", Some(temperature_kelvin), Some(tint_milli)) => {
            WhiteBalanceIntent::TemperatureTint {
                temperature_kelvin,
                tint_milli,
            }
        }
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    if intent.within_payload_bounds() {
        Ok(intent)
    } else {
        Err(rusqlite::Error::InvalidQuery)
    }
}

fn random_uuid_v4() -> Result<String, PersistenceError> {
    let mut bytes = [0_u8; 16];
    let mut offset = 0;
    while offset < bytes.len() {
        // SAFETY: the buffer is valid writable storage and the length is exact.
        let result = unsafe {
            libc::getrandom(bytes[offset..].as_mut_ptr().cast(), bytes.len() - offset, 0)
        };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(PersistenceError::Storage);
        }
        if result == 0 {
            return Err(PersistenceError::Storage);
        }
        offset += usize::try_from(result).map_err(|_| PersistenceError::Storage)?;
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    ))
}

fn allocate_library_id(
    transaction: &Transaction<'_>,
    reserved: &mut HashSet<String>,
) -> Result<String, PersistenceError> {
    let id = random_uuid_v4()?;
    reserve_library_id(transaction, reserved, id)
}

fn reserve_library_id(
    transaction: &Transaction<'_>,
    reserved: &mut HashSet<String>,
    id: String,
) -> Result<String, PersistenceError> {
    let collision = reserved.contains(&id)
        || transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM original_files WHERE id=?) OR EXISTS(SELECT 1 FROM photos WHERE id=?)",
                params![id, id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(|_| PersistenceError::Storage)?;
    if collision {
        return Err(PersistenceError::IdCollision);
    }
    reserved.insert(id.clone());
    Ok(id)
}

fn uuid_v4() -> Result<String, MutationError> {
    random_uuid_v4().map_err(|_| MutationError::Persistence)
}

fn unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

/// The key the Library keeps its removal-marker high water mark under. A
/// Library that predates the mark falls back to the greatest marker it still
/// holds, so the first marker written after an upgrade cannot repeat one.
const REMOVAL_MARKER_HIGH_WATER: &str = "removal_marker_high_water";

/// The next removal marker: the clock reading, and strictly greater than every
/// marker this Library assigned before.
///
/// A restore is a compare-and-set against the marker it read, so two removals
/// of one Photo must never carry the same marker — a clock reading alone can
/// repeat when a Photo is restored and removed again inside one millisecond,
/// which would let a stale listing clear the newer removal. The high water
/// mark is durable, so it also holds across restarts and rescan deletions that
/// leave no removed row behind to read a maximum from.
fn next_removal_marker(transaction: &Transaction<'_>) -> Result<i64, MutationError> {
    let stored: Option<String> = transaction
        .query_row(
            "SELECT value FROM library_metadata WHERE key=?",
            [REMOVAL_MARKER_HIGH_WATER],
            |row| row.get(0),
        )
        .optional()
        .map_err(mutation_error_from_sqlite)?;
    let high_water = match stored.and_then(|value| value.parse::<i64>().ok()) {
        Some(value) => value,
        // A Library written before this high water mark existed has no row to
        // read: the markers it still holds are the floor, so the next marker
        // cannot repeat one of them either.
        None => transaction
            .query_row("SELECT max(removed_at_ms) FROM photos", [], |row| {
                row.get::<_, Option<i64>>(0)
            })
            .map_err(mutation_error_from_sqlite)?
            .unwrap_or(0),
    };
    let marker = unix_millis().max(high_water.saturating_add(1));
    transaction
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES(?,?)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![REMOVAL_MARKER_HIGH_WATER, marker.to_string()],
        )
        .map_err(mutation_error_from_sqlite)?;
    Ok(marker)
}

fn list_albums(connection: &Connection) -> Result<Vec<AlbumRecord>, PersistenceError> {
    let albums = connection
        .prepare("SELECT id,name FROM albums ORDER BY created_at,id")
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    let mut result = Vec::with_capacity(albums.len());
    for (id, name) in albums {
        let members = connection
            .prepare(
                "SELECT m.photo_id,m.position,p.available,p.selection_state,p.rating
                 FROM album_members m JOIN photos p ON p.id=m.photo_id
                 WHERE m.album_id=? AND p.removed_at_ms IS NULL ORDER BY m.position",
            )
            .map_err(|_| PersistenceError::Storage)?
            .query_map([id.as_str()], |row| {
                Ok(AlbumMember {
                    photo_id: row.get(0)?,
                    position: row
                        .get::<_, i64>(1)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    available: row.get::<_, i64>(2)? != 0,
                    selection_state: parse_selection_state(&row.get::<_, String>(3)?)?,
                    rating: row
                        .get::<_, i64>(4)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                })
            })
            .map_err(|_| PersistenceError::Storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| PersistenceError::Storage)?;
        let last_reviewed_photo_id = connection
            .query_row(
                "SELECT photo_id FROM album_progress WHERE album_id=?",
                [id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| PersistenceError::Storage)?;
        result.push(AlbumRecord {
            id,
            name,
            last_reviewed_photo_id,
            members,
        });
    }
    Ok(result)
}

fn list_album_summaries(
    connection: &Connection,
    versions: &MutationVersions,
) -> Result<Vec<AlbumSummary>, PersistenceError> {
    let rows = connection
        .prepare(
            "SELECT a.id, a.name,
                    (SELECT count(*) FROM album_members m
                       JOIN photos p ON p.id = m.photo_id
                      WHERE m.album_id = a.id AND p.removed_at_ms IS NULL),
                    EXISTS(SELECT 1 FROM album_progress p WHERE p.album_id = a.id)
             FROM albums a ORDER BY a.created_at, a.id",
        )
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    rows.into_iter()
        .map(|(id, name, photo_count, has_saved_position)| {
            Ok(AlbumSummary {
                album_version: versions.album(&id),
                id,
                name,
                photo_count: photo_count
                    .try_into()
                    .map_err(|_| PersistenceError::Storage)?,
                has_saved_position: has_saved_position != 0,
            })
        })
        .collect()
}

fn read_album(
    connection: &Connection,
    versions: &MutationVersions,
    album_id: &str,
) -> Result<Option<AlbumSummary>, PersistenceError> {
    let row = connection
        .query_row(
            "SELECT a.id, a.name,
                    (SELECT count(*) FROM album_members m WHERE m.album_id = a.id),
                    EXISTS(SELECT 1 FROM album_progress p WHERE p.album_id = a.id)
             FROM albums a WHERE a.id=?",
            [album_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    row.map(|(id, name, photo_count, has_saved_position)| {
        Ok(AlbumSummary {
            album_version: versions.album(&id),
            id,
            name,
            photo_count: photo_count
                .try_into()
                .map_err(|_| PersistenceError::Storage)?,
            has_saved_position: has_saved_position != 0,
        })
    })
    .transpose()
}

fn create_album_query(
    connection: &Connection,
    filter: AlbumQueryFilter,
    maximum_results: usize,
) -> Result<Vec<String>, PhotoQueryError> {
    if maximum_results == 0 || maximum_results == usize::MAX {
        return Err(PhotoQueryError::Invalid);
    }
    let sql_limit = i64::try_from(maximum_results + 1).map_err(|_| PhotoQueryError::Invalid)?;
    let (sql, parameters): (&str, Vec<Value>) = match filter {
        AlbumQueryFilter::All => (
            "SELECT a.id FROM albums a ORDER BY a.created_at,a.id LIMIT ?",
            vec![sql_limit.into()],
        ),
        AlbumQueryFilter::ExactName(name) if !name.is_empty() => (
            "SELECT a.id FROM albums a WHERE a.name=? COLLATE NOCASE ORDER BY a.created_at,a.id LIMIT ?",
            vec![name.into(), sql_limit.into()],
        ),
        AlbumQueryFilter::ContainsPhoto(photo_id) if !photo_id.is_empty() => (
            "SELECT a.id FROM albums a JOIN album_members m ON m.album_id=a.id WHERE m.photo_id=? ORDER BY a.created_at,a.id LIMIT ?",
            vec![photo_id.into(), sql_limit.into()],
        ),
        _ => return Err(PhotoQueryError::Invalid),
    };
    let mut statement = connection
        .prepare(sql)
        .map_err(|_| PhotoQueryError::Storage)?;
    let ids = statement
        .query_map(params_from_iter(parameters), |row| row.get::<_, String>(0))
        .map_err(|_| PhotoQueryError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PhotoQueryError::Storage)?;
    if ids.len() > maximum_results {
        Err(PhotoQueryError::ResultLimitExceeded {
            limit: maximum_results,
        })
    } else {
        Ok(ids)
    }
}

fn read_photo(
    connection: &Connection,
    versions: &MutationVersions,
    photo_id: &str,
) -> Result<Option<PhotoRead>, PersistenceError> {
    let row = connection
        .query_row(
            "SELECT p.id,o.relative_path,o.kind,o.available,p.selection_state,p.rating,
                    o.capture_metadata_state,o.capture_order_key,o.capture_time_field,
                    o.capture_offset_minutes,o.capture_source_revision,p.preview_state,
                    p.preview_source_revision,p.preview_width,p.preview_height,
                    EXISTS(SELECT 1 FROM edit_recipes e WHERE e.photo_id=p.id)
             FROM photos p JOIN original_files o ON o.id=p.original_id WHERE p.id=?",
            [photo_id],
            |row| {
                let path = row.get::<_, String>(1)?;
                let kind = parse_kind(&row.get::<_, String>(2)?)?;
                let preview_state = parse_preview_state(&row.get::<_, String>(11)?)?;
                let preview_source_revision: Option<String> = row.get(12)?;
                let preview_width = parse_dimension(row.get(13)?)?;
                let preview_height = parse_dimension(row.get(14)?)?;
                let ready = preview_state == PreviewState::Ready;
                Ok(PhotoRead {
                    decision_version: versions.photo(photo_id),
                    id: row.get(0)?,
                    filename: path.rsplit('/').next().unwrap_or(&path).to_owned(),
                    original_kind: kind,
                    original_available: row.get::<_, i64>(3)? != 0,
                    selection_state: parse_selection_state(&row.get::<_, String>(4)?)?,
                    rating: row
                        .get::<_, i64>(5)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    capture: parse_capture_fact(
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                    )?,
                    preview_state,
                    preview_source: ready.then(|| kind.preview_source()),
                    preview_source_revision: ready.then_some(preview_source_revision).flatten(),
                    preview_width: ready.then_some(preview_width).flatten(),
                    preview_height: ready.then_some(preview_height).flatten(),
                    has_saved_edits: row.get::<_, i64>(15)? != 0,
                })
            },
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    Ok(row)
}

fn read_projected_photo(
    connection: &Connection,
    versions: &MutationVersions,
    projection: &PhotoQueryProjection,
    photo_id: &str,
) -> Result<Option<PhotoRead>, PersistenceError> {
    let Some(candidate) = projection.get(photo_id) else {
        return Ok(None);
    };
    let decisions = connection
        .query_row(
            "SELECT p.selection_state,p.rating,
                    EXISTS(SELECT 1 FROM edit_recipes e WHERE e.photo_id=p.id)
             FROM photos p WHERE p.id=?",
            [photo_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)? != 0,
                ))
            },
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    let Some((selection_state, rating, has_saved_edits)) = decisions else {
        return Ok(None);
    };
    let selection_state =
        parse_selection_state(&selection_state).map_err(|_| PersistenceError::Storage)?;
    let rating = rating.try_into().map_err(|_| PersistenceError::Storage)?;
    let ready = candidate.preview_state == PreviewState::Ready;
    Ok(Some(PhotoRead {
        id: candidate.photo_id.clone(),
        filename: candidate
            .relative_path
            .rsplit('/')
            .next()
            .unwrap_or(&candidate.relative_path)
            .to_owned(),
        original_kind: candidate.original_kind,
        original_available: candidate.original_available,
        selection_state,
        rating,
        decision_version: versions.photo(photo_id),
        capture: candidate.capture.clone(),
        preview_state: candidate.preview_state,
        preview_source: ready.then(|| candidate.original_kind.preview_source()),
        preview_source_revision: ready
            .then(|| candidate.preview_source_revision.clone())
            .flatten(),
        preview_width: ready.then_some(candidate.preview_width).flatten(),
        preview_height: ready.then_some(candidate.preview_height).flatten(),
        has_saved_edits,
    }))
}

fn valid_folder_location(location: &str) -> bool {
    !location.starts_with('/')
        && !location.ends_with('/')
        && !location.contains('\0')
        && !location
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
}

fn candidate_in_folder(candidate: &PhotoQueryCandidate, location: &str) -> bool {
    location.is_empty()
        || candidate
            .relative_path
            .strip_prefix(location)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn candidate_matches(
    query: &PhotoQuery,
    candidate: &PhotoQueryCandidate,
    selection_state: SelectionState,
    rating: u8,
) -> bool {
    query
        .selection_state
        .is_none_or(|expected| selection_state == expected)
        && query.rating_minimum.is_none_or(|minimum| rating >= minimum)
        && query.rating_maximum.is_none_or(|maximum| rating <= maximum)
        && query
            .original_kind
            .is_none_or(|kind| candidate.original_kind == kind)
        && query
            .original_available
            .is_none_or(|available| candidate.original_available == available)
        && query.captured_from.as_ref().is_none_or(|from| {
            candidate
                .capture_order_key()
                .is_some_and(|value| value >= from.as_str())
        })
        && query.captured_before.as_ref().is_none_or(|before| {
            candidate
                .capture_order_key()
                .is_some_and(|value| value < before.as_str())
        })
}

fn push_query_match(
    ids: &mut Vec<String>,
    photo_id: &str,
    maximum_results: usize,
) -> Result<(), PhotoQueryError> {
    ids.push(photo_id.to_owned());
    if ids.len() > maximum_results {
        Err(PhotoQueryError::ResultLimitExceeded {
            limit: maximum_results,
        })
    } else {
        Ok(())
    }
}

fn create_photo_query(
    connection: &Connection,
    query: PhotoQuery,
    projection: &PhotoQueryProjection,
    maximum_results: usize,
) -> Result<Vec<String>, PhotoQueryError> {
    if maximum_results == 0
        || maximum_results == usize::MAX
        || query.rating_minimum.is_some_and(|value| value > 5)
        || query.rating_maximum.is_some_and(|value| value > 5)
        || query
            .rating_minimum
            .zip(query.rating_maximum)
            .is_some_and(|(minimum, maximum)| minimum > maximum)
        || query
            .captured_from
            .as_ref()
            .zip(query.captured_before.as_ref())
            .is_some_and(|(from, before)| from.as_str() >= before.as_str())
        || (query.order == PhotoQueryOrder::AlbumOrder
            && !matches!(query.source, PhotoQuerySource::Album(_)))
    {
        return Err(PhotoQueryError::Invalid);
    }

    if let PhotoQuerySource::Folder(location) = &query.source {
        if !location.is_empty() && !valid_folder_location(location) {
            return Err(PhotoQueryError::Invalid);
        }
        if !location.is_empty()
            && !projection
                .ascending()
                .iter()
                .any(|candidate| candidate_in_folder(candidate, location))
        {
            return Err(PhotoQueryError::SourceNotFound);
        }
    }

    let album_id = match &query.source {
        PhotoQuerySource::Album(album_id) if !album_id.is_empty() => {
            let exists = connection
                .query_row("SELECT 1 FROM albums WHERE id=?", [album_id], |_| Ok(()))
                .optional()
                .map_err(|_| PhotoQueryError::Storage)?;
            if exists.is_none() {
                return Err(PhotoQueryError::SourceNotFound);
            }
            Some(album_id.as_str())
        }
        PhotoQuerySource::Album(_) => return Err(PhotoQueryError::Invalid),
        _ => None,
    };

    let mut ids = Vec::with_capacity(maximum_results.min(64).saturating_add(1));
    if query.order == PhotoQueryOrder::AlbumOrder {
        let album_id = album_id.expect("Album order was validated with an Album source");
        let mut statement = connection
            .prepare(
                "SELECT m.photo_id,p.selection_state,p.rating
                 FROM album_members m JOIN photos p ON p.id=m.photo_id
                 WHERE m.album_id=? AND p.removed_at_ms IS NULL ORDER BY m.position",
            )
            .map_err(|_| PhotoQueryError::Storage)?;
        let mut rows = statement
            .query([album_id])
            .map_err(|_| PhotoQueryError::Storage)?;
        while let Some(row) = rows.next().map_err(|_| PhotoQueryError::Storage)? {
            let photo_id = row
                .get::<_, String>(0)
                .map_err(|_| PhotoQueryError::Storage)?;
            let Some(candidate) = projection.get(&photo_id) else {
                continue;
            };
            let selection = parse_selection_state(
                &row.get::<_, String>(1)
                    .map_err(|_| PhotoQueryError::Storage)?,
            )
            .map_err(|_| PhotoQueryError::Storage)?;
            let rating = row
                .get::<_, i64>(2)
                .ok()
                .and_then(|value| value.try_into().ok())
                .ok_or(PhotoQueryError::Storage)?;
            if candidate_matches(&query, candidate, selection, rating) {
                push_query_match(&mut ids, &photo_id, maximum_results)?;
            }
        }
        return Ok(ids);
    }

    let mut current = connection
        .prepare(if album_id.is_some() {
            "SELECT p.selection_state,p.rating FROM photos p
             JOIN album_members m ON m.photo_id=p.id
             WHERE p.id=?1 AND m.album_id=?2 AND p.removed_at_ms IS NULL"
        } else {
            "SELECT selection_state,rating FROM photos WHERE id=?1 AND removed_at_ms IS NULL"
        })
        .map_err(|_| PhotoQueryError::Storage)?;
    let candidates: Box<dyn Iterator<Item = &PhotoQueryCandidate>> = match query.order {
        PhotoQueryOrder::CaptureTimeAscending => Box::new(projection.ascending().iter()),
        PhotoQueryOrder::CaptureTimeDescending => Box::new(projection.descending()),
        PhotoQueryOrder::AlbumOrder => unreachable!(),
    };
    for candidate in candidates {
        if let PhotoQuerySource::Folder(location) = &query.source
            && !candidate_in_folder(candidate, location)
        {
            continue;
        }
        let facts = if let Some(album_id) = album_id {
            current
                .query_row(params![candidate.photo_id, album_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .optional()
        } else {
            current
                .query_row([&candidate.photo_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .optional()
        }
        .map_err(|_| PhotoQueryError::Storage)?;
        let Some((selection, rating)) = facts else {
            continue;
        };
        let selection = parse_selection_state(&selection).map_err(|_| PhotoQueryError::Storage)?;
        let rating = rating.try_into().map_err(|_| PhotoQueryError::Storage)?;
        if candidate_matches(&query, candidate, selection, rating) {
            push_query_match(&mut ids, &candidate.photo_id, maximum_results)?;
        }
    }
    Ok(ids)
}

fn photo_albums(
    connection: &Connection,
    photo_id: &str,
) -> Result<Option<Vec<PhotoAlbumMembership>>, PersistenceError> {
    let exists = connection
        .query_row("SELECT 1 FROM photos WHERE id=?", [photo_id], |_| Ok(()))
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    if exists.is_none() {
        return Ok(None);
    }
    connection
        .prepare(
            "SELECT a.id, a.name
             FROM album_members m JOIN albums a ON a.id = m.album_id
             WHERE m.photo_id = ? ORDER BY a.created_at, a.id",
        )
        .map_err(|_| PersistenceError::Storage)?
        .query_map([photo_id], |row| {
            Ok(PhotoAlbumMembership {
                album_id: row.get(0)?,
                album_name: row.get(1)?,
            })
        })
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
        .map_err(|_| PersistenceError::Storage)
}

fn album_browse_target(
    connection: &Connection,
    album_id: &str,
) -> Result<Option<AlbumBrowseTarget>, PersistenceError> {
    let exists = connection
        .query_row("SELECT 1 FROM albums WHERE id=?", [album_id], |_| Ok(()))
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    if exists.is_none() {
        return Ok(None);
    }
    let members = connection
        .prepare(
            "SELECT m.photo_id, p.available
             FROM album_members m JOIN photos p ON p.id=m.photo_id
             WHERE m.album_id=? AND p.removed_at_ms IS NULL ORDER BY m.position",
        )
        .map_err(|_| PersistenceError::Storage)?
        .query_map([album_id], |row| {
            Ok(AlbumBrowseMember {
                photo_id: row.get(0)?,
                available: row.get::<_, i64>(1)? != 0,
            })
        })
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    let saved_photo_id = connection
        .query_row(
            "SELECT photo_id FROM album_progress WHERE album_id=?",
            [album_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    Ok(Some(AlbumBrowseTarget {
        members,
        saved_photo_id,
    }))
}

#[derive(Eq, PartialEq)]
struct AlbumVersionState {
    name: String,
    ordered_photo_ids: Vec<String>,
    saved_photo_id: Option<String>,
}

fn album_version_state(
    connection: &Connection,
    album_id: &str,
) -> Result<Option<AlbumVersionState>, PersistenceError> {
    let name = connection
        .query_row("SELECT name FROM albums WHERE id=?", [album_id], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    let Some(name) = name else {
        return Ok(None);
    };
    let ordered_photo_ids = connection
        .prepare("SELECT photo_id FROM album_members WHERE album_id=? ORDER BY position")
        .map_err(|_| PersistenceError::Storage)?
        .query_map([album_id], |row| row.get(0))
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    let saved_photo_id = connection
        .query_row(
            "SELECT photo_id FROM album_progress WHERE album_id=?",
            [album_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    Ok(Some(AlbumVersionState {
        name,
        ordered_photo_ids,
        saved_photo_id,
    }))
}

struct AlbumVersionPlan {
    album_id: String,
    advance: bool,
    deleted: bool,
}

fn album_version_plan(
    connection: &Connection,
    mutation: &AlbumMutation,
) -> Result<AlbumVersionPlan, MutationError> {
    let (album_id, advance, deleted) = match mutation {
        // A new opaque Album ID begins at counter zero in this process epoch.
        AlbumMutation::Create { .. } => {
            return Ok(AlbumVersionPlan {
                album_id: String::new(),
                advance: false,
                deleted: false,
            });
        }
        AlbumMutation::Rename { album_id, name } => {
            let before = album_version_state(connection, album_id)
                .map_err(|_| MutationError::Persistence)?;
            (
                album_id,
                before.is_some_and(|before| before.name != *name),
                false,
            )
        }
        AlbumMutation::Delete { album_id } => (album_id, false, true),
        AlbumMutation::AddMembers {
            album_id,
            photo_ids,
        }
        | AlbumMutation::AddFolderMembers {
            album_id,
            photo_ids,
        } => {
            let before = album_version_state(connection, album_id)
                .map_err(|_| MutationError::Persistence)?;
            let advance = before.is_some_and(|before| {
                photo_ids
                    .iter()
                    .any(|photo_id| !before.ordered_photo_ids.contains(photo_id))
            });
            (album_id, advance, false)
        }
        AlbumMutation::RemoveMember { album_id, .. } => (album_id, true, false),
        AlbumMutation::Reorder {
            album_id,
            photo_ids,
        } => {
            let before = album_version_state(connection, album_id)
                .map_err(|_| MutationError::Persistence)?;
            (
                album_id,
                before.is_some_and(|before| before.ordered_photo_ids != *photo_ids),
                false,
            )
        }
        AlbumMutation::SetProgress { album_id, .. } => (album_id, false, false),
    };
    Ok(AlbumVersionPlan {
        album_id: album_id.clone(),
        advance,
        deleted,
    })
}

fn conflicting_album_id(
    connection: &Connection,
    name: &str,
    except_album_id: Option<&str>,
) -> Result<Option<String>, AlbumWriteError> {
    let existing = connection
        .query_row(
            "SELECT id FROM albums WHERE name=? COLLATE NOCASE",
            [name],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| AlbumWriteError::Persistence)?;
    Ok(existing.filter(|album_id| Some(album_id.as_str()) != except_album_id))
}

fn require_checked_photos(
    connection: &Connection,
    photo_ids: &[String],
) -> Result<(), AlbumWriteError> {
    for photo_id in photo_ids {
        let exists = connection
            .query_row("SELECT 1 FROM photos WHERE id=?", [photo_id], |_| Ok(()))
            .optional()
            .map_err(|_| AlbumWriteError::Persistence)?;
        if exists.is_none() {
            return Err(AlbumWriteError::PhotoNotFound {
                photo_id: photo_id.clone(),
            });
        }
    }
    Ok(())
}

fn create_album_checked(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    versions: &MutationVersions,
    name: String,
) -> Result<AlbumCreationResult, AlbumWriteError> {
    if let Some(album_id) = conflicting_album_id(connection, &name, None)? {
        return Err(AlbumWriteError::NameConflict { name, album_id });
    }
    let result = mutate_album(
        state,
        database_name,
        connection,
        AlbumMutation::Create { name: name.clone() },
    )
    .map_err(album_write_error_from_mutation)?;
    Ok(AlbumCreationResult {
        album: AlbumSummary {
            album_version: versions.album(&result.album_id),
            id: result.album_id,
            name,
            photo_count: 0,
            has_saved_position: false,
        },
    })
}

fn mutate_album_checked(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    versions: &mut MutationVersions,
    mutation: CheckedAlbumMutation,
) -> Result<CheckedAlbumMutationResult, AlbumWriteError> {
    let identity = match &mutation {
        CheckedAlbumMutation::Rename {
            album_id,
            expected_version,
            ..
        }
        | CheckedAlbumMutation::Delete {
            album_id,
            expected_version,
        }
        | CheckedAlbumMutation::AddMembers {
            album_id,
            expected_version,
            ..
        }
        | CheckedAlbumMutation::RemoveMembers {
            album_id,
            expected_version,
            ..
        }
        | CheckedAlbumMutation::Reorder {
            album_id,
            expected_version,
            ..
        } => (album_id.clone(), expected_version.clone()),
    };
    let (album_id, expected_version) = (&identity.0, &identity.1);
    let Some(mut summary) =
        read_album(connection, versions, album_id).map_err(|_| AlbumWriteError::Persistence)?
    else {
        return Err(AlbumWriteError::AlbumNotFound {
            album_id: album_id.clone(),
        });
    };
    // The guard is deliberately checked before classifying an otherwise
    // idempotent request. A changed-away-and-back Album still conflicts.
    if summary.album_version != *expected_version {
        return Err(AlbumWriteError::VersionConflict {
            album_id: album_id.clone(),
            current_version: summary.album_version,
        });
    }
    let current = album_version_state(connection, album_id)
        .map_err(|_| AlbumWriteError::Persistence)?
        .ok_or_else(|| AlbumWriteError::AlbumNotFound {
            album_id: album_id.clone(),
        })?;

    match mutation {
        CheckedAlbumMutation::Rename { name, .. } => {
            if let Some(conflicting_id) = conflicting_album_id(connection, &name, Some(album_id))? {
                return Err(AlbumWriteError::NameConflict {
                    name,
                    album_id: conflicting_id,
                });
            }
            let renamed = current.name != name;
            if renamed && !versions.can_advance_album(album_id) {
                return Err(AlbumWriteError::Persistence);
            }
            mutate_album(
                state,
                database_name,
                connection,
                AlbumMutation::Rename {
                    album_id: album_id.clone(),
                    name: name.clone(),
                },
            )
            .map_err(album_write_error_from_mutation)?;
            if renamed {
                versions
                    .advance_album(album_id)
                    .map_err(album_write_error_from_mutation)?;
            }
            summary.name = name;
            summary.album_version = versions.album(album_id);
            Ok(CheckedAlbumMutationResult::Renamed {
                album: summary,
                renamed,
            })
        }
        CheckedAlbumMutation::Delete { .. } => {
            mutate_album(
                state,
                database_name,
                connection,
                AlbumMutation::Delete {
                    album_id: album_id.clone(),
                },
            )
            .map_err(album_write_error_from_mutation)?;
            versions.album.remove(album_id);
            Ok(CheckedAlbumMutationResult::Deleted {
                album_id: album_id.clone(),
            })
        }
        CheckedAlbumMutation::AddMembers { photo_ids, .. } => {
            require_checked_photos(connection, &photo_ids)?;
            let existing = current
                .ordered_photo_ids
                .iter()
                .map(String::as_str)
                .collect::<HashSet<_>>();
            let added_photo_ids = photo_ids
                .iter()
                .filter(|photo_id| !existing.contains(photo_id.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            let already_member_photo_ids = photo_ids
                .iter()
                .filter(|photo_id| existing.contains(photo_id.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            if !added_photo_ids.is_empty() && !versions.can_advance_album(album_id) {
                return Err(AlbumWriteError::Persistence);
            }
            mutate_album_membership(
                state,
                database_name,
                connection,
                AlbumMembershipMutation::Add {
                    album_id: album_id.clone(),
                    photo_ids,
                },
            )
            .map_err(album_write_error_from_mutation)?;
            if !added_photo_ids.is_empty() {
                versions
                    .advance_album(album_id)
                    .map_err(album_write_error_from_mutation)?;
            }
            summary.photo_count += added_photo_ids.len();
            summary.album_version = versions.album(album_id);
            Ok(CheckedAlbumMutationResult::Added {
                album: summary,
                added_photo_ids,
                already_member_photo_ids,
            })
        }
        CheckedAlbumMutation::RemoveMembers { photo_ids, .. } => {
            require_checked_photos(connection, &photo_ids)?;
            let existing = current
                .ordered_photo_ids
                .iter()
                .map(String::as_str)
                .collect::<HashSet<_>>();
            let removed_photo_ids = photo_ids
                .iter()
                .filter(|photo_id| existing.contains(photo_id.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            let already_absent_photo_ids = photo_ids
                .iter()
                .filter(|photo_id| !existing.contains(photo_id.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            if !removed_photo_ids.is_empty() && !versions.can_advance_album(album_id) {
                return Err(AlbumWriteError::Persistence);
            }
            mutate_album_membership(
                state,
                database_name,
                connection,
                AlbumMembershipMutation::RemoveAdded {
                    album_id: album_id.clone(),
                    photo_ids,
                },
            )
            .map_err(album_write_error_from_mutation)?;
            if !removed_photo_ids.is_empty() {
                versions
                    .advance_album(album_id)
                    .map_err(album_write_error_from_mutation)?;
            }
            let saved_photo_id = current
                .saved_photo_id
                .filter(|saved| !removed_photo_ids.iter().any(|removed| removed == saved));
            summary.photo_count -= removed_photo_ids.len();
            summary.has_saved_position = saved_photo_id.is_some();
            summary.album_version = versions.album(album_id);
            Ok(CheckedAlbumMutationResult::Removed {
                album: summary,
                removed_photo_ids,
                already_absent_photo_ids,
                saved_photo_id,
            })
        }
        CheckedAlbumMutation::Reorder { photo_ids, .. } => {
            require_checked_photos(connection, &photo_ids)?;
            if current.ordered_photo_ids.len() > ALBUM_MEMBERSHIP_BATCH_MAX {
                return Err(AlbumWriteError::LimitExceeded {
                    limit: ALBUM_MEMBERSHIP_BATCH_MAX,
                    actual: current.ordered_photo_ids.len(),
                });
            }
            let current_ids = current
                .ordered_photo_ids
                .iter()
                .map(String::as_str)
                .collect::<HashSet<_>>();
            let requested_ids = photo_ids.iter().map(String::as_str).collect::<HashSet<_>>();
            if current_ids != requested_ids {
                return Err(AlbumWriteError::MembershipConflict {
                    album_id: album_id.clone(),
                    current_version: summary.album_version,
                });
            }
            let reordered = current.ordered_photo_ids != photo_ids;
            if reordered && !versions.can_advance_album(album_id) {
                return Err(AlbumWriteError::Persistence);
            }
            mutate_album(
                state,
                database_name,
                connection,
                AlbumMutation::Reorder {
                    album_id: album_id.clone(),
                    photo_ids: photo_ids.clone(),
                },
            )
            .map_err(album_write_error_from_mutation)?;
            if reordered {
                versions
                    .advance_album(album_id)
                    .map_err(album_write_error_from_mutation)?;
            }
            summary.album_version = versions.album(album_id);
            Ok(CheckedAlbumMutationResult::Reordered {
                album: summary,
                ordered_photo_ids: photo_ids,
                reordered,
            })
        }
    }
}

fn mutation_error_from_sqlite(error: rusqlite::Error) -> MutationError {
    if matches!(
        error,
        rusqlite::Error::SqliteFailure(ref failure, _)
            if failure.code == ErrorCode::ConstraintViolation
    ) {
        MutationError::Conflict
    } else {
        MutationError::Persistence
    }
}

fn mutation_transaction<T>(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    operation: impl FnOnce(&Transaction<'_>) -> Result<T, MutationError>,
) -> Result<T, MutationError> {
    state
        .admit_sidecars(database_name)
        .map_err(|_| MutationError::Persistence)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| MutationError::Persistence)?;
    let result = operation(&transaction)?;
    transaction
        .commit()
        .map_err(|_| MutationError::Persistence)?;
    Ok(result)
}

fn require_album(transaction: &Transaction<'_>, album_id: &str) -> Result<(), MutationError> {
    transaction
        .query_row("SELECT 1 FROM albums WHERE id=?", [album_id], |_| Ok(()))
        .optional()
        .map_err(mutation_error_from_sqlite)?
        .ok_or(MutationError::NotFound)
}

fn mutate_album(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    mutation: AlbumMutation,
) -> Result<AlbumMutationResult, MutationError> {
    mutation_transaction(state, database_name, connection, |transaction| {
        let (album_id, added_count, already_member_count) = match mutation {
            AlbumMutation::Create { name } => {
                let id = uuid_v4()?;
                transaction
                    .execute(
                        "INSERT INTO albums(id,name,created_at) VALUES(?,?,?)",
                        params![id, name, unix_millis()],
                    )
                    .map_err(mutation_error_from_sqlite)?;
                (id, 0, 0)
            }
            AlbumMutation::Rename { album_id, name } => {
                require_album(transaction, &album_id)?;
                transaction
                    .execute(
                        "UPDATE albums SET name=? WHERE id=?",
                        params![name, album_id],
                    )
                    .map_err(mutation_error_from_sqlite)?;
                (album_id, 0, 0)
            }
            AlbumMutation::Delete { album_id } => {
                require_album(transaction, &album_id)?;
                transaction
                    .execute("DELETE FROM albums WHERE id=?", [&album_id])
                    .map_err(mutation_error_from_sqlite)?;
                (album_id, 0, 0)
            }
            AlbumMutation::AddMembers {
                album_id,
                photo_ids,
            } => {
                require_album(transaction, &album_id)?;
                for photo_id in &photo_ids {
                    transaction
                        .query_row("SELECT 1 FROM photos WHERE id=?", [photo_id], |_| Ok(()))
                        .optional()
                        .map_err(mutation_error_from_sqlite)?
                        .ok_or(MutationError::NotFound)?;
                }
                let mut position: i64 = transaction
                    .query_row(
                        "SELECT COALESCE(MAX(position)+1,0) FROM album_members WHERE album_id=?",
                        [&album_id],
                        |row| row.get(0),
                    )
                    .map_err(mutation_error_from_sqlite)?;
                let mut added_count = 0;
                let mut already_member_count = 0;
                for photo_id in photo_ids {
                    let already_member = transaction
                        .query_row(
                            "SELECT 1 FROM album_members WHERE album_id=? AND photo_id=?",
                            params![album_id, photo_id],
                            |_| Ok(()),
                        )
                        .optional()
                        .map_err(mutation_error_from_sqlite)?;
                    if already_member.is_some() {
                        // Adding an existing member is idempotent: the
                        // persisted position is kept and no row is added.
                        already_member_count += 1;
                        continue;
                    }
                    transaction
                        .execute(
                            "INSERT INTO album_members(album_id,photo_id,position) VALUES(?,?,?)",
                            params![album_id, photo_id, position],
                        )
                        .map_err(mutation_error_from_sqlite)?;
                    position = position.checked_add(1).ok_or(MutationError::Conflict)?;
                    added_count += 1;
                }
                (album_id, added_count, already_member_count)
            }
            AlbumMutation::AddFolderMembers {
                album_id,
                photo_ids,
            } => {
                require_album(transaction, &album_id)?;
                for photo_id in &photo_ids {
                    transaction
                        .query_row("SELECT 1 FROM photos WHERE id=?", [photo_id], |_| Ok(()))
                        .optional()
                        .map_err(mutation_error_from_sqlite)?
                        .ok_or(MutationError::NotFound)?;
                }
                let mut position: i64 = transaction
                    .query_row(
                        "SELECT COALESCE(MAX(position)+1,0) FROM album_members WHERE album_id=?",
                        [&album_id],
                        |row| row.get(0),
                    )
                    .map_err(mutation_error_from_sqlite)?;
                let mut added_count = 0;
                let mut already_member_count = 0;
                for photo_id in photo_ids {
                    let already_member = transaction
                        .query_row(
                            "SELECT 1 FROM album_members WHERE album_id=? AND photo_id=?",
                            params![album_id, photo_id],
                            |_| Ok(()),
                        )
                        .optional()
                        .map_err(mutation_error_from_sqlite)?;
                    if already_member.is_some() {
                        already_member_count += 1;
                        continue;
                    }
                    transaction
                        .execute(
                            "INSERT INTO album_members(album_id,photo_id,position) VALUES(?,?,?)",
                            params![album_id, photo_id, position],
                        )
                        .map_err(mutation_error_from_sqlite)?;
                    position = position.checked_add(1).ok_or(MutationError::Conflict)?;
                    added_count += 1;
                }
                (album_id, added_count, already_member_count)
            }
            AlbumMutation::RemoveMember { album_id, photo_id } => {
                let position: i64 = transaction
                    .query_row(
                        "SELECT position FROM album_members WHERE album_id=? AND photo_id=?",
                        params![album_id, photo_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(mutation_error_from_sqlite)?
                    .ok_or(MutationError::NotFound)?;
                transaction
                    .execute(
                        "DELETE FROM album_members WHERE album_id=? AND photo_id=?",
                        params![album_id, photo_id],
                    )
                    .map_err(mutation_error_from_sqlite)?;
                transaction
                    .execute(
                        "UPDATE album_members SET position=position-1 WHERE album_id=? AND position>?",
                        params![album_id, position],
                    )
                    .map_err(mutation_error_from_sqlite)?;
                (album_id, 0, 0)
            }
            AlbumMutation::Reorder {
                album_id,
                photo_ids,
            } => {
                require_album(transaction, &album_id)?;
                let current = transaction
                    .prepare(
                        "SELECT photo_id,position FROM album_members WHERE album_id=? ORDER BY position",
                    )
                    .map_err(mutation_error_from_sqlite)?
                    .query_map([&album_id], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                    })
                    .map_err(mutation_error_from_sqlite)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(mutation_error_from_sqlite)?;
                let current_ids = current
                    .iter()
                    .map(|(photo_id, _)| photo_id.clone())
                    .collect::<std::collections::BTreeSet<_>>();
                let requested_ids = photo_ids
                    .iter()
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>();
                if current
                    .iter()
                    .enumerate()
                    .any(|(index, (_, position))| *position != index as i64)
                    || current_ids != requested_ids
                    || requested_ids.len() != photo_ids.len()
                {
                    return Err(MutationError::Conflict);
                }
                // SQLite's schema requires nonnegative positions. After validating the
                // dense 0..n-1 invariant, n is strictly above every current and final
                // position, so the temporary range [n, 2n) cannot collide with either.
                let temporary_offset =
                    i64::try_from(current.len()).map_err(|_| MutationError::Conflict)?;
                if temporary_offset.checked_mul(2).is_none() {
                    return Err(MutationError::Conflict);
                }
                transaction
                    .execute(
                        "UPDATE album_members SET position=position+? WHERE album_id=?",
                        params![temporary_offset, album_id],
                    )
                    .map_err(mutation_error_from_sqlite)?;
                for (position, photo_id) in photo_ids.iter().enumerate() {
                    transaction
                        .execute(
                            "UPDATE album_members SET position=? WHERE album_id=? AND photo_id=?",
                            params![position as i64, album_id, photo_id],
                        )
                        .map_err(mutation_error_from_sqlite)?;
                }
                (album_id, 0, 0)
            }
            AlbumMutation::SetProgress { album_id, photo_id } => {
                transaction
                    .query_row(
                        "SELECT 1 FROM album_members WHERE album_id=? AND photo_id=?",
                        params![album_id, photo_id],
                        |_| Ok(()),
                    )
                    .optional()
                    .map_err(mutation_error_from_sqlite)?
                    .ok_or(MutationError::NotFound)?;
                transaction
                    .execute(
                        "INSERT INTO album_progress(album_id,photo_id) VALUES(?,?)
                         ON CONFLICT(album_id) DO UPDATE SET photo_id=excluded.photo_id",
                        params![album_id, photo_id],
                    )
                    .map_err(mutation_error_from_sqlite)?;
                (album_id, 0, 0)
            }
        };
        Ok(AlbumMutationResult {
            album_id,
            added_count,
            already_member_count,
        })
    })
}

/// Applies one bounded identity-bearing membership operation. The add result
/// distinguishes newly inserted IDs from existing members; the compensation
/// result distinguishes removed IDs from members already absent. Both result
/// lists preserve request order so the browser can retain an exact, bounded
/// record without reconstructing membership from aggregate counts.
fn mutate_album_membership(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    mutation: AlbumMembershipMutation,
) -> Result<AlbumMembershipResult, MutationError> {
    mutation_transaction(state, database_name, connection, |transaction| {
        let (album_id, photo_ids) = match &mutation {
            AlbumMembershipMutation::Add {
                album_id,
                photo_ids,
            }
            | AlbumMembershipMutation::RemoveAdded {
                album_id,
                photo_ids,
            } => (album_id, photo_ids),
        };
        require_album(transaction, album_id)?;
        // Resolve every identity before the first membership write. An
        // unknown Photo is a malformed compensation record, not an implicit
        // already-absent result, and therefore cannot produce a partial batch.
        for photo_id in photo_ids {
            transaction
                .query_row("SELECT 1 FROM photos WHERE id=?", [photo_id], |_| Ok(()))
                .optional()
                .map_err(mutation_error_from_sqlite)?
                .ok_or(MutationError::NotFound)?;
        }

        match mutation {
            AlbumMembershipMutation::Add {
                album_id,
                photo_ids,
            } => {
                let mut position: i64 = transaction
                    .query_row(
                        "SELECT COALESCE(MAX(position)+1,0) FROM album_members WHERE album_id=?",
                        [&album_id],
                        |row| row.get(0),
                    )
                    .map_err(mutation_error_from_sqlite)?;
                let mut added_photo_ids = Vec::new();
                let mut already_member_photo_ids = Vec::new();
                for photo_id in photo_ids {
                    let already_member = transaction
                        .query_row(
                            "SELECT 1 FROM album_members WHERE album_id=? AND photo_id=?",
                            params![album_id, photo_id],
                            |_| Ok(()),
                        )
                        .optional()
                        .map_err(mutation_error_from_sqlite)?;
                    if already_member.is_some() {
                        already_member_photo_ids.push(photo_id);
                        continue;
                    }
                    transaction
                        .execute(
                            "INSERT INTO album_members(album_id,photo_id,position) VALUES(?,?,?)",
                            params![album_id, photo_id, position],
                        )
                        .map_err(mutation_error_from_sqlite)?;
                    position = position.checked_add(1).ok_or(MutationError::Conflict)?;
                    added_photo_ids.push(photo_id);
                }
                Ok(AlbumMembershipResult {
                    album_id,
                    added_photo_ids,
                    already_member_photo_ids,
                    removed_photo_ids: Vec::new(),
                    already_absent_photo_ids: Vec::new(),
                })
            }
            AlbumMembershipMutation::RemoveAdded {
                album_id,
                photo_ids,
            } => {
                let mut removed_photo_ids = Vec::new();
                let mut already_absent_photo_ids = Vec::new();
                for photo_id in photo_ids {
                    let position = transaction
                        .query_row(
                            "SELECT position FROM album_members WHERE album_id=? AND photo_id=?",
                            params![album_id, photo_id],
                            |row| row.get::<_, i64>(0),
                        )
                        .optional()
                        .map_err(mutation_error_from_sqlite)?;
                    let Some(position) = position else {
                        already_absent_photo_ids.push(photo_id);
                        continue;
                    };
                    transaction
                        .execute(
                            "DELETE FROM album_members WHERE album_id=? AND photo_id=?",
                            params![album_id, photo_id],
                        )
                        .map_err(mutation_error_from_sqlite)?;
                    transaction
                        .execute(
                            "UPDATE album_members SET position=position-1 WHERE album_id=? AND position>?",
                            params![album_id, position],
                        )
                        .map_err(mutation_error_from_sqlite)?;
                    removed_photo_ids.push(photo_id);
                }
                Ok(AlbumMembershipResult {
                    album_id,
                    added_photo_ids: Vec::new(),
                    already_member_photo_ids: Vec::new(),
                    removed_photo_ids,
                    already_absent_photo_ids,
                })
            }
        }
    })
}

fn state_value(
    selection: &str,
    rating: i64,
    field: PhotoStateField,
) -> Result<PhotoStateValue, MutationError> {
    match field {
        PhotoStateField::SelectionState => parse_selection_state(selection)
            .map(PhotoStateValue::Selection)
            .map_err(|_| MutationError::Persistence),
        PhotoStateField::Rating => rating
            .try_into()
            .map(PhotoStateValue::Rating)
            .map_err(|_| MutationError::Persistence),
    }
}

fn mutate_photo_state(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    mutation: PhotoStateMutation,
) -> Result<PhotoStateMutationResult, MutationError> {
    mutation_transaction(state, database_name, connection, |transaction| {
        let (selection, rating): (String, i64) = transaction
            .query_row(
                "SELECT selection_state,rating FROM photos WHERE id=?",
                [&mutation.photo_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(mutation_error_from_sqlite)?
            .ok_or(MutationError::NotFound)?;
        let prior = state_value(&selection, rating, mutation.field)?;
        if mutation
            .expected_current
            .is_some_and(|expected| expected != prior)
        {
            return Err(MutationError::Conflict);
        }
        if let Some(album_id) = mutation.album_id.as_deref() {
            transaction
                .query_row(
                    "SELECT 1 FROM album_members WHERE album_id=? AND photo_id=?",
                    params![album_id, mutation.photo_id],
                    |_| Ok(()),
                )
                .optional()
                .map_err(mutation_error_from_sqlite)?
                .ok_or(MutationError::NotFound)?;
        }
        match mutation.value {
            PhotoStateValue::Selection(value) => {
                transaction
                    .execute(
                        "UPDATE photos SET selection_state=? WHERE id=?",
                        params![selection_state_value(value), mutation.photo_id],
                    )
                    .map_err(mutation_error_from_sqlite)?;
            }
            PhotoStateValue::Rating(value) => {
                transaction
                    .execute(
                        "UPDATE photos SET rating=? WHERE id=?",
                        params![i64::from(value), mutation.photo_id],
                    )
                    .map_err(mutation_error_from_sqlite)?;
            }
        }
        if let Some(album_id) = mutation.album_id.as_deref() {
            transaction
                .execute(
                    "INSERT INTO album_progress(album_id,photo_id) VALUES(?,?)
                     ON CONFLICT(album_id) DO UPDATE SET photo_id=excluded.photo_id",
                    params![album_id, mutation.photo_id],
                )
                .map_err(mutation_error_from_sqlite)?;
        }
        Ok(PhotoStateMutationResult {
            photo_id: mutation.photo_id.clone(),
            undo: PhotoStateUndo {
                photo_id: mutation.photo_id,
                field: mutation.field,
                prior_value: prior,
                expected_current: mutation.value,
            },
        })
    })
}

/// One bounded batch Selection State write. Every requested Photo is resolved
/// inside one transaction and reports exactly one outcome, so matching Photos
/// can be confirmed even when another requested Photo is missing or changed.
fn mutate_photo_state_batch(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    mutation: PhotoStateBatchMutation,
) -> Result<PhotoStateBatchResult, MutationError> {
    mutation_transaction(state, database_name, connection, |transaction| {
        let mut applied = Vec::with_capacity(mutation.photos.len());
        let mut changed_elsewhere = Vec::new();
        let mut missing = Vec::new();
        for item in &mutation.photos {
            let row: Option<String> = transaction
                .query_row(
                    "SELECT selection_state FROM photos WHERE id=?",
                    [&item.photo_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(mutation_error_from_sqlite)?;
            let Some(selection) = row else {
                missing.push(PhotoStateBatchMissing {
                    photo_id: item.photo_id.clone(),
                });
                continue;
            };
            let prior_value =
                parse_selection_state(&selection).map_err(|_| MutationError::Persistence)?;
            if prior_value != item.expected_current {
                changed_elsewhere.push(PhotoStateBatchChangedElsewhere {
                    photo_id: item.photo_id.clone(),
                    current_value: prior_value,
                });
                continue;
            }
            transaction
                .execute(
                    "UPDATE photos SET selection_state=? WHERE id=?",
                    params![selection_state_value(mutation.value), &item.photo_id],
                )
                .map_err(mutation_error_from_sqlite)?;
            applied.push(PhotoStateBatchApplied {
                photo_id: item.photo_id.clone(),
                prior_value,
            });
        }
        Ok(PhotoStateBatchResult {
            applied,
            changed_elsewhere,
            missing,
        })
    })
}

/// The largest number of Photo identities one removal or restore statement
/// addresses at once. Outcomes are still reported per requested Photo in
/// request order; the bound only keeps one SQLite statement small.
const PHOTO_REMOVAL_CHUNK: usize = 500;

/// One confirmed removal of a reviewed rejected result. Every requested Photo
/// is resolved inside one transaction and reports exactly one outcome, so a
/// Photo that changed elsewhere stays in the Library instead of being removed
/// silently.
fn remove_photos(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    mutation: PhotoRemovalMutation,
) -> Result<PhotoRemovalResult, MutationError> {
    mutation_transaction(state, database_name, connection, |transaction| {
        // The marker is assigned on the first Photo this request removes, so a
        // request that removes nothing leaves the Library exactly as it was.
        let mut marker: Option<i64> = None;
        let mut result = PhotoRemovalResult {
            operation_id: mutation.operation_id.clone(),
            counts: PhotoRemovalCounts::default(),
            removed: Vec::new(),
            changed_elsewhere: Vec::new(),
            missing: Vec::new(),
            already_removed: Vec::new(),
        };
        for chunk in mutation.photo_ids.chunks(PHOTO_REMOVAL_CHUNK) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let mut statement = transaction
                .prepare(&format!(
                    "SELECT id,selection_state,removed_at_ms,removed_operation
                     FROM photos WHERE id IN ({placeholders})"
                ))
                .map_err(mutation_error_from_sqlite)?;
            let rows = statement
                .query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                })
                .map_err(mutation_error_from_sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(mutation_error_from_sqlite)?;
            let mut facts = std::collections::HashMap::with_capacity(rows.len());
            for (photo_id, selection_state, removed_at, removed_operation) in rows {
                facts.insert(photo_id, (selection_state, removed_at, removed_operation));
            }
            for photo_id in chunk {
                let Some((selection_state, removed_at, removed_operation)) = facts.get(photo_id)
                else {
                    result.missing.push(photo_id.clone());
                    continue;
                };
                if removed_at.is_some() {
                    // A retried request repeats its own operation, so what
                    // this operation already removed is still its own result.
                    if removed_operation.as_deref() == Some(result.operation_id.as_str()) {
                        result.removed.push(photo_id.clone());
                    } else {
                        result.already_removed.push(photo_id.clone());
                    }
                    continue;
                }
                if parse_selection_state(selection_state).map_err(|_| MutationError::Persistence)?
                    != SelectionState::Rejected
                {
                    result.changed_elsewhere.push(photo_id.clone());
                    continue;
                }
                let removed_at_ms = match marker {
                    Some(marker) => marker,
                    None => {
                        let assigned = next_removal_marker(transaction)?;
                        marker = Some(assigned);
                        assigned
                    }
                };
                transaction
                    .execute(
                        "UPDATE photos SET removed_at_ms=?,removed_operation=? WHERE id=?",
                        params![removed_at_ms, result.operation_id, photo_id],
                    )
                    .map_err(mutation_error_from_sqlite)?;
                result.removed.push(photo_id.clone());
            }
        }
        // One outcome per requested Photo: the counts are the lists, so a
        // request can never report a total its identities contradict.
        result.counts = PhotoRemovalCounts {
            removed: result.removed.len(),
            changed_elsewhere: result.changed_elsewhere.len(),
            missing: result.missing.len(),
            already_removed: result.already_removed.len(),
        };
        Ok(result)
    })
}

/// One restore request: every Photo one operation still owns, or an explicit
/// set of Photos with the removal marker each was reviewed at. Each requested
/// Photo is compared and set inside one transaction, so a removal that changed
/// after the caller read it is reported instead of overwritten.
fn restore_photos(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    restoration: PhotoRestoration,
) -> Result<PhotoRestorationResult, MutationError> {
    mutation_transaction(state, database_name, connection, |transaction| {
        let mut result = PhotoRestorationResult {
            restored: Vec::new(),
            counts: PhotoRestorationCounts::default(),
            changed_elsewhere: Vec::new(),
            missing: Vec::new(),
            operations: Vec::new(),
        };
        // Operations this request changed, so the response can report how many
        // Photos each still owns. The named operation counts even when it owns
        // nothing, because that is exactly what its Undo surface must learn.
        let mut touched = std::collections::BTreeSet::new();
        let requests = match restoration {
            PhotoRestoration::Operation(operation_id) => {
                touched.insert(operation_id.clone());
                let photo_ids = transaction
                    .prepare(
                        "SELECT id FROM photos
                         WHERE removed_operation=? AND removed_at_ms IS NOT NULL
                         ORDER BY removed_at_ms, id",
                    )
                    .map_err(mutation_error_from_sqlite)?
                    .query_map([operation_id.as_str()], |row| row.get::<_, String>(0))
                    .map_err(mutation_error_from_sqlite)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(mutation_error_from_sqlite)?;
                photo_ids
                    .into_iter()
                    .map(|photo_id| (photo_id, None))
                    .collect::<Vec<_>>()
            }
            PhotoRestoration::Photos(markers) => markers
                .into_iter()
                .map(|marker| (marker.photo_id, Some(marker.removed_at_ms)))
                .collect(),
        };
        for chunk in requests.chunks(PHOTO_REMOVAL_CHUNK) {
            let placeholders = std::iter::repeat_n("?", chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let mut statement = transaction
                .prepare(&format!(
                    "SELECT id,removed_at_ms,removed_operation FROM photos WHERE id IN ({placeholders})"
                ))
                .map_err(mutation_error_from_sqlite)?;
            let rows = statement
                .query_map(
                    rusqlite::params_from_iter(chunk.iter().map(|(photo_id, _)| photo_id)),
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<i64>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                        ))
                    },
                )
                .map_err(mutation_error_from_sqlite)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(mutation_error_from_sqlite)?;
            let mut facts = std::collections::HashMap::with_capacity(rows.len());
            for (photo_id, removed_at, removed_operation) in rows {
                facts.insert(photo_id, (removed_at, removed_operation));
            }
            for (photo_id, expected_removed_at) in chunk {
                let Some((removed_at, removed_operation)) = facts.get(photo_id) else {
                    result.missing.push(photo_id.clone());
                    continue;
                };
                // An explicit request restores the removal it reviewed. A
                // Photo whose marker moved on — restored and removed again, or
                // removed by another operation — is reported, never cleared.
                let reviewed = expected_removed_at.is_none_or(|expected| {
                    removed_at.is_some_and(|removed_at| removed_at == expected)
                });
                if !reviewed {
                    result.changed_elsewhere.push(photo_id.clone());
                    continue;
                }
                let Some(removed_at) = removed_at else {
                    result.changed_elsewhere.push(photo_id.clone());
                    continue;
                };
                let updated = transaction
                    .execute(
                        "UPDATE photos SET removed_at_ms=NULL,removed_operation=NULL
                         WHERE id=? AND removed_at_ms=?",
                        params![photo_id, removed_at],
                    )
                    .map_err(mutation_error_from_sqlite)?;
                if updated == 0 {
                    result.changed_elsewhere.push(photo_id.clone());
                    continue;
                }
                if let Some(operation_id) = removed_operation {
                    touched.insert(operation_id.clone());
                }
                result.restored.push(photo_id.clone());
            }
        }
        result.counts = PhotoRestorationCounts {
            restored: result.restored.len(),
            changed_elsewhere: result.changed_elsewhere.len(),
            missing: result.missing.len(),
        };
        result.operations = operation_remainders(transaction, &touched)?;
        Ok(result)
    })
}

/// How many Photos each named operation still owns. An operation with nothing
/// left is reported with zero rather than omitted, so a surface holding an
/// Undo for it can stop offering a count the Library no longer holds.
fn operation_remainders(
    transaction: &Transaction<'_>,
    operation_ids: &std::collections::BTreeSet<String>,
) -> Result<Vec<PhotoOperationRemainder>, MutationError> {
    let mut remainders = operation_ids
        .iter()
        .map(|operation_id| PhotoOperationRemainder {
            operation_id: operation_id.clone(),
            removed: 0,
        })
        .collect::<Vec<_>>();
    let ids = operation_ids.iter().cloned().collect::<Vec<_>>();
    for chunk in ids.chunks(PHOTO_REMOVAL_CHUNK) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let mut statement = transaction
            .prepare(&format!(
                "SELECT removed_operation,count(*) FROM photos
                 WHERE removed_at_ms IS NOT NULL AND removed_operation IN ({placeholders})
                 GROUP BY removed_operation"
            ))
            .map_err(mutation_error_from_sqlite)?;
        let rows = statement
            .query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(mutation_error_from_sqlite)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(mutation_error_from_sqlite)?;
        for (operation_id, remaining) in rows {
            if let Some(remainder) = remainders
                .iter_mut()
                .find(|remainder| remainder.operation_id == operation_id)
            {
                remainder.removed = usize::try_from(remaining).unwrap_or(usize::MAX);
            }
        }
    }
    Ok(remainders)
}

/// One bounded page of removed Photos, newest removal first.
fn removed_photos(connection: &Connection, start: usize, limit: usize) -> RemovedPhotoPageResult {
    let total: i64 = connection
        .query_row(
            "SELECT count(*) FROM photos WHERE removed_at_ms IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .map_err(|_| PersistenceError::Storage)?;
    let records = connection
        .prepare(
            "SELECT id,removed_at_ms FROM photos WHERE removed_at_ms IS NOT NULL
             ORDER BY removed_at_ms DESC, id LIMIT ? OFFSET ?",
        )
        .map_err(|_| PersistenceError::Storage)?
        .query_map(params![limit as i64, start as i64], |row| {
            Ok(RemovedPhotoRecord {
                photo_id: row.get(0)?,
                removed_at_ms: row.get(1)?,
            })
        })
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    Ok((records, usize::try_from(total).unwrap_or(usize::MAX)))
}

fn selection_state_value(value: SelectionState) -> &'static str {
    match value {
        SelectionState::Undecided => "undecided",
        SelectionState::Selected => "selected",
        SelectionState::Rejected => "rejected",
    }
}

/// The current decision facts together with their guard, as one serialized
/// owner operation sees them.
fn photo_decision_snapshot(
    versions: &MutationVersions,
    facts: PhotoDecisionFacts,
    photo_id: &str,
) -> PhotoDecisionSnapshot {
    PhotoDecisionSnapshot {
        selection_state: facts.selection_state,
        rating: facts.rating,
        decision_version: versions.photo(photo_id),
    }
}

/// One version-checked Photo decision batch. Classification, effective
/// writes, and version advancement run in this single serialized owner
/// command: every effective change commits in one transaction, storage
/// failure rolls back all of them and preserves versions, and conflicts or
/// missing records are per-item domain results in request order.
fn mutate_photo_decision_checked(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    versions: &mut MutationVersions,
    mutation: CheckedPhotoDecisionMutation,
) -> Result<CheckedPhotoDecisionResult, PhotoDecisionWriteError> {
    enum Classification {
        Missing,
        Conflict { current: PhotoDecisionSnapshot },
        Unchanged { current: PhotoDecisionSnapshot },
        Change { prior: PhotoDecisionFacts },
    }
    let current_value = |facts: PhotoDecisionFacts| match mutation.field {
        PhotoStateField::SelectionState => PhotoStateValue::Selection(facts.selection_state),
        PhotoStateField::Rating => PhotoStateValue::Rating(facts.rating),
    };
    let mut classified = Vec::with_capacity(mutation.photos.len());
    for item in &mutation.photos {
        let facts = connection
            .query_row(
                "SELECT selection_state,rating FROM photos WHERE id=?",
                [&item.photo_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .map_err(|_| PhotoDecisionWriteError::Persistence)
            .and_then(|row| {
                row.map(|(selection_state, rating)| {
                    Ok(PhotoDecisionFacts {
                        selection_state: parse_selection_state(&selection_state)
                            .map_err(|_| PhotoDecisionWriteError::Persistence)?,
                        rating: rating
                            .try_into()
                            .map_err(|_| PhotoDecisionWriteError::Persistence)?,
                    })
                })
                .transpose()
            })?;
        // The guard is deliberately checked before classifying an otherwise
        // idempotent request. A changed-away-and-back decision still conflicts.
        let classification = match facts {
            None => Classification::Missing,
            Some(facts) => {
                let current = photo_decision_snapshot(versions, facts, &item.photo_id);
                if current.decision_version != item.expected_version {
                    Classification::Conflict { current }
                } else if current_value(facts) == mutation.value {
                    Classification::Unchanged { current }
                } else {
                    Classification::Change { prior: facts }
                }
            }
        };
        classified.push((item.photo_id.clone(), classification));
    }
    let effective = classified
        .iter()
        .filter(|(_, classification)| matches!(classification, Classification::Change { .. }))
        .map(|(photo_id, _)| photo_id.clone())
        .collect::<Vec<_>>();
    // Counter saturation fails closed before any write rather than wrapping.
    if effective
        .iter()
        .any(|photo_id| !versions.can_advance_photo(photo_id))
    {
        return Err(PhotoDecisionWriteError::Persistence);
    }
    mutation_transaction(state, database_name, connection, |transaction| {
        for photo_id in &effective {
            match mutation.value {
                PhotoStateValue::Selection(value) => {
                    transaction
                        .execute(
                            "UPDATE photos SET selection_state=? WHERE id=?",
                            params![selection_state_value(value), photo_id],
                        )
                        .map_err(mutation_error_from_sqlite)?;
                }
                PhotoStateValue::Rating(value) => {
                    transaction
                        .execute(
                            "UPDATE photos SET rating=? WHERE id=?",
                            params![i64::from(value), photo_id],
                        )
                        .map_err(mutation_error_from_sqlite)?;
                }
            }
        }
        Ok(())
    })
    .map_err(photo_decision_write_error_from_mutation)?;
    // Counters become visible only after the committed transaction; a
    // rollback above leaves every prior version unchanged.
    for photo_id in &effective {
        versions
            .advance_photo(photo_id)
            .map_err(photo_decision_write_error_from_mutation)?;
    }
    let mut counts = CheckedPhotoDecisionCounts::default();
    let mut results = Vec::with_capacity(classified.len());
    for (photo_id, classification) in classified {
        let outcome = match classification {
            Classification::Missing => {
                counts.missing += 1;
                CheckedPhotoDecisionOutcome::Missing
            }
            Classification::Conflict { current } => {
                counts.conflict += 1;
                CheckedPhotoDecisionOutcome::Conflict { current }
            }
            Classification::Unchanged { current } => {
                counts.unchanged += 1;
                CheckedPhotoDecisionOutcome::Unchanged { current }
            }
            Classification::Change { prior } => {
                counts.changed += 1;
                let after = PhotoDecisionFacts {
                    selection_state: match (mutation.field, mutation.value) {
                        (PhotoStateField::SelectionState, PhotoStateValue::Selection(value)) => {
                            value
                        }
                        _ => prior.selection_state,
                    },
                    rating: match (mutation.field, mutation.value) {
                        (PhotoStateField::Rating, PhotoStateValue::Rating(value)) => value,
                        _ => prior.rating,
                    },
                };
                CheckedPhotoDecisionOutcome::Changed {
                    prior,
                    current: photo_decision_snapshot(versions, after, &photo_id),
                }
            }
        };
        results.push(CheckedPhotoDecisionItemResult { photo_id, outcome });
    }
    Ok(CheckedPhotoDecisionResult { results, counts })
}

fn table_exists(connection: &Connection, name: &str) -> Result<bool, PersistenceError> {
    connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?",
            [name],
            |_| Ok(()),
        )
        .optional()
        .map(|value| value.is_some())
        .map_err(|_| PersistenceError::Storage)
}

fn names(connection: &Connection, kind: &str) -> Result<Vec<String>, PersistenceError> {
    connection
        .prepare("SELECT name FROM sqlite_master WHERE type=? AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .and_then(|mut statement| {
            statement
                .query_map([kind], |row| row.get(0))?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|_| PersistenceError::Storage)
}

fn table_columns(connection: &Connection, table: &str) -> Result<Vec<String>, PersistenceError> {
    connection
        .prepare(&format!("PRAGMA table_info(\"{table}\")"))
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get(1))?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|_| PersistenceError::Storage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::source_revision;
    use crate::{
        CaptureTimeBound, CheckedPhotoDecisionItem, LibraryRoot, PhotoRemovalMarker,
        PhotoStateBatchItem, identity::original_id,
    };
    use serde::Deserialize;
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_TEMP_TREE: AtomicU64 = AtomicU64::new(0);

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RejectionFixture {
        name: String,
        version: u32,
        sql: String,
        expected_error: String,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct CaptureOrderVector {
        name: String,
        raw_path: Option<String>,
        raw_order_key: Option<String>,
        jpeg_path: Option<String>,
        jpeg_order_key: Option<String>,
        order_key: Option<String>,
        #[serde(default)]
        expected_paths: Vec<String>,
        expected_photo_ids: Option<Vec<String>>,
    }

    fn capture_order_vectors() -> Vec<CaptureOrderVector> {
        serde_json::from_str(include_str!(
            "../../../../compatibility/metadata/capture-order.json"
        ))
        .unwrap()
    }

    struct TempTree(PathBuf);
    impl TempTree {
        fn new() -> Self {
            loop {
                let nonce = NEXT_TEMP_TREE.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir()
                    .join(format!("slipstream-owner-{}-{nonce}", std::process::id()));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("temporary owner fixture could not be created: {error}"),
                }
            }
        }
    }
    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fixture() -> (TempTree, LibraryRoot, StateDirectory, DatabaseName, PathBuf) {
        let base = TempTree::new();
        let originals = base.0.join("originals");
        let state_path = base.0.join("state");
        fs::create_dir(&originals).unwrap();
        fs::create_dir(&state_path).unwrap();
        fs::set_permissions(&state_path, fs::Permissions::from_mode(0o700)).unwrap();
        let library = LibraryRoot::open(&originals).unwrap();
        let state = StateDirectory::open_or_create(&library, &state_path).unwrap();
        let database_path = state_path.join("library.sqlite");
        (
            base,
            library,
            state,
            DatabaseName::parse("library.sqlite").unwrap(),
            database_path,
        )
    }

    fn seed(path: &Path, sql: &str) {
        let connection = Connection::open(path).unwrap();
        connection.execute_batch(sql).unwrap();
    }

    struct RecipeTestPhoto<'a> {
        original_id: &'a str,
        photo_id: &'a str,
        relative_path: &'a str,
        kind: &'a str,
        available: bool,
        size: i64,
        mtime_ms: f64,
    }

    fn add_recipe_test_photo(connection: &Connection, photo: RecipeTestPhoto<'_>) {
        connection
            .execute(
                "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,capture_metadata_state)
                 VALUES(?,?,?,?,?,?,'pending')",
                params![
                    photo.original_id,
                    photo.relative_path,
                    photo.kind,
                    photo.size,
                    photo.mtime_ms,
                    i64::from(photo.available)
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO photos(id,original_id,available,preview_state,sort_path,selection_state,rating)
                 VALUES(?,?,?,'inspection-pending',?,'undecided',0)",
                params![
                    photo.photo_id,
                    photo.original_id,
                    i64::from(photo.available),
                    photo.relative_path
                ],
            )
            .unwrap();
    }

    #[tokio::test]
    async fn initializes_current_schema_and_runs_fifo_writes() {
        let (_base, library, state, name, path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        assert_eq!(persistence.probe().await.unwrap(), 1);
        persistence.write_probe().await.unwrap();
        assert_eq!(persistence.probe().await.unwrap(), 3);
        let (configuration_send, configuration_receive) = oneshot::channel();
        persistence
            .submit(Command::Configuration(configuration_send))
            .unwrap();
        assert_eq!(
            configuration_receive.await.unwrap().unwrap(),
            ("delete".to_owned(), 1)
        );
        persistence.shutdown().unwrap();
        let connection = Connection::open(path).unwrap();
        validate_canonical_schema(&connection, SchemaVersion::V9).unwrap();
    }

    #[tokio::test]
    async fn v6_to_v7_migration_preserves_existing_library_rows() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v6.sql"),
        );
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
                [library.canonical_path().to_str().unwrap()],
            )
            .unwrap();
        add_recipe_test_photo(
            &connection,
            RecipeTestPhoto {
                original_id: "raw-original",
                photo_id: "raw-photo",
                relative_path: "shoot/one.ARW",
                kind: "raw",
                available: true,
                size: 17,
                mtime_ms: 1_000.0,
            },
        );
        connection
            .execute(
                "INSERT INTO original_fingerprints(original_id,digest,size,mtime_ms) VALUES(?,?,?,?)",
                params!["raw-original", "a".repeat(64), 17_i64, 1_000.0_f64],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO albums(id,name,created_at) VALUES('album-one','Preserved',9)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO album_members(album_id,photo_id,position) VALUES('album-one','raw-photo',0)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE photos SET selection_state='selected',rating=4 WHERE id='raw-photo'",
                [],
            )
            .unwrap();
        drop(connection);

        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence.snapshot().await.unwrap();
        assert_eq!(snapshot.originals.len(), 1);
        assert_eq!(snapshot.originals[0].id, "raw-original");
        assert_eq!(
            snapshot.originals[0].relative_path.as_str(),
            "shoot/one.ARW"
        );
        assert_eq!(snapshot.originals[0].facts.size, 17);
        assert_eq!(snapshot.photos.len(), 1);
        assert_eq!(snapshot.photos[0].id, "raw-photo");
        assert_eq!(snapshot.photos[0].selection_state, SelectionState::Selected);
        assert_eq!(snapshot.photos[0].rating, 4);
        assert!(!snapshot.photos[0].has_saved_edits);
        assert_eq!(
            persistence.list_albums().await.unwrap()[0].members[0].photo_id,
            "raw-photo"
        );
        persistence.shutdown().unwrap();

        let connection = Connection::open(path).unwrap();
        validate_canonical_schema(&connection, SchemaVersion::V9).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                .unwrap(),
            9
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT digest,size,mtime_ms FROM original_fingerprints WHERE original_id='raw-original'",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, f64>(2)?)),
                )
                .unwrap(),
            ("a".repeat(64), 17, 1_000.0)
        );
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM edit_recipes", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn edit_recipe_compare_and_set_rebind_and_read_model_are_guarded() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v6.sql"),
        );
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
                [library.canonical_path().to_str().unwrap()],
            )
            .unwrap();
        add_recipe_test_photo(
            &connection,
            RecipeTestPhoto {
                original_id: "raw-original",
                photo_id: "raw-photo",
                relative_path: "shoot/one.ARW",
                kind: "raw",
                available: true,
                size: 17,
                mtime_ms: 1_000.0,
            },
        );
        drop(connection);

        let persistence = Persistence::open(
            state,
            name.clone(),
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let initial_source = source_revision("shoot/one.ARW", 17, 1_000.0).unwrap();
        let initial = persistence
            .edit_recipe_receiver("raw-photo")
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(initial.recipe.is_none());
        assert!(initial.source_available);
        assert_eq!(initial.current_source_revision, initial_source);

        let mutation = SaveEditRecipe {
            photo_id: "raw-photo".to_owned(),
            request_id: "first-save".to_owned(),
            expected_recipe_version: None,
            expected_source_revision: initial_source.clone(),
            settings: EditRecipeSettings {
                exposure_ev: 0.0,
                white_balance: WhiteBalanceIntent::AsShot,
            },
        };
        let first = persistence
            .save_edit_recipe_receiver(mutation.clone())
            .unwrap();
        let concurrent = persistence
            .save_edit_recipe_receiver(SaveEditRecipe {
                request_id: "concurrent-save".to_owned(),
                ..mutation.clone()
            })
            .unwrap();
        let (first, concurrent) = tokio::join!(first, concurrent);
        let first = first.unwrap().unwrap();
        let concurrent = concurrent.unwrap().unwrap();
        let recipe = match first {
            EditRecipeWriteOutcome::Saved(recipe) => recipe,
            outcome => panic!("first compare-and-set should save, got {outcome:?}"),
        };
        assert!(matches!(
            concurrent,
            EditRecipeWriteOutcome::Conflict(EditRecipeRead {
                recipe: Some(_),
                ..
            })
        ));
        assert_eq!(recipe.settings.exposure_ev, 0.0);

        let unchanged = persistence
            .save_edit_recipe_receiver(SaveEditRecipe {
                photo_id: "raw-photo".to_owned(),
                request_id: "unchanged-save".to_owned(),
                expected_recipe_version: Some(recipe.revision.clone()),
                expected_source_revision: initial_source.clone(),
                settings: recipe.settings,
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(unchanged, EditRecipeWriteOutcome::Unchanged(recipe.clone()));

        let replay = persistence
            .save_edit_recipe_receiver(SaveEditRecipe {
                photo_id: "raw-photo".to_owned(),
                request_id: "first-save".to_owned(),
                expected_recipe_version: None,
                expected_source_revision: initial_source.clone(),
                settings: EditRecipeSettings {
                    exposure_ev: 0.0,
                    white_balance: WhiteBalanceIntent::AsShot,
                },
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(replay, EditRecipeWriteOutcome::Replayed(recipe.clone()));

        let request_conflict = persistence
            .save_edit_recipe_receiver(SaveEditRecipe {
                photo_id: "raw-photo".to_owned(),
                request_id: "first-save".to_owned(),
                expected_recipe_version: None,
                expected_source_revision: initial_source.clone(),
                settings: EditRecipeSettings {
                    exposure_ev: 1.0,
                    white_balance: WhiteBalanceIntent::AsShot,
                },
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(request_conflict, EditRecipeWriteOutcome::RequestConflict);

        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE original_files SET size=18,mtime_ms=2_000.0 WHERE id='raw-original'",
                [],
            )
            .unwrap();
        drop(connection);
        let changed_source = source_revision("shoot/one.ARW", 18, 2_000.0).unwrap();
        let stale_save = persistence
            .save_edit_recipe_receiver(SaveEditRecipe {
                photo_id: "raw-photo".to_owned(),
                request_id: "stale-save".to_owned(),
                expected_recipe_version: Some(recipe.revision.clone()),
                expected_source_revision: initial_source.clone(),
                settings: EditRecipeSettings {
                    exposure_ev: 1.0,
                    white_balance: WhiteBalanceIntent::AsShot,
                },
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            stale_save,
            EditRecipeWriteOutcome::SourceChanged(EditRecipeRead {
                recipe: Some(_),
                ..
            })
        ));
        let rebound = persistence
            .rebind_edit_recipe_receiver(RebindEditRecipe {
                photo_id: "raw-photo".to_owned(),
                request_id: "rebind-1".to_owned(),
                expected_recipe_version: recipe.revision.clone(),
                new_source_revision: changed_source.clone(),
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let rebound = match rebound {
            EditRecipeWriteOutcome::Saved(recipe) => recipe,
            outcome => panic!("explicit rebind should save, got {outcome:?}"),
        };
        assert_ne!(rebound.revision, recipe.revision);
        assert_eq!(rebound.source_revision, changed_source);
        assert_eq!(rebound.settings, recipe.settings);

        // A replay stays exact even after another write advanced the recipe:
        // persistence decides the outcome inside the write transaction, so
        // the receipt's version is reported whatever happened since.
        persistence
            .save_edit_recipe_receiver(SaveEditRecipe {
                photo_id: "raw-photo".to_owned(),
                request_id: "advance-save".to_owned(),
                expected_recipe_version: Some(rebound.revision.clone()),
                expected_source_revision: rebound.source_revision.clone(),
                settings: EditRecipeSettings {
                    exposure_ev: 0.5,
                    white_balance: WhiteBalanceIntent::AsShot,
                },
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let stale_replay = persistence
            .save_edit_recipe_receiver(SaveEditRecipe {
                photo_id: "raw-photo".to_owned(),
                request_id: "first-save".to_owned(),
                expected_recipe_version: None,
                expected_source_revision: initial_source.clone(),
                settings: EditRecipeSettings {
                    exposure_ev: 0.0,
                    white_balance: WhiteBalanceIntent::AsShot,
                },
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stale_replay,
            EditRecipeWriteOutcome::Replayed(recipe.clone()),
            "the replay must carry the receipt's version, not the current one"
        );

        // The rebind identity replays exactly like a save identity: the same
        // payload replays the receipt, a different payload is refused.
        let rebind_replay = persistence
            .rebind_edit_recipe_receiver(RebindEditRecipe {
                photo_id: "raw-photo".to_owned(),
                request_id: "rebind-1".to_owned(),
                expected_recipe_version: recipe.revision.clone(),
                new_source_revision: changed_source.clone(),
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            rebind_replay,
            EditRecipeWriteOutcome::Replayed(rebound.clone())
        );
        let rebind_conflict = persistence
            .rebind_edit_recipe_receiver(RebindEditRecipe {
                photo_id: "raw-photo".to_owned(),
                request_id: "rebind-1".to_owned(),
                expected_recipe_version: rebound.revision.clone(),
                new_source_revision: rebound.source_revision.clone(),
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rebind_conflict, EditRecipeWriteOutcome::RequestConflict);
        assert!(matches!(
            persistence
                .save_edit_recipe_receiver(SaveEditRecipe {
                    photo_id: "raw-photo".to_owned(),
                    request_id: "after-rebind-save".to_owned(),
                    expected_recipe_version: Some(recipe.revision),
                    expected_source_revision: changed_source,
                    settings: EditRecipeSettings {
                        exposure_ev: 2.0,
                        white_balance: WhiteBalanceIntent::AsShot,
                    },
                })
                .unwrap()
                .await
                .unwrap()
                .unwrap(),
            EditRecipeWriteOutcome::Conflict(_)
        ));
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE original_files SET available=0 WHERE id='raw-original'",
                [],
            )
            .unwrap();
        connection
            .execute("UPDATE photos SET available=0 WHERE id='raw-photo'", [])
            .unwrap();
        drop(connection);
        let photo = persistence
            .photo_receiver("raw-photo")
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!photo.original_available);
        assert!(photo.has_saved_edits);
        let edit_read = persistence
            .edit_recipe_receiver("raw-photo")
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!edit_read.source_available);
        assert!(persistence.snapshot().await.unwrap().photos[0].has_saved_edits);
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn pre_upgrade_receipts_replay_with_the_original_digest_formula() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v8.sql"),
        );
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
                [library.canonical_path().to_str().unwrap()],
            )
            .unwrap();

        // A receipt written by a release before the internal recipe-version
        // rename: its digest covers the original serialized key set, and its
        // shape carries no temperature or tint values because the as-shot
        // intent predates the value-carrying columns.
        let source_revision = "legacy-source-revision";
        let legacy_payload = serde_json::json!({
            "photo_id": "raw-photo",
            "expected_recipe_revision": serde_json::Value::Null,
            "expected_source_revision": source_revision,
            "exposure_ev": 0.25,
            "white_balance": "as-shot",
        });
        let legacy_digest = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&legacy_payload).unwrap())
        );
        let legacy_receipt = serde_json::json!({
            "photo_id": "raw-photo",
            "payload_digest": legacy_digest,
            "outcome": "Saved",
            "revision": "legacy-recipe-version",
            "source_revision": source_revision,
            "exposure_ev": 0.25,
            "white_balance_mode": "as-shot",
        });
        connection
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('edit_recipe_receipt:legacy-save',?)",
                [serde_json::to_string(&legacy_receipt).unwrap()],
            )
            .unwrap();
        drop(connection);

        // The same logical save retried after the upgrade must replay the
        // committed receipt, not refuse the identity as conflicted.
        let persistence = Persistence::open(
            state,
            name.clone(),
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let replay = persistence
            .save_edit_recipe_receiver(SaveEditRecipe {
                photo_id: "raw-photo".to_owned(),
                request_id: "legacy-save".to_owned(),
                expected_recipe_version: None,
                expected_source_revision: source_revision.to_owned(),
                settings: EditRecipeSettings {
                    exposure_ev: 0.25,
                    white_balance: WhiteBalanceIntent::AsShot,
                },
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            replay,
            EditRecipeWriteOutcome::Replayed(EditRecipe {
                photo_id: "raw-photo".to_owned(),
                revision: "legacy-recipe-version".to_owned(),
                source_revision: source_revision.to_owned(),
                settings: EditRecipeSettings {
                    exposure_ev: 0.25,
                    white_balance: WhiteBalanceIntent::AsShot,
                },
            }),
            "a pre-upgrade receipt must keep replaying with the committed version"
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn edit_recipe_writes_reject_missing_unavailable_and_non_raw_photos() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v6.sql"),
        );
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
                [library.canonical_path().to_str().unwrap()],
            )
            .unwrap();
        add_recipe_test_photo(
            &connection,
            RecipeTestPhoto {
                original_id: "missing-raw-original",
                photo_id: "missing-raw-photo",
                relative_path: "shoot/missing.ARW",
                kind: "raw",
                available: false,
                size: 13,
                mtime_ms: 1_000.0,
            },
        );
        add_recipe_test_photo(
            &connection,
            RecipeTestPhoto {
                original_id: "jpeg-original",
                photo_id: "jpeg-photo",
                relative_path: "shoot/one.JPG",
                kind: "jpeg",
                available: true,
                size: 11,
                mtime_ms: 2_000.0,
            },
        );
        drop(connection);
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();

        let unavailable = persistence
            .save_edit_recipe_receiver(SaveEditRecipe {
                photo_id: "missing-raw-photo".to_owned(),
                request_id: "unavailable-save".to_owned(),
                expected_recipe_version: None,
                expected_source_revision: source_revision("shoot/missing.ARW", 13, 1_000.0)
                    .unwrap(),
                settings: EditRecipeSettings {
                    exposure_ev: 0.0,
                    white_balance: WhiteBalanceIntent::AsShot,
                },
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(unavailable, EditRecipeWriteOutcome::Unavailable);

        let unsupported = persistence
            .save_edit_recipe_receiver(SaveEditRecipe {
                photo_id: "jpeg-photo".to_owned(),
                request_id: "jpeg-save".to_owned(),
                expected_recipe_version: None,
                expected_source_revision: source_revision("shoot/one.JPG", 11, 2_000.0).unwrap(),
                settings: EditRecipeSettings {
                    exposure_ev: 0.0,
                    white_balance: WhiteBalanceIntent::AsShot,
                },
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(unsupported, EditRecipeWriteOutcome::UnsupportedPhoto);

        let missing = persistence
            .save_edit_recipe_receiver(SaveEditRecipe {
                photo_id: "not-a-photo".to_owned(),
                request_id: "missing-save".to_owned(),
                expected_recipe_version: None,
                expected_source_revision: "source-revision".to_owned(),
                settings: EditRecipeSettings {
                    exposure_ev: 0.0,
                    white_balance: WhiteBalanceIntent::AsShot,
                },
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(missing, EditRecipeWriteOutcome::MissingPhoto);
        persistence.shutdown().unwrap();
    }

    #[test]
    fn retire_and_bind_refuses_a_photo_with_saved_recipe() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v7.sql"),
        );
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
                [library.canonical_path().to_str().unwrap()],
            )
            .unwrap();
        add_recipe_test_photo(
            &connection,
            RecipeTestPhoto {
                original_id: "missing-original",
                photo_id: "missing-photo",
                relative_path: "shoot/missing.ARW",
                kind: "raw",
                available: false,
                size: 11,
                mtime_ms: 1_000.0,
            },
        );
        add_recipe_test_photo(
            &connection,
            RecipeTestPhoto {
                original_id: "occupant-original",
                photo_id: "occupant-photo",
                relative_path: "moved/occupied.ARW",
                kind: "raw",
                available: true,
                size: 19,
                mtime_ms: 2_000.0,
            },
        );
        connection
            .execute(
                "INSERT INTO edit_recipes(photo_id,revision,source_revision,exposure_ev,white_balance_mode)
                 VALUES(?,?,?,?, 'as-shot')",
                params![
                    "occupant-photo",
                    "recipe-revision",
                    source_revision("moved/occupied.ARW", 19, 2_000.0).unwrap(),
                    0.0_f64
                ],
            )
            .unwrap();

        let result = apply_manual_relocations(
            &state,
            &name,
            &mut Connection::open(&path).unwrap(),
            &[RequestedRelocation {
                original_id: "missing-original".to_owned(),
                to_location: "moved/occupied.ARW".to_owned(),
                facts: crate::OriginalFacts {
                    size: 19,
                    mtime_ms: 2_000.0,
                    device: 0,
                    inode: 0,
                },
                retire_destination: true,
            }],
        );
        assert!(matches!(
            result,
            Err(PersistenceError::InvalidRecoveryMapping {
                reason: "occupied",
                ..
            })
        ));

        let connection = Connection::open(path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM photos WHERE id IN ('missing-photo','occupant-photo')",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM edit_recipes WHERE photo_id='occupant-photo'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
    }

    // album-language-legacy:start v4-migration-test
    #[tokio::test]
    async fn v4_migration_preserves_album_state_through_current_schema() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v4.sql"),
        );
        let first = "00000000-0000-4000-8000-000000000031";
        let second = "00000000-0000-4000-8000-000000000032";
        let photo_one = "photo-one";
        let photo_two = "photo-two";
        let connection = Connection::open(&path).unwrap();
        for (id, path_text, sort_path) in [
            ("original-one", "one.JPG", "one.JPG"),
            ("original-two", "two.JPG", "two.JPG"),
        ] {
            connection
                .execute(
                    "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,capture_metadata_state) VALUES(?,?, 'jpeg',9,1.0,1,'pending')",
                    params![id, path_text],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO photos(id,jpeg_original_id,ambiguous,available,preview_state,sort_path,selection_state,rating) VALUES(?,?,0,1,'inspection-pending',?,'undecided',0)",
                    params![sort_path.replace(".JPG", ""), id, sort_path],
                )
                .unwrap();
        }
        connection
            .execute(
                "UPDATE photos SET id=? WHERE jpeg_original_id='original-one'",
                [photo_one],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE photos SET id=? WHERE jpeg_original_id='original-two'",
                [photo_two],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO photo_sets(id,name,created_at) VALUES(?,?,?)",
                params![first, "Shoot", 7_i64],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO photo_sets(id,name,created_at) VALUES(?,?,?)",
                params![second, "Client", 9_i64],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO photo_set_members(photo_set_id,photo_id,position) VALUES(?,?,1)",
                params![first, photo_two],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO photo_set_members(photo_set_id,photo_id,position) VALUES(?,?,0)",
                params![first, photo_one],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO review_progress(photo_set_id,photo_id) VALUES(?,?)",
                params![first, photo_two],
            )
            .unwrap();
        drop(connection);
        let persistence = Persistence::open(
            state,
            name.clone(),
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let albums = persistence.list_albums().await.unwrap();
        assert_eq!(albums.len(), 2);
        assert_eq!(albums[0].id, first);
        assert_eq!(albums[0].name, "Shoot");
        assert_eq!(albums[1].id, second);
        assert_eq!(albums[1].name, "Client");
        assert_eq!(albums[1].members.len(), 0);
        assert_eq!(albums[1].last_reviewed_photo_id, None);
        let members = &albums[0].members;
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].photo_id, photo_one);
        assert_eq!(members[0].position, 0);
        assert_eq!(members[1].photo_id, photo_two);
        assert_eq!(members[1].position, 1);
        assert_eq!(albums[0].last_reviewed_photo_id.as_deref(), Some(photo_two));
        persistence.shutdown().unwrap();
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                .unwrap(),
            9
        );
        validate_canonical_schema(&connection, SchemaVersion::V9).unwrap();
        // The legacy photo-set tables are gone rather than left as aliases.
        for legacy in ["photo_sets", "photo_set_members", "review_progress"] {
            assert!(!table_exists(&connection, legacy).unwrap(), "{legacy}");
        }
    }
    // album-language-legacy:end v4-migration-test

    #[test]
    fn newer_v10_database_is_rejected_without_changes() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v5.sql"),
        );
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", 10)
            .unwrap();
        let before = fs::read(&path).unwrap();
        assert!(matches!(
            Persistence::open(
                state,
                name,
                library.canonical_path().to_str().unwrap().to_owned(),
            ),
            Err(PersistenceError::NewerSchema)
        ));
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn legacy_v4_binary_fence_rejects_canonical_v5_without_changes() {
        let (_base, library, _state, _name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v5.sql"),
        );
        let connection = Connection::open(&path).unwrap();
        validate_canonical_schema(&connection, SchemaVersion::V5).unwrap();

        let sidecars = ["-journal", "-wal", "-shm"]
            .map(|suffix| path.with_file_name(format!("library.sqlite{suffix}")));
        let persisted_paths = std::iter::once(path.clone())
            .chain(sidecars.iter().cloned())
            .collect::<Vec<_>>();
        let before = persisted_paths
            .iter()
            .map(|path| fs::read(path).ok())
            .collect::<Vec<_>>();

        assert!(matches!(
            preflight_schema_for_max_version(
                &connection,
                library.canonical_path().to_str().unwrap(),
                4,
            ),
            Err(PersistenceError::NewerSchema)
        ));

        let after = persisted_paths
            .iter()
            .map(|path| fs::read(path).ok())
            .collect::<Vec<_>>();
        assert_eq!(after, before);
    }

    #[test]
    fn legacy_v3_binary_fence_rejects_canonical_v4() {
        let (_base, library, _state, _name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v4.sql"),
        );
        let connection = Connection::open(path).unwrap();
        assert!(matches!(
            preflight_schema_for_max_version(
                &connection,
                library.canonical_path().to_str().unwrap(),
                3,
            ),
            Err(PersistenceError::NewerSchema)
        ));
    }

    #[tokio::test]
    async fn migrates_shared_v0_and_v1_to_current_schema_and_rejects_malformed_v2() {
        for sql in [
            include_str!("../../../../compatibility/sqlite/v0.sql"),
            include_str!("../../../../compatibility/sqlite/v1.sql"),
        ] {
            let (_base, library, state, name, path) = fixture();
            seed(&path, sql);
            let persistence = Persistence::open(
                state,
                name,
                library.canonical_path().to_string_lossy().into_owned(),
            )
            .unwrap();
            persistence.shutdown().unwrap();
            let connection = Connection::open(path).unwrap();
            validate_canonical_schema(&connection, SchemaVersion::V9).unwrap();
        }
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/malformed-v2.sql"),
        );
        assert!(matches!(
            Persistence::open(
                state,
                name,
                library.canonical_path().to_string_lossy().into_owned()
            ),
            Err(PersistenceError::UnsupportedSchema)
        ));
        assert_eq!(
            Connection::open(path)
                .unwrap()
                .pragma_query_value::<u8, _>(None, "user_version", |row| row.get(0))
                .unwrap(),
            2
        );
    }

    #[test]
    fn shared_rejection_fixtures_are_rejected_without_database_changes() {
        let fixtures: Vec<RejectionFixture> = serde_json::from_str(include_str!(
            "../../../../compatibility/sqlite/rejections.json"
        ))
        .unwrap();
        for rejection in fixtures {
            let (_base, library, state, name, path) = fixture();
            seed(&path, &rejection.sql);
            let before = fs::read(&path).unwrap();
            let result = Persistence::open(
                state,
                name,
                library.canonical_path().to_str().unwrap().to_owned(),
            );
            assert!(
                matches!(
                    result,
                    Err(PersistenceError::UnsupportedSchema | PersistenceError::InvalidLegacyData)
                ),
                "{} expected {}",
                rejection.name,
                rejection.expected_error
            );
            assert_eq!(fs::read(&path).unwrap(), before, "{}", rejection.name);
            assert_eq!(
                Connection::open(&path)
                    .unwrap()
                    .pragma_query_value::<u32, _>(None, "user_version", |row| row.get(0))
                    .unwrap(),
                rejection.version,
                "{}",
                rejection.name
            );
        }
    }

    // album-language-legacy:start v3-migration-test
    #[tokio::test]
    async fn v3_migration_preserves_every_row_identity_and_user_owned_state() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v3.sql"),
        );
        let original_id = original_id("shoot/A.JPG");
        let photo_id = "photo-preserved";
        let legacy_album_id = "00000000-0000-4000-8000-000000000027";
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO original_files VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
                params![
                    original_id,
                    "shoot/A.JPG",
                    "jpeg",
                    12_i64,
                    1_000.0_f64,
                    1_i64,
                    Option::<String>::None,
                    Option::<String>::None,
                    "known",
                    "2026-01-01T10:00:00.000000000",
                    "date-time-original",
                    60_i64,
                    "capture-revision"
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO photos(id,jpeg_original_id,ambiguous,available,preview_state,preview_candidate,preview_source,preview_source_revision,preview_width,preview_height,cache_revision,sort_path,selection_state,rating) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                params![photo_id, original_id, 0_i64, 1_i64, "ready", "matching-jpeg", "matching-jpeg", source_revision("shoot/A.JPG", 12, 1000.0).unwrap(), 8_i64, 4_i64, "cache-revision", "shoot/A.JPG", "selected", 5_i64],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO photo_sets(id,name,created_at) VALUES(?,?,?)",
                params![legacy_album_id, "Preserved", 1_i64],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO photo_set_members(photo_set_id,photo_id,position) VALUES(?,?,?)",
                params![legacy_album_id, photo_id, 0_i64],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO review_progress(photo_set_id,photo_id) VALUES(?,?)",
                params![legacy_album_id, photo_id],
            )
            .unwrap();
        drop(connection);
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence.snapshot().await.unwrap();
        let photo = &snapshot.photos[0];
        assert_eq!(photo.id, photo_id);
        assert_eq!(photo.original_id, original_id);
        assert!(photo.available);
        assert_eq!(photo.preview_state, PreviewState::Ready);
        assert_eq!(
            photo.preview_source_revision.as_deref(),
            Some(source_revision("shoot/A.JPG", 12, 1000.0).unwrap().as_str())
        );
        assert_eq!(photo.preview_width, Some(8));
        assert_eq!(photo.preview_height, Some(4));
        assert_eq!(photo.cache_revision.as_deref(), Some("cache-revision"));
        assert_eq!(photo.sort_path, "shoot/A.JPG");
        assert_eq!(photo.selection_state, SelectionState::Selected);
        assert_eq!(photo.rating, 5);
        let original = &snapshot.originals[0];
        assert_eq!(original.id, original_id);
        assert_eq!(original.relative_path.as_str(), "shoot/A.JPG");
        assert_eq!(original.kind, OriginalKind::Jpeg);
        assert_eq!(original.facts.size, 12);
        assert_eq!(original.facts.mtime_ms, 1_000.0);
        assert!(original.available);
        assert_eq!(original.error_category, None);
        assert_eq!(original.error_message, None);
        assert_eq!(
            original.capture,
            CaptureFact {
                state: CaptureMetadataState::Known,
                order_key: Some("2026-01-01T10:00:00.000000000".to_owned()),
                field: Some(CaptureTimeField::DateTimeOriginal),
                offset_minutes: Some(60),
                source_revision: Some("capture-revision".to_owned()),
            }
        );
        let album = persistence.list_albums().await.unwrap().remove(0);
        assert_eq!(album.id, legacy_album_id);
        assert_eq!(album.name, "Preserved");
        assert_eq!(album.members.len(), 1);
        assert_eq!(album.members[0].photo_id, photo_id);
        assert_eq!(album.members[0].position, 0);
        assert!(album.members[0].available);
        assert_eq!(album.members[0].selection_state, SelectionState::Selected);
        assert_eq!(album.members[0].rating, 5);
        assert_eq!(album.last_reviewed_photo_id.as_deref(), Some(photo_id));
        persistence.shutdown().unwrap();
        let connection = Connection::open(path).unwrap();
        validate_canonical_schema(&connection, SchemaVersion::V9).unwrap();
    }
    // album-language-legacy:end v3-migration-test

    #[tokio::test]
    async fn every_present_sidecar_rejects_startup_before_creating_database() {
        for suffix in ["-journal", "-wal", "-shm"] {
            let (_base, library, state, name, path) = fixture();
            let sidecar = path.with_file_name(format!("library.sqlite{suffix}"));
            fs::write(&sidecar, b"operator recovery data").unwrap();
            fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o600)).unwrap();
            let result = Persistence::open(
                state,
                name,
                library.canonical_path().to_str().unwrap().to_owned(),
            );
            assert!(matches!(result, Err(PersistenceError::RecoveryRequired)));
            assert!(
                !path.exists(),
                "{suffix} must be checked before database creation"
            );
            assert_eq!(fs::read(sidecar).unwrap(), b"operator recovery data");
        }
    }

    #[tokio::test]
    async fn every_present_sidecar_blocks_writes_without_changing_database() {
        for suffix in ["-journal", "-wal", "-shm"] {
            let (_base, library, state, name, path) = fixture();
            let persistence = Persistence::open(
                state,
                name,
                library.canonical_path().to_str().unwrap().to_owned(),
            )
            .unwrap();
            let sidecar = path.with_file_name(format!("library.sqlite{suffix}"));
            fs::write(&sidecar, b"operator recovery data").unwrap();
            fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o600)).unwrap();
            let before = fs::read(&path).unwrap();
            assert_eq!(
                persistence
                    .mutate_album(AlbumMutation::Create {
                        name: format!("Blocked {suffix}"),
                    })
                    .await,
                Err(MutationError::Persistence)
            );
            assert_eq!(
                fs::read(&path).unwrap(),
                before,
                "{suffix} changed database"
            );
            assert_eq!(fs::read(&sidecar).unwrap(), b"operator recovery data");
            persistence.shutdown().unwrap();
        }
    }

    #[tokio::test]
    async fn malformed_v2_wal_rejection_preserves_database_and_sidecars() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/malformed-v2.sql"),
        );
        let connection = Connection::open(&path).unwrap();
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .unwrap();
        connection
            .pragma_update(None, "wal_autocheckpoint", 0)
            .unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata VALUES('wal_probe','unchanged')",
                [],
            )
            .unwrap();
        let paths = [
            path.clone(),
            path.with_file_name("library.sqlite-wal"),
            path.with_file_name("library.sqlite-shm"),
        ];
        let before = paths
            .iter()
            .map(|path| fs::read(path).ok())
            .collect::<Vec<_>>();
        let result = Persistence::open(
            state,
            name,
            library.canonical_path().to_str().unwrap().to_owned(),
        );
        assert!(matches!(result, Err(PersistenceError::RecoveryRequired)));
        let after = paths
            .iter()
            .map(|path| fs::read(path).ok())
            .collect::<Vec<_>>();
        assert_eq!(after, before);
        drop(connection);
    }

    #[tokio::test]
    async fn root_mismatch_rejects_before_migration_without_changes() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v1.sql"),
        );
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata VALUES('canonical_root','/different')",
                [],
            )
            .unwrap();
        drop(connection);
        let before = fs::read(&path).unwrap();
        assert!(matches!(
            Persistence::open(
                state,
                name,
                library.canonical_path().to_string_lossy().into_owned()
            ),
            Err(PersistenceError::RootMismatch)
        ));
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[tokio::test]
    async fn wal_only_root_binding_is_rejected_without_changing_database_or_sidecars() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v2.sql"),
        );
        let connection = Connection::open(&path).unwrap();
        connection
            .pragma_update(None, "journal_mode", "WAL")
            .unwrap();
        connection
            .pragma_update(None, "wal_autocheckpoint", 0)
            .unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata VALUES('canonical_root','/different')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata VALUES('wal_probe','unchanged')",
                [],
            )
            .unwrap();
        let paths = [
            path.clone(),
            path.with_file_name("library.sqlite-wal"),
            path.with_file_name("library.sqlite-shm"),
        ];
        let before = paths
            .iter()
            .map(|path| fs::read(path).ok())
            .collect::<Vec<_>>();
        let open_result = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        );
        assert!(matches!(
            open_result,
            Err(PersistenceError::RecoveryRequired)
        ));
        let after = paths
            .iter()
            .map(|path| fs::read(path).ok())
            .collect::<Vec<_>>();
        assert_eq!(after, before);
        assert_eq!(
            connection
                .query_row(
                    "SELECT value FROM library_metadata WHERE key='wal_probe'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "unchanged"
        );
        drop(connection);
    }

    fn discovered(path: &str, kind: OriginalKind, size: u64, mtime_ms: f64) -> DiscoveredOriginal {
        DiscoveredOriginal {
            path: crate::RelativeOriginalPath::parse(path).unwrap(),
            kind,
            facts: OriginalFacts {
                size,
                mtime_ms,
                device: 1,
                inode: 1,
            },
            error_category: None,
            error_message: None,
            capture: CaptureFact::pending(),
        }
    }

    fn query_projection(snapshot: &ScanSnapshot) -> Arc<PhotoQueryProjection> {
        let originals = snapshot
            .originals
            .iter()
            .map(|original| (original.id.as_str(), original))
            .collect::<HashMap<_, _>>();
        let candidates = snapshot
            .photos
            .iter()
            .map(|photo| {
                let original = originals[photo.original_id.as_str()];
                PhotoQueryCandidate {
                    photo_id: photo.id.clone(),
                    relative_path: original.relative_path.as_str().to_owned(),
                    sort_path: photo.sort_path.clone(),
                    original_kind: original.kind,
                    original_available: original.available,
                    capture: original.capture.clone(),
                    preview_state: photo.preview_state,
                    preview_source_revision: photo.preview_source_revision.clone(),
                    preview_width: photo.preview_width,
                    preview_height: photo.preview_height,
                }
            })
            .collect::<Vec<_>>();
        let mut descending = (0..candidates.len()).collect::<Vec<_>>();
        descending.sort_by(|a, b| {
            let a = &candidates[*a];
            let b = &candidates[*b];
            a.capture_order_key()
                .is_none()
                .cmp(&b.capture_order_key().is_none())
                .then_with(|| match (a.capture_order_key(), b.capture_order_key()) {
                    (Some(a), Some(b)) => b.cmp(a),
                    _ => std::cmp::Ordering::Equal,
                })
                .then_with(|| a.sort_path.cmp(&b.sort_path))
                .then_with(|| a.photo_id.cmp(&b.photo_id))
        });
        Arc::new(PhotoQueryProjection::new(candidates, descending).unwrap())
    }

    #[tokio::test]
    async fn bounded_photo_queries_fix_ordered_membership_and_read_current_facts() {
        let (_base, library, state, name, _path) = fixture();
        let mut early = discovered("shoot/early.JPG", OriginalKind::Jpeg, 1, 1.0);
        early.capture = CaptureFact {
            state: CaptureMetadataState::Known,
            order_key: Some("2026-01-01T09:00:00.000000000".to_owned()),
            field: Some(CaptureTimeField::DateTimeOriginal),
            offset_minutes: Some(90),
            source_revision: Some("early-revision".to_owned()),
        };
        let mut late = discovered("shoot/nested/late.RAF", OriginalKind::Raw, 2, 2.0);
        late.capture = CaptureFact {
            state: CaptureMetadataState::Known,
            order_key: Some("2026-01-01T10:00:00.000000000".to_owned()),
            field: Some(CaptureTimeField::DateTimeOriginal),
            offset_minutes: None,
            source_revision: Some("late-revision".to_owned()),
        };
        let missing_time = discovered("other/missing.JPG", OriginalKind::Jpeg, 3, 3.0);
        let upper = discovered("Shoot/upper.JPG", OriginalKind::Jpeg, 4, 4.0);
        let short = discovered("a/one.JPG", OriginalKind::Jpeg, 5, 5.0);
        let sibling = discovered("ab/two.JPG", OriginalKind::Jpeg, 6, 6.0);
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![late, missing_time, early, upper, short, sibling],
                Vec::new(),
            )
            .await
            .unwrap();
        let projection = query_projection(&snapshot);
        let by_path = snapshot
            .photos
            .iter()
            .map(|photo| (photo.sort_path.as_str(), photo.id.clone()))
            .collect::<HashMap<_, _>>();
        let early_id = by_path["shoot/early.JPG"].clone();
        let late_id = by_path["shoot/nested/late.RAF"].clone();
        let album_id = persistence
            .mutate_album(AlbumMutation::Create {
                name: "Order".to_owned(),
            })
            .await
            .unwrap()
            .album_id;
        persistence
            .mutate_album(AlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: vec![late_id.clone(), early_id.clone()],
            })
            .await
            .unwrap();
        let album_order = persistence
            .create_photo_query_receiver(
                PhotoQuery {
                    source: PhotoQuerySource::Album(album_id),
                    selection_state: None,
                    rating_minimum: None,
                    rating_maximum: None,
                    original_kind: None,
                    original_available: None,
                    captured_from: None,
                    captured_before: None,
                    order: PhotoQueryOrder::AlbumOrder,
                },
                Arc::clone(&projection),
                10,
            )
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(album_order, vec![late_id.clone(), early_id.clone()]);

        persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: early_id.clone(),
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Selected),
                expected_current: None,
                album_id: None,
            })
            .await
            .unwrap();
        persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: early_id.clone(),
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(4),
                expected_current: None,
                album_id: None,
            })
            .await
            .unwrap();

        let query = PhotoQuery {
            source: PhotoQuerySource::Folder("shoot".to_owned()),
            selection_state: Some(SelectionState::Selected),
            rating_minimum: Some(4),
            rating_maximum: Some(5),
            original_kind: Some(OriginalKind::Jpeg),
            original_available: Some(true),
            captured_from: Some(CaptureTimeBound::parse("2026-01-01T08:00:00").unwrap()),
            captured_before: Some(CaptureTimeBound::parse("2026-01-01T10:00:00").unwrap()),
            order: PhotoQueryOrder::CaptureTimeAscending,
        };
        let ids = persistence
            .create_photo_query_receiver(query, Arc::clone(&projection), 10)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ids, vec![early_id.clone()]);
        let narrow = persistence
            .create_photo_query_receiver(
                PhotoQuery {
                    source: PhotoQuerySource::AllPhotos,
                    selection_state: Some(SelectionState::Selected),
                    rating_minimum: Some(4),
                    rating_maximum: None,
                    original_kind: None,
                    original_available: None,
                    captured_from: None,
                    captured_before: None,
                    order: PhotoQueryOrder::CaptureTimeAscending,
                },
                Arc::clone(&projection),
                1,
            )
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(narrow, vec![early_id.clone()]);
        for (folder, expected_path) in [("Shoot", "Shoot/upper.JPG"), ("a", "a/one.JPG")] {
            let ids = persistence
                .create_photo_query_receiver(
                    PhotoQuery {
                        source: PhotoQuerySource::Folder(folder.to_owned()),
                        selection_state: None,
                        rating_minimum: None,
                        rating_maximum: None,
                        original_kind: None,
                        original_available: None,
                        captured_from: None,
                        captured_before: None,
                        order: PhotoQueryOrder::CaptureTimeAscending,
                    },
                    Arc::clone(&projection),
                    10,
                )
                .unwrap()
                .await
                .unwrap()
                .unwrap();
            assert_eq!(ids, vec![by_path[expected_path].clone()]);
        }
        assert!(matches!(
            persistence
                .create_photo_query_receiver(
                    PhotoQuery {
                        source: PhotoQuerySource::Folder("SHOOT".to_owned()),
                        selection_state: None,
                        rating_minimum: None,
                        rating_maximum: None,
                        original_kind: None,
                        original_available: None,
                        captured_from: None,
                        captured_before: None,
                        order: PhotoQueryOrder::CaptureTimeAscending,
                    },
                    Arc::clone(&projection),
                    10,
                )
                .unwrap()
                .await
                .unwrap(),
            Err(PhotoQueryError::SourceNotFound)
        ));
        // Query membership is fixed, while a later owner read returns current
        // facts even when the Photo no longer matches the creation filter.
        persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: early_id.clone(),
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Rejected),
                expected_current: None,
                album_id: None,
            })
            .await
            .unwrap();
        assert!(matches!(
            persistence
                .create_photo_query_receiver(
                    PhotoQuery {
                        source: PhotoQuerySource::Folder("missing".to_owned()),
                        selection_state: None,
                        rating_minimum: None,
                        rating_maximum: None,
                        original_kind: None,
                        original_available: None,
                        captured_from: None,
                        captured_before: None,
                        order: PhotoQueryOrder::CaptureTimeAscending,
                    },
                    Arc::clone(&projection),
                    10,
                )
                .unwrap()
                .await
                .unwrap(),
            Err(PhotoQueryError::SourceNotFound)
        ));
        assert!(matches!(
            persistence
                .create_photo_query_receiver(
                    PhotoQuery {
                        source: PhotoQuerySource::AllPhotos,
                        selection_state: None,
                        rating_minimum: None,
                        rating_maximum: None,
                        original_kind: None,
                        original_available: None,
                        captured_from: None,
                        captured_before: None,
                        order: PhotoQueryOrder::CaptureTimeDescending,
                    },
                    Arc::clone(&projection),
                    2,
                )
                .unwrap()
                .await
                .unwrap(),
            Err(PhotoQueryError::ResultLimitExceeded { limit: 2 })
        ));

        let current = persistence
            .photos_by_id_receiver(
                vec![late_id, early_id.clone(), "removed".to_owned()],
                Arc::clone(&projection),
            )
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.len(), 3);
        assert_eq!(current[1].as_ref().unwrap().filename, "early.JPG");
        assert_eq!(current[1].as_ref().unwrap().rating, 4);
        assert_eq!(
            current[1].as_ref().unwrap().selection_state,
            SelectionState::Rejected
        );
        assert_eq!(
            current[1].as_ref().unwrap().capture.offset_minutes,
            Some(90)
        );
        assert!(current[2].is_none());
    }

    #[tokio::test]
    async fn effective_writers_advance_versions_while_noops_and_progress_do_not() {
        let (_base, library, state, name, path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0)],
                Vec::new(),
            )
            .await
            .unwrap();
        let photo_id = snapshot.photos[0].id.clone();
        let read_photo = || async {
            persistence
                .photo_receiver(&photo_id)
                .unwrap()
                .await
                .unwrap()
                .unwrap()
                .unwrap()
        };
        let initial_photo_version = read_photo().await.decision_version;
        persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: photo_id.clone(),
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(0),
                expected_current: None,
                album_id: None,
            })
            .await
            .unwrap();
        assert_eq!(read_photo().await.decision_version, initial_photo_version);
        persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: photo_id.clone(),
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Selected),
                expected_current: None,
                album_id: None,
            })
            .await
            .unwrap();
        let changed_photo_version = read_photo().await.decision_version;
        assert_ne!(changed_photo_version, initial_photo_version);
        persistence
            .mutate_photo_state_batch_receiver(PhotoStateBatchMutation {
                photos: vec![PhotoStateBatchItem {
                    photo_id: photo_id.clone(),
                    expected_current: SelectionState::Selected,
                }],
                value: SelectionState::Selected,
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(read_photo().await.decision_version, changed_photo_version);
        assert!(
            persistence
                .mutate_photo_state(PhotoStateMutation {
                    photo_id: photo_id.clone(),
                    field: PhotoStateField::SelectionState,
                    value: PhotoStateValue::Selection(SelectionState::Rejected),
                    expected_current: Some(PhotoStateValue::Selection(SelectionState::Undecided)),
                    album_id: None,
                })
                .await
                .is_err()
        );
        assert_eq!(read_photo().await.decision_version, changed_photo_version);
        persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: photo_id.clone(),
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Undecided),
                expected_current: Some(PhotoStateValue::Selection(SelectionState::Selected)),
                album_id: None,
            })
            .await
            .unwrap();
        let changed_back_photo_version = read_photo().await.decision_version;
        assert_ne!(changed_back_photo_version, initial_photo_version);
        assert_ne!(changed_back_photo_version, changed_photo_version);

        let album_id = persistence
            .mutate_album(AlbumMutation::Create {
                name: "Review".to_owned(),
            })
            .await
            .unwrap()
            .album_id;
        let read_album = || async {
            persistence
                .album_receiver(&album_id)
                .unwrap()
                .await
                .unwrap()
                .unwrap()
                .unwrap()
        };
        let initial_album_version = read_album().await.album_version;
        let named_ids = persistence
            .create_album_query_receiver(AlbumQueryFilter::ExactName("review".to_owned()), 10)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(named_ids, vec![album_id.clone()]);
        persistence
            .mutate_album_membership(AlbumMembershipMutation::Add {
                album_id: album_id.clone(),
                photo_ids: vec![photo_id.clone()],
            })
            .await
            .unwrap();
        let membership_version = read_album().await.album_version;
        assert_ne!(membership_version, initial_album_version);
        let containing_ids = persistence
            .create_album_query_receiver(AlbumQueryFilter::ContainsPhoto(photo_id.clone()), 10)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(containing_ids, vec![album_id.clone()]);
        let album_window = persistence
            .albums_by_id_receiver(vec![album_id.clone(), "removed".to_owned()])
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(album_window[0].as_ref().unwrap().photo_count, 1);
        assert!(album_window[1].is_none());
        persistence
            .mutate_album(AlbumMutation::SetProgress {
                album_id: album_id.clone(),
                photo_id: photo_id.clone(),
            })
            .await
            .unwrap();
        assert_eq!(read_album().await.album_version, membership_version);
        persistence
            .mutate_album(AlbumMutation::Rename {
                album_id: album_id.clone(),
                name: "Review".to_owned(),
            })
            .await
            .unwrap();
        assert_eq!(read_album().await.album_version, membership_version);
        persistence
            .mutate_album(AlbumMutation::Rename {
                album_id: album_id.clone(),
                name: "Final".to_owned(),
            })
            .await
            .unwrap();
        let final_album_version = read_album().await.album_version;
        assert_ne!(final_album_version, membership_version);

        // A sidecar detected at write admission refuses the transaction. The
        // previously issued guards remain valid because no commit occurred.
        fs::write(path.with_file_name("library.sqlite-wal"), b"blocked").unwrap();
        assert!(
            persistence
                .mutate_photo_state(PhotoStateMutation {
                    photo_id: photo_id.clone(),
                    field: PhotoStateField::Rating,
                    value: PhotoStateValue::Rating(5),
                    expected_current: None,
                    album_id: None,
                })
                .await
                .is_err()
        );
        assert_eq!(
            read_photo().await.decision_version,
            changed_back_photo_version
        );
        assert!(
            persistence
                .mutate_album(AlbumMutation::Rename {
                    album_id: album_id.clone(),
                    name: "Blocked".to_owned(),
                })
                .await
                .is_err()
        );
        assert_eq!(read_album().await.album_version, final_album_version);
    }

    #[tokio::test]
    async fn every_existing_album_writer_participates_in_version_invalidation() {
        let (_base, library, state, name, _path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![
                    discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0),
                    discovered("two.JPG", OriginalKind::Jpeg, 2, 2.0),
                    discovered("three.JPG", OriginalKind::Jpeg, 3, 3.0),
                ],
                Vec::new(),
            )
            .await
            .unwrap();
        let photo_ids = snapshot
            .photos
            .iter()
            .map(|photo| photo.id.clone())
            .collect::<Vec<_>>();
        let album_id = persistence
            .mutate_album(AlbumMutation::Create {
                name: "Writers".to_owned(),
            })
            .await
            .unwrap()
            .album_id;
        let version = || async {
            persistence
                .album_receiver(&album_id)
                .unwrap()
                .await
                .unwrap()
                .unwrap()
                .unwrap()
                .album_version
        };
        let mut prior = version().await;

        persistence
            .mutate_album(AlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: vec![photo_ids[0].clone()],
            })
            .await
            .unwrap();
        let after_add = version().await;
        assert_ne!(after_add, prior);
        prior = after_add;
        persistence
            .mutate_album(AlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: vec![photo_ids[0].clone()],
            })
            .await
            .unwrap();
        assert_eq!(version().await, prior);

        persistence
            .mutate_album(AlbumMutation::AddFolderMembers {
                album_id: album_id.clone(),
                photo_ids: vec![photo_ids[1].clone()],
            })
            .await
            .unwrap();
        let after_folder_add = version().await;
        assert_ne!(after_folder_add, prior);
        prior = after_folder_add;
        persistence
            .mutate_album(AlbumMutation::Reorder {
                album_id: album_id.clone(),
                photo_ids: vec![photo_ids[1].clone(), photo_ids[0].clone()],
            })
            .await
            .unwrap();
        let after_reorder = version().await;
        assert_ne!(after_reorder, prior);
        prior = after_reorder;
        persistence
            .mutate_album(AlbumMutation::RemoveMember {
                album_id: album_id.clone(),
                photo_id: photo_ids[0].clone(),
            })
            .await
            .unwrap();
        let after_remove = version().await;
        assert_ne!(after_remove, prior);
        prior = after_remove;

        persistence
            .mutate_album_membership(AlbumMembershipMutation::Add {
                album_id: album_id.clone(),
                photo_ids: vec![photo_ids[2].clone()],
            })
            .await
            .unwrap();
        let after_membership_add = version().await;
        assert_ne!(after_membership_add, prior);
        prior = after_membership_add;
        persistence
            .mutate_album_membership(AlbumMembershipMutation::RemoveAdded {
                album_id: album_id.clone(),
                photo_ids: vec![photo_ids[2].clone()],
            })
            .await
            .unwrap();
        let after_compensation = version().await;
        assert_ne!(after_compensation, prior);
        prior = after_compensation;
        persistence
            .mutate_album(AlbumMutation::SetProgress {
                album_id: album_id.clone(),
                photo_id: photo_ids[1].clone(),
            })
            .await
            .unwrap();
        assert_eq!(version().await, prior);

        let photo_version = persistence
            .photo_receiver(&photo_ids[0])
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .decision_version;
        persistence
            .mutate_photo_state_batch_receiver(PhotoStateBatchMutation {
                photos: vec![PhotoStateBatchItem {
                    photo_id: photo_ids[0].clone(),
                    expected_current: SelectionState::Undecided,
                }],
                value: SelectionState::Selected,
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let after_batch = persistence
            .photo_receiver(&photo_ids[0])
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .decision_version;
        assert_ne!(after_batch, photo_version);
    }

    #[tokio::test]
    async fn reopening_persistence_invalidates_process_epoch_versions() {
        let (base, library, state, name, _path) = fixture();
        let canonical_root = library.canonical_path().to_string_lossy().into_owned();
        let persistence = Persistence::open(state, name.clone(), canonical_root.clone()).unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0)],
                Vec::new(),
            )
            .await
            .unwrap();
        let photo_id = snapshot.photos[0].id.clone();
        let first = persistence
            .photo_receiver(&photo_id)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .decision_version;
        let album = persistence
            .create_album_checked("Epoch".to_owned())
            .await
            .unwrap()
            .album;
        persistence.shutdown().unwrap();

        let reopened_state =
            StateDirectory::open_or_create(&library, base.0.join("state")).unwrap();
        let reopened = Persistence::open(reopened_state, name, canonical_root).unwrap();
        let second = reopened
            .photo_receiver(&photo_id)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .decision_version;
        assert_ne!(first, second);
        let reopened_album = reopened
            .album_receiver(&album.id)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_ne!(album.album_version, reopened_album.album_version);
        assert_eq!(
            reopened
                .mutate_album_checked(CheckedAlbumMutation::Rename {
                    album_id: album.id.clone(),
                    name: album.name,
                    expected_version: album.album_version,
                })
                .await,
            Err(AlbumWriteError::VersionConflict {
                album_id: album.id,
                current_version: reopened_album.album_version,
            })
        );
    }

    #[test]
    fn capture_time_bounds_reject_offsets_and_invalid_calendar_values() {
        assert!(CaptureTimeBound::parse("2026-02-28T23:59:59").is_ok());
        assert!(CaptureTimeBound::parse("2024-02-29T00:00:00").is_ok());
        for invalid in [
            "0000-01-01T00:00:00",
            "2026-02-29T00:00:00",
            "2026-01-01T00:00:00Z",
            "2026-01-01T00:00:00+01:00",
            "2026-13-01T00:00:00",
        ] {
            assert!(CaptureTimeBound::parse(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn new_library_id_collision_is_rejected_before_insertion() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(include_str!(
                "../../../../compatibility/sqlite/schema-v4.sql"
            ))
            .unwrap();
        connection.execute(
            "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,capture_metadata_state) VALUES('collision','a.JPG','jpeg',1,1,1,'pending')",
            [],
        ).unwrap();
        let transaction = connection.unchecked_transaction().unwrap();
        assert!(matches!(
            reserve_library_id(&transaction, &mut HashSet::new(), "collision".to_owned()),
            Err(PersistenceError::IdCollision)
        ));
        assert_eq!(
            transaction
                .query_row("SELECT count(*) FROM original_files", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn library_snapshot_orders_raw_first_capture_then_missing_paths() {
        let (_base, library, state, name, _path) = fixture();
        let mut z = discovered("shoot/Z.JPG", OriginalKind::Jpeg, 1, 1.0);
        let mut a = discovered("shoot/A.JPG", OriginalKind::Jpeg, 2, 2.0);
        let b = discovered("shoot/B.JPG", OriginalKind::Jpeg, 3, 3.0);
        z.capture = CaptureFact {
            state: CaptureMetadataState::Known,
            order_key: Some("2026-01-01T09:00:00.000000000".to_owned()),
            field: Some(CaptureTimeField::DateTimeOriginal),
            offset_minutes: None,
            source_revision: Some("z-revision".to_owned()),
        };
        a.capture = CaptureFact {
            state: CaptureMetadataState::Known,
            order_key: Some("2026-01-01T10:00:00.000000000".to_owned()),
            field: Some(CaptureTimeField::DateTimeOriginal),
            offset_minutes: Some(60),
            source_revision: Some("a-revision".to_owned()),
        };
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(vec![a, b, z], Vec::new())
            .await
            .unwrap();
        assert_eq!(
            snapshot
                .photos
                .iter()
                .map(|photo| photo.sort_path.as_str())
                .collect::<Vec<_>>(),
            ["shoot/Z.JPG", "shoot/A.JPG", "shoot/B.JPG"]
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn capture_order_orders_each_photo_by_its_own_original_and_retains_unavailable_facts() {
        let (_base, library, state, name, _path) = fixture();
        let vectors = capture_order_vectors();
        let disagreement = vectors
            .iter()
            .find(|vector| vector.name == "independent-photos-order-by-their-own-capture-time")
            .unwrap();
        let missing_partition = vectors
            .iter()
            .find(|vector| vector.name == "missing-capture-time-is-a-final-path-partition")
            .unwrap();
        let known = |key: &str, revision: &str| CaptureFact {
            state: CaptureMetadataState::Known,
            order_key: Some(key.to_owned()),
            field: Some(CaptureTimeField::DateTimeOriginal),
            offset_minutes: None,
            source_revision: Some(revision.to_owned()),
        };
        let mut raw = discovered(
            disagreement.raw_path.as_deref().unwrap(),
            OriginalKind::Raw,
            1,
            1.0,
        );
        raw.capture = known(disagreement.raw_order_key.as_deref().unwrap(), "raw");
        let mut paired_jpeg = discovered(
            disagreement.jpeg_path.as_deref().unwrap(),
            OriginalKind::Jpeg,
            1,
            1.0,
        );
        paired_jpeg.capture = known(disagreement.jpeg_order_key.as_deref().unwrap(), "jpeg");
        let mut middle = discovered("middle.JPG", OriginalKind::Jpeg, 1, 1.0);
        middle.capture = known("2026-01-01T10:30:00.000000000", "middle");
        let mut z = discovered("z.JPG", OriginalKind::Jpeg, 1, 1.0);
        z.capture = known("2026-01-01T12:00:00.000000000", "z");
        let mut a = discovered("a.JPG", OriginalKind::Jpeg, 1, 1.0);
        a.capture = known("2026-01-01T12:00:00.000000000", "a");
        let missing = discovered("missing.JPG", OriginalKind::Jpeg, 1, 1.0);
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let first = persistence
            .apply_scan(
                vec![raw.clone(), paired_jpeg, middle, z, a, missing],
                Vec::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            first
                .photos
                .iter()
                .map(|photo| photo.sort_path.as_str())
                .collect::<Vec<_>>(),
            disagreement
                .expected_paths
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
        assert_eq!(missing_partition.expected_paths, ["missing.JPG"]);
        let unavailable = persistence
            .apply_scan(Vec::new(), Vec::new())
            .await
            .unwrap();
        assert_eq!(
            unavailable
                .originals
                .iter()
                .find(|original| original.relative_path.as_str() == "pair.ARW")
                .unwrap()
                .capture,
            raw.capture
        );
        raw.facts.size = 2;
        raw.capture = CaptureFact {
            state: CaptureMetadataState::Missing,
            order_key: None,
            field: None,
            offset_minutes: None,
            source_revision: Some("raw-replaced".to_owned()),
        };
        let replacement_fact = raw.capture.clone();
        let replaced = persistence.apply_scan(vec![raw], Vec::new()).await.unwrap();
        assert_eq!(
            replaced
                .originals
                .iter()
                .find(|original| original.relative_path.as_str() == "pair.ARW")
                .unwrap()
                .capture,
            replacement_fact
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn equal_capture_and_path_ties_use_photo_id_bytes() {
        let (_base, library, state, name, path) = fixture();
        let vectors = capture_order_vectors();
        let tie = vectors
            .iter()
            .find(|vector| vector.name == "equal-time-ties-use-path-then-photo-id-bytes")
            .unwrap();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v3.sql"),
        );
        let connection = Connection::open(&path).unwrap();
        for (original_id, path) in [
            ("original-a", "source-a.JPG"),
            ("original-z", "source-z.JPG"),
        ] {
            connection.execute(
                "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,error_category,error_message,capture_metadata_state,capture_order_key,capture_time_field,capture_offset_minutes,capture_source_revision) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
                params![original_id, path, "jpeg", 1_i64, 1.0_f64, 1_i64, Option::<String>::None, Option::<String>::None, "known", tie.order_key.as_deref().unwrap(), "date-time-original", Option::<i64>::None, "revision"],
            ).unwrap();
        }
        for (photo_id, original_id) in [("z-photo", "original-z"), ("a-photo", "original-a")] {
            connection.execute(
                "INSERT INTO photos(id,jpeg_original_id,ambiguous,available,preview_state,sort_path,selection_state,rating) VALUES(?,?,?,?,?,?,?,?)",
                params![photo_id, original_id, 0_i64, 1_i64, "inspection-pending", "same.JPG", "undecided", 0_i64],
            ).unwrap();
        }
        drop(connection);
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        assert_eq!(
            persistence
                .snapshot()
                .await
                .unwrap()
                .photos
                .iter()
                .map(|photo| photo.id.as_str())
                .collect::<Vec<_>>(),
            tie.expected_photo_ids
                .as_ref()
                .unwrap()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn applies_scans_transactionally_and_preserves_unavailable_pair_identity() {
        let (_base, library, state, name, _path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let raw = discovered("one.ARW", OriginalKind::Raw, 3, 1000.0);
        let jpeg = discovered("one.JPG", OriginalKind::Jpeg, 4, 1000.0);
        let first = persistence
            .apply_scan(vec![raw.clone(), jpeg.clone()], Vec::new())
            .await
            .unwrap();
        assert_eq!(first.originals.len(), 2);
        assert_eq!(first.photos.len(), 2);
        let raw_photo = first
            .photos
            .iter()
            .find(|photo| {
                first.originals.iter().any(|original| {
                    original.id == photo.original_id && original.kind == OriginalKind::Raw
                })
            })
            .unwrap();
        let jpeg_photo = first
            .photos
            .iter()
            .find(|photo| photo.id != raw_photo.id)
            .unwrap();
        assert!(raw_photo.available);
        assert!(jpeg_photo.available);
        assert_eq!(raw_photo.preview_state, PreviewState::InspectionPending);

        let unavailable = persistence
            .apply_scan(Vec::new(), Vec::new())
            .await
            .unwrap();
        let missing = unavailable
            .photos
            .iter()
            .find(|photo| photo.id == raw_photo.id)
            .unwrap();
        assert_eq!(missing.id, raw_photo.id);
        assert_eq!(missing.original_id, raw_photo.original_id);
        assert!(!missing.available);
        assert_eq!(missing.preview_state, PreviewState::Unavailable);

        let restored = persistence.apply_scan(vec![raw], Vec::new()).await.unwrap();
        let restored_photo = restored
            .photos
            .iter()
            .find(|photo| photo.id == raw_photo.id)
            .unwrap();
        assert_eq!(restored_photo.id, raw_photo.id);
        assert!(restored_photo.available);
        assert_eq!(restored_photo.original_id, raw_photo.original_id);
        assert_eq!(
            restored_photo.preview_state,
            PreviewState::InspectionPending
        );
        persistence.shutdown().unwrap();
    }

    // album-language-legacy:start v2-reconciliation-test
    #[tokio::test]
    async fn preserves_decisions_and_memberships_while_reconciling_rows() {
        let (_base, library, state, name, path) = fixture();
        let raw = discovered("one.ARW", OriginalKind::Raw, 3, 1000.0);
        let jpeg = discovered("one.JPG", OriginalKind::Jpeg, 4, 1000.0);
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v2.sql"),
        );
        let raw_id = original_id(raw.path.as_str());
        let jpeg_id = original_id(jpeg.path.as_str());
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO original_files VALUES(?,?,?,?,?,?,?,?)",
                params![
                    raw_id,
                    "one.ARW",
                    "raw",
                    3_i64,
                    1000.0_f64,
                    1_i64,
                    Option::<String>::None,
                    Option::<String>::None
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO original_files VALUES(?,?,?,?,?,?,?,?)",
                params![
                    jpeg_id,
                    "one.JPG",
                    "jpeg",
                    4_i64,
                    1000.0_f64,
                    1_i64,
                    Option::<String>::None,
                    Option::<String>::None
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO photos(id,raw_original_id,jpeg_original_id,ambiguous,available,preview_state,preview_candidate,selection_state,rating,sort_path) VALUES(?,?,?,?,?,?,?,?,?,?)",
                params!["stable-photo", raw_id, jpeg_id, 0_i64, 1_i64, "inspection-pending", "matching-jpeg", "selected", 5_i64, "one.ARW"],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO photo_sets(id,name,created_at) VALUES(?,?,?)",
                params!["00000000-0000-4000-8000-000000000021", "Keep", 1_i64],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO photo_set_members(photo_set_id,photo_id,position) VALUES(?,?,?)",
                params![
                    "00000000-0000-4000-8000-000000000021",
                    "stable-photo",
                    0_i64
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO review_progress(photo_set_id,photo_id) VALUES(?,?)",
                params!["00000000-0000-4000-8000-000000000021", "stable-photo"],
            )
            .unwrap();
        drop(connection);
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(vec![raw, jpeg], Vec::new())
            .await
            .unwrap();
        assert_eq!(snapshot.photos[0].id, "stable-photo");
        assert_eq!(snapshot.photos[0].selection_state, SelectionState::Selected);
        assert_eq!(snapshot.photos[0].rating, 5);
        persistence.shutdown().unwrap();
        let connection = Connection::open(path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT photo_id FROM album_progress WHERE album_id=?",
                    ["00000000-0000-4000-8000-000000000021"],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "stable-photo"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM album_members WHERE album_id=?",
                    ["00000000-0000-4000-8000-000000000021"],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
    }
    // album-language-legacy:end v2-reconciliation-test

    #[tokio::test]
    async fn preserves_preview_facts_only_for_unchanged_original_and_uses_cas() {
        let (_base, library, state, name, _path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let first = persistence
            .apply_scan(
                vec![discovered("one.JPG", OriginalKind::Jpeg, 4, 1000.0)],
                Vec::new(),
            )
            .await
            .unwrap();
        let photo_id = first.photos[0].id.clone();
        let revision = source_revision("one.JPG", 4, 1000.0).unwrap();
        assert_eq!(
            persistence
                .seed_preview(PreviewSeed {
                    photo_id: photo_id.clone(),
                    state: PreviewState::Ready,
                    source: crate::PreviewSource::JpegOriginal,
                    expected_source_revision: revision.clone(),
                    width: Some(100),
                    height: Some(50),
                    cache_revision: Some("cache-v1".to_owned()),
                })
                .await
                .unwrap(),
            PreviewSeedResult::Applied
        );
        let unchanged = persistence
            .apply_scan(
                vec![discovered("one.JPG", OriginalKind::Jpeg, 4, 1000.0)],
                Vec::new(),
            )
            .await
            .unwrap();
        assert_eq!(unchanged.photos[0].preview_state, PreviewState::Ready);
        assert_eq!(
            unchanged.photos[0].cache_revision.as_deref(),
            Some("cache-v1")
        );

        let changed = persistence
            .apply_scan(
                vec![discovered("one.JPG", OriginalKind::Jpeg, 5, 1001.0)],
                Vec::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            changed.photos[0].preview_state,
            PreviewState::InspectionPending
        );
        assert_eq!(
            persistence
                .seed_preview(PreviewSeed {
                    photo_id,
                    state: PreviewState::Ready,
                    source: crate::PreviewSource::JpegOriginal,
                    expected_source_revision: revision,
                    width: Some(100),
                    height: Some(50),
                    cache_revision: Some("stale".to_owned()),
                })
                .await
                .unwrap(),
            PreviewSeedResult::StaleIgnored
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn relocation_resets_preview_and_moves_identity_in_one_transaction() {
        let (_base, library, state, name, _path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let first = persistence
            .apply_scan(
                vec![discovered("one.JPG", OriginalKind::Jpeg, 4, 1000.0)],
                Vec::new(),
            )
            .await
            .unwrap();
        let photo = &first.photos[0];
        let original_id = photo.original_id.clone();
        let photo_id = photo.id.clone();
        let revision = source_revision("one.JPG", 4, 1000.0).unwrap();
        assert_eq!(
            persistence
                .seed_preview(PreviewSeed {
                    photo_id: photo_id.clone(),
                    state: PreviewState::Ready,
                    source: crate::PreviewSource::JpegOriginal,
                    expected_source_revision: revision,
                    width: Some(100),
                    height: Some(50),
                    cache_revision: Some("cache-v1".to_owned()),
                })
                .await
                .unwrap(),
            PreviewSeedResult::Applied
        );

        // The same content re-discovered at a new Location with a proven
        // relocation keeps the Photo identity, resets Preview inspection, and
        // records the fresh fingerprint bound to the new Location.
        let digest = crate::recovery::digest_bytes(b"payload");
        let _ = digest;
        let recovery = ScanRecoveryPlan {
            relocations: [("moved/two.JPG".to_owned(), original_id.clone())].into(),
            fingerprints: vec![DiscoveredFingerprint {
                path: "moved/two.JPG".to_owned(),
                digest: crate::recovery::digest_bytes(&[]),
            }],
        };
        let relocated = persistence
            .apply_scan_recovered(
                vec![discovered("moved/two.JPG", OriginalKind::Jpeg, 4, 1000.0)],
                Vec::new(),
                recovery,
            )
            .await
            .unwrap();
        assert_eq!(relocated.snapshot.photos.len(), 1);
        assert_eq!(relocated.snapshot.photos[0].id, photo_id);
        assert_eq!(relocated.relocated_originals, 1);
        assert_eq!(
            relocated.snapshot.originals[0].relative_path.as_str(),
            "moved/two.JPG"
        );
        assert_eq!(
            relocated.snapshot.photos[0].preview_state,
            PreviewState::InspectionPending
        );
        assert!(relocated.snapshot.photos[0].cache_revision.is_none());
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn rolls_back_scan_and_keeps_the_prior_snapshot_without_partial_rows() {
        let (_base, library, state, name, _path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let initial = persistence
            .apply_scan(
                vec![discovered("one.JPG", OriginalKind::Jpeg, 4, 1000.0)],
                Vec::new(),
            )
            .await
            .unwrap();
        let failed = persistence
            .apply_scan_failure(
                vec![
                    discovered("new.JPG", OriginalKind::Jpeg, 5, 1001.0),
                    discovered("second.JPG", OriginalKind::Jpeg, 6, 1002.0),
                ],
                Vec::new(),
            )
            .await;
        assert!(matches!(failed, Err(PersistenceError::Storage)));
        let after = persistence.snapshot().await.unwrap();
        assert_eq!(after, initial);
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn stale_fallback_preview_completion_is_ignored_after_candidate_change() {
        let (_base, library, state, name, _path) = fixture();
        let raw = discovered("one.ARW", OriginalKind::Raw, 3, 1000.0);
        let first = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let initial = first
            .apply_scan(vec![raw.clone()], Vec::new())
            .await
            .unwrap();
        let photo_id = initial.photos[0].id.clone();
        let raw_revision = source_revision("one.ARW", 3, 1000.0).unwrap();
        assert_eq!(
            first
                .seed_preview(PreviewSeed {
                    photo_id: photo_id.clone(),
                    state: PreviewState::Ready,
                    source: crate::PreviewSource::RawEmbeddedJpeg,
                    expected_source_revision: raw_revision.clone(),
                    width: Some(512),
                    height: Some(341),
                    cache_revision: Some("raw-cache".to_owned()),
                })
                .await
                .unwrap(),
            PreviewSeedResult::Applied
        );
        // A second scan with changed RAW facts makes the old revision stale.
        let changed = discovered("one.ARW", OriginalKind::Raw, 7, 1002.0);
        let updated = first.apply_scan(vec![changed], Vec::new()).await.unwrap();
        let updated_photo = updated
            .photos
            .iter()
            .find(|photo| photo.id == photo_id)
            .unwrap();
        assert_eq!(updated_photo.preview_state, PreviewState::InspectionPending);
        assert_eq!(
            first
                .seed_preview(PreviewSeed {
                    photo_id,
                    state: PreviewState::Ready,
                    source: crate::PreviewSource::JpegOriginal,
                    expected_source_revision: raw_revision,
                    width: Some(512),
                    height: Some(341),
                    cache_revision: Some("stale".to_owned()),
                })
                .await
                .unwrap(),
            PreviewSeedResult::StaleIgnored
        );
        first.shutdown().unwrap();
    }

    fn photo_ids(snapshot: &ScanSnapshot) -> Vec<String> {
        snapshot
            .photos
            .iter()
            .map(|photo| photo.id.clone())
            .collect()
    }

    /// A batch that names no Photo, names one twice, or exceeds the bound is
    /// the request's own defect, so it is classified as invalid before any
    /// state is read or written.
    #[test]
    fn batch_photo_state_validation_classifies_malformed_requests_as_invalid() {
        let item = |photo_id: &str| crate::PhotoStateBatchItem {
            photo_id: photo_id.to_owned(),
            expected_current: SelectionState::Undecided,
        };
        let valid = PhotoStateBatchMutation {
            photos: vec![item("one"), item("two")],
            value: SelectionState::Selected,
        };
        assert_eq!(validate_photo_state_batch_mutation(&valid), Ok(()));
        let malformed = [
            PhotoStateBatchMutation {
                photos: Vec::new(),
                value: SelectionState::Rejected,
            },
            PhotoStateBatchMutation {
                photos: vec![item("one"), item("one")],
                value: SelectionState::Rejected,
            },
            PhotoStateBatchMutation {
                photos: vec![item("one")],
                value: SelectionState::Undecided,
            },
            PhotoStateBatchMutation {
                photos: (0..=crate::PHOTO_STATE_BATCH_MAX)
                    .map(|index| item(&format!("photo-{index}")))
                    .collect(),
                value: SelectionState::Selected,
            },
        ];
        for mutation in malformed {
            assert_eq!(
                validate_photo_state_batch_mutation(&mutation),
                Err(MutationError::Invalid)
            );
        }
    }

    #[tokio::test]
    async fn batch_photo_state_compares_each_photo_and_reports_a_complete_partition() {
        let (_base, library, state, name, _path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![
                    discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0),
                    discovered("two.JPG", OriginalKind::Jpeg, 2, 1.0),
                ],
                Vec::new(),
            )
            .await
            .unwrap();
        let ids = photo_ids(&snapshot);
        persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: ids[0].clone(),
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Selected),
                expected_current: Some(PhotoStateValue::Selection(SelectionState::Undecided)),
                album_id: None,
            })
            .await
            .unwrap();

        let result = persistence
            .mutate_photo_state_batch_receiver(PhotoStateBatchMutation {
                photos: vec![
                    PhotoStateBatchItem {
                        photo_id: ids[0].clone(),
                        expected_current: SelectionState::Undecided,
                    },
                    PhotoStateBatchItem {
                        photo_id: ids[1].clone(),
                        expected_current: SelectionState::Undecided,
                    },
                    PhotoStateBatchItem {
                        photo_id: "missing-photo".to_owned(),
                        expected_current: SelectionState::Undecided,
                    },
                ],
                value: SelectionState::Rejected,
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            result.applied,
            vec![PhotoStateBatchApplied {
                photo_id: ids[1].clone(),
                prior_value: SelectionState::Undecided,
            }]
        );
        assert_eq!(
            result.changed_elsewhere,
            vec![PhotoStateBatchChangedElsewhere {
                photo_id: ids[0].clone(),
                current_value: SelectionState::Selected,
            }]
        );
        assert_eq!(
            result.missing,
            vec![PhotoStateBatchMissing {
                photo_id: "missing-photo".to_owned(),
            }]
        );
        let after = persistence.snapshot().await.unwrap();
        assert_eq!(
            after
                .photos
                .iter()
                .map(|photo| photo.selection_state)
                .collect::<Vec<_>>(),
            vec![SelectionState::Selected, SelectionState::Rejected]
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn batch_photo_state_rolls_back_earlier_matches_when_a_later_write_fails() {
        let (_base, library, state, name, path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![
                    discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0),
                    discovered("two.JPG", OriginalKind::Jpeg, 2, 1.0),
                ],
                Vec::new(),
            )
            .await
            .unwrap();
        let ids = photo_ids(&snapshot);
        let escaped_id = ids[1].replace('\'', "''");
        Connection::open(&path)
            .unwrap()
            .execute_batch(&format!(
                "CREATE TRIGGER fail_batch_second BEFORE UPDATE OF selection_state ON photos\n                 WHEN OLD.id = '{escaped_id}'\n                 BEGIN SELECT RAISE(ABORT, 'forced batch failure'); END;"
            ))
            .unwrap();

        let result = persistence
            .mutate_photo_state_batch_receiver(PhotoStateBatchMutation {
                photos: ids
                    .iter()
                    .map(|photo_id| PhotoStateBatchItem {
                        photo_id: photo_id.clone(),
                        expected_current: SelectionState::Undecided,
                    })
                    .collect(),
                value: SelectionState::Selected,
            })
            .unwrap()
            .await
            .unwrap();
        assert!(result.is_err());
        let after = persistence.snapshot().await.unwrap();
        assert!(
            after
                .photos
                .iter()
                .all(|photo| photo.selection_state == SelectionState::Undecided)
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn album_summaries_and_browse_target_avoid_member_materialization() {
        let (_base, library, state, name, path) = fixture();
        let originals = vec![
            discovered("one.ARW", OriginalKind::Raw, 3, 1000.0),
            discovered("two.ARW", OriginalKind::Raw, 4, 1000.0),
        ];
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v5.sql"),
        );
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence.apply_scan(originals, Vec::new()).await.unwrap();
        let photo_one = snapshot.photos[0].id.clone();
        let photo_two = snapshot.photos[1].id.clone();
        let created = persistence
            .mutate_album(AlbumMutation::Create {
                name: "Bounded".to_owned(),
            })
            .await
            .unwrap();
        let album_id = created.album_id;
        // Empty summary before any member exists.
        let summaries = persistence
            .list_album_summaries_receiver()
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, album_id);
        assert_eq!(summaries[0].photo_count, 0);
        assert!(!summaries[0].has_saved_position);
        // Browse target for the empty album exists with no members.
        let target = persistence
            .album_browse_target_receiver(&album_id)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(target.as_ref().unwrap().members.len(), 0);
        assert_eq!(target.as_ref().unwrap().saved_photo_id, None);
        persistence
            .mutate_album(AlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: vec![photo_one.clone(), photo_two.clone()],
            })
            .await
            .unwrap();
        persistence
            .mutate_album(AlbumMutation::SetProgress {
                album_id: album_id.clone(),
                photo_id: photo_two.clone(),
            })
            .await
            .unwrap();
        let summaries = persistence
            .list_album_summaries_receiver()
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(summaries[0].photo_count, 2);
        assert!(summaries[0].has_saved_position);
        let target = persistence
            .album_browse_target_receiver(&album_id)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(target.members.len(), 2);
        assert_eq!(target.members[0].photo_id, photo_one);
        assert!(target.members[0].available);
        assert_eq!(target.members[1].photo_id, photo_two);
        assert_eq!(target.saved_photo_id.as_deref(), Some(photo_two.as_str()));
        // Unknown albums report no browse target.
        assert!(
            persistence
                .album_browse_target_receiver("00000000-0000-4000-8000-00000000dead")
                .unwrap()
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn checked_album_changes_are_guarded_atomic_and_identity_bearing() {
        let (_base, library, state, name, path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![
                    discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0),
                    discovered("two.JPG", OriginalKind::Jpeg, 2, 2.0),
                    discovered("three.JPG", OriginalKind::Jpeg, 3, 3.0),
                ],
                Vec::new(),
            )
            .await
            .unwrap();
        let ids = photo_ids(&snapshot);
        let created = persistence
            .create_album_checked("  Picks  ".to_owned())
            .await
            .unwrap();
        let album_id = created.album.id.clone();
        assert_eq!(created.album.name, "Picks");
        assert_eq!(created.album.photo_count, 0);
        assert!(!created.album.has_saved_position);
        assert_eq!(
            persistence.create_album_checked("pIcKs".to_owned()).await,
            Err(AlbumWriteError::NameConflict {
                name: "pIcKs".to_owned(),
                album_id: album_id.clone(),
            })
        );

        let initial_version = created.album.album_version;
        assert_eq!(
            persistence
                .mutate_album_checked(CheckedAlbumMutation::AddMembers {
                    album_id: album_id.clone(),
                    photo_ids: (0..=ALBUM_MEMBERSHIP_BATCH_MAX)
                        .map(|index| format!("photo-{index}"))
                        .collect(),
                    expected_version: initial_version.clone(),
                })
                .await,
            Err(AlbumWriteError::LimitExceeded {
                limit: ALBUM_MEMBERSHIP_BATCH_MAX,
                actual: ALBUM_MEMBERSHIP_BATCH_MAX + 1,
            })
        );
        let unchanged = persistence
            .mutate_album_checked(CheckedAlbumMutation::Rename {
                album_id: album_id.clone(),
                name: "Picks".to_owned(),
                expected_version: initial_version.clone(),
            })
            .await
            .unwrap();
        let CheckedAlbumMutationResult::Renamed { album, renamed } = unchanged else {
            panic!("expected rename result");
        };
        assert!(!renamed);
        assert_eq!(album.album_version, initial_version);

        assert_eq!(
            persistence
                .mutate_album_checked(CheckedAlbumMutation::AddMembers {
                    album_id: album_id.clone(),
                    photo_ids: vec![ids[0].clone(), "missing".to_owned()],
                    expected_version: initial_version.clone(),
                })
                .await,
            Err(AlbumWriteError::PhotoNotFound {
                photo_id: "missing".to_owned(),
            })
        );
        let after_refusal = persistence
            .album_receiver(&album_id)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(after_refusal.album_version, initial_version);
        assert_eq!(after_refusal.photo_count, 0);

        let added = persistence
            .mutate_album_checked(CheckedAlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: vec![ids[0].clone(), ids[1].clone()],
                expected_version: initial_version.clone(),
            })
            .await
            .unwrap();
        let CheckedAlbumMutationResult::Added {
            album,
            added_photo_ids,
            already_member_photo_ids,
        } = added
        else {
            panic!("expected add result");
        };
        assert_eq!(added_photo_ids, vec![ids[0].clone(), ids[1].clone()]);
        assert!(already_member_photo_ids.is_empty());
        assert_eq!(album.photo_count, 2);
        let added_version = album.album_version;
        assert_ne!(added_version, initial_version);

        assert_eq!(
            persistence
                .mutate_album_checked(CheckedAlbumMutation::AddMembers {
                    album_id: album_id.clone(),
                    photo_ids: vec![ids[0].clone()],
                    expected_version: initial_version,
                })
                .await,
            Err(AlbumWriteError::VersionConflict {
                album_id: album_id.clone(),
                current_version: added_version.clone(),
            })
        );
        persistence
            .mutate_album(AlbumMutation::SetProgress {
                album_id: album_id.clone(),
                photo_id: ids[1].clone(),
            })
            .await
            .unwrap();

        let removed = persistence
            .mutate_album_checked(CheckedAlbumMutation::RemoveMembers {
                album_id: album_id.clone(),
                photo_ids: vec![ids[1].clone(), ids[2].clone()],
                expected_version: added_version,
            })
            .await
            .unwrap();
        let CheckedAlbumMutationResult::Removed {
            album,
            removed_photo_ids,
            already_absent_photo_ids,
            saved_photo_id,
        } = removed
        else {
            panic!("expected remove result");
        };
        assert_eq!(removed_photo_ids, vec![ids[1].clone()]);
        assert_eq!(already_absent_photo_ids, vec![ids[2].clone()]);
        assert_eq!(saved_photo_id, None);
        assert!(!album.has_saved_position);
        let removed_version = album.album_version;

        let added = persistence
            .mutate_album_checked(CheckedAlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: vec![ids[2].clone(), ids[0].clone()],
                expected_version: removed_version,
            })
            .await
            .unwrap();
        let CheckedAlbumMutationResult::Added {
            album,
            added_photo_ids,
            already_member_photo_ids,
        } = added
        else {
            panic!("expected add result");
        };
        assert_eq!(added_photo_ids, vec![ids[2].clone()]);
        assert_eq!(already_member_photo_ids, vec![ids[0].clone()]);
        let reordered = persistence
            .mutate_album_checked(CheckedAlbumMutation::Reorder {
                album_id: album_id.clone(),
                photo_ids: vec![ids[2].clone(), ids[0].clone()],
                expected_version: album.album_version,
            })
            .await
            .unwrap();
        let CheckedAlbumMutationResult::Reordered {
            album,
            ordered_photo_ids,
            reordered,
        } = reordered
        else {
            panic!("expected reorder result");
        };
        assert!(reordered);
        assert_eq!(ordered_photo_ids, vec![ids[2].clone(), ids[0].clone()]);
        let reordered_version = album.album_version;
        assert_eq!(
            persistence
                .mutate_album_checked(CheckedAlbumMutation::Reorder {
                    album_id: album_id.clone(),
                    photo_ids: vec![ids[0].clone()],
                    expected_version: reordered_version.clone(),
                })
                .await,
            Err(AlbumWriteError::MembershipConflict {
                album_id: album_id.clone(),
                current_version: reordered_version.clone(),
            })
        );

        let renamed = persistence
            .mutate_album_checked(CheckedAlbumMutation::Rename {
                album_id: album_id.clone(),
                name: "Final".to_owned(),
                expected_version: reordered_version,
            })
            .await
            .unwrap();
        let CheckedAlbumMutationResult::Renamed { album, renamed } = renamed else {
            panic!("expected rename result");
        };
        assert!(renamed);
        assert_eq!(album.name, "Final");
        let observed_final_version = album.album_version;
        persistence
            .mutate_album(AlbumMutation::Rename {
                album_id: album_id.clone(),
                name: "Away".to_owned(),
            })
            .await
            .unwrap();
        persistence
            .mutate_album(AlbumMutation::Rename {
                album_id: album_id.clone(),
                name: "Final".to_owned(),
            })
            .await
            .unwrap();
        let final_version = persistence
            .album_receiver(&album_id)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .album_version;
        assert_eq!(
            persistence
                .mutate_album_checked(CheckedAlbumMutation::Rename {
                    album_id: album_id.clone(),
                    name: "Final".to_owned(),
                    expected_version: observed_final_version,
                })
                .await,
            Err(AlbumWriteError::VersionConflict {
                album_id: album_id.clone(),
                current_version: final_version.clone(),
            })
        );

        let delete_target = persistence
            .create_album_checked("Delete me".to_owned())
            .await
            .unwrap()
            .album;
        assert_eq!(
            persistence
                .mutate_album_checked(CheckedAlbumMutation::Delete {
                    album_id: delete_target.id.clone(),
                    expected_version: delete_target.album_version,
                })
                .await
                .unwrap(),
            CheckedAlbumMutationResult::Deleted {
                album_id: delete_target.id.clone(),
            }
        );
        assert!(
            persistence
                .album_receiver(&delete_target.id)
                .unwrap()
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );

        let sidecar = path.with_file_name("library.sqlite-wal");
        fs::write(&sidecar, b"blocked").unwrap();
        assert_eq!(
            persistence
                .mutate_album_checked(CheckedAlbumMutation::Rename {
                    album_id: album_id.clone(),
                    name: "Blocked".to_owned(),
                    expected_version: final_version.clone(),
                })
                .await,
            Err(AlbumWriteError::Persistence)
        );
        assert_eq!(
            persistence
                .album_receiver(&album_id)
                .unwrap()
                .await
                .unwrap()
                .unwrap()
                .unwrap()
                .album_version,
            final_version
        );
    }

    #[tokio::test]
    async fn checked_reorder_refuses_a_large_album_without_mutation() {
        let (_base, library, state, name, _path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let originals = (0..=ALBUM_MEMBERSHIP_BATCH_MAX)
            .map(|index| {
                discovered(
                    &format!("photo-{index:03}.JPG"),
                    OriginalKind::Jpeg,
                    index as u64 + 1,
                    index as f64 + 1.0,
                )
            })
            .collect();
        let snapshot = persistence.apply_scan(originals, Vec::new()).await.unwrap();
        let ids = photo_ids(&snapshot);
        let album = persistence
            .create_album_checked("Large".to_owned())
            .await
            .unwrap()
            .album;
        persistence
            .mutate_album(AlbumMutation::AddFolderMembers {
                album_id: album.id.clone(),
                photo_ids: ids.clone(),
            })
            .await
            .unwrap();
        let current = persistence
            .album_receiver(&album.id)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            persistence
                .mutate_album_checked(CheckedAlbumMutation::Reorder {
                    album_id: album.id.clone(),
                    photo_ids: ids[..ALBUM_MEMBERSHIP_BATCH_MAX].to_vec(),
                    expected_version: current.album_version.clone(),
                })
                .await,
            Err(AlbumWriteError::LimitExceeded {
                limit: ALBUM_MEMBERSHIP_BATCH_MAX,
                actual: ALBUM_MEMBERSHIP_BATCH_MAX + 1,
            })
        );
        let after = persistence
            .album_receiver(&album.id)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(after.album_version, current.album_version);
        assert_eq!(after.photo_count, ids.len());
        assert_eq!(
            persistence.list_albums().await.unwrap()[0]
                .members
                .iter()
                .map(|member| member.photo_id.clone())
                .collect::<Vec<_>>(),
            ids
        );
    }

    #[tokio::test]
    async fn checked_photo_decisions_classify_guard_and_invalidate_across_writers() {
        let (base, library, state, name, _path) = fixture();
        let canonical_root = library.canonical_path().to_string_lossy().into_owned();
        let persistence = Persistence::open(state, name.clone(), canonical_root.clone()).unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![
                    discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0),
                    discovered("two.JPG", OriginalKind::Jpeg, 2, 2.0),
                    discovered("three.JPG", OriginalKind::Jpeg, 3, 3.0),
                ],
                Vec::new(),
            )
            .await
            .unwrap();
        let ids = photo_ids(&snapshot);
        let read_photo = |photo_id: String| {
            let persistence = persistence.clone();
            async move {
                persistence
                    .photo_receiver(&photo_id)
                    .unwrap()
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap()
            }
        };

        // One single-field change reports both decision fields before and
        // after, advances the version, and leaves the other field untouched.
        let initial = read_photo(ids[0].clone()).await;
        let changed = persistence
            .mutate_photo_decision_checked(CheckedPhotoDecisionMutation {
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Selected),
                photos: vec![CheckedPhotoDecisionItem {
                    photo_id: ids[0].clone(),
                    expected_version: initial.decision_version.clone(),
                }],
            })
            .await
            .unwrap();
        assert_eq!(changed.counts.changed, 1);
        assert_eq!(changed.counts.unchanged, 0);
        assert_eq!(changed.counts.conflict, 0);
        assert_eq!(changed.counts.missing, 0);
        assert_eq!(
            changed.results,
            vec![CheckedPhotoDecisionItemResult {
                photo_id: ids[0].clone(),
                outcome: CheckedPhotoDecisionOutcome::Changed {
                    prior: PhotoDecisionFacts {
                        selection_state: SelectionState::Undecided,
                        rating: 0,
                    },
                    current: PhotoDecisionSnapshot {
                        selection_state: SelectionState::Selected,
                        rating: 0,
                        decision_version: read_photo(ids[0].clone()).await.decision_version,
                    },
                },
            }]
        );
        let observed = read_photo(ids[0].clone()).await;
        assert_eq!(observed.selection_state, SelectionState::Selected);
        assert_eq!(observed.rating, 0);
        let selected_version = observed.decision_version;
        assert_ne!(selected_version, initial.decision_version);

        // An intervening Web write that changes the value away and back must
        // still conflict with the version read before those edits. Web Undo
        // replays the same single-Photo writer, so it shares this guard.
        persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: ids[0].clone(),
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Undecided),
                expected_current: Some(PhotoStateValue::Selection(SelectionState::Selected)),
                album_id: None,
            })
            .await
            .unwrap();
        let undone = read_photo(ids[0].clone()).await;
        assert_eq!(undone.selection_state, SelectionState::Undecided);
        let away_and_back_version = undone.decision_version.clone();
        assert_ne!(away_and_back_version, selected_version);
        let away_and_back = persistence
            .mutate_photo_decision_checked(CheckedPhotoDecisionMutation {
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Undecided),
                photos: vec![CheckedPhotoDecisionItem {
                    photo_id: ids[0].clone(),
                    expected_version: selected_version.clone(),
                }],
            })
            .await
            .unwrap();
        assert_eq!(
            away_and_back.results,
            vec![CheckedPhotoDecisionItemResult {
                photo_id: ids[0].clone(),
                outcome: CheckedPhotoDecisionOutcome::Conflict {
                    current: PhotoDecisionSnapshot {
                        selection_state: SelectionState::Undecided,
                        rating: 0,
                        decision_version: away_and_back_version.clone(),
                    },
                },
            }]
        );

        // A mixed batch keeps request order, reports one outcome and exact
        // counts per Photo, and leaves no-op versions untouched. ids[1]
        // already holds the target Rating; ids[2] carries a stale version
        // because an intervening Web write advanced it.
        let photo_two_initial = read_photo(ids[1].clone()).await;
        persistence
            .mutate_photo_decision_checked(CheckedPhotoDecisionMutation {
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(4),
                photos: vec![CheckedPhotoDecisionItem {
                    photo_id: ids[1].clone(),
                    expected_version: photo_two_initial.decision_version.clone(),
                }],
            })
            .await
            .unwrap();
        let photo_two = read_photo(ids[1].clone()).await;
        assert_eq!(photo_two.rating, 4);
        let photo_three_stale = read_photo(ids[2].clone()).await;
        persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: ids[2].clone(),
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Selected),
                expected_current: None,
                album_id: None,
            })
            .await
            .unwrap();
        let photo_three = read_photo(ids[2].clone()).await;
        assert_ne!(
            photo_three.decision_version,
            photo_three_stale.decision_version
        );
        let photo_one = read_photo(ids[0].clone()).await;
        let mixed = persistence
            .mutate_photo_decision_checked(CheckedPhotoDecisionMutation {
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(4),
                photos: vec![
                    CheckedPhotoDecisionItem {
                        photo_id: ids[0].clone(),
                        expected_version: photo_one.decision_version.clone(),
                    },
                    CheckedPhotoDecisionItem {
                        photo_id: ids[2].clone(),
                        expected_version: photo_three_stale.decision_version.clone(),
                    },
                    CheckedPhotoDecisionItem {
                        photo_id: "missing-photo".to_owned(),
                        expected_version: photo_three.decision_version.clone(),
                    },
                    CheckedPhotoDecisionItem {
                        photo_id: ids[1].clone(),
                        expected_version: photo_two.decision_version.clone(),
                    },
                ],
            })
            .await
            .unwrap();
        assert_eq!(mixed.counts.changed, 1);
        assert_eq!(mixed.counts.unchanged, 1);
        assert_eq!(mixed.counts.conflict, 1);
        assert_eq!(mixed.counts.missing, 1);
        assert_eq!(
            mixed
                .results
                .iter()
                .map(|result| result.photo_id.clone())
                .collect::<Vec<_>>(),
            vec![
                ids[0].clone(),
                ids[2].clone(),
                "missing-photo".to_owned(),
                ids[1].clone(),
            ]
        );
        assert!(matches!(
            mixed.results[0].outcome,
            CheckedPhotoDecisionOutcome::Changed { .. }
        ));
        assert_eq!(
            mixed.results[1].outcome,
            CheckedPhotoDecisionOutcome::Conflict {
                current: PhotoDecisionSnapshot {
                    selection_state: SelectionState::Selected,
                    rating: 0,
                    decision_version: photo_three.decision_version.clone(),
                },
            }
        );
        assert_eq!(
            mixed.results[2].outcome,
            CheckedPhotoDecisionOutcome::Missing
        );
        assert_eq!(
            mixed.results[3].outcome,
            CheckedPhotoDecisionOutcome::Unchanged {
                current: PhotoDecisionSnapshot {
                    selection_state: SelectionState::Undecided,
                    rating: 4,
                    decision_version: photo_two.decision_version.clone(),
                },
            }
        );
        // A no-op does not advance the version; the conflict left the stale
        // item's facts and version untouched.
        assert_eq!(
            read_photo(ids[1].clone()).await.decision_version,
            photo_two.decision_version
        );
        let after_three = read_photo(ids[2].clone()).await;
        assert_eq!(after_three.rating, 0);
        assert_eq!(after_three.selection_state, SelectionState::Selected);

        // Restart issues a fresh process epoch: the old token conflicts, and
        // a fresh read allows one explicit next write.
        persistence.shutdown().unwrap();
        let reopened_state =
            StateDirectory::open_or_create(&library, base.0.join("state")).unwrap();
        let reopened = Persistence::open(reopened_state, name, canonical_root).unwrap();
        let fresh = reopened
            .photo_receiver(&ids[0])
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_ne!(fresh.decision_version, photo_one.decision_version);
        assert_eq!(fresh.rating, 4);
        let expired = reopened
            .mutate_photo_decision_checked(CheckedPhotoDecisionMutation {
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(5),
                photos: vec![CheckedPhotoDecisionItem {
                    photo_id: ids[0].clone(),
                    expected_version: photo_one.decision_version.clone(),
                }],
            })
            .await
            .unwrap();
        assert_eq!(expired.counts.conflict, 1);
        let explicit = reopened
            .mutate_photo_decision_checked(CheckedPhotoDecisionMutation {
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(5),
                photos: vec![CheckedPhotoDecisionItem {
                    photo_id: ids[0].clone(),
                    expected_version: fresh.decision_version.clone(),
                }],
            })
            .await
            .unwrap();
        assert_eq!(explicit.counts.changed, 1);
        reopened.shutdown().unwrap();
    }

    #[tokio::test]
    async fn checked_photo_decision_refusals_write_nothing_and_change_one_field() {
        let (_base, library, state, name, _path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![
                    discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0),
                    discovered("two.JPG", OriginalKind::Jpeg, 2, 2.0),
                ],
                Vec::new(),
            )
            .await
            .unwrap();
        let ids = photo_ids(&snapshot);
        let read_photo = |photo_id: String| {
            let persistence = persistence.clone();
            async move {
                persistence
                    .photo_receiver(&photo_id)
                    .unwrap()
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap()
            }
        };
        let first = read_photo(ids[0].clone()).await;
        let refusal = |mutation| persistence.mutate_photo_decision_checked(mutation);
        // Malformed batches are refused before any write.
        assert_eq!(
            refusal(CheckedPhotoDecisionMutation {
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(4),
                photos: vec![],
            })
            .await,
            Err(PhotoDecisionWriteError::Invalid)
        );
        assert_eq!(
            refusal(CheckedPhotoDecisionMutation {
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Rating(4),
                photos: vec![CheckedPhotoDecisionItem {
                    photo_id: ids[0].clone(),
                    expected_version: first.decision_version.clone(),
                }],
            })
            .await,
            Err(PhotoDecisionWriteError::Invalid)
        );
        assert_eq!(
            refusal(CheckedPhotoDecisionMutation {
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(MAXIMUM_PHOTO_RATING + 1),
                photos: vec![CheckedPhotoDecisionItem {
                    photo_id: ids[0].clone(),
                    expected_version: first.decision_version.clone(),
                }],
            })
            .await,
            Err(PhotoDecisionWriteError::Invalid)
        );
        assert_eq!(
            refusal(CheckedPhotoDecisionMutation {
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(4),
                photos: vec![
                    CheckedPhotoDecisionItem {
                        photo_id: ids[0].clone(),
                        expected_version: first.decision_version.clone(),
                    },
                    CheckedPhotoDecisionItem {
                        photo_id: ids[0].clone(),
                        expected_version: first.decision_version.clone(),
                    },
                ],
            })
            .await,
            Err(PhotoDecisionWriteError::Invalid)
        );
        assert_eq!(
            refusal(CheckedPhotoDecisionMutation {
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(4),
                photos: vec![CheckedPhotoDecisionItem {
                    photo_id: ids[0].clone(),
                    expected_version: String::new(),
                }],
            })
            .await,
            Err(PhotoDecisionWriteError::Invalid)
        );
        assert_eq!(
            refusal(CheckedPhotoDecisionMutation {
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(4),
                photos: (0..=crate::PHOTO_STATE_BATCH_MAX)
                    .map(|index| CheckedPhotoDecisionItem {
                        photo_id: format!("photo-{index}"),
                        expected_version: first.decision_version.clone(),
                    })
                    .collect(),
            })
            .await,
            Err(PhotoDecisionWriteError::LimitExceeded {
                limit: crate::PHOTO_STATE_BATCH_MAX,
                actual: crate::PHOTO_STATE_BATCH_MAX + 1,
            })
        );
        let untouched = read_photo(ids[0].clone()).await;
        assert_eq!(untouched.selection_state, first.selection_state);
        assert_eq!(untouched.rating, first.rating);
        assert_eq!(untouched.decision_version, first.decision_version);

        // A checked decision write changes neither the other decision field
        // nor an Album's saved browsing position or version.
        let album_id = persistence
            .mutate_album(AlbumMutation::Create {
                name: "Resume".to_owned(),
            })
            .await
            .unwrap()
            .album_id;
        persistence
            .mutate_album(AlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: vec![ids[0].clone()],
            })
            .await
            .unwrap();
        persistence
            .mutate_album(AlbumMutation::SetProgress {
                album_id: album_id.clone(),
                photo_id: ids[0].clone(),
            })
            .await
            .unwrap();
        let before = persistence
            .album_receiver(&album_id)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(before.has_saved_position);
        let changed = persistence
            .mutate_photo_decision_checked(CheckedPhotoDecisionMutation {
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(3),
                photos: vec![CheckedPhotoDecisionItem {
                    photo_id: ids[0].clone(),
                    expected_version: untouched.decision_version.clone(),
                }],
            })
            .await
            .unwrap();
        assert_eq!(changed.counts.changed, 1);
        let after_photo = read_photo(ids[0].clone()).await;
        assert_eq!(after_photo.rating, 3);
        assert_eq!(after_photo.selection_state, SelectionState::Undecided);
        let after_album = persistence
            .album_receiver(&album_id)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(after_album.has_saved_position);
        assert_eq!(after_album.album_version, before.album_version);
        assert_eq!(after_album.photo_count, before.photo_count);
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn checked_photo_decision_batch_rolls_back_siblings_and_preserves_versions() {
        let (_base, library, state, name, path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![
                    discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0),
                    discovered("two.JPG", OriginalKind::Jpeg, 2, 2.0),
                ],
                Vec::new(),
            )
            .await
            .unwrap();
        let ids = photo_ids(&snapshot);
        let escaped_id = ids[1].replace('\'', "''");
        Connection::open(&path)
            .unwrap()
            .execute_batch(&format!(
                "CREATE TRIGGER fail_checked_second BEFORE UPDATE OF selection_state ON photos\n                 WHEN OLD.id = '{escaped_id}'\n                 BEGIN SELECT RAISE(ABORT, 'forced checked failure'); END;"
            ))
            .unwrap();
        let read_photo = |photo_id: String| {
            let persistence = persistence.clone();
            async move {
                persistence
                    .photo_receiver(&photo_id)
                    .unwrap()
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap()
            }
        };
        let first = read_photo(ids[0].clone()).await;
        let second = read_photo(ids[1].clone()).await;
        assert_eq!(
            persistence
                .mutate_photo_decision_checked(CheckedPhotoDecisionMutation {
                    field: PhotoStateField::SelectionState,
                    value: PhotoStateValue::Selection(SelectionState::Selected),
                    photos: vec![
                        CheckedPhotoDecisionItem {
                            photo_id: ids[0].clone(),
                            expected_version: first.decision_version.clone(),
                        },
                        CheckedPhotoDecisionItem {
                            photo_id: ids[1].clone(),
                            expected_version: second.decision_version.clone(),
                        },
                    ],
                })
                .await,
            Err(PhotoDecisionWriteError::Persistence)
        );
        // Both siblings are unchanged and every version survives the
        // rollback; the next explicit write still uses the observed version.
        let after_first = read_photo(ids[0].clone()).await;
        let after_second = read_photo(ids[1].clone()).await;
        assert_eq!(after_first.selection_state, SelectionState::Undecided);
        assert_eq!(after_second.selection_state, SelectionState::Undecided);
        assert_eq!(after_first.decision_version, first.decision_version);
        assert_eq!(after_second.decision_version, second.decision_version);
        let recovered = persistence
            .mutate_photo_decision_checked(CheckedPhotoDecisionMutation {
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Rejected),
                photos: vec![CheckedPhotoDecisionItem {
                    photo_id: ids[0].clone(),
                    expected_version: first.decision_version.clone(),
                }],
            })
            .await
            .unwrap();
        assert_eq!(recovered.counts.changed, 1);
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn adding_an_existing_album_member_is_idempotent() {
        let (_base, library, state, name, path) = fixture();
        let originals = vec![
            discovered("one.ARW", OriginalKind::Raw, 3, 1000.0),
            discovered("two.ARW", OriginalKind::Raw, 4, 1000.0),
            discovered("three.ARW", OriginalKind::Raw, 5, 1000.0),
        ];
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v5.sql"),
        );
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence.apply_scan(originals, Vec::new()).await.unwrap();
        let photo_one = snapshot.photos[0].id.clone();
        let photo_two = snapshot.photos[1].id.clone();
        let photo_three = snapshot.photos[2].id.clone();
        let created = persistence
            .mutate_album(AlbumMutation::Create {
                name: "Idempotent".to_owned(),
            })
            .await
            .unwrap();
        let album_id = created.album_id;
        persistence
            .mutate_album(AlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: vec![photo_one.clone(), photo_two.clone()],
            })
            .await
            .unwrap();
        // Re-adding a persisted member succeeds and keeps its position.
        persistence
            .mutate_album(AlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: vec![photo_two.clone()],
            })
            .await
            .unwrap();
        let album = &persistence.list_albums().await.unwrap()[0];
        assert_eq!(album.members.len(), 2);
        assert_eq!(album.members[0].photo_id, photo_one);
        assert_eq!(album.members[0].position, 0);
        assert_eq!(album.members[1].photo_id, photo_two);
        assert_eq!(album.members[1].position, 1);
        // A mixed request appends only the new member after existing ones.
        persistence
            .mutate_album(AlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: vec![photo_two, photo_three.clone()],
            })
            .await
            .unwrap();
        let album = &persistence.list_albums().await.unwrap()[0];
        assert_eq!(album.members.len(), 3);
        assert_eq!(album.members[2].photo_id, photo_three);
        assert_eq!(album.members[2].position, 2);
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn membership_batch_reports_identities_and_compensates_idempotently() {
        let (_base, library, state, name, _path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![
                    discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0),
                    discovered("two.JPG", OriginalKind::Jpeg, 2, 2.0),
                    discovered("three.JPG", OriginalKind::Jpeg, 3, 3.0),
                ],
                Vec::new(),
            )
            .await
            .unwrap();
        let ids = photo_ids(&snapshot);
        let album_id = persistence
            .mutate_album(AlbumMutation::Create {
                name: "Compensation".to_owned(),
            })
            .await
            .unwrap()
            .album_id;
        persistence
            .mutate_album(AlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: vec![ids[0].clone(), ids[1].clone()],
            })
            .await
            .unwrap();
        persistence
            .mutate_album(AlbumMutation::SetProgress {
                album_id: album_id.clone(),
                photo_id: ids[1].clone(),
            })
            .await
            .unwrap();

        let add = persistence
            .mutate_album_membership(AlbumMembershipMutation::Add {
                album_id: album_id.clone(),
                photo_ids: vec![ids[1].clone(), ids[2].clone()],
            })
            .await
            .unwrap();
        assert_eq!(add.added_photo_ids, vec![ids[2].clone()]);
        assert_eq!(add.already_member_photo_ids, vec![ids[1].clone()]);
        let album = &persistence.list_albums().await.unwrap()[0];
        assert_eq!(
            album
                .members
                .iter()
                .map(|member| member.photo_id.clone())
                .collect::<Vec<_>>(),
            vec![ids[0].clone(), ids[1].clone(), ids[2].clone()]
        );
        assert_eq!(album.members[0].position, 0);
        assert_eq!(album.members[1].position, 1);
        assert_eq!(album.members[2].position, 2);
        assert_eq!(
            album.last_reviewed_photo_id.as_deref(),
            Some(ids[1].as_str())
        );

        let removed = persistence
            .mutate_album_membership(AlbumMembershipMutation::RemoveAdded {
                album_id: album_id.clone(),
                photo_ids: vec![ids[1].clone()],
            })
            .await
            .unwrap();
        assert_eq!(removed.removed_photo_ids, vec![ids[1].clone()]);
        assert!(removed.already_absent_photo_ids.is_empty());
        let album = &persistence.list_albums().await.unwrap()[0];
        assert_eq!(
            album
                .members
                .iter()
                .map(|member| member.photo_id.clone())
                .collect::<Vec<_>>(),
            vec![ids[0].clone(), ids[2].clone()]
        );
        assert_eq!(album.members[0].position, 0);
        assert_eq!(album.members[1].position, 1);
        assert_eq!(album.last_reviewed_photo_id, None);

        let repeated = persistence
            .mutate_album_membership(AlbumMembershipMutation::RemoveAdded {
                album_id: album_id.clone(),
                photo_ids: vec![ids[1].clone()],
            })
            .await
            .unwrap();
        assert!(repeated.removed_photo_ids.is_empty());
        assert_eq!(repeated.already_absent_photo_ids, vec![ids[1].clone()]);
        let removed_tail = persistence
            .mutate_album_membership(AlbumMembershipMutation::RemoveAdded {
                album_id: album_id.clone(),
                photo_ids: vec![ids[2].clone()],
            })
            .await
            .unwrap();
        assert_eq!(removed_tail.removed_photo_ids, vec![ids[2].clone()]);
        assert!(removed_tail.already_absent_photo_ids.is_empty());
        let album = &persistence.list_albums().await.unwrap()[0];
        assert_eq!(album.members.len(), 1);
        assert_eq!(album.last_reviewed_photo_id, None);
        assert_eq!(
            persistence
                .mutate_album_membership(AlbumMembershipMutation::RemoveAdded {
                    album_id: album_id.clone(),
                    photo_ids: vec!["00000000-0000-4000-8000-00000000dead".to_owned()],
                })
                .await,
            Err(MutationError::NotFound)
        );
        assert_eq!(persistence.list_albums().await.unwrap()[0].members.len(), 1);
        assert_eq!(
            persistence
                .mutate_album_membership(AlbumMembershipMutation::Add {
                    album_id,
                    photo_ids: vec![ids[0].clone(), ids[0].clone()],
                })
                .await,
            Err(MutationError::Invalid)
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn folder_members_mutation_is_atomic_and_reports_idempotent_counts() {
        let (_base, library, state, name, _path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![
                    discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0),
                    discovered("two.JPG", OriginalKind::Jpeg, 2, 2.0),
                    discovered("three.JPG", OriginalKind::Jpeg, 3, 3.0),
                ],
                Vec::new(),
            )
            .await
            .unwrap();
        let ids = photo_ids(&snapshot);
        let album_id = persistence
            .mutate_album(AlbumMutation::Create {
                name: "Folder".to_owned(),
            })
            .await
            .unwrap()
            .album_id;

        let first = persistence
            .mutate_album(AlbumMutation::AddFolderMembers {
                album_id: album_id.clone(),
                photo_ids: vec![ids[0].clone(), ids[1].clone()],
            })
            .await
            .unwrap();
        assert_eq!(first.added_count, 2);
        assert_eq!(first.already_member_count, 0);

        let repeated = persistence
            .mutate_album(AlbumMutation::AddFolderMembers {
                album_id: album_id.clone(),
                photo_ids: vec![ids[1].clone(), ids[2].clone(), ids[0].clone()],
            })
            .await
            .unwrap();
        assert_eq!(repeated.added_count, 1);
        assert_eq!(repeated.already_member_count, 2);
        let members = &persistence.list_albums().await.unwrap()[0].members;
        assert_eq!(
            members
                .iter()
                .map(|member| member.photo_id.clone())
                .collect::<Vec<_>>(),
            vec![ids[0].clone(), ids[1].clone(), ids[2].clone()]
        );
        assert_eq!(
            persistence
                .mutate_album(AlbumMutation::AddFolderMembers {
                    album_id,
                    photo_ids: vec![ids[0].clone(), ids[0].clone()],
                })
                .await,
            Err(MutationError::Conflict)
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn album_name_uniqueness_folds_ascii_case_only() {
        let (_base, library, state, name, _path) = fixture();
        fs::write(library.canonical_path().join("one.JPG"), b"bytes").unwrap();
        let photo = discovered("one.JPG", OriginalKind::Jpeg, 4, 1000.0);
        let persistence = Persistence::open(
            state,
            name.clone(),
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        persistence
            .apply_scan(vec![photo], Vec::new())
            .await
            .unwrap();

        // Exact duplicates conflict in every script, including caseless ones.
        persistence
            .mutate_album(AlbumMutation::Create {
                name: "春节".to_owned(),
            })
            .await
            .unwrap();
        assert!(matches!(
            persistence
                .mutate_album(AlbumMutation::Create {
                    name: "春节".to_owned(),
                })
                .await,
            Err(MutationError::Conflict)
        ));

        // ASCII letter case folds: Trip and trip are the same Album name.
        persistence
            .mutate_album(AlbumMutation::Create {
                name: "Trip".to_owned(),
            })
            .await
            .unwrap();
        assert!(matches!(
            persistence
                .mutate_album(AlbumMutation::Create {
                    name: "trip".to_owned(),
                })
                .await,
            Err(MutationError::Conflict)
        ));

        // Documented boundary: folding applies to ASCII letters only. A name
        // whose case difference is carried by a non-ASCII letter (É vs é)
        // does not fold and stays a distinct Album name.
        persistence
            .mutate_album(AlbumMutation::Create {
                name: "Éclair".to_owned(),
            })
            .await
            .unwrap();
        persistence
            .mutate_album(AlbumMutation::Create {
                name: "éclair".to_owned(),
            })
            .await
            .unwrap();
        let names = persistence
            .list_albums()
            .await
            .unwrap()
            .into_iter()
            .map(|album| album.name)
            .collect::<Vec<_>>();
        assert!(names.contains(&"Éclair".to_owned()));
        assert!(names.contains(&"éclair".to_owned()));
    }

    #[tokio::test]
    async fn album_crud_normalizes_names_and_preserves_rows_across_restart() {
        let (_base, library, state, name, path) = fixture();
        let original_path = library.canonical_path().join("one.JPG");
        fs::write(&original_path, b"original bytes").unwrap();
        let original_bytes = fs::read(&original_path).unwrap();
        let photo = discovered("one.JPG", OriginalKind::Jpeg, 4, 1000.0);
        let persistence = Persistence::open(
            state,
            name.clone(),
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(vec![photo], Vec::new())
            .await
            .unwrap();
        let photo_id = snapshot.photos[0].id.clone();
        let created = persistence
            .mutate_album(AlbumMutation::Create {
                name: "  Picks  ".to_owned(),
            })
            .await
            .unwrap();
        let album_id = created.album_id;
        persistence
            .mutate_album(AlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: vec![photo_id.clone()],
            })
            .await
            .unwrap();
        assert_eq!(
            persistence
                .mutate_album(AlbumMutation::Create {
                    name: "pIcKs".to_owned(),
                })
                .await,
            Err(MutationError::Conflict)
        );
        persistence
            .mutate_album(AlbumMutation::Rename {
                album_id: album_id.clone(),
                name: " Renamed ".to_owned(),
            })
            .await
            .unwrap();
        let albums = persistence.list_albums().await.unwrap();
        assert_eq!(albums[0].name, "Renamed");
        persistence.shutdown().unwrap();

        let state = StateDirectory::open_or_create(&library, path.parent().unwrap()).unwrap();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let albums = persistence.list_albums().await.unwrap();
        assert_eq!(albums[0].id, album_id);
        assert_eq!(albums[0].members[0].photo_id, photo_id);
        persistence
            .mutate_album(AlbumMutation::Delete { album_id })
            .await
            .unwrap();
        assert!(persistence.list_albums().await.unwrap().is_empty());
        let connection = Connection::open(path).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM photos", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(fs::read(&original_path).unwrap(), original_bytes);
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn membership_batches_are_atomic_and_order_operations_are_dense() {
        let (_base, library, state, name, path) = fixture();
        let persistence = Persistence::open(
            state,
            name.clone(),
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![
                    discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0),
                    discovered("two.JPG", OriginalKind::Jpeg, 2, 2.0),
                    discovered("three.JPG", OriginalKind::Jpeg, 3, 3.0),
                ],
                Vec::new(),
            )
            .await
            .unwrap();
        let ids = photo_ids(&snapshot);
        let album_id = persistence
            .mutate_album(AlbumMutation::Create {
                name: "Order".to_owned(),
            })
            .await
            .unwrap()
            .album_id;
        assert_eq!(
            persistence
                .mutate_album(AlbumMutation::AddMembers {
                    album_id: album_id.clone(),
                    photo_ids: vec![ids[0].clone(), ids[0].clone()],
                })
                .await,
            Err(MutationError::Conflict)
        );
        assert_eq!(persistence.list_albums().await.unwrap()[0].members.len(), 0);
        assert_eq!(
            persistence
                .mutate_album(AlbumMutation::AddMembers {
                    album_id: album_id.clone(),
                    photo_ids: vec![ids[0].clone(), "unknown".to_owned()],
                })
                .await,
            Err(MutationError::NotFound)
        );
        assert_eq!(persistence.list_albums().await.unwrap()[0].members.len(), 0);
        assert_eq!(
            persistence
                .mutate_album(AlbumMutation::AddMembers {
                    album_id: album_id.clone(),
                    photo_ids: (0..=100).map(|index| format!("unknown-{index}")).collect(),
                })
                .await,
            Err(MutationError::Conflict)
        );
        persistence
            .mutate_album(AlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: ids.clone(),
            })
            .await
            .unwrap();
        assert_eq!(
            persistence
                .mutate_album(AlbumMutation::Reorder {
                    album_id: album_id.clone(),
                    photo_ids: vec![ids[2].clone(), ids[0].clone(), ids[1].clone()],
                })
                .await
                .unwrap()
                .album_id,
            album_id
        );
        assert_eq!(
            persistence.list_albums().await.unwrap()[0]
                .members
                .iter()
                .map(|member| member.photo_id.clone())
                .collect::<Vec<_>>(),
            vec![ids[2].clone(), ids[0].clone(), ids[1].clone()]
        );
        assert_eq!(
            persistence
                .mutate_album(AlbumMutation::Reorder {
                    album_id: album_id.clone(),
                    photo_ids: vec![ids[0].clone(), ids[0].clone(), ids[1].clone()],
                })
                .await,
            Err(MutationError::Conflict)
        );
        persistence
            .mutate_album(AlbumMutation::RemoveMember {
                album_id: album_id.clone(),
                photo_id: ids[0].clone(),
            })
            .await
            .unwrap();
        let members = &persistence.list_albums().await.unwrap()[0].members;
        assert_eq!(
            members
                .iter()
                .map(|member| member.position)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        persistence.shutdown().unwrap();

        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE album_members SET position=9 WHERE album_id=? AND photo_id=?",
                params![album_id, ids[1]],
            )
            .unwrap();
        drop(connection);
        let state = StateDirectory::open_or_create(&library, path.parent().unwrap()).unwrap();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        assert_eq!(
            persistence
                .mutate_album(AlbumMutation::Reorder {
                    album_id: album_id.clone(),
                    photo_ids: vec![ids[1].clone(), ids[2].clone()],
                })
                .await,
            Err(MutationError::Conflict)
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn progress_state_cas_undo_and_atomic_progress_are_scoped_and_global() {
        let (_base, library, state, name, _path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![
                    discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0),
                    discovered("two.JPG", OriginalKind::Jpeg, 2, 2.0),
                ],
                Vec::new(),
            )
            .await
            .unwrap();
        let ids = photo_ids(&snapshot);
        let album_a = persistence
            .mutate_album(AlbumMutation::Create {
                name: "A".to_owned(),
            })
            .await
            .unwrap()
            .album_id;
        let album_b = persistence
            .mutate_album(AlbumMutation::Create {
                name: "B".to_owned(),
            })
            .await
            .unwrap()
            .album_id;
        for album_id in [&album_a, &album_b] {
            persistence
                .mutate_album(AlbumMutation::AddMembers {
                    album_id: album_id.clone(),
                    photo_ids: ids.clone(),
                })
                .await
                .unwrap();
        }
        persistence
            .mutate_album(AlbumMutation::SetProgress {
                album_id: album_a.clone(),
                photo_id: ids[0].clone(),
            })
            .await
            .unwrap();
        let albums = persistence.list_albums().await.unwrap();
        assert_eq!(
            albums
                .iter()
                .find(|album| album.id == album_a)
                .unwrap()
                .last_reviewed_photo_id
                .as_deref(),
            Some(ids[0].as_str())
        );
        let result = persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: ids[0].clone(),
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Selected),
                expected_current: Some(PhotoStateValue::Selection(SelectionState::Undecided)),
                album_id: Some(album_b.clone()),
            })
            .await
            .unwrap();
        assert_eq!(
            result.undo.prior_value,
            PhotoStateValue::Selection(SelectionState::Undecided)
        );
        let albums = persistence.list_albums().await.unwrap();
        let current_a = albums.iter().find(|album| album.id == album_a).unwrap();
        let current_b = albums.iter().find(|album| album.id == album_b).unwrap();
        assert_eq!(
            current_a.members[0].selection_state,
            SelectionState::Selected
        );
        assert_eq!(
            current_b.members[0].selection_state,
            SelectionState::Selected
        );
        assert_eq!(
            current_b.last_reviewed_photo_id.as_deref(),
            Some(ids[0].as_str())
        );
        assert_eq!(
            persistence
                .mutate_photo_state(PhotoStateMutation {
                    photo_id: ids[0].clone(),
                    field: PhotoStateField::SelectionState,
                    value: result.undo.prior_value,
                    expected_current: Some(PhotoStateValue::Selection(SelectionState::Undecided)),
                    album_id: None,
                })
                .await,
            Err(MutationError::Conflict)
        );
        persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: ids[0].clone(),
                field: PhotoStateField::SelectionState,
                value: result.undo.prior_value,
                expected_current: Some(result.undo.expected_current),
                album_id: None,
            })
            .await
            .unwrap();
        persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: ids[1].clone(),
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(5),
                expected_current: Some(PhotoStateValue::Rating(0)),
                album_id: Some(album_a.clone()),
            })
            .await
            .unwrap();
        let albums = persistence.list_albums().await.unwrap();
        let current_a = albums.iter().find(|album| album.id == album_a).unwrap();
        let current_b = albums.iter().find(|album| album.id == album_b).unwrap();
        assert_eq!(current_a.members[1].rating, 5);
        assert_eq!(current_b.members[1].rating, 5);
        assert_eq!(
            current_a.last_reviewed_photo_id.as_deref(),
            Some(ids[1].as_str())
        );
        persistence
            .mutate_album(AlbumMutation::RemoveMember {
                album_id: album_a.clone(),
                photo_id: ids[1].clone(),
            })
            .await
            .unwrap();
        let albums = persistence.list_albums().await.unwrap();
        assert_eq!(
            albums
                .iter()
                .find(|album| album.id == album_a)
                .unwrap()
                .last_reviewed_photo_id,
            None
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn unavailable_members_keep_state_and_sidecar_admission_blocks_writes() {
        let (_base, library, state, name, path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0)],
                Vec::new(),
            )
            .await
            .unwrap();
        let photo_id = snapshot.photos[0].id.clone();
        persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: photo_id.clone(),
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(4),
                expected_current: None,
                album_id: None,
            })
            .await
            .unwrap();
        let album_id = persistence
            .mutate_album(AlbumMutation::Create {
                name: "Keep".to_owned(),
            })
            .await
            .unwrap()
            .album_id;
        persistence
            .mutate_album(AlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: vec![photo_id.clone()],
            })
            .await
            .unwrap();
        persistence
            .apply_scan(Vec::new(), Vec::new())
            .await
            .unwrap();
        let member = &persistence.list_albums().await.unwrap()[0].members[0];
        assert!(!member.available);
        assert_eq!(member.rating, 4);
        let sidecar = path.with_file_name("library.sqlite-journal");
        fs::write(&sidecar, b"operator recovery data").unwrap();
        fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o600)).unwrap();
        let before = fs::read(&path).unwrap();
        assert_eq!(
            persistence
                .mutate_album(AlbumMutation::Create {
                    name: "Blocked".to_owned()
                })
                .await,
            Err(MutationError::Persistence)
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::read(&sidecar).unwrap(), b"operator recovery data");
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn shutdown_rejects_library_mutations_after_lifecycle_close() {
        let (_base, library_root, state, name, _path) = fixture();
        let library = crate::Library::open(crate::LibraryConfig {
            library_root: library_root.canonical_path().to_owned(),
            state_directory: state.canonical_path().to_owned(),
            database_basename: name.as_os_str().to_string_lossy().into_owned(),
            ..crate::LibraryConfig::default()
        })
        .unwrap();
        library.shutdown().unwrap();
        assert!(matches!(
            library.list_albums().await,
            Err(crate::LibraryError::Closed)
        ));
        assert!(matches!(
            library
                .mutate_album(AlbumMutation::Create {
                    name: "Nope".to_owned()
                })
                .await,
            Err(crate::LibraryError::Closed)
        ));
    }

    /// The v8 fixture carries the Photos, decisions, and Album membership the
    /// migration must preserve, and the new removal marker starts empty.
    #[tokio::test]
    async fn v8_to_v9_migration_preserves_photos_and_starts_unremoved() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v8.sql"),
        );
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
                [library.canonical_path().to_str().unwrap()],
            )
            .unwrap();
        add_recipe_test_photo(
            &connection,
            RecipeTestPhoto {
                original_id: "raw-original",
                photo_id: "raw-photo",
                relative_path: "shoot/one.ARW",
                kind: "raw",
                available: true,
                size: 17,
                mtime_ms: 1_000.0,
            },
        );
        connection
            .execute(
                "UPDATE photos SET selection_state='rejected',rating=4 WHERE id='raw-photo'",
                [],
            )
            .unwrap();
        drop(connection);

        let persistence = Persistence::open(
            state,
            name.clone(),
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .snapshot_receiver()
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.photos.len(), 1);
        assert_eq!(snapshot.photos[0].selection_state, SelectionState::Rejected);
        assert_eq!(snapshot.photos[0].rating, 4);
        assert!(!snapshot.photos[0].removed);

        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                .unwrap(),
            9
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT removed_at_ms,removed_operation FROM photos WHERE id='raw-photo'",
                    [],
                    |row| Ok((
                        row.get::<_, Option<i64>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                    )),
                )
                .unwrap(),
            (None, None)
        );
        drop(connection);
        persistence.shutdown().unwrap();
    }

    /// One removal reports exactly one outcome per requested Photo, a retried
    /// request adopts what its own operation already removed, and restore
    /// compares against the current marker instead of overwriting it.
    #[tokio::test]
    async fn a_library_without_the_marker_high_water_never_repeats_a_marker_it_still_holds() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v9.sql"),
        );
        // A Library written before the high water mark existed carries removal
        // markers but no row for them. The greatest marker it still holds is
        // the floor for the next one, so the marker already stored for the
        // Photo cannot be assigned to a newer removal of it.
        let held = unix_millis() + 60_000;
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
                [library.canonical_path().to_str().unwrap()],
            )
            .unwrap();
        for index in [1, 2] {
            add_recipe_test_photo(
                &connection,
                RecipeTestPhoto {
                    original_id: &format!("original-{index}"),
                    photo_id: &format!("photo-{index}"),
                    relative_path: &format!("shoot/one-{index}.ARW"),
                    kind: "raw",
                    available: true,
                    size: 17,
                    mtime_ms: 1_000.0,
                },
            );
            connection
                .execute(
                    "UPDATE photos SET selection_state='rejected' WHERE id=?",
                    [format!("photo-{index}")],
                )
                .unwrap();
        }
        connection
            .execute(
                "UPDATE photos SET removed_at_ms=?,removed_operation='operation-held' WHERE id='photo-1'",
                [held],
            )
            .unwrap();
        let high_water: Option<String> = connection
            .query_row(
                "SELECT value FROM library_metadata WHERE key='removal_marker_high_water'",
                [],
                |row| row.get(0),
            )
            .optional()
            .unwrap();
        assert_eq!(high_water, None);
        drop(connection);

        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let removed = persistence
            .remove_photos_receiver(PhotoRemovalMutation {
                photo_ids: vec!["photo-2".to_owned()],
                operation_id: "operation-next".to_owned(),
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(removed.counts.removed, 1);
        let (records, _) = persistence
            .removed_photos_receiver(0, 10)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let marker = records
            .iter()
            .find(|record| record.photo_id == "photo-2")
            .unwrap()
            .removed_at_ms;
        assert!(marker > held);
        let held_marker = records
            .iter()
            .find(|record| record.photo_id == "photo-1")
            .unwrap()
            .removed_at_ms;
        assert_eq!(held_marker, held);
    }

    #[tokio::test]
    async fn removal_markers_never_repeat_even_when_the_clock_does_not_advance() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v8.sql"),
        );
        // The high water mark is seeded far ahead of the clock, so a marker
        // derived from the clock alone would repeat the marker already stored
        // for the Photo and this test would see a stale listing clear a newer
        // removal.
        let ahead = unix_millis() + 60_000;
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
                [library.canonical_path().to_str().unwrap()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('removal_marker_high_water',?)",
                [ahead.to_string()],
            )
            .unwrap();
        add_recipe_test_photo(
            &connection,
            RecipeTestPhoto {
                original_id: "original-1",
                photo_id: "photo-1",
                relative_path: "shoot/one-1.ARW",
                kind: "raw",
                available: true,
                size: 17,
                mtime_ms: 1_000.0,
            },
        );
        connection
            .execute(
                "UPDATE photos SET selection_state='rejected' WHERE id='photo-1'",
                [],
            )
            .unwrap();
        drop(connection);

        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let remove = |operation_id: &str| {
            persistence
                .remove_photos_receiver(PhotoRemovalMutation {
                    photo_ids: vec!["photo-1".to_owned()],
                    operation_id: operation_id.to_owned(),
                })
                .unwrap()
        };
        let restore = |marker: i64| {
            persistence
                .restore_photos_receiver(PhotoRestoration::Photos(vec![PhotoRemovalMarker {
                    photo_id: "photo-1".to_owned(),
                    removed_at_ms: marker,
                }]))
                .unwrap()
        };
        let marker_of = async |persistence: &Persistence| -> Option<i64> {
            let (records, _) = persistence
                .removed_photos_receiver(0, 10)
                .unwrap()
                .await
                .unwrap()
                .unwrap();
            records
                .iter()
                .find(|record| record.photo_id == "photo-1")
                .map(|record| record.removed_at_ms)
        };

        assert_eq!(
            remove("operation-one")
                .await
                .unwrap()
                .unwrap()
                .counts
                .removed,
            1
        );
        let first = marker_of(&persistence).await.unwrap();
        assert!(first > ahead);
        assert_eq!(restore(first).await.unwrap().unwrap().counts.restored, 1);

        assert_eq!(
            remove("operation-two")
                .await
                .unwrap()
                .unwrap()
                .counts
                .removed,
            1
        );
        let second = marker_of(&persistence).await.unwrap();
        assert!(second > first);

        // The listing read under the first removal names a marker the Library
        // no longer assigns to this Photo, so it restores nothing.
        let superseded = restore(first).await.unwrap().unwrap();
        assert_eq!(superseded.counts.restored, 0);
        assert_eq!(superseded.changed_elsewhere, vec!["photo-1".to_owned()]);
        assert_eq!(marker_of(&persistence).await, Some(second));
        assert_eq!(restore(second).await.unwrap().unwrap().counts.restored, 1);
        assert_eq!(marker_of(&persistence).await, None);
    }

    #[tokio::test]
    async fn removal_outcomes_operation_identity_and_restore_are_exact() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v8.sql"),
        );
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
                [library.canonical_path().to_str().unwrap()],
            )
            .unwrap();
        for (index, state_value) in [(1, "rejected"), (2, "undecided"), (3, "rejected")] {
            add_recipe_test_photo(
                &connection,
                RecipeTestPhoto {
                    original_id: &format!("original-{index}"),
                    photo_id: &format!("photo-{index}"),
                    relative_path: &format!("shoot/one-{index}.ARW"),
                    kind: "raw",
                    available: true,
                    size: 17,
                    mtime_ms: 1_000.0,
                },
            );
            connection
                .execute(
                    "UPDATE photos SET selection_state=? WHERE id=?",
                    params![state_value, format!("photo-{index}")],
                )
                .unwrap();
        }
        drop(connection);

        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let remove = |photo_ids: Vec<&str>, operation_id: &str| {
            persistence
                .remove_photos_receiver(PhotoRemovalMutation {
                    photo_ids: photo_ids.into_iter().map(str::to_owned).collect(),
                    operation_id: operation_id.to_owned(),
                })
                .unwrap()
        };
        let result = remove(vec!["photo-1", "photo-2", "photo-missing"], "operation-one")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.removed, vec!["photo-1".to_owned()]);
        assert_eq!(result.changed_elsewhere, vec!["photo-2".to_owned()]);
        assert_eq!(result.missing, vec!["photo-missing".to_owned()]);
        assert!(result.already_removed.is_empty());
        // One outcome per requested Photo: the counts are the lists.
        assert_eq!(
            result.counts,
            PhotoRemovalCounts {
                removed: 1,
                changed_elsewhere: 1,
                missing: 1,
                already_removed: 0,
            }
        );

        // A retried request repeats its own operation instead of reporting a
        // second outcome set.
        let retried = remove(vec!["photo-1", "photo-3"], "operation-one")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retried.counts.removed, 2);
        assert_eq!(
            retried.removed,
            vec!["photo-1".to_owned(), "photo-3".to_owned()]
        );
        assert!(retried.already_removed.is_empty());

        let other = remove(vec!["photo-1"], "operation-two")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(other.counts.removed, 0);
        assert_eq!(other.counts.already_removed, 1);
        assert_eq!(other.already_removed, vec!["photo-1".to_owned()]);

        let (records, total) = persistence
            .removed_photos_receiver(0, 10)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(total, 2);
        assert_eq!(records.len(), 2);
        assert!(records.iter().all(|record| record.removed_at_ms >= 0));

        // Restore by operation returns the group that operation still owns.
        let restored = persistence
            .restore_photos_receiver(PhotoRestoration::Operation("operation-one".to_owned()))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restored.counts.restored, 2);
        assert_eq!(restored.counts.missing, 0);
        assert_eq!(
            restored.restored.iter().cloned().collect::<HashSet<_>>(),
            HashSet::from(["photo-1".to_owned(), "photo-3".to_owned()])
        );
        let second = persistence
            .restore_photos_receiver(PhotoRestoration::Operation("operation-one".to_owned()))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second.counts.restored, 0);

        // A named restore compares and sets against the marker it reviewed:
        // a Photo already in the Library, and a Photo that does not exist, are
        // reported instead of cleared.
        let named = persistence
            .restore_photos_receiver(PhotoRestoration::Photos(vec![
                PhotoRemovalMarker {
                    photo_id: "photo-1".to_owned(),
                    removed_at_ms: 0,
                },
                PhotoRemovalMarker {
                    photo_id: "photo-missing".to_owned(),
                    removed_at_ms: 0,
                },
            ]))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(named.changed_elsewhere, vec!["photo-1".to_owned()]);
        assert_eq!(named.missing, vec!["photo-missing".to_owned()]);
        assert!(named.operations.is_empty());

        // A stale marker never overwrites a newer removal: the Photo is
        // reported as changed elsewhere and stays removed by its own
        // operation, whose remaining count the response carries.
        persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: "photo-2".to_owned(),
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Rejected),
                expected_current: None,
                album_id: None,
            })
            .await
            .unwrap();
        let re_removed = remove(vec!["photo-2"], "operation-two")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(re_removed.counts.removed, 1);
        let (records, _) = persistence
            .removed_photos_receiver(0, 10)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let stale_marker = records
            .iter()
            .find(|record| record.photo_id == "photo-2")
            .unwrap()
            .removed_at_ms;
        let stale = persistence
            .restore_photos_receiver(PhotoRestoration::Photos(vec![PhotoRemovalMarker {
                photo_id: "photo-2".to_owned(),
                removed_at_ms: stale_marker - 1,
            }]))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stale.counts.restored, 0);
        assert_eq!(stale.changed_elsewhere, vec!["photo-2".to_owned()]);
        assert!(stale.operations.is_empty());
        let (records, total) = persistence
            .removed_photos_receiver(0, 10)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(total, 1);
        assert_eq!(records[0].photo_id, "photo-2");

        // The reviewed marker restores exactly that removal, and the response
        // reports that the operation now owns nothing.
        let exact = persistence
            .restore_photos_receiver(PhotoRestoration::Photos(vec![PhotoRemovalMarker {
                photo_id: "photo-2".to_owned(),
                removed_at_ms: stale_marker,
            }]))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(exact.restored, vec!["photo-2".to_owned()]);
        assert_eq!(
            exact.operations,
            vec![PhotoOperationRemainder {
                operation_id: "operation-two".to_owned(),
                removed: 0,
            }]
        );
        let (records, total) = persistence
            .removed_photos_receiver(0, 10)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(total, 0);
        assert!(records.is_empty());

        // A marker is never reused. A Photo restored and removed again carries
        // a strictly greater marker, so the listing read under the first
        // removal can never clear the second one — even when both removals
        // fall inside the same clock millisecond.
        let re_removed = remove(vec!["photo-2"], "operation-three")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(re_removed.counts.removed, 1);
        let (records, _) = persistence
            .removed_photos_receiver(0, 10)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let second_marker = records
            .iter()
            .find(|record| record.photo_id == "photo-2")
            .unwrap()
            .removed_at_ms;
        assert!(second_marker > stale_marker);
        let superseded = persistence
            .restore_photos_receiver(PhotoRestoration::Photos(vec![PhotoRemovalMarker {
                photo_id: "photo-2".to_owned(),
                removed_at_ms: stale_marker,
            }]))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(superseded.counts.restored, 0);
        assert_eq!(superseded.changed_elsewhere, vec!["photo-2".to_owned()]);
        let (records, total) = persistence
            .removed_photos_receiver(0, 10)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(total, 1);
        assert_eq!(records[0].removed_at_ms, second_marker);

        // Removal is Library state only: decisions and identity are untouched.
        let snapshot = persistence
            .snapshot_receiver()
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let photo = snapshot
            .photos
            .iter()
            .find(|photo| photo.id == "photo-1")
            .unwrap();
        assert_eq!(photo.selection_state, SelectionState::Rejected);
        assert!(!photo.removed);
        assert_eq!(photo.original_id, "original-1");
        persistence.shutdown().unwrap();
    }

    /// A removed Photo leaves every normal source and Album count while its
    /// membership rows stay intact for restore.
    #[tokio::test]
    async fn removed_photos_leave_normal_sources_and_album_counts() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v8.sql"),
        );
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
                [library.canonical_path().to_str().unwrap()],
            )
            .unwrap();
        for index in 1..=2 {
            add_recipe_test_photo(
                &connection,
                RecipeTestPhoto {
                    original_id: &format!("original-{index}"),
                    photo_id: &format!("photo-{index}"),
                    relative_path: &format!("shoot/one-{index}.ARW"),
                    kind: "raw",
                    available: true,
                    size: 17,
                    mtime_ms: 1_000.0,
                },
            );
            connection
                .execute(
                    "UPDATE photos SET selection_state='rejected' WHERE id=?",
                    [format!("photo-{index}")],
                )
                .unwrap();
        }
        drop(connection);

        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let album = persistence
            .mutate_album_receiver(AlbumMutation::Create {
                name: "Keepers".to_owned(),
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        persistence
            .mutate_album_receiver(AlbumMutation::AddMembers {
                album_id: album.album_id.clone(),
                photo_ids: vec!["photo-1".to_owned(), "photo-2".to_owned()],
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        persistence
            .remove_photos_receiver(PhotoRemovalMutation {
                photo_ids: vec!["photo-1".to_owned()],
                operation_id: "operation-one".to_owned(),
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();

        let summaries = persistence
            .list_album_summaries_receiver()
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].photo_count, 1);
        let target = persistence
            .album_browse_target_receiver(&album.album_id)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            target
                .members
                .iter()
                .map(|member| member.photo_id.clone())
                .collect::<Vec<_>>(),
            vec!["photo-2".to_owned()]
        );
        let albums = persistence
            .list_albums_receiver()
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(albums[0].members.len(), 1);

        // The membership row survives for restore.
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM album_members WHERE album_id=?",
                    [&album.album_id],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
        drop(connection);

        let snapshot = persistence
            .snapshot_receiver()
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let projection = query_projection(&snapshot);
        let ids = persistence
            .create_photo_query_receiver(
                PhotoQuery {
                    source: PhotoQuerySource::AllPhotos,
                    selection_state: None,
                    rating_minimum: None,
                    rating_maximum: None,
                    original_kind: None,
                    original_available: None,
                    captured_from: None,
                    captured_before: None,
                    order: PhotoQueryOrder::CaptureTimeAscending,
                },
                projection,
                100,
            )
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ids, vec!["photo-2".to_owned()]);
        persistence.shutdown().unwrap();
    }

    /// Builds the scan-owned query projection one Published Library would
    /// share, from the same persisted snapshot the server publishes.
    #[tokio::test]
    async fn saturation_and_shutdown_drain_are_explicit() {
        let (_base, library, state, name, _path) = fixture();
        let persistence = Persistence::open_with_capacity(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
            NonZeroUsize::new(1).unwrap(),
        )
        .unwrap();
        let (entered_send, entered_receive) = oneshot::channel();
        let (release_send, release_receive) = std::sync::mpsc::channel();
        let (reply, receive) = oneshot::channel();
        persistence
            .submit(Command::Block {
                entered: entered_send,
                release: release_receive,
                reply,
            })
            .unwrap();
        entered_receive.await.unwrap();
        let (queued_reply, queued_receive) = oneshot::channel();
        persistence.submit(Command::Probe(queued_reply)).unwrap();
        let (full_reply, _) = oneshot::channel();
        assert!(matches!(
            persistence.submit(Command::Probe(full_reply)),
            Err(PersistenceError::Saturated)
        ));

        let shutdown_handle = persistence.clone();
        let shutdown = tokio::task::spawn_blocking(move || shutdown_handle.shutdown());
        tokio::task::yield_now().await;
        release_send.send(()).unwrap();
        assert!(receive.await.unwrap().is_ok());
        assert!(queued_receive.await.unwrap().is_ok());
        assert!(shutdown.await.unwrap().is_ok());
        assert!(persistence.shutdown().is_ok());
        assert!(matches!(
            persistence.probe().await,
            Err(PersistenceError::Closed)
        ));
    }

    // Export lifecycle: submit snapshot capture and guarded rejection, request
    // identity replay, exactly-once settlement and cancellation, retry against
    // a retained snapshot, capacity reservation, and retention with leases.

    fn export_test_revision(relative_path: &str, size: i64, mtime_ms: f64) -> String {
        crate::source_revision(relative_path, u64::try_from(size).unwrap(), mtime_ms).unwrap()
    }

    fn seed_current_schema(library: &LibraryRoot, path: &Path) {
        seed(
            path,
            include_str!("../../../../compatibility/sqlite/schema-v8.sql"),
        );
        let connection = Connection::open(path).unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
                [library.canonical_path().to_str().unwrap()],
            )
            .unwrap();
    }

    fn seed_export_photo(connection: &Connection) -> (String, String) {
        add_recipe_test_photo(
            connection,
            RecipeTestPhoto {
                original_id: "raw-original",
                photo_id: "raw-photo",
                relative_path: "shoot/one.ARW",
                kind: "raw",
                available: true,
                size: 17,
                mtime_ms: 1_000.0,
            },
        );
        connection
            .execute(
                "INSERT INTO edit_recipes(photo_id,revision,source_revision,exposure_ev,white_balance_mode)
                 VALUES('raw-photo','recipe-rev-1',?1,0.5,'as-shot')",
                params![crate::source_revision("shoot/one.ARW", 17_u64, 1_000.0).unwrap()],
            )
            .unwrap();
        add_recipe_test_photo(
            connection,
            RecipeTestPhoto {
                original_id: "jpeg-original",
                photo_id: "jpeg-photo",
                relative_path: "shoot/two.JPG",
                kind: "jpeg",
                available: true,
                size: 19,
                mtime_ms: 1_000.0,
            },
        );
        (
            "recipe-rev-1".to_owned(),
            export_test_revision("shoot/one.ARW", 17, 1_000.0),
        )
    }

    fn export_submission(
        request_id: &str,
        recipe_revision: &str,
        source_revision: &str,
        allowance: u64,
    ) -> ExportSubmission {
        ExportSubmission {
            request_id: request_id.to_owned(),
            photo_id: "raw-photo".to_owned(),
            source_profile_id: "sony-ilce-7rm5-arw".to_owned(),
            policy_id: "a".repeat(64),
            bundle_id: "b".repeat(64),
            expected_recipe_revision: recipe_revision.to_owned(),
            expected_source_revision: source_revision.to_owned(),
            exposure_range: ExportExposureRange {
                minimum_milli_ev: 0,
                maximum_milli_ev: 1000,
            },
            retained_output_bytes_max: allowance,
        }
    }

    #[tokio::test]
    async fn export_submit_captures_snapshot_and_replays_identity_exactly() {
        let (_base, library, state, name, path) = fixture();
        seed_current_schema(&library, &path);
        let (recipe_revision, source_revision) = {
            let connection = Connection::open(&path).unwrap();
            seed_export_photo(&connection)
        };
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();

        let created = persistence
            .submit_export_receiver(export_submission(
                "request-1",
                &recipe_revision,
                &source_revision,
                8 * 1024 * 1024 * 1024,
            ))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let ExportSubmitOutcome::Created(record) = created else {
            panic!("first submission must be created");
        };
        assert_eq!(record.state, ExportState::Queued);
        assert_eq!(record.snapshot.photo_id, "raw-photo");
        assert_eq!(record.snapshot.recipe_revision, "recipe-rev-1");
        assert_eq!(record.snapshot.settings.exposure_ev, 0.5);
        assert_eq!(record.snapshot.source_revision, source_revision);
        assert_eq!(record.snapshot.source_profile_id, "sony-ilce-7rm5-arw");
        assert_eq!(record.snapshot.workload, "development-tiff");
        assert_eq!(record.attempt, None);
        let payload = ExportRecipePayload::capture(
            &record.snapshot.settings,
            ExportExposureRange {
                minimum_milli_ev: 0,
                maximum_milli_ev: 1000,
            },
        )
        .unwrap();
        assert_eq!(record.snapshot.recipe_digest, payload.digest());
        assert_eq!(payload.exposure_milli_ev, 500);
        assert!(record.settled_at.is_none() && record.retain_until.is_none());
        assert_eq!(record.source, None);

        let replayed = persistence
            .submit_export_receiver(export_submission(
                "request-1",
                &recipe_revision,
                &source_revision,
                8 * 1024 * 1024 * 1024,
            ))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let ExportSubmitOutcome::Existing(replayed) = replayed else {
            panic!("identical replay must resolve to the existing Export");
        };
        assert_eq!(replayed.id, record.id);

        let conflicting = persistence
            .submit_export_receiver(export_submission(
                "request-1",
                "other-revision",
                &source_revision,
                8 * 1024 * 1024 * 1024,
            ))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(conflicting, ExportSubmitOutcome::RequestConflict);

        let stale = persistence
            .submit_export_receiver(export_submission(
                "request-2",
                "older-recipe-rev",
                &source_revision,
                8 * 1024 * 1024 * 1024,
            ))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let ExportSubmitOutcome::RecipeConflict(facts) = stale else {
            panic!("stale recipe revision must conflict");
        };
        assert_eq!(facts.recipe.as_ref().unwrap().revision, "recipe-rev-1");

        let changed = persistence
            .submit_export_receiver(export_submission(
                "request-3",
                &recipe_revision,
                &export_test_revision("shoot/one.ARW", 17, 2_000.0),
                8 * 1024 * 1024 * 1024,
            ))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(changed, ExportSubmitOutcome::SourceChanged(_)));

        let unsupported = persistence
            .submit_export_receiver(ExportSubmission {
                photo_id: "jpeg-photo".to_owned(),
                ..export_submission(
                    "request-4",
                    &recipe_revision,
                    &source_revision,
                    8 * 1024 * 1024 * 1024,
                )
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(unsupported, ExportSubmitOutcome::UnsupportedPhoto);

        let unknown = persistence
            .submit_export_receiver(ExportSubmission {
                photo_id: "absent-photo".to_owned(),
                ..export_submission(
                    "request-5",
                    &recipe_revision,
                    &source_revision,
                    8 * 1024 * 1024 * 1024,
                )
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(unknown, ExportSubmitOutcome::UnknownPhoto);

        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn export_settle_cancel_race_settles_exactly_once_and_retry_rearms() {
        let (_base, library, state, name, path) = fixture();
        seed_current_schema(&library, &path);
        let (recipe_revision, source_revision) = {
            let connection = Connection::open(&path).unwrap();
            seed_export_photo(&connection)
        };
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let allowance = 8 * 1024 * 1024 * 1024;
        macro_rules! submit_export {
            ($request_id:expr) => {{
                let outcome = persistence
                    .submit_export_receiver(export_submission(
                        $request_id,
                        &recipe_revision,
                        &source_revision,
                        allowance,
                    ))
                    .unwrap()
                    .await
                    .unwrap()
                    .unwrap();
                match outcome {
                    ExportSubmitOutcome::Created(record) => record,
                    _ => panic!("submission must be created"),
                }
            }};
        }

        let cancelled = submit_export!("request-cancel");
        let record = persistence
            .cancel_export_receiver(&cancelled.id)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(record.state, ExportState::Cancelled);
        assert!(record.settled_at.is_some());
        assert!(record.retain_until.is_some());
        let again = persistence
            .cancel_export_receiver(&cancelled.id)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(again.state, ExportState::Cancelled);
        assert_eq!(again.settled_at, record.settled_at);
        let late_completion = persistence
            .settle_export_receiver(
                &cancelled.id,
                ExportSettlement::Succeeded {
                    artifact_size: 10,
                    artifact_sha256: "c".repeat(64),
                    published_at: export_unix_seconds(),
                    artifact_width: 2,
                    artifact_height: 1,
                    artifact_profile_identity: "e".repeat(64),
                },
            )
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(late_completion.state, ExportState::Cancelled);

        let failed = submit_export!("request-failed");
        let record = persistence
            .settle_export_receiver(
                &failed.id,
                ExportSettlement::Failed {
                    outcome: "processing attempt did not complete: engine-failed".to_owned(),
                    settled_at: export_unix_seconds(),
                },
            )
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(record.state, ExportState::Failed);
        let retried = persistence
            .retry_export_receiver(&failed.id, "retry-1", &"b".repeat(64), allowance)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let ExportRetryOutcome::Retried(record) = retried else {
            panic!("failed Export must retry");
        };
        assert_eq!(record.state, ExportState::Queued);
        assert_eq!(record.outcome, None);
        assert_eq!(record.attempt, None);
        assert_eq!(record.snapshot.recipe_revision, "recipe-rev-1");
        // The accepted retry identity replays to the current record without
        // starting work.
        assert!(matches!(
            persistence
                .retry_export_receiver(&failed.id, "retry-1", &"b".repeat(64), allowance)
                .unwrap()
                .await
                .unwrap()
                .unwrap(),
            ExportRetryOutcome::Replayed(_)
        ));
        // The consumed retry identity against a different Export conflicts.
        let queued = submit_export!("request-queued");
        assert_eq!(
            persistence
                .retry_export_receiver(&queued.id, "retry-1", &"b".repeat(64), allowance)
                .unwrap()
                .await
                .unwrap()
                .unwrap(),
            ExportRetryOutcome::RequestConflict
        );
        // An unfinished Export is never retried.
        assert_eq!(
            persistence
                .retry_export_receiver(&queued.id, "retry-2", &"b".repeat(64), allowance)
                .unwrap()
                .await
                .unwrap()
                .unwrap(),
            ExportRetryOutcome::NotRetriable
        );

        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn export_capacity_leases_and_expiry_refuse_before_acceptance() {
        let (_base, library, state, name, path) = fixture();
        seed_current_schema(&library, &path);
        let (recipe_revision, source_revision) = {
            let connection = Connection::open(&path).unwrap();
            seed_export_photo(&connection)
        };
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let allowance = 8 * 1024 * 1024 * 1024;

        let refused = persistence
            .submit_export_receiver(export_submission(
                "request-full",
                &recipe_revision,
                &source_revision,
                1024,
            ))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(refused, ExportSubmitOutcome::RetainedOutputFull);

        let outcome = persistence
            .submit_export_receiver(export_submission(
                "request-published",
                &recipe_revision,
                &source_revision,
                allowance,
            ))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let ExportSubmitOutcome::Created(record) = outcome else {
            panic!("submission must be created");
        };
        let published_at = export_unix_seconds();
        let settled = persistence
            .settle_export_receiver(
                &record.id,
                ExportSettlement::Succeeded {
                    artifact_size: 4096,
                    artifact_sha256: "d".repeat(64),
                    published_at,
                    artifact_width: 16,
                    artifact_height: 9,
                    artifact_profile_identity: "e".repeat(64),
                },
            )
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(settled.state, ExportState::Succeeded);
        let artifact = settled.artifact.clone().unwrap();
        assert_eq!(artifact.size, 4096);
        assert_eq!(artifact.sha256, "d".repeat(64));
        assert_eq!(artifact.expires_at, published_at + EXPORT_RETENTION_SECONDS);
        assert_eq!(artifact.width, 16);
        assert_eq!(artifact.height, 9);
        assert_eq!(artifact.profile_identity, "e".repeat(64));
        assert_eq!(
            settled.retain_until,
            Some(published_at + EXPORT_RETENTION_SECONDS)
        );

        let after_retention = published_at + EXPORT_RETENTION_SECONDS + 1;
        // The lease is fresh relative to the sweep; a week-old lease would be
        // crash debris and reclaimed by the same sweep.
        let lease = persistence
            .acquire_export_lease_receiver(&record.id, after_retention - 3600)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let ExportLeaseOutcome::Acquired {
            lease_id,
            artifact: leased,
        } = lease
        else {
            panic!("a live artifact must lease");
        };
        assert_eq!(leased, artifact);
        let after_retention = published_at + EXPORT_RETENTION_SECONDS + 1;
        let sweep = persistence
            .sweep_export_expiry_receiver(after_retention)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(sweep.record_expiry_ids.is_empty());
        assert!(
            persistence
                .export_receiver(&record.id)
                .unwrap()
                .await
                .unwrap()
                .unwrap()
                .is_some()
        );

        assert!(
            persistence
                .release_export_lease_receiver(&lease_id)
                .unwrap()
                .await
                .unwrap()
                .unwrap()
        );
        let sweep = persistence
            .sweep_export_expiry_receiver(after_retention)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(sweep.record_expiry_ids, vec![record.id.clone()]);
        assert!(
            persistence
                .export_receiver(&record.id)
                .unwrap()
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
        let expired = persistence
            .submit_export_receiver(export_submission(
                "request-published",
                &recipe_revision,
                &source_revision,
                allowance,
            ))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(expired, ExportSubmitOutcome::Expired);

        persistence.shutdown().unwrap();
    }
}
