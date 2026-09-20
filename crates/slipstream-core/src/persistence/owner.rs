use super::{
    DatabaseName, SchemaVersion, StateDirectory, StateError, StateFileIdentity,
    admission::StateDatabaseLock, validate_canonical_schema,
};
use crate::{
    ALBUM_MEMBERSHIP_BATCH_MAX, AlbumBrowseMember, AlbumBrowseTarget, AlbumMember,
    AlbumMembershipMutation, AlbumMembershipResult, AlbumMutation, AlbumMutationResult,
    AlbumRecord, AlbumSummary, AppliedRelocations, CaptureFact, CaptureMetadataState,
    CaptureTimeField, DiscoveredOriginal, LibraryRoot, MAXIMUM_FOLDER_ALBUM_PHOTOS,
    OriginalErrorCategory, OriginalFacts, OriginalFingerprint, OriginalKind, OriginalRecord,
    OriginalScanError, PhotoAlbumMembership, PhotoRecord, PhotoStateBatchApplied,
    PhotoStateBatchChangedElsewhere, PhotoStateBatchMissing, PhotoStateBatchMutation,
    PhotoStateBatchResult, PhotoStateField, PhotoStateMutation, PhotoStateMutationResult,
    PhotoStateUndo, PhotoStateValue, PreviewSeed, PreviewSeedResult, PreviewState, RecoverySurvey,
    RelativeOriginalPath, RequestedRelocation, ScanLimits, ScanSnapshot, SelectionState,
    UnavailablePhotoRecord,
    identity::classify_name,
    reconcile::{preview_should_preserve, reconcile, selected_source},
};
use rusqlite::{
    Connection, ErrorCode, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
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
    MutatePhotoState(
        PhotoStateMutation,
        oneshot::Sender<Result<PhotoStateMutationResult, MutationError>>,
    ),
    MutatePhotoStateBatch(
        PhotoStateBatchMutation,
        oneshot::Sender<Result<PhotoStateBatchResult, MutationError>>,
    ),
    WriteProbe(Reply<()>),
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
        Ok(connection) => {
            let _ = startup.send(Ok(()));
            connection
        }
        Err(error) => {
            let _ = startup.send(Err(error));
            return;
        }
    };
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
                let _ = reply.send(list_album_summaries(&connection));
            }
            Command::PhotoAlbums { photo_id, reply } => {
                let _ = reply.send(photo_albums(&connection, &photo_id));
            }
            Command::AlbumBrowseTarget { album_id, reply } => {
                let _ = reply.send(album_browse_target(&connection, &album_id));
            }
            Command::MutateAlbum(mutation, reply) => {
                let result = mutate_album(&state, &database_name, &mut connection, mutation);
                let _ = reply.send(result);
            }
            Command::MutateAlbumMembership(mutation, reply) => {
                let result =
                    mutate_album_membership(&state, &database_name, &mut connection, mutation);
                let _ = reply.send(result);
            }
            Command::MutatePhotoState(mutation, reply) => {
                let result = mutate_photo_state(&state, &database_name, &mut connection, mutation);
                let _ = reply.send(result);
            }
            Command::MutatePhotoStateBatch(mutation, reply) => {
                let result =
                    mutate_photo_state_batch(&state, &database_name, &mut connection, mutation);
                let _ = reply.send(result);
            }
            Command::WriteProbe(reply) => {
                let result = write_transaction(&state, &database_name, &mut connection, |_| Ok(()));
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
    preflight_schema_for_max_version(connection, canonical_root, 6)
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
    if version > 6 {
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
        _ => unreachable!(),
    }
    if version != 6 {
        migrate_v5(&transaction)?;
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
    validate_canonical_schema(&transaction, SchemaVersion::V6)
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
                 WHERE p.available=0
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
            .prepare("SELECT DISTINCT photo_id FROM album_members")
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
                        "SELECT id,rating,selection_state FROM photos WHERE original_id=?",
                        params![owner_id],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, i64>(1)?,
                                row.get::<_, String>(2)?,
                            ))
                        },
                    )
                    .optional()
                    .map_err(|_| PersistenceError::Storage)?;
                if let Some((photo_id, rating, selection_state)) = occupant {
                    if rating != 0 || selection_state != "undecided" {
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
    validate_canonical_schema(&readonly, SchemaVersion::V6)
        .map_err(|_| PersistenceError::UnsupportedSchema)?;
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
    validate_canonical_schema(&connection, SchemaVersion::V6)
        .map_err(|_| PersistenceError::UnsupportedSchema)?;
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
    validate_canonical_schema(&transaction, SchemaVersion::V6)
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
    validate_canonical_schema(&transaction, SchemaVersion::V6)
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
                    p.sort_path,p.selection_state,p.rating
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
                 WHERE m.album_id=? ORDER BY m.position",
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

fn list_album_summaries(connection: &Connection) -> Result<Vec<AlbumSummary>, PersistenceError> {
    connection
        .prepare(
            "SELECT a.id, a.name,
                    (SELECT count(*) FROM album_members m WHERE m.album_id = a.id),
                    EXISTS(SELECT 1 FROM album_progress p WHERE p.album_id = a.id)
             FROM albums a ORDER BY a.created_at, a.id",
        )
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], |row| {
            Ok(AlbumSummary {
                id: row.get(0)?,
                name: row.get(1)?,
                photo_count: row
                    .get::<_, i64>(2)?
                    .try_into()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                has_saved_position: row.get::<_, i64>(3)? != 0,
            })
        })
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)
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
             WHERE m.album_id=? ORDER BY m.position",
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

fn selection_state_value(value: SelectionState) -> &'static str {
    match value {
        SelectionState::Undecided => "undecided",
        SelectionState::Selected => "selected",
        SelectionState::Rejected => "rejected",
    }
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
    use crate::{LibraryRoot, PhotoStateBatchItem, identity::original_id};
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

    #[tokio::test]
    async fn initializes_exact_v5_and_runs_fifo_writes() {
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
        validate_canonical_schema(&connection, SchemaVersion::V6).unwrap();
    }

    // album-language-legacy:start v4-migration-test
    #[tokio::test]
    async fn v4_migration_to_v5_preserves_album_state_in_one_step() {
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
            6
        );
        validate_canonical_schema(&connection, SchemaVersion::V6).unwrap();
        // The legacy photo-set tables are gone rather than left as aliases.
        for legacy in ["photo_sets", "photo_set_members", "review_progress"] {
            assert!(!table_exists(&connection, legacy).unwrap(), "{legacy}");
        }
    }
    // album-language-legacy:end v4-migration-test

    #[test]
    fn newer_v7_database_is_rejected_without_changes() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v5.sql"),
        );
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", 7)
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
    async fn migrates_shared_v0_and_v1_to_v5_and_rejects_malformed_v2() {
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
            validate_canonical_schema(&connection, SchemaVersion::V6).unwrap();
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
        validate_canonical_schema(&connection, SchemaVersion::V6).unwrap();
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
