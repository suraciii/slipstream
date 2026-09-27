use super::albums::AlbumWriteError;
use super::scan::{FingerprintCounts, FingerprintTarget, ScanApplication, ScanRecoveryPlan};
use super::{
    DatabaseName, StateDirectory, StateError, StateFileIdentity, admission::StateDatabaseLock,
    albums, decisions, edit_recipe, export, metadata, migrations, mutation, queries, removal, scan,
};
use crate::{
    AlbumBrowseTarget, AlbumCreationResult, AlbumMembershipMutation, AlbumMembershipResult,
    AlbumMutation, AlbumMutationResult, AlbumQueryFilter, AlbumRecord, AlbumSummary,
    AppliedRelocations, CheckedAlbumMutation, CheckedAlbumMutationResult,
    CheckedPhotoDecisionMutation, CheckedPhotoDecisionResult, DiscoveredOriginal, EditRecipeRead,
    EditRecipeWriteOutcome, ExplicitPhotoRemovalMutation, ExplicitPhotoRestoreMutation,
    ExplicitPhotoRestoreResult, ExportAttempt, ExportLeaseOutcome, ExportRecord,
    ExportRetryOutcome, ExportSettlement, ExportSubmission, ExportSubmissionResolution,
    ExportSubmitOutcome, ExportSweepResult, MAXIMUM_PHOTO_RATING, OriginalFingerprint,
    OriginalScanError, PermanentDeletionItemState, PermanentDeletionSelection,
    PermanentDeletionTarget, PhotoAlbumMembership, PhotoOperationRemainder, PhotoQuery,
    PhotoQueryError, PhotoQueryProjection, PhotoRead, PhotoRemovalMutation, PhotoRemovalResult,
    PhotoRestoration, PhotoRestorationResult, PhotoStateBatchMutation, PhotoStateBatchResult,
    PhotoStateField, PhotoStateMutation, PhotoStateMutationResult, PhotoStateValue, PreviewSeed,
    PreviewSeedResult, RebindEditRecipe, RecoverySurvey, RemovedPhotoRecord, RequestedRelocation,
    SaveEditRecipe, ScanSnapshot, SelectionState, WhiteBalanceIntent,
};

use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    num::NonZeroUsize,
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

pub(super) struct MutationVersions {
    pub(super) epoch: String,
    pub(super) photo: HashMap<String, u64>,
    pub(super) album: HashMap<String, u64>,
}

impl MutationVersions {
    pub(super) fn new() -> Result<Self, PersistenceError> {
        Ok(Self {
            epoch: random_uuid_v4()?,
            photo: HashMap::new(),
            album: HashMap::new(),
        })
    }

    pub(super) fn photo(&self, id: &str) -> String {
        self.token("photo", id, *self.photo.get(id).unwrap_or(&0))
    }

    pub(super) fn album(&self, id: &str) -> String {
        self.token("album", id, *self.album.get(id).unwrap_or(&0))
    }

    pub(super) fn can_advance_photo(&self, id: &str) -> bool {
        self.photo.get(id).copied().unwrap_or(0) < u64::MAX
    }

    pub(super) fn can_advance_album(&self, id: &str) -> bool {
        self.album.get(id).copied().unwrap_or(0) < u64::MAX
    }

    pub(super) fn advance_photo(&mut self, id: &str) -> Result<(), MutationError> {
        advance_counter(&mut self.photo, id)
    }

    pub(super) fn advance_album(&mut self, id: &str) -> Result<(), MutationError> {
        advance_counter(&mut self.album, id)
    }

