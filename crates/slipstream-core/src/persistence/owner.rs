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

pub(super) fn validate_photo_state_batch_mutation(
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

pub(super) enum Command {
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

    pub(super) fn submit(&self, command: Command) -> Result<(), PersistenceError> {
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

pub(super) fn reserve_library_id(
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
    use crate::persistence::admission::StateDirectory;
    use crate::persistence::albums;
    use crate::persistence::test_support::*;
    use crate::{
        AlbumMembershipMutation, AlbumMutation, AlbumQueryFilter, CheckedAlbumMutation,
        OriginalKind, PhotoStateBatchItem, PhotoStateBatchMutation, PhotoStateField,
        PhotoStateMutation, PhotoStateValue, SelectionState,
    };
    use std::{fs, os::unix::fs::PermissionsExt};
    use tokio::sync::oneshot;

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
}