    pub(super) fn token(&self, kind: &str, id: &str, counter: u64) -> String {
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

pub(super) fn photo_decision_write_error_from_mutation(
    error: MutationError,
) -> PhotoDecisionWriteError {
    match error {
        MutationError::Saturated => PhotoDecisionWriteError::Saturated,
        MutationError::Closed => PhotoDecisionWriteError::Closed,
        _ => PhotoDecisionWriteError::Persistence,
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
pub(super) type ExportPublicationClaimReply = Result<Option<(String, u64)>, PersistenceError>;
/// One bounded page of removed Photos with the complete removed count and the
/// newest removal operation that still owns at least one Photo.
type RemovedPhotoPage = (
    Vec<RemovedPhotoRecord>,
    usize,
    Option<PhotoOperationRemainder>,
);
pub(super) type RemovedPhotoPageResult = Result<RemovedPhotoPage, PersistenceError>;
type RemovedPhotoPageReceiver = oneshot::Receiver<RemovedPhotoPageResult>;

type MetadataWork = Box<dyn FnOnce(&Connection) + Send>;

enum Command {
    Probe(Reply<u64>),
    Metadata(MetadataWork),
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
        removal::PhotoRemovalRequest,
        oneshot::Sender<Result<PhotoRemovalResult, MutationError>>,
    ),
    ReadPhotoRemovalOperation {
        operation_id: String,
        reply: oneshot::Sender<Result<Option<PhotoRemovalResult>, MutationError>>,
    },
    RestorePhotosExplicit(
        ExplicitPhotoRestoreMutation,
        oneshot::Sender<Result<ExplicitPhotoRestoreResult, MutationError>>,
    ),
    ReadPhotoRestoreOperation {
        operation_id: String,
        reply: oneshot::Sender<Result<Option<ExplicitPhotoRestoreResult>, MutationError>>,
    },
    RestorePhotos(
        PhotoRestoration,
        oneshot::Sender<Result<PhotoRestorationResult, MutationError>>,
    ),
    RemovedPhotos {
        start: usize,
        limit: usize,
        reply: Reply<RemovedPhotoPage>,
    },
    TrashCandidates {
        selection: PermanentDeletionSelection,
        reply: oneshot::Sender<removal::TrashCandidateResult>,
    },
    PreparePermanentDeletion {
        operation_id: String,
        targets: Vec<PermanentDeletionTarget>,
        reply: oneshot::Sender<removal::PermanentDeletionReviewResult>,
    },
    PermanentDeletionWork {
        operation_id: String,
        retry_unresolved: bool,
        reply: oneshot::Sender<removal::PermanentDeletionWorkResult>,
    },
    MarkPermanentDeletionDeleting {
        operation_id: String,
        photo_id: String,
        reply: oneshot::Sender<Result<(), MutationError>>,
    },
    PermanentlyDeletedOriginalIds(Reply<Vec<String>>),
    SettlePermanentDeletion {
        operation_id: String,
        photo_id: String,
        state: PermanentDeletionItemState,
        message: Option<String>,
        reply: oneshot::Sender<Result<(), MutationError>>,
    },
    ReadPermanentDeletion {
        operation_id: String,
        reply: oneshot::Sender<removal::PermanentDeletionResultReply>,
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
    pub(crate) fn with_metadata_receiver<R: Send + 'static>(
        &self,
        photo_id: String,
        work: impl FnOnce(&metadata::MetadataContext<'_>) -> Result<R, metadata::MetadataStoreError>
        + Send
        + 'static,
    ) -> Result<
        oneshot::Receiver<Result<R, metadata::MetadataStoreError>>,
        metadata::MetadataStoreError,
    > {
        let (send, receive) = oneshot::channel();
        self.submit(Command::Metadata(Box::new(move |connection| {
            let result = metadata::metadata_context(connection, &photo_id)
                .and_then(|context| work(&context));
            let _ = send.send(result);
        })))
        .map_err(|_| metadata::MetadataStoreError::Storage)?;
        Ok(receive)
    }

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

    pub(crate) fn snapshot_blocking(&self) -> Result<ScanSnapshot, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::Snapshot(send))?;
        receive
            .blocking_recv()
            .unwrap_or(Err(PersistenceError::OwnerStopped))
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
        let mutation = albums::normalize_album_mutation(mutation)?;
        let (send, receive) = oneshot::channel();
        self.submit(Command::MutateAlbum(mutation, send))
            .map_err(mutation::mutation_error_from_persistence)?;
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
        let mutation = albums::normalize_album_membership_mutation(mutation)?;
        let (send, receive) = oneshot::channel();
        self.submit(Command::MutateAlbumMembership(mutation, send))
            .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }

    pub async fn create_album_checked(
        &self,
        name: String,
    ) -> Result<AlbumCreationResult, albums::AlbumWriteError> {
        let receive = self.create_album_checked_receiver(name)?;
        receive
            .await
            .unwrap_or(Err(albums::AlbumWriteError::Persistence))
    }

    pub(crate) fn create_album_checked_receiver(
        &self,
        name: String,
    ) -> Result<
        oneshot::Receiver<Result<AlbumCreationResult, albums::AlbumWriteError>>,
        albums::AlbumWriteError,
    > {
        let name = albums::normalize_album_name(name)?;
        let (send, receive) = oneshot::channel();
        self.submit(Command::CreateAlbum(name, send))
            .map_err(albums::album_write_error_from_persistence)?;
        Ok(receive)
    }

    pub async fn mutate_album_checked(
        &self,
        mutation: CheckedAlbumMutation,
    ) -> Result<CheckedAlbumMutationResult, albums::AlbumWriteError> {
        let receive = self.mutate_album_checked_receiver(mutation)?;
        receive
            .await
            .unwrap_or(Err(albums::AlbumWriteError::Persistence))
    }

    pub(crate) fn mutate_album_checked_receiver(
        &self,
        mutation: CheckedAlbumMutation,
    ) -> Result<
        oneshot::Receiver<Result<CheckedAlbumMutationResult, albums::AlbumWriteError>>,
        albums::AlbumWriteError,
    > {
        let mutation = albums::normalize_checked_album_mutation(mutation)?;
        let (send, receive) = oneshot::channel();
        self.submit(Command::MutateAlbumChecked(mutation, send))
            .map_err(albums::album_write_error_from_persistence)?;
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
            .map_err(mutation::mutation_error_from_persistence)?;
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
            .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }

    pub(crate) fn remove_photos_receiver(
        &self,
        mutation: PhotoRemovalMutation,
    ) -> Result<oneshot::Receiver<Result<PhotoRemovalResult, MutationError>>, MutationError> {
        if mutation.photo_ids.is_empty()
            || mutation.operation_id.is_empty()
            || mutation.photo_ids.iter().collect::<HashSet<_>>().len() != mutation.photo_ids.len()
        {
            return Err(MutationError::Invalid);
        }
        let (send, receive) = oneshot::channel();
        self.submit(Command::RemovePhotos(
            removal::PhotoRemovalRequest::Browse(mutation),
            send,
        ))
        .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }
    pub(crate) fn remove_photos_explicit_receiver(
        &self,
        mutation: ExplicitPhotoRemovalMutation,
    ) -> Result<oneshot::Receiver<Result<PhotoRemovalResult, MutationError>>, MutationError> {
        if mutation.operation_id.is_empty()
            || mutation.photos.is_empty()
            || mutation.photos.len() > crate::PHOTO_REMOVAL_MAX
            || mutation
                .photos
                .iter()
                .map(|photo| &photo.photo_id)
                .collect::<HashSet<_>>()
                .len()
                != mutation.photos.len()
            || mutation.photos.iter().any(|photo| {
                photo.photo_id.is_empty()
                    || photo.expected_decision_version.is_empty()
                    || photo.expected_selection_state != SelectionState::Rejected
                    || photo.expected_removed_at_ms.is_some_and(|value| value < 0)
            })
        {
            return Err(MutationError::Invalid);
        }
        let (send, receive) = oneshot::channel();
        self.submit(Command::RemovePhotos(
            removal::PhotoRemovalRequest::Explicit(mutation),
            send,
        ))
        .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }
    pub(crate) fn photo_removal_operation_receiver(
        &self,
        operation_id: String,
    ) -> Result<oneshot::Receiver<Result<Option<PhotoRemovalResult>, MutationError>>, MutationError>
    {
        if operation_id.is_empty() {
            return Err(MutationError::Invalid);
        }
        let (send, receive) = oneshot::channel();
        self.submit(Command::ReadPhotoRemovalOperation {
            operation_id,
            reply: send,
        })
        .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }

    pub(crate) fn restore_photos_explicit_receiver(
        &self,
        mutation: ExplicitPhotoRestoreMutation,
    ) -> Result<oneshot::Receiver<Result<ExplicitPhotoRestoreResult, MutationError>>, MutationError>
    {
        if mutation.operation_id.is_empty()
            || mutation.photos.is_empty()
            || mutation.photos.len() > crate::PHOTO_REMOVAL_MAX
            || mutation
                .photos
                .iter()
                .map(|photo| &photo.photo_id)
                .collect::<HashSet<_>>()
                .len()
                != mutation.photos.len()
            || mutation
                .photos
                .iter()
                .any(|photo| photo.photo_id.is_empty() || photo.removed_at_ms < 0)
        {
            return Err(MutationError::Invalid);
        }
        let (send, receive) = oneshot::channel();
        self.submit(Command::RestorePhotosExplicit(mutation, send))
            .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }

    pub(crate) fn photo_restore_operation_receiver(
        &self,
        operation_id: String,
    ) -> Result<
        oneshot::Receiver<Result<Option<ExplicitPhotoRestoreResult>, MutationError>>,
        MutationError,
    > {
        if operation_id.is_empty() {
            return Err(MutationError::Invalid);
        }
        let (send, receive) = oneshot::channel();
        self.submit(Command::ReadPhotoRestoreOperation {
            operation_id,
            reply: send,
        })
        .map_err(mutation::mutation_error_from_persistence)?;
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
            .map_err(mutation::mutation_error_from_persistence)?;
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

    pub(crate) fn trash_candidates_receiver(
        &self,
        selection: PermanentDeletionSelection,
    ) -> Result<oneshot::Receiver<removal::TrashCandidateResult>, MutationError> {
        let valid = match &selection {
            PermanentDeletionSelection::Photos(photo_ids) => {
                !photo_ids.is_empty()
                    && photo_ids.len() <= crate::PERMANENT_DELETION_MAX
                    && photo_ids.iter().all(|id| !id.is_empty())
                    && photo_ids.iter().collect::<HashSet<_>>().len() == photo_ids.len()
            }
            PermanentDeletionSelection::All { exclude_photo_ids } => {
                exclude_photo_ids.len() <= crate::PERMANENT_DELETION_MAX
                    && exclude_photo_ids.iter().all(|id| !id.is_empty())
                    && exclude_photo_ids.iter().collect::<HashSet<_>>().len()
                        == exclude_photo_ids.len()
            }
        };
        if !valid {
            return Err(MutationError::Invalid);
        }
        let (send, receive) = oneshot::channel();
        self.submit(Command::TrashCandidates {
            selection,
            reply: send,
        })
        .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }

    pub(crate) fn prepare_permanent_deletion_receiver(
        &self,
        operation_id: String,
        targets: Vec<PermanentDeletionTarget>,
    ) -> Result<oneshot::Receiver<removal::PermanentDeletionReviewResult>, MutationError> {
        if operation_id.is_empty()
            || targets.len() > crate::PERMANENT_DELETION_MAX
            || targets.iter().any(|target| target.photo_id.is_empty())
            || targets
                .iter()
                .map(|target| &target.photo_id)
                .collect::<HashSet<_>>()
                .len()
                != targets.len()
        {
            return Err(MutationError::Invalid);
        }
        let (send, receive) = oneshot::channel();
        self.submit(Command::PreparePermanentDeletion {
            operation_id,
            targets,
            reply: send,
        })
        .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }

    pub(crate) fn permanent_deletion_work_receiver(
        &self,
        operation_id: String,
        retry_unresolved: bool,
    ) -> Result<oneshot::Receiver<removal::PermanentDeletionWorkResult>, MutationError> {
        if operation_id.is_empty() {
            return Err(MutationError::Invalid);
        }
        let (send, receive) = oneshot::channel();
        self.submit(Command::PermanentDeletionWork {
            operation_id,
            retry_unresolved,
            reply: send,
        })
        .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }

    pub(crate) fn mark_permanent_deletion_deleting_receiver(
        &self,
        operation_id: String,
        photo_id: String,
    ) -> Result<oneshot::Receiver<Result<(), MutationError>>, MutationError> {
        if operation_id.is_empty() || photo_id.is_empty() {
            return Err(MutationError::Invalid);
        }
        let (send, receive) = oneshot::channel();
        self.submit(Command::MarkPermanentDeletionDeleting {
            operation_id,
            photo_id,
            reply: send,
        })
        .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }

    /// The Original identities a scan must not re-adopt or relocate: their
    /// Photo was confirmed permanently deleted.
    pub(crate) fn permanently_deleted_original_ids_blocking(
        &self,
    ) -> Result<Vec<String>, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::PermanentlyDeletedOriginalIds(send))?;
        receive
            .blocking_recv()
            .unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn settle_permanent_deletion_receiver(
        &self,
        operation_id: String,
        photo_id: String,
        state: PermanentDeletionItemState,
        message: Option<String>,
    ) -> Result<oneshot::Receiver<Result<(), MutationError>>, MutationError> {
        if operation_id.is_empty() || photo_id.is_empty() {
            return Err(MutationError::Invalid);
        }
        let (send, receive) = oneshot::channel();
        self.submit(Command::SettlePermanentDeletion {
            operation_id,
            photo_id,
            state,
            message,
            reply: send,
        })
        .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }

    pub(crate) fn read_permanent_deletion_receiver(
        &self,
        operation_id: String,
    ) -> Result<oneshot::Receiver<removal::PermanentDeletionResultReply>, MutationError> {
        if operation_id.is_empty() {
            return Err(MutationError::Invalid);
        }
        let (send, receive) = oneshot::channel();
        self.submit(Command::ReadPermanentDeletion {
            operation_id,
            reply: send,
        })
        .map_err(mutation::mutation_error_from_persistence)?;
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
            Command::Metadata(work) => work(&connection),
            Command::Probe(reply) => {
                let _ = reply.send(Ok(sequence));
            }
            Command::Snapshot(reply) => {
                let result = scan::snapshot(&connection);
                let _ = reply.send(result);
            }
            Command::ApplyScan {
                discovered,
                errors,
                recovery,
                failure_after_first,
                reply,
            } => {
                let result = scan::apply_scan(
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
                let result = scan::recovery_facts(&connection, &original_ids);
                let _ = reply.send(result);
            }
            Command::NextFingerprintTarget(reply) => {
                let result = scan::next_fingerprint_target(&connection);
                let _ = reply.send(result);
            }
            Command::StoreFingerprint(fingerprint, reply) => {
                let result =
                    scan::store_fingerprint(&state, &database_name, &mut connection, fingerprint);
                let _ = reply.send(result);
            }
            Command::FingerprintCounts(reply) => {
                let result = scan::fingerprint_counts(&connection);
                let _ = reply.send(result);
            }
            Command::RecoverySurvey(reply) => {
                let result = scan::recovery_survey(&connection);
                let _ = reply.send(result);
            }
            Command::ApplyRelocations { relocations, reply } => {
                let result = scan::apply_manual_relocations(
                    &state,
                    &database_name,
                    &mut connection,
                    &relocations,
                );
                let _ = reply.send(result);
            }
            Command::Preview(preview, reply) => {
                let result = scan::seed_preview(&state, &database_name, &mut connection, preview);
                let _ = reply.send(result);
            }
            Command::ListAlbums(reply) => {
                let _ = reply.send(albums::list_albums(&connection));
            }
            Command::ListAlbumSummaries(reply) => {
                let _ = reply.send(albums::list_album_summaries(&connection, &versions));
            }
            Command::ReadAlbum { album_id, reply } => {
                let _ = reply.send(albums::read_album(&connection, &versions, &album_id));
            }
            Command::ReadAlbums { album_ids, reply } => {
                let result = album_ids
                    .iter()
                    .map(|album_id| albums::read_album(&connection, &versions, album_id))
                    .collect();
                let _ = reply.send(result);
            }
            Command::CreateAlbumQuery {
                filter,
                maximum_results,
                reply,
            } => {
                let _ = reply.send(albums::create_album_query(
                    &connection,
                    filter,
                    maximum_results,
                ));
            }
            Command::ReadPhoto { photo_id, reply } => {
                let _ = reply.send(queries::read_photo(&connection, &versions, &photo_id));
            }
            Command::ReadEditRecipe { photo_id, reply } => {
                let _ = reply.send(edit_recipe::read_edit_recipe(&connection, &photo_id));
            }
            Command::SaveEditRecipe(mutation, reply) => {
                let result = edit_recipe::save_edit_recipe(
                    &state,
                    &database_name,
                    &mut connection,
                    mutation,
                );
                let _ = reply.send(result);
            }
            Command::RebindEditRecipe(mutation, reply) => {
                let result = edit_recipe::rebind_edit_recipe(
                    &state,
                    &database_name,
                    &mut connection,
                    mutation,
                );
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
                        queries::read_projected_photo(&connection, &versions, &projection, photo_id)
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
                let _ = reply.send(queries::create_photo_query(
                    &connection,
                    query,
                    &projection,
                    maximum_results,
                ));
            }
            Command::PhotoAlbums { photo_id, reply } => {
                let _ = reply.send(albums::photo_albums(&connection, &photo_id));
            }
            Command::AlbumBrowseTarget { album_id, reply } => {
                let _ = reply.send(albums::album_browse_target(&connection, &album_id));
            }
            Command::MutateAlbum(mutation, reply) => {
                let plan = albums::album_version_plan(&connection, &mutation);
                let result = match plan {
                    Ok(plan) if !plan.advance || versions.can_advance_album(&plan.album_id) => {
                        let result =
                            albums::mutate_album(&state, &database_name, &mut connection, mutation);
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
                    albums::mutate_album_membership(
                        &state,
                        &database_name,
                        &mut connection,
                        mutation,
                    )
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
                let result = albums::create_album_checked(
                    &state,
                    &database_name,
                    &mut connection,
                    &versions,
                    name,
                );
                let _ = reply.send(result);
            }
            Command::MutateAlbumChecked(mutation, reply) => {
                let result = albums::mutate_album_checked(
                    &state,
                    &database_name,
                    &mut connection,
                    &mut versions,
                    mutation,
                );
                let _ = reply.send(result);
            }
            Command::MutatePhotoDecisionChecked(mutation, reply) => {
                let result = decisions::mutate_photo_decision_checked(
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
                    decisions::mutate_photo_state(&state, &database_name, &mut connection, mutation)
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
                    decisions::mutate_photo_state_batch(
                        &state,
                        &database_name,
                        &mut connection,
                        mutation,
                    )
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
            Command::RemovePhotos(request, reply) => {
                let result = removal::remove_photos(
                    &state,
                    &database_name,
                    &mut connection,
                    &versions,
                    request,
                );
                if let Ok(result) = &result {
                    for photo_id in &result.newly_removed {
                        let _ = versions.advance_photo(photo_id);
                    }
                }
                let _ = reply.send(result);
            }
            Command::ReadPhotoRemovalOperation {
                operation_id,
                reply,
            } => {
                let result = removal::read_photo_removal_operation(&connection, &operation_id)
                    .and_then(|receipt| {
                        receipt
                            .map(|receipt| {
                                removal::photo_removal_result_from_receipt(&operation_id, receipt)
                            })
                            .transpose()
                    });
                let _ = reply.send(result);
            }
            Command::RestorePhotosExplicit(mutation, reply) => {
                let result = removal::restore_photos_explicit(
                    &state,
                    &database_name,
                    &mut connection,
                    mutation,
                );
                if let Ok(result) = &result {
                    for photo_id in &result.restored {
                        let _ = versions.advance_photo(photo_id);
                    }
                }
                let _ = reply.send(result);
            }
            Command::ReadPhotoRestoreOperation {
                operation_id,
                reply,
            } => {
                let result = removal::read_photo_restore_operation(&connection, &operation_id)
                    .and_then(|receipt| {
                        receipt
                            .map(|receipt| {
                                removal::photo_restore_result_from_receipt(&operation_id, receipt)
                            })
                            .transpose()
                    });
                let _ = reply.send(result);
            }
            Command::RestorePhotos(restoration, reply) => {
                let result =
                    removal::restore_photos(&state, &database_name, &mut connection, restoration);
                if let Ok(result) = &result {
                    for photo_id in &result.restored {
                        let _ = versions.advance_photo(photo_id);
                    }
                }
                let _ = reply.send(result);
            }
            Command::RemovedPhotos {
                start,
                limit,
                reply,
            } => {
                let _ = reply.send(removal::removed_photos(&connection, start, limit));
            }
            Command::WriteProbe(reply) => {
                let result = write_transaction(&state, &database_name, &mut connection, |_| Ok(()));
                let _ = reply.send(result);
            }
            Command::SubmitExport(submission, reply) => {
                let result =
                    export::submit_export(&state, &database_name, &mut connection, submission);
                let _ = reply.send(result);
            }
            Command::ReadExport { export_id, reply } => {
                let _ = reply.send(export::export_record(&connection, &export_id));
            }
            Command::TrashCandidates { selection, reply } => {
                let _ = reply.send(removal::trash_candidates(&connection, selection));
            }
            Command::PreparePermanentDeletion {
                operation_id,
                targets,
                reply,
            } => {
                let result = removal::prepare_permanent_deletion(
                    &state,
                    &database_name,
                    &mut connection,
                    operation_id,
                    targets,
                );
                let _ = reply.send(result);
            }
            Command::PermanentDeletionWork {
                operation_id,
                retry_unresolved,
                reply,
            } => {
                let _ = reply.send(removal::permanent_deletion_work(
                    &connection,
                    &operation_id,
                    retry_unresolved,
                ));
            }
            Command::MarkPermanentDeletionDeleting {
                operation_id,
                photo_id,
                reply,
            } => {
                let result = removal::mark_permanent_deletion_deleting(
                    &state,
                    &database_name,
                    &mut connection,
                    &operation_id,
                    &photo_id,
                );
                let _ = reply.send(result);
            }
            Command::PermanentlyDeletedOriginalIds(reply) => {
                let _ = reply.send(permanently_deleted_original_ids(&connection));
            }
            Command::SettlePermanentDeletion {
                operation_id,
                photo_id,
                state: item_state,
                message,
                reply,
            } => {
                let result = removal::settle_permanent_deletion(
                    &state,
                    &database_name,
                    &mut connection,
                    &operation_id,
                    &photo_id,
                    item_state,
                    message,
                );
                let _ = reply.send(result);
            }
            Command::ReadPermanentDeletion {
                operation_id,
                reply,
            } => {
                let _ = reply.send(removal::read_permanent_deletion(&connection, &operation_id));
            }
            Command::ListPhotoExports { photo_id, reply } => {
                let _ = reply.send(export::list_photo_exports(&connection, &photo_id));
            }
            Command::CancelExport { export_id, reply } => {
                let result =
                    export::cancel_export(&state, &database_name, &mut connection, &export_id);
                let _ = reply.send(result);
            }
            Command::SettleExport {
                export_id,
                settlement,
                reply,
            } => {
                let result = export::settle_export(
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
                let result = export::begin_export_attempt(
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
                let result = export::record_export_source(
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
                let result = export::retry_export(
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
                let _ = reply.send(Ok(export::resolve_export_submission(
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
                let _ = reply.send(export::claim_export_publication(
                    &mut connection,
                    &export_id,
                    &incarnation,
                    sequence,
                ));
            }
            Command::ExportPublicationClaim { export_id, reply } => {
                let _ = reply.send(Ok(export::export_publication_claim(
                    &connection,
                    &export_id,
                )));
            }
            Command::RenewExportLease {
                lease_id,
                now,
                reply,
            } => {
                let _ = reply.send(Ok(export::renew_export_lease(
                    &mut connection,
                    &lease_id,
                    now,
                )));
            }
            Command::SweepExportExpiry { now, reply } => {
                let result =
                    export::sweep_export_expiry(&state, &database_name, &mut connection, now);
                let _ = reply.send(result);
            }
            Command::UnfinishedExports(reply) => {
                let _ = reply.send(export::unfinished_exports(&connection));
            }
            Command::AcquireExportLease {
                export_id,
                now,
                reply,
            } => {
                let result = export::acquire_export_lease(
                    &state,
                    &database_name,
                    &mut connection,
                    &export_id,
                    now,
                );
                let _ = reply.send(result);
            }
            Command::ReleaseExportLease { lease_id, reply } => {
                let result = export::release_export_lease(
                    &state,
                    &database_name,
                    &mut connection,
                    &lease_id,
                );
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
    migrations::preflight_schema(&readonly, canonical_root)?;
    drop(readonly);
    state.admit_sidecars(database_name)?;
    let mut connection = Connection::open(state.sqlite_path(database_name))
        .map_err(|_| PersistenceError::Storage)?;
    state.verify_database(database_name, identity)?;
    state.admit_sidecars(database_name)?;
    migrations::validate_root_binding(&connection, canonical_root)?;
    connection
        .pragma_update(None, "journal_mode", "DELETE")
        .map_err(|_| PersistenceError::Storage)?;
    state.admit_sidecars(database_name)?;
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(|_| PersistenceError::Storage)?;
    migrations::startup_schema(state, database_name, &mut connection, canonical_root)?;
    Ok(connection)
}

pub(super) fn write_transaction<T>(
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

pub(super) fn photo_processing_source(
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
                    scan::parse_kind(&row.get::<_, String>(0)?)?,
                    row.get::<_, i64>(1)? != 0 && row.get::<_, i64>(2)? != 0,
                ))
            },
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)
}

pub(super) fn white_balance_intent_name(intent: WhiteBalanceIntent) -> &'static str {
    intent.mode_name()
}

pub(super) fn white_balance_intent_values(
    intent: WhiteBalanceIntent,
) -> (Option<i32>, Option<i32>) {
    match intent {
        WhiteBalanceIntent::AsShot => (None, None),
        WhiteBalanceIntent::TemperatureTint {
            temperature_kelvin,
            tint_milli,
        } => (Some(temperature_kelvin), Some(tint_milli)),
    }
}

pub(super) fn parse_white_balance_intent(
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

pub(super) fn random_uuid_v4() -> Result<String, PersistenceError> {
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

pub(super) fn allocate_library_id(
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

pub(super) fn uuid_v4() -> Result<String, MutationError> {
    random_uuid_v4().map_err(|_| MutationError::Persistence)
}

pub(super) fn unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

/// The Original identities whose Photo was confirmed permanently deleted. A
/// scan must neither re-adopt nor relocate them.
/// Reserved Location that retires the Original row of a permanently deleted
/// Photo once a scan discovers another file at its reviewed Location. No
/// basename below it carries a supported Original File extension, so the
/// scanner never recognizes a retired row as a Library path and cannot collide
/// with a real Original.
pub(super) const PERMANENT_DELETION_RETIRED_LOCATION_PREFIX: &str = ".slipstream-deleted/";

/// The Original identities a scan must not re-adopt or relocate:
/// `permanent_deletion_deleted_original:<original>`.
pub(super) const PERMANENT_DELETION_DELETED_ORIGINAL_PREFIX: &str =
    "permanent_deletion_deleted_original:";

pub(super) fn permanently_deleted_original_ids(
    connection: &Connection,
) -> Result<Vec<String>, PersistenceError> {
    let mut statement = connection
        .prepare("SELECT key FROM library_metadata WHERE key LIKE ? ORDER BY key")
        .map_err(|_| PersistenceError::Storage)?;
    let keys = statement
        .query_map(
            [format!("{PERMANENT_DELETION_DELETED_ORIGINAL_PREFIX}%")],
            |row| row.get::<_, String>(0),
        )
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    Ok(keys
        .into_iter()
        .filter_map(|key| {
            key.strip_prefix(PERMANENT_DELETION_DELETED_ORIGINAL_PREFIX)
                .map(str::to_owned)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::source_revision;
    use crate::persistence::migrations::table_exists;
    use crate::persistence::removal::*;
    use crate::persistence::{DiscoveredFingerprint, SchemaVersion, validate_canonical_schema};
    use crate::{
        ALBUM_MEMBERSHIP_BATCH_MAX, PhotoQueryCandidate, PhotoQueryOrder, PhotoQuerySource,
        PreviewState,
    };
    use crate::{CaptureFact, CaptureMetadataState, CaptureTimeField, OriginalFacts, OriginalKind};
    use crate::{
        CaptureTimeBound, CheckedPhotoDecisionItem, EXPORT_DEVELOPMENT_TIFF_WORKLOAD,
        EXPORT_RETENTION_SECONDS, EditRecipe, EditRecipeSettings, ExportExposureRange,
        ExportRecipePayload, ExportState, LibraryRoot, PhotoRemovalMarker, PhotoRemovalTarget,
        PhotoStateBatchItem, identity::original_id,
    };
    use crate::{
        CheckedPhotoDecisionItemResult, CheckedPhotoDecisionOutcome, PhotoDecisionFacts,
        PhotoDecisionSnapshot, PhotoRemovalCounts, PhotoStateBatchApplied,
        PhotoStateBatchChangedElsewhere, PhotoStateBatchMissing,
    };
    use serde::Deserialize;
    use sha2::{Digest, Sha256};
    use std::{
        fs,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_TEMP_TREE: AtomicU64 = AtomicU64::new(0);

    /// The retained review is read back from its own JSON row, so the facts it
    /// compares against the filesystem must survive that round trip exactly.
    /// A milliseconds mtime needs 17 significant digits for some files, and a
    /// decimal parse of those returns a neighboring f64, so the row keeps the
    /// bits.
    #[test]
    fn reviewed_facts_survive_the_retained_review_round_trip() {
        let reviewed = OriginalFacts {
            size: 629,
            mtime_ms: 1_790_379_911_790.761_5,
            device: 64_513,
            inode: 21_758_498,
        };
        let stored = PermanentDeletionStoredItem {
            photo_id: "photo".to_owned(),
            removed_at_ms: 1_790_379_911_955,
            original_id: "original".to_owned(),
            relative_path: "b.jpg".to_owned(),
            kind: "jpeg".to_owned(),
            size: reviewed.size,
            mtime_bits: reviewed.mtime_ms.to_bits(),
            device: reviewed.device,
            inode: reviewed.inode,
            albums: Vec::new(),
        };
        let row = serde_json::to_string(&stored).unwrap();
        let read: PermanentDeletionStoredItem = serde_json::from_str(&row).unwrap();
        assert_eq!(
            reviewed_facts(read.size, read.mtime_bits, read.device, read.inode),
            reviewed
        );
    }

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

    fn metadata_fixture() -> (TempTree, Persistence, PathBuf) {
        let (base, root, state, name, path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            root.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        seed(&path, "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,capture_metadata_state)
             VALUES('original','dir/photo.JPG','jpeg',1,1,1,'pending'),('other-original','other.JPG','jpeg',1,1,1,'pending');
             INSERT INTO photos(id,original_id,available,preview_state,sort_path,selection_state,rating)
             VALUES('photo','original',1,'inspection-pending','dir/photo.JPG','undecided',0),
             ('other','other-original',1,'inspection-pending','other.JPG','undecided',0);");
        (base, persistence, path)
    }

    fn metadata_observation(size: u64) -> metadata::ObservedSidecar {
        metadata::ObservedSidecar {
            state: metadata::ObservedSidecarState::Eligible {
                path: "dir/photo.xmp".to_owned(),
                size,
                mtime_ms: 1234.5,
                digest: "a".repeat(64),
            },
        }
    }

    #[tokio::test]
    async fn with_metadata_reads_removed_but_rejects_missing_and_unavailable() {
        let (_base, persistence, path) = metadata_fixture();
        assert_eq!(
            persistence
                .with_metadata_receiver("missing".into(), |_| Ok(()))
                .unwrap()
                .await
                .unwrap(),
            Err(metadata::MetadataStoreError::PhotoMissing)
        );
        seed(
            &path,
            "UPDATE photos SET removed_at_ms=1,removed_operation='remove',rating=4 WHERE id='photo';",
        );
        let removed = persistence
            .with_metadata_receiver("photo".into(), |context| Ok(context.record().clone()))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(removed.removed);
        assert_eq!(removed.library_rating, 4);
        seed(
            &path,
            "UPDATE original_files SET available=0 WHERE id='original';",
        );
        assert_eq!(
            persistence
                .with_metadata_receiver("photo".into(), |_| Ok(()))
                .unwrap()
                .await
                .unwrap(),
            Err(metadata::MetadataStoreError::OriginalUnavailable)
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn with_metadata_observation_round_trips_and_protects_foreign_owner() {
        let (_base, persistence, path) = metadata_fixture();
        persistence
            .with_metadata_receiver("photo".into(), |context| {
                context.record_observation(&metadata_observation(7))
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let record = persistence
            .with_metadata_receiver("photo".into(), |context| Ok(context.record().clone()))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.kind, "jpeg");
        assert_eq!(record.original_id, "original");
        assert_eq!(record.relative_path, "dir/photo.JPG");
        assert_eq!(
            record.active,
            Some(metadata::ActiveAssociation {
                sidecar_path: "dir/photo.xmp".into(),
                observed_size: Some(7),
                observed_mtime_ms: Some(1234.5),
                observed_digest: Some("a".repeat(64))
            })
        );
        assert_eq!(
            persistence
                .with_metadata_receiver("other".into(), |context| context
                    .record_observation(&metadata_observation(8)))
                .unwrap()
                .await
                .unwrap(),
            Err(metadata::MetadataStoreError::Storage)
        );
        assert_eq!(
            Connection::open(&path)
                .unwrap()
                .query_row(
                    "SELECT observed_size FROM sidecar_associations WHERE photo_id='photo'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            7
        );
        persistence
            .with_metadata_receiver("photo".into(), |context| {
                context.record_observation(&metadata::ObservedSidecar {
                    state: metadata::ObservedSidecarState::Absent,
                })
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(
            persistence
                .with_metadata_receiver("photo".into(), |context| Ok(context
                    .record()
                    .active
                    .is_none()))
                .unwrap()
                .await
                .unwrap()
                .unwrap()
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn with_metadata_raw_claim_displaces_jpeg_owner_and_invalidates_evidence() {
        let (_base, persistence, path) = metadata_fixture();
        seed(
            &path,
            "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,capture_metadata_state)
             VALUES('raw-original','dir/photo.ARW','raw',1,1,1,'pending'),
                   ('raw-twin-original','dir/photo.CR2','raw',1,1,1,'pending');
             INSERT INTO photos(id,original_id,available,preview_state,sort_path,selection_state,rating)
             VALUES('raw-photo','raw-original',1,'inspection-pending','dir/photo.ARW','undecided',0),
                   ('raw-twin','raw-twin-original',1,'inspection-pending','dir/photo.CR2','undecided',0);",
        );
        persistence
            .with_metadata_receiver("photo".into(), |context| {
                context.record_observation(&metadata_observation(7))
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        // RAW priority hands the Sidecar to the RAW Photo; the displaced JPEG
        // owner loses its claim and its held evidence fails the generation.
        persistence
            .with_metadata_receiver("raw-photo".into(), |context| {
                context.record_observation(&metadata_observation(7))
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT photo_id FROM sidecar_associations WHERE sidecar_path='dir/photo.xmp'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "raw-photo"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sidecar_associations WHERE photo_id='photo'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        drop(connection);
        assert_eq!(association_generation(&path, "photo"), 2);
        assert_eq!(association_generation(&path, "raw-photo"), 1);
        // The displaced JPEG cannot reclaim while the RAW owner stands.
        assert_eq!(
            persistence
                .with_metadata_receiver("photo".into(), |context| context
                    .record_observation(&metadata_observation(8)))
                .unwrap()
                .await
                .unwrap(),
            Err(metadata::MetadataStoreError::Storage)
        );
        // A second available RAW of the same basename is a standing conflict.
        assert_eq!(
            persistence
                .with_metadata_receiver("raw-twin".into(), |context| context
                    .record_observation(&metadata_observation(8)))
                .unwrap()
                .await
                .unwrap(),
            Err(metadata::MetadataStoreError::Storage)
        );
        // The owner keeps updating its own claim.
        persistence
            .with_metadata_receiver("raw-photo".into(), |context| {
                context.record_observation(&metadata_observation(8))
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn with_metadata_claim_displaces_unavailable_owner_and_raw_claims_back() {
        let (_base, persistence, path) = metadata_fixture();
        seed(
            &path,
            "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,capture_metadata_state)
             VALUES('raw-original','dir/photo.ARW','raw',1,1,1,'pending');
             INSERT INTO photos(id,original_id,available,preview_state,sort_path,selection_state,rating)
             VALUES('raw-photo','raw-original',1,'inspection-pending','dir/photo.ARW','undecided',0);",
        );
        persistence
            .with_metadata_receiver("raw-photo".into(), |context| {
                context.record_observation(&metadata_observation(7))
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        seed(
            &path,
            "UPDATE original_files SET available=0 WHERE id='raw-original';",
        );
        // Without its Original the owner cannot write: the JPEG claim at the
        // stem displaces it and invalidates its evidence.
        persistence
            .with_metadata_receiver("photo".into(), |context| {
                context.record_observation(&metadata_observation(8))
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT photo_id FROM sidecar_associations WHERE sidecar_path='dir/photo.xmp'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "photo"
        );
        drop(connection);
        assert_eq!(association_generation(&path, "raw-photo"), 2);
        assert_eq!(association_generation(&path, "photo"), 1);
        // Once the RAW Original is available again, RAW priority reclaims the
        // Sidecar and the displaced JPEG's evidence fails.
        seed(
            &path,
            "UPDATE original_files SET available=1 WHERE id='raw-original';",
        );
        persistence
            .with_metadata_receiver("raw-photo".into(), |context| {
                context.record_observation(&metadata_observation(9))
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(association_generation(&path, "photo"), 2);
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn with_metadata_changed_clears_stale_claim_at_the_stem() {
        let (_base, persistence, path) = metadata_fixture();
        seed(
            &path,
            "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,capture_metadata_state)
             VALUES('raw-original','dir/photo.ARW','raw',1,1,1,'pending');
             INSERT INTO photos(id,original_id,available,preview_state,sort_path,selection_state,rating)
             VALUES('raw-photo','raw-original',1,'inspection-pending','dir/photo.ARW','undecided',0);",
        );
        persistence
            .with_metadata_receiver("photo".into(), |context| {
                context.record_observation(&metadata_observation(7))
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        // A Sidecar that no longer reads as recorded drops the stale JPEG
        // claim at the stem and invalidates its held evidence.
        persistence
            .with_metadata_receiver("raw-photo".into(), |context| {
                context.record_observation(&metadata::ObservedSidecar {
                    state: metadata::ObservedSidecarState::Changed,
                })
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let record = persistence
            .with_metadata_receiver("photo".into(), |context| Ok(context.record().clone()))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.active, None);
        assert_eq!(association_generation(&path, "photo"), 2);
        // The next readable inspection claims the Sidecar without a conflict.
        persistence
            .with_metadata_receiver("raw-photo".into(), |context| {
                context.record_observation(&metadata_observation(7))
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT photo_id FROM sidecar_associations WHERE sidecar_path='dir/photo.xmp'",
                    [],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "raw-photo"
        );
        drop(connection);
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn with_metadata_retains_unchanged_orphan_and_clears_corrections() {
        let (_base, persistence, path) = metadata_fixture();
        let sql = format!(
            "INSERT INTO retained_sidecar_orphans VALUES('dir/photo.xmp','retired','old.JPG','jpeg',3,7,1234.5,'{}');",
            "a".repeat(64)
        );
        seed(&path, &sql);
        persistence
            .with_metadata_receiver("photo".into(), |context| {
                context.record_observation(&metadata_observation(7))
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let orphan = persistence
            .with_metadata_receiver("photo".into(), |context| {
                Ok(context.record().orphan.clone())
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(orphan.retired_photo_id, "retired");
        assert_eq!(orphan.retired_generation, 3);
        persistence
            .with_metadata_receiver("photo".into(), |context| {
                context.record_observation(&metadata::ObservedSidecar {
                    state: metadata::ObservedSidecarState::Absent,
                })
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(
            persistence
                .with_metadata_receiver("photo".into(), |context| Ok(context
                    .record()
                    .orphan
                    .is_some()))
                .unwrap()
                .await
                .unwrap()
                .unwrap()
        );
        persistence
            .with_metadata_receiver("photo".into(), |context| {
                context.record_observation(&metadata_observation(8))
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(
            persistence
                .with_metadata_receiver("photo".into(), |context| Ok(context
                    .record()
                    .orphan
                    .is_none()))
                .unwrap()
                .await
                .unwrap()
                .unwrap()
        );
        seed(&path, &sql);
        persistence
            .with_metadata_receiver("photo".into(), |context| {
                context.record_observation(&metadata::ObservedSidecar {
                    state: metadata::ObservedSidecarState::Changed,
                })
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(
            persistence
                .with_metadata_receiver("photo".into(), |context| Ok(context
                    .record()
                    .orphan
                    .is_none()))
                .unwrap()
                .await
                .unwrap()
                .unwrap()
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn with_metadata_retains_uppercase_orphans_without_folding_basename() {
        let (_base, persistence, path) = metadata_fixture();
        seed(&path, &format!(
            "INSERT INTO retained_sidecar_orphans VALUES('dir/photo.XMP','retired','old.JPG','jpeg',3,7,1234.5,'{}');
             INSERT INTO retained_sidecar_orphans VALUES('dir/Photo.xmp','other','other.JPG','jpeg',3,7,1234.5,'{}');",
            "a".repeat(64), "a".repeat(64)
        ));
        let orphan = persistence
            .with_metadata_receiver("photo".into(), |context| {
                Ok(context.record().orphan.clone())
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(orphan.sidecar_path, "dir/photo.XMP");
        persistence
            .with_metadata_receiver("photo".into(), |context| {
                context.record_observation(&metadata::ObservedSidecar {
                    state: metadata::ObservedSidecarState::Changed,
                })
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(
            persistence
                .with_metadata_receiver("photo".into(), |context| {
                    Ok(context.record().orphan.is_none())
                })
                .unwrap()
                .await
                .unwrap()
                .unwrap()
        );
        assert_eq!(
            Connection::open(&path)
                .unwrap()
                .query_row(
                    "SELECT sidecar_path FROM retained_sidecar_orphans",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "dir/Photo.xmp"
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn with_metadata_save_serializes_before_concurrent_remove() {
        let (_base, persistence, path) = metadata_fixture();
        seed(
            &path,
            "UPDATE photos SET selection_state='rejected' WHERE id='photo';",
        );
        let (started_send, started_receive) = oneshot::channel();
        let (release_send, release_receive) = std::sync::mpsc::channel();
        let save = persistence
            .with_metadata_receiver("photo".into(), move |context| {
                started_send.send(()).unwrap();
                release_receive.recv().unwrap();
                context.record_observation(&metadata_observation(7))?;
                Ok(context.record().association_generation)
            })
            .unwrap();
        started_receive.await.unwrap();
        let remove = persistence
            .remove_photos_receiver(PhotoRemovalMutation {
                photo_ids: vec!["photo".into()],
                operation_id: "remove".into(),
            })
            .unwrap();
        assert_eq!(association_generation(&path, "photo"), 1);
        release_send.send(()).unwrap();
        assert_eq!(save.await.unwrap().unwrap(), 1);
        assert_eq!(remove.await.unwrap().unwrap().newly_removed, vec!["photo"]);
        assert_eq!(association_generation(&path, "photo"), 2);
        persistence.shutdown().unwrap();
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
        validate_canonical_schema(&connection, SchemaVersion::V11).unwrap();
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
        let connection = Connection::open(&path).unwrap();
        validate_canonical_schema(&connection, SchemaVersion::V11).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                .unwrap(),
            11
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

    fn sidecar_config(
        root: &LibraryRoot,
        state: &StateDirectory,
        name: &DatabaseName,
    ) -> crate::LibraryConfig {
        crate::LibraryConfig {
            library_root: root.canonical_path().to_owned(),
            state_directory: state.canonical_path().to_owned(),
            database_basename: name.as_os_str().to_string_lossy().into_owned(),
            ..crate::LibraryConfig::default()
        }
    }

    fn seed_sidecar(path: &Path, photo: &str, sidecar: &str) {
        Connection::open(path)
            .unwrap()
            .execute(
                "INSERT INTO sidecar_associations VALUES(?,?,7,1234.5,?)",
                params![photo, sidecar, "a".repeat(64)],
            )
            .unwrap();
    }

    fn association_generation(path: &Path, photo: &str) -> i64 {
        Connection::open(path)
            .unwrap()
            .query_row(
                "SELECT association_generation FROM photos WHERE id=?",
                [photo],
                |r| r.get(0),
            )
            .unwrap()
    }

    fn assert_retired(path: &Path, photo: &str, original: &str, sidecar: &str, generation: i64) {
        let connection = Connection::open(path).unwrap();
        let value: (String, String, String, i64, i64, f64, String) = connection.query_row(
            "SELECT retired_photo_id,retired_original_path,original_kind,retired_generation,observed_size,observed_mtime_ms,observed_digest FROM retained_sidecar_orphans WHERE sidecar_path=?",
            [sidecar], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
        ).unwrap();
        assert_eq!(
            value,
            (
                photo.to_owned(),
                original.to_owned(),
                "jpeg".to_owned(),
                generation,
                7,
                1234.5,
                "a".repeat(64)
            )
        );
        assert!(
            !connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sidecar_associations WHERE photo_id=?)",
                    [photo],
                    |r| r.get::<_, bool>(0)
                )
                .unwrap()
        );
    }

    async fn reject_and_remove(library: &crate::Library, photo: &str) {
        library
            .mutate_photo_state(PhotoStateMutation {
                photo_id: photo.to_owned(),
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Rejected),
                expected_current: None,
                album_id: None,
            })
            .await
            .unwrap();
        assert_eq!(
            library
                .remove_photos(PhotoRemovalMutation {
                    photo_ids: vec![photo.to_owned()],
                    operation_id: "remove-sidecar".to_owned(),
                })
                .await
                .unwrap()
                .removed,
            vec![photo.to_owned()]
        );
    }

    #[tokio::test]
    async fn with_metadata_library_round_trip() {
        let (_base, root, state, name, _path) = fixture();
        fs::write(root.canonical_path().join("one.JPG"), b"one").unwrap();
        let library = crate::Library::open(sidecar_config(&root, &state, &name)).unwrap();
        let snapshot = library.scan().await.unwrap();
        let photo = snapshot.photos[0].id.clone();
        library
            .with_metadata(photo.clone(), |context| {
                context.record_observation(&metadata::ObservedSidecar {
                    state: metadata::ObservedSidecarState::Eligible {
                        path: "one.xmp".into(),
                        size: 7,
                        mtime_ms: 1234.5,
                        digest: "a".repeat(64),
                    },
                })
            })
            .await
            .unwrap();
        let record = library
            .with_metadata(photo.clone(), |context| Ok(context.record().clone()))
            .await
            .unwrap();
        assert_eq!(record.photo_id, photo);
        assert_eq!(record.relative_path, "one.JPG");
        assert_eq!(record.active.unwrap().sidecar_path, "one.xmp");
        library.shutdown().unwrap();
        assert_eq!(
            library.with_metadata(photo, |_| Ok(())).await,
            Err(metadata::MetadataStoreError::Storage)
        );
    }

    #[tokio::test]
    async fn removal_and_restore_bump_association_generation() {
        let (_base, root, state, name, path) = fixture();
        fs::write(root.canonical_path().join("one.JPG"), b"one").unwrap();
        fs::write(root.canonical_path().join("two.JPG"), b"two").unwrap();
        let library = crate::Library::open(sidecar_config(&root, &state, &name)).unwrap();
        let snapshot = library.scan().await.unwrap();
        let photo = &snapshot.photos[0].id;
        let sibling = &snapshot.photos[1].id;
        seed_sidecar(&path, photo, "dir/photo.xmp");
        let before = association_generation(&path, photo);
        let sibling_before = association_generation(&path, sibling);
        reject_and_remove(&library, photo).await;
        let removed = association_generation(&path, photo);
        assert!(removed > before);
        assert_eq!(association_generation(&path, sibling), sibling_before);
        assert_eq!(
            library
                .restore_photos(PhotoRestoration::Operation("remove-sidecar".to_owned()))
                .await
                .unwrap()
                .restored,
            vec![photo.clone()]
        );
        assert!(association_generation(&path, photo) > removed);
        assert_eq!(association_generation(&path, sibling), sibling_before);
        library.shutdown().unwrap();
    }

    #[tokio::test]
    async fn scan_relocation_retires_sidecar_association() {
        let (_base, root, state, name, path) = fixture();
        fs::write(root.canonical_path().join("one.JPG"), b"one").unwrap();
        let library = crate::Library::open(sidecar_config(&root, &state, &name)).unwrap();
        let snapshot = library.scan().await.unwrap();
        let photo = &snapshot.photos[0].id;
        seed_sidecar(&path, photo, "dir/photo.xmp");
        let before = association_generation(&path, photo);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while library.fingerprint_counts().enrolled != 1 {
            assert!(
                std::time::Instant::now() < deadline,
                "fingerprint enrollment timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        library.shutdown().unwrap();
        fs::rename(
            root.canonical_path().join("one.JPG"),
            root.canonical_path().join("moved.JPG"),
        )
        .unwrap();
        let library = crate::Library::open(sidecar_config(&root, &state, &name)).unwrap();
        let relocated = library.scan().await.unwrap();
        assert_eq!(relocated.photos[0].id, *photo);
        assert_eq!(relocated.originals[0].relative_path.as_str(), "moved.JPG");
        let after = association_generation(&path, photo);
        assert!(after > before);
        assert_retired(&path, photo, "one.JPG", "dir/photo.xmp", after);
        library.shutdown().unwrap();
    }

    #[tokio::test]
    async fn permanent_deletion_retirement_and_expansion() {
        let (_base, parent_root, state, name, path) = fixture();
        fs::create_dir(parent_root.canonical_path().join("shoot")).unwrap();
        let root = LibraryRoot::open(parent_root.canonical_path().join("shoot")).unwrap();
        fs::write(root.canonical_path().join("one.JPG"), b"one").unwrap();
        fs::write(root.canonical_path().join("two.JPG"), b"two").unwrap();
        let config = sidecar_config(&root, &state, &name);
        let library = crate::Library::open(config.clone()).unwrap();
        let snapshot = library.scan().await.unwrap();
        let photo = &snapshot.photos[0].id;
        let sibling = &snapshot.photos[1].id;
        let original = &snapshot
            .originals
            .iter()
            .find(|o| o.id == snapshot.photos[0].original_id)
            .unwrap()
            .relative_path;
        seed_sidecar(&path, photo, "dir/photo.xmp");
        seed_sidecar(&path, sibling, "dir/sibling.xmp");
        reject_and_remove(&library, photo).await;
        library
            .prepare_permanent_deletion(
                "delete-sidecar".to_owned(),
                PermanentDeletionSelection::Photos(vec![photo.clone()]),
            )
            .await
            .unwrap();
        let deleted = library
            .permanently_delete("delete-sidecar".to_owned())
            .await
            .unwrap();
        assert_eq!(deleted.items[0].state, PermanentDeletionItemState::Deleted);
        let before = association_generation(&path, photo);
        fs::write(
            root.canonical_path().join(original.as_str()),
            b"replacement",
        )
        .unwrap();
        library.scan().await.unwrap();
        let retired = association_generation(&path, photo);
        assert!(retired > before);
        assert_retired(&path, photo, original.as_str(), "dir/photo.xmp", retired);
        library.shutdown().unwrap();

        // Expansion requires supported Original paths, unlike deletion's reserved Locations.
        let (_expansion_base, parent_root, state, name, path) = fixture();
        fs::create_dir(parent_root.canonical_path().join("shoot")).unwrap();
        let root = LibraryRoot::open(parent_root.canonical_path().join("shoot")).unwrap();
        let mut config = sidecar_config(&root, &state, &name);
        fs::write(root.canonical_path().join("one.JPG"), b"one").unwrap();
        fs::write(root.canonical_path().join("two.JPG"), b"two").unwrap();
        let library = crate::Library::open(config.clone()).unwrap();
        let snapshot = library.scan().await.unwrap();
        let photo = &snapshot.photos[0].id;
        let sibling = &snapshot.photos[1].id;
        seed_sidecar(&path, sibling, "dir/sibling.xmp");
        let retired = association_generation(&path, photo);
        seed(
            &path,
            &format!(
                "INSERT INTO retained_sidecar_orphans VALUES('dir/photo.xmp','{}','old.JPG','jpeg',{},7,1234.5,'{}')",
                photo,
                retired,
                "a".repeat(64),
            ),
        );
        let connection = Connection::open(&path).unwrap();
        let generations: Vec<(String, i64)> = connection
            .prepare("SELECT id,association_generation FROM photos ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        drop(connection);
        library.shutdown().unwrap();
        config.library_root = parent_root.canonical_path().to_owned();
        crate::expand_library(config).unwrap();
        for (photo, before) in generations {
            assert_eq!(association_generation(&path, &photo), before + 1);
        }
        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT sidecar_path FROM sidecar_associations WHERE photo_id=?",
                    [sibling],
                    |r| r.get::<_, String>(0)
                )
                .unwrap(),
            "shoot/dir/sibling.xmp"
        );
        assert_retired(&path, photo, "old.JPG", "shoot/dir/photo.xmp", retired);
    }

    #[tokio::test]
    async fn retire_and_bind_retires_before_delete() {
        let (_base, root, state, name, path) = fixture();
        fs::write(root.canonical_path().join("one.JPG"), b"one").unwrap();
        fs::write(root.canonical_path().join("two.JPG"), b"different").unwrap();
        let library = crate::Library::open(sidecar_config(&root, &state, &name)).unwrap();
        let snapshot = library.scan().await.unwrap();
        let destination = &snapshot.photos[0];
        let retiring = &snapshot.photos[1];
        let old_path = snapshot
            .originals
            .iter()
            .find(|o| o.id == destination.original_id)
            .unwrap()
            .relative_path
            .as_str();
        let new_path = snapshot
            .originals
            .iter()
            .find(|o| o.id == retiring.original_id)
            .unwrap()
            .relative_path
            .clone();
        seed_sidecar(&path, &destination.id, "dir/source.xmp");
        seed_sidecar(&path, &retiring.id, "dir/destination.xmp");
        let destination_before = association_generation(&path, &destination.id);
        let retiring_before = association_generation(&path, &retiring.id);
        fs::remove_file(root.canonical_path().join(old_path)).unwrap();
        library.scan().await.unwrap();
        let facts = root
            .original(new_path.clone())
            .unwrap()
            .facts_if_present()
            .unwrap()
            .unwrap();
        library
            .apply_relocations(vec![RequestedRelocation {
                original_id: destination.original_id.clone(),
                to_location: new_path.to_string(),
                facts,
                retire_destination: true,
            }])
            .await
            .unwrap();
        let after = association_generation(&path, &destination.id);
        assert!(after > destination_before);
        assert_retired(
            &path,
            &retiring.id,
            new_path.as_str(),
            "dir/destination.xmp",
            retiring_before + 1,
        );
        assert_retired(
            &path,
            &destination.id,
            new_path.as_str(),
            "dir/source.xmp",
            after,
        );
        let connection = Connection::open(&path).unwrap();
        assert!(
            !connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM photos WHERE id=?)",
                    [&retiring.id],
                    |r| r.get::<_, bool>(0)
                )
                .unwrap()
        );
        assert!(
            !connection
                .prepare("PRAGMA foreign_key_check")
                .unwrap()
                .exists([])
                .unwrap()
        );
        library.shutdown().unwrap();
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

        let result = scan::apply_manual_relocations(
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
            11
        );
        validate_canonical_schema(&connection, SchemaVersion::V11).unwrap();
        // The legacy photo-set tables are gone rather than left as aliases.
        for legacy in ["photo_sets", "photo_set_members", "review_progress"] {
            assert!(!table_exists(&connection, legacy).unwrap(), "{legacy}");
        }
    }
    // album-language-legacy:end v4-migration-test

    #[test]
    fn newer_v12_database_is_rejected_without_changes() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v5.sql"),
        );
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", 12)
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
    fn legacy_binary_fence_rejects_canonical_schema_above_the_max_version_without_changes() {
        // Every legacy database whose canonical version exceeds the supported
        // maximum is rejected without touching a byte, for both fence gaps.
        for (sql, version, max_version) in [
            (
                include_str!("../../../../compatibility/sqlite/schema-v5.sql"),
                SchemaVersion::V5,
                4,
            ),
            (
                include_str!("../../../../compatibility/sqlite/schema-v4.sql"),
                SchemaVersion::V4,
                3,
            ),
        ] {
            let (_base, library, _state, _name, path) = fixture();
            seed(&path, sql);
            let connection = Connection::open(&path).unwrap();
            validate_canonical_schema(&connection, version).unwrap();

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
                migrations::preflight_schema_for_max_version(
                    &connection,
                    library.canonical_path().to_str().unwrap(),
                    max_version,
                ),
                Err(PersistenceError::NewerSchema)
            ));

            let after = persisted_paths
                .iter()
                .map(|path| fs::read(path).ok())
                .collect::<Vec<_>>();
            assert_eq!(after, before);
        }
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
            let connection = Connection::open(&path).unwrap();
            validate_canonical_schema(&connection, SchemaVersion::V11).unwrap();
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
        let connection = Connection::open(&path).unwrap();
        validate_canonical_schema(&connection, SchemaVersion::V11).unwrap();
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
    async fn the_library_folder_root_resolves_while_no_photo_is_projected() {
        let (_base, library, state, name, _path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(Vec::new(), Vec::new())
            .await
            .unwrap();
        let projection = query_projection(&snapshot);
        // The Library Folder root is the Library itself, so it stays a valid
        // source while no Photo is present to prove it; a named Folder still
        // needs a member.
        let ids = persistence
            .create_photo_query_receiver(
                PhotoQuery {
                    source: PhotoQuerySource::Folder(String::new()),
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
        assert!(ids.is_empty());
        assert!(matches!(
            persistence
                .create_photo_query_receiver(
                    PhotoQuery {
                        source: PhotoQuerySource::Folder("shoot".to_owned()),
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
            Err(albums::AlbumWriteError::VersionConflict {
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
        // An unchanged rescan keeps the seeded preview facts bound to the
        // unchanged original.
        let unchanged = first
            .apply_scan(
                vec![discovered("one.ARW", OriginalKind::Raw, 3, 1000.0)],
                Vec::new(),
            )
            .await
            .unwrap();
        assert_eq!(unchanged.photos[0].preview_state, PreviewState::Ready);
        assert_eq!(
            unchanged.photos[0].cache_revision.as_deref(),
            Some("raw-cache")
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
        // Seeding with the superseded revision must lose the compare-and-swap
        // on the RAW revision itself, not merely a source-kind guard.
        assert_eq!(
            first
                .seed_preview(PreviewSeed {
                    photo_id,
                    state: PreviewState::Ready,
                    source: crate::PreviewSource::RawEmbeddedJpeg,
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
            Err(albums::AlbumWriteError::NameConflict {
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
            Err(albums::AlbumWriteError::LimitExceeded {
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
            Err(albums::AlbumWriteError::PhotoNotFound {
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
            Err(albums::AlbumWriteError::VersionConflict {
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
            Err(albums::AlbumWriteError::MembershipConflict {
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
            Err(albums::AlbumWriteError::VersionConflict {
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
            Err(albums::AlbumWriteError::Persistence)
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
            Err(albums::AlbumWriteError::LimitExceeded {
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
            11
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

    // Issue #276 metadata records join the Film v10 schema as v11. The
    // migration must add the sidecar records without disturbing the Film
    // export rows, their download leases, or any Photo's identity and
    // user-owned state, and every Photo starts at the first generation.
    #[tokio::test]
    async fn v10_to_v11_migration_preserves_film_export_state_and_starts_generation() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v10.sql"),
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
                "UPDATE photos SET selection_state='selected',rating=4 WHERE id='raw-photo'",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO exports(id,photo_id,target,state,recipe_revision,exposure_ev,white_balance_mode,source_revision,source_profile_id,source_kind,recipe_digest,policy_id,bundle_id,workload,created_at)
                 VALUES('export-one','raw-photo','film-jpeg','succeeded','recipe-1',0.25,'as-shot','source-1','profile-1','raw',?,?,?,'film-jpeg',1)",
                params!["d".repeat(64), "e".repeat(64), "f".repeat(64)],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO export_download_leases(id,export_id,created_at) VALUES('lease-one','export-one',2)",
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
        persistence.shutdown().unwrap();

        let connection = Connection::open(&path).unwrap();
        assert_eq!(
            connection
                .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
                .unwrap(),
            11
        );
        validate_canonical_schema(&connection, SchemaVersion::V11).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT target,state,workload,recipe_digest,policy_id,bundle_id,exposure_ev,white_balance_mode
                     FROM exports WHERE id='export-one'",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, f64>(6)?,
                            row.get::<_, String>(7)?,
                        ))
                    },
                )
                .unwrap(),
            (
                "film-jpeg".to_owned(),
                "succeeded".to_owned(),
                "film-jpeg".to_owned(),
                "d".repeat(64),
                "e".repeat(64),
                "f".repeat(64),
                0.25,
                "as-shot".to_owned(),
            )
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT export_id,created_at FROM export_download_leases WHERE id='lease-one'",
                    [],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )
                .unwrap(),
            ("export-one".to_owned(), 2)
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT id,sort_path,selection_state,rating,association_generation
                     FROM photos WHERE id='raw-photo'",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, i64>(4)?,
                        ))
                    },
                )
                .unwrap(),
            (
                "raw-photo".to_owned(),
                "shoot/one.ARW".to_owned(),
                "selected".to_owned(),
                4,
                1,
            )
        );
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
        let (records, _, _) = persistence
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
            let (records, _, _) = persistence
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
        let retried = remove(vec!["photo-1", "photo-2", "photo-missing"], "operation-one")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retried.counts.removed, 1);
        assert_eq!(retried.changed_elsewhere, vec!["photo-2".to_owned()]);
        assert_eq!(retried.missing, vec!["photo-missing".to_owned()]);
        assert!(retried.already_removed.is_empty());

        let other = remove(vec!["photo-1"], "operation-two")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(other.counts.removed, 0);
        assert_eq!(other.counts.already_removed, 1);
        assert_eq!(other.already_removed, vec!["photo-1".to_owned()]);

        let (records, total, _) = persistence
            .removed_photos_receiver(0, 10)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(total, 1);
        assert_eq!(records.len(), 1);
        assert!(records.iter().all(|record| record.removed_at_ms >= 0));

        // Restore by operation returns the group that operation still owns.
        let restored = persistence
            .restore_photos_receiver(PhotoRestoration::Operation("operation-one".to_owned()))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restored.counts.restored, 1);
        assert_eq!(restored.counts.missing, 0);
        assert_eq!(
            restored.restored.iter().cloned().collect::<HashSet<_>>(),
            HashSet::from(["photo-1".to_owned()])
        );
        // The durable receipt makes a retry after an explicit restore return
        // the original outcome without removing the Photo again.
        let retry_after_restore =
            remove(vec!["photo-1", "photo-2", "photo-missing"], "operation-one")
                .await
                .unwrap()
                .unwrap();
        assert_eq!(retry_after_restore.counts.removed, 1);
        assert_eq!(
            persistence
                .removed_photos_receiver(0, 10)
                .unwrap()
                .await
                .unwrap()
                .unwrap()
                .1,
            0
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
        let re_removed = remove(vec!["photo-2"], "operation-three")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(re_removed.counts.removed, 1);
        let (records, _, _) = persistence
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
        let (records, total, _) = persistence
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
                operation_id: "operation-three".to_owned(),
                removed: 0,
            }]
        );
        let (records, total, _) = persistence
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
        let re_removed = remove(vec!["photo-2"], "operation-four")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(re_removed.counts.removed, 1);
        let (records, _, _) = persistence
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
        let (records, total, _) = persistence
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

    #[tokio::test]
    async fn explicit_removal_and_restore_replay_stale_evidence_and_restart_safe_receipts() {
        let (base, library, state, name, path) = fixture();
        let canonical_root = library.canonical_path().to_string_lossy().into_owned();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v8.sql"),
        );
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata(key,value) VALUES('canonical_root',?)",
                [&canonical_root],
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

        let persistence = Persistence::open(state, name.clone(), canonical_root.clone()).unwrap();
        let stale = persistence
            .photo_receiver("photo-1")
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: "photo-1".to_owned(),
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Selected),
                expected_current: Some(PhotoStateValue::Selection(SelectionState::Rejected)),
                album_id: None,
            })
            .await
            .unwrap();
        persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: "photo-1".to_owned(),
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Rejected),
                expected_current: Some(PhotoStateValue::Selection(SelectionState::Selected)),
                album_id: None,
            })
            .await
            .unwrap();
        let stale_result = persistence
            .remove_photos_explicit_receiver(ExplicitPhotoRemovalMutation {
                operation_id: "remove-stale".to_owned(),
                photos: vec![PhotoRemovalTarget {
                    photo_id: "photo-1".to_owned(),
                    expected_selection_state: SelectionState::Rejected,
                    expected_decision_version: stale.decision_version,
                    expected_removed_at_ms: None,
                }],
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stale_result.counts.changed_elsewhere, 1);
        assert!(stale_result.removed.is_empty());

        let current = persistence
            .photo_receiver("photo-1")
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let remove = persistence
            .remove_photos_explicit_receiver(ExplicitPhotoRemovalMutation {
                operation_id: "remove-explicit".to_owned(),
                photos: vec![PhotoRemovalTarget {
                    photo_id: current.id.clone(),
                    expected_selection_state: current.selection_state,
                    expected_decision_version: current.decision_version,
                    expected_removed_at_ms: None,
                }],
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(remove.counts.removed, 1);
        let marker = remove.removed_markers[0].clone();
        persistence.shutdown().unwrap();
        let reopened_state =
            StateDirectory::open_or_create(&library, base.0.join("state")).unwrap();
        let persistence = Persistence::open(reopened_state, name, canonical_root).unwrap();
        let replay = persistence
            .photo_removal_operation_receiver("remove-explicit".to_owned())
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(replay.removed, remove.removed);
        assert_eq!(replay.removed_markers, remove.removed_markers);
        assert_eq!(replay.counts, remove.counts);

        let restore_mutation = ExplicitPhotoRestoreMutation {
            operation_id: "restore-explicit".to_owned(),
            photos: vec![marker.clone()],
        };
        let restored = persistence
            .restore_photos_explicit_receiver(restore_mutation.clone())
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restored.counts.restored, 1);
        let restore_replay = persistence
            .photo_restore_operation_receiver("restore-explicit".to_owned())
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(restore_replay, restored);

        let removed_after_restore = persistence
            .removed_photos_receiver(0, 10)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(removed_after_restore.0.is_empty());

        let current = persistence
            .photo_receiver("photo-1")
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let second = persistence
            .remove_photos_explicit_receiver(ExplicitPhotoRemovalMutation {
                operation_id: "remove-again".to_owned(),
                photos: vec![PhotoRemovalTarget {
                    photo_id: current.id,
                    expected_selection_state: current.selection_state,
                    expected_decision_version: current.decision_version,
                    expected_removed_at_ms: None,
                }],
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second.counts.removed, 1);
        assert!(second.removed_markers[0].removed_at_ms > marker.removed_at_ms);

        let old_restore_replay = persistence
            .restore_photos_explicit_receiver(restore_mutation)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(old_restore_replay, restored);
        let current_removed = persistence
            .removed_photos_receiver(0, 10)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current_removed.0.len(), 1);
        assert_eq!(
            current_removed.0[0].removed_at_ms,
            second.removed_markers[0].removed_at_ms
        );
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
        let album_read = persistence
            .album_receiver(&album.album_id)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(album_read.photo_count, 1);
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
            workload: EXPORT_DEVELOPMENT_TIFF_WORKLOAD.to_owned(),
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
                    published_at: export::export_unix_seconds(),
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
                    settled_at: export::export_unix_seconds(),
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
        let published_at = export::export_unix_seconds();
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
