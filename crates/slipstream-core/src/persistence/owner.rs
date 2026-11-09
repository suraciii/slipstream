use super::album_write::{
    AlbumWriteError, album_write_error_from_persistence, normalize_album_membership_mutation,
    normalize_album_mutation, normalize_album_name, normalize_checked_album_mutation,
};
use super::photo_decision_write::{
    PhotoDecisionWriteError, normalize_checked_photo_decision_mutation,
    photo_decision_write_error_from_persistence, validate_photo_state_batch_mutation,
    validate_photo_state_mutation,
};
use super::scan::ScanApplication;
use super::scan_recovery::ScanRecoveryPlan;
use super::{
    DatabaseName, StateDirectory, StateFileIdentity, admission::StateDatabaseLock, album_queries,
    albums, decisions, development_proxy, edit_recipe, metadata, migrations, mutation,
    owner_export, owner_recovery, queries, removal, scan, trash_cleanup, xmp,
};
use crate::{
    AlbumBrowseTarget, AlbumCreationResult, AlbumMembershipMutation, AlbumMembershipResult,
    AlbumMutation, AlbumMutationResult, AlbumQueryFilter, AlbumRecord, AlbumSummary,
    CheckedAlbumMutation, CheckedAlbumMutationResult, CheckedPhotoDecisionMutation,
    CheckedPhotoDecisionResult, DiscoveredOriginal, EditRecipeRead, ExplicitPhotoRestoreResult,
    LibraryRoot, OriginalScanError, PhotoAlbumMembership, PhotoOperationRemainder, PhotoQuery,
    PhotoQueryError, PhotoQueryProjection, PhotoRead, PhotoRemovalResult, PhotoRestoration,
    PhotoRestorationResult, PhotoStateBatchMutation, PhotoStateBatchResult, PhotoStateMutation,
    PhotoStateMutationResult, PreviewSeed, PreviewSeedResult, RecoveryRecord, RemovedPhotoRecord,
    ScanSnapshot, SelectionState, TrashAbsence, TrashCleanupPreview, TrashCleanupResult,
    TrashCleanupTarget, TrashInspectionRecord, TrashSelection, WhiteBalanceIntent, XmpExportRecord,
};

use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior, params};
use std::{
    collections::HashSet,
    num::NonZeroUsize,
    sync::{
        Arc,
        mpsc::{Receiver, sync_channel},
    },
    thread,
};
use tokio::sync::oneshot;

mod library_requests;
mod lifecycle;
pub(super) mod processing;
mod removal_admission;
#[cfg(test)]
#[path = "owner_tests.rs"]
mod tests;
mod trash_requests;
#[cfg(test)]
const DEFAULT_QUEUE_CAPACITY: usize = 64;

pub use super::errors::{MutationError, PersistenceError};
use super::identity::random_uuid_v4;
use super::write_session::{MutationVersions, WriteSession};

pub(super) type Reply<T> = oneshot::Sender<Result<T, PersistenceError>>;
type EditRecipeSurfaceReceiver =
    oneshot::Receiver<Result<Option<(PhotoRead, EditRecipeRead)>, PersistenceError>>;

/// Bounded per-Photo Album membership query result.
type PhotoAlbums = Result<Option<Vec<PhotoAlbumMembership>>, PersistenceError>;
type AlbumReadWindow = Result<Vec<Option<AlbumSummary>>, PersistenceError>;
type AlbumReadWindowReceiver = oneshot::Receiver<AlbumReadWindow>;
type PhotoReadWindow = Result<Vec<Option<PhotoRead>>, PersistenceError>;
type PhotoReadWindowReceiver = oneshot::Receiver<PhotoReadWindow>;
/// One bounded page of removed Photos with the complete removed count and the
/// newest removal operation that still owns at least one Photo.
type RemovedPhotoPage = (
    Vec<RemovedPhotoRecord>,
    usize,
    Option<PhotoOperationRemainder>,
);
pub(super) type RemovedPhotoPageResult = Result<RemovedPhotoPage, PersistenceError>;
type RemovedPhotoPageReceiver = oneshot::Receiver<RemovedPhotoPageResult>;

type MetadataWork = Box<dyn FnOnce(&StateDirectory, &DatabaseName, &Connection) + Send>;

/// One review-identity read keeps its requested order, so every id gets a
/// slot and `None` means the record no longer exists.
pub(super) type RecoveryRecords = Vec<Option<RecoveryRecord>>;

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
    Recovery(owner_recovery::RecoveryCommand),
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
    ReadEditRecipeSurface {
        photo_id: String,
        reply: Reply<Option<(PhotoRead, EditRecipeRead)>>,
    },
    ReadEditRecipe {
        photo_id: String,
        reply: Reply<Option<EditRecipeRead>>,
    },
    Processing(processing::Command),
    ReadXmp {
        export_id: String,
        reply: Reply<Option<XmpExportRecord>>,
    },
    ListPhotoXmp {
        photo_id: String,
        reply: Reply<Option<Vec<XmpExportRecord>>>,
    },
    ReadDevelopmentProxy {
        photo_id: String,
        reply: Reply<Option<crate::DevelopmentProxyRecord>>,
    },
    RecordDevelopmentProxy(crate::DevelopmentProxyRecord, Reply<bool>),
    RemoveDevelopmentProxy {
        photo_id: String,
        reply: Reply<bool>,
    },
    AllDevelopmentProxies(Reply<Vec<crate::DevelopmentProxyRecord>>),
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
        removal::PhotoRestoreRequest,
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
    TrashInspection(oneshot::Sender<Result<Vec<TrashInspectionRecord>, MutationError>>),
    CleanupTrash {
        root: LibraryRoot,
        operation_id: String,
        targets: Vec<TrashCleanupTarget>,
        reply: oneshot::Sender<Result<TrashCleanupResult, MutationError>>,
    },
    PreviewTrashCleanup {
        root: LibraryRoot,
        targets: Vec<TrashCleanupTarget>,
        reply: oneshot::Sender<Result<TrashCleanupPreview, MutationError>>,
    },
    ReadTrashCleanup {
        operation_id: String,
        reply: oneshot::Sender<Result<TrashCleanupResult, MutationError>>,
    },
    PermanentlyDeletedOriginalIds(Reply<Vec<String>>),
    WriteProbe(Reply<()>),
    Export(owner_export::ExportCommand),
    #[cfg(test)]
    Configuration(Reply<(String, u8)>),
    #[cfg(test)]
    SetPhotoVersion {
        photo_id: String,
        counter: u64,
        reply: Reply<()>,
    },
    #[cfg(test)]
    Block {
        entered: oneshot::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
        reply: Reply<()>,
    },
}

#[derive(Clone)]
pub struct Persistence {
    inner: Arc<lifecycle::OwnerLifecycle>,
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
        self.submit(Command::Metadata(Box::new(
            move |state, database_name, connection| {
                let result =
                    metadata::metadata_context(state, database_name, connection, &photo_id)
                        .and_then(|context| work(&context));
                let _ = send.send(result);
            },
        )))
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
            inner: Arc::new(lifecycle::OwnerLifecycle::new(sender, join)),
        })
    }

    pub async fn probe(&self) -> Result<u64, PersistenceError> {
        let receive = self.submit_persistence_receiver(Command::Probe)?;
        receive.await.unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub async fn snapshot(&self) -> Result<ScanSnapshot, PersistenceError> {
        let receive = self.snapshot_receiver()?;
        receive.await.unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn snapshot_receiver(
        &self,
    ) -> Result<oneshot::Receiver<Result<ScanSnapshot, PersistenceError>>, PersistenceError> {
        self.submit_persistence_receiver(Command::Snapshot)
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
        self.submit_persistence_receiver(|reply| Command::Preview(preview, reply))
    }

    pub(crate) fn snapshot_blocking(&self) -> Result<ScanSnapshot, PersistenceError> {
        self.submit_persistence_receiver(Command::Snapshot)?
            .blocking_recv()
            .unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    /// The Original identities a scan must not re-adopt or relocate: their
    /// Photo was confirmed permanently deleted.
    pub(crate) fn permanently_deleted_original_ids_blocking(
        &self,
    ) -> Result<Vec<String>, PersistenceError> {
        self.submit_persistence_receiver(Command::PermanentlyDeletedOriginalIds)?
            .blocking_recv()
            .unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub async fn write_probe(&self) -> Result<(), PersistenceError> {
        let receive = self.submit_persistence_receiver(Command::WriteProbe)?;
        receive.await.unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(super) fn submit(&self, command: Command) -> Result<(), PersistenceError> {
        self.inner.submit(command)
    }

    pub(super) fn submit_persistence_receiver<T>(
        &self,
        build: impl FnOnce(Reply<T>) -> Command,
    ) -> Result<oneshot::Receiver<Result<T, PersistenceError>>, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(build(send))?;
        Ok(receive)
    }

    pub fn shutdown(&self) -> Result<(), PersistenceError> {
        self.inner.shutdown()
    }
}

fn inspect_trash_targets(
    root: &LibraryRoot,
    connection: &Connection,
    targets: &mut [TrashCleanupTarget],
) {
    for target in targets {
        let binding_valid = LibraryRoot::open(root.canonical_path())
            .and_then(|current| root.identifies_same_directory(&current))
            .unwrap_or(false);
        let synthetic_legacy = target
            .relative_path
            .as_str()
            .starts_with(PERMANENT_DELETION_RETIRED_LOCATION_PREFIX)
            && trash_cleanup::photo_is_permanently_deleted(connection, &target.photo_id)
                .unwrap_or(true);
        target.absence = if !binding_valid || synthetic_legacy {
            TrashAbsence::Uninspectable
        } else {
            match root
                .original(target.relative_path.clone())
                .and_then(|original| original.facts_if_present())
            {
                Ok(None) => TrashAbsence::Absent,
                Ok(Some(_)) => TrashAbsence::Present,
                Err(_) => TrashAbsence::Uninspectable,
            }
        };
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
            Command::Metadata(work) => work(&state, &database_name, &connection),
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
            Command::Recovery(command) => {
                owner_recovery::dispatch(command, &state, &database_name, &mut connection);
            }
            Command::Preview(preview, reply) => {
                let result = scan::seed_preview(&state, &database_name, &mut connection, preview);
                let _ = reply.send(result);
            }
            Command::ListAlbums(reply) => {
                let _ = reply.send(album_queries::list_albums(&connection));
            }
            Command::ListAlbumSummaries(reply) => {
                let _ = reply.send(album_queries::list_album_summaries(&connection, &versions));
            }
            Command::ReadAlbum { album_id, reply } => {
                let _ = reply.send(album_queries::read_album(&connection, &versions, &album_id));
            }
            Command::ReadAlbums { album_ids, reply } => {
                let result = album_ids
                    .iter()
                    .map(|album_id| album_queries::read_album(&connection, &versions, album_id))
                    .collect();
                let _ = reply.send(result);
            }
            Command::CreateAlbumQuery {
                filter,
                maximum_results,
                reply,
            } => {
                let _ = reply.send(album_queries::create_album_query(
                    &connection,
                    filter,
                    maximum_results,
                ));
            }
            Command::ReadPhoto { photo_id, reply } => {
                let _ = reply.send(queries::read_photo(&connection, &versions, &photo_id));
            }
            Command::ReadEditRecipeSurface { photo_id, reply } => {
                // One serialized owner operation: the Photo facts and the
                // recipe read below cannot straddle a scan publication, so
                // every guard derives from a single published state.
                let _ = reply.send(
                    queries::read_photo(&connection, &versions, &photo_id).and_then(|photo| {
                        photo
                            .map(|photo| {
                                edit_recipe::read_edit_recipe(&connection, &photo_id)
                                    .map(|read| read.map(|read| (photo, read)))
                            })
                            .transpose()
                            .map(|surface| surface.flatten())
                    }),
                );
            }
            Command::ReadEditRecipe { photo_id, reply } => {
                let _ = reply.send(edit_recipe::read_edit_recipe(&connection, &photo_id));
            }
            Command::ReadDevelopmentProxy { photo_id, reply } => {
                let _ = reply.send(development_proxy::read_development_proxy(
                    &connection,
                    &photo_id,
                ));
            }
            Command::RecordDevelopmentProxy(record, reply) => {
                let result = development_proxy::record_development_proxy(
                    &state,
                    &database_name,
                    &mut connection,
                    record,
                );
                let _ = reply.send(result);
            }
            Command::RemoveDevelopmentProxy { photo_id, reply } => {
                let _ = reply.send(development_proxy::remove_development_proxy(
                    &state,
                    &database_name,
                    &mut connection,
                    &photo_id,
                ));
            }
            Command::AllDevelopmentProxies(reply) => {
                let _ = reply.send(development_proxy::all_development_proxies(&connection));
            }
            Command::Processing(command) => {
                processing::dispatch(command, &state, &database_name, &mut connection);
            }
            Command::ReadXmp { export_id, reply } => {
                let _ = reply.send(xmp::read(&connection, &export_id));
            }
            Command::ListPhotoXmp { photo_id, reply } => {
                let _ = reply.send(xmp::list(&connection, &photo_id));
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
                let _ = reply.send(album_queries::photo_albums(&connection, &photo_id));
            }
            Command::AlbumBrowseTarget { album_id, reply } => {
                let _ = reply.send(album_queries::album_browse_target(&connection, &album_id));
            }
            Command::MutateAlbum(mutation, reply) => {
                let mut session =
                    WriteSession::new(&state, &database_name, &mut connection, &mut versions);
                let result = albums::mutate_album(&mut session, mutation);
                let _ = reply.send(result);
            }
            Command::MutateAlbumMembership(mutation, reply) => {
                let album_id = match &mutation {
                    AlbumMembershipMutation::Add { album_id, .. }
                    | AlbumMembershipMutation::RemoveAdded { album_id, .. } => album_id,
                };
                let result = if versions.can_advance_album(album_id) {
                    let mut session =
                        WriteSession::new(&state, &database_name, &mut connection, &mut versions);
                    albums::mutate_album_membership(&mut session, mutation)
                } else {
                    Err(MutationError::Persistence)
                };
                let _ = reply.send(result);
            }
            Command::CreateAlbum(name, reply) => {
                let mut session =
                    WriteSession::new(&state, &database_name, &mut connection, &mut versions);
                let result = albums::create_album_checked(&mut session, name);
                let _ = reply.send(result);
            }
            Command::MutateAlbumChecked(mutation, reply) => {
                let mut session =
                    WriteSession::new(&state, &database_name, &mut connection, &mut versions);
                let result = albums::mutate_album_checked(&mut session, mutation);
                let _ = reply.send(result);
            }
            Command::MutatePhotoDecisionChecked(mutation, reply) => {
                let mut session =
                    WriteSession::new(&state, &database_name, &mut connection, &mut versions);
                let result = decisions::mutate_photo_decision_checked(&mut session, mutation);
                let _ = reply.send(result);
            }
            Command::MutatePhotoState(mutation, reply) => {
                let mut session =
                    WriteSession::new(&state, &database_name, &mut connection, &mut versions);
                let result = decisions::mutate_photo_state(&mut session, mutation);
                let _ = reply.send(result);
            }
            Command::MutatePhotoStateBatch(mutation, reply) => {
                let mut session =
                    WriteSession::new(&state, &database_name, &mut connection, &mut versions);
                let result = decisions::mutate_photo_state_batch(&mut session, mutation);
                let _ = reply.send(result);
            }
            Command::RemovePhotos(request, reply) => {
                let mut session =
                    WriteSession::new(&state, &database_name, &mut connection, &mut versions);
                let result = removal::remove_photos(&mut session, request);
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
                let mut session =
                    WriteSession::new(&state, &database_name, &mut connection, &mut versions);
                let result = removal::restore_photos_explicit(&mut session, mutation);
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
                let mut session =
                    WriteSession::new(&state, &database_name, &mut connection, &mut versions);
                let result = removal::restore_photos(&mut session, restoration);
                let _ = reply.send(result);
            }
            Command::RemovedPhotos {
                start,
                limit,
                reply,
            } => {
                let _ = reply.send(removal::removed_photos(&connection, start, limit));
            }
            Command::TrashInspection(reply) => {
                let _ = reply.send(trash_cleanup::trash_inspection(&connection));
            }
            Command::CleanupTrash {
                root,
                operation_id,
                mut targets,
                reply,
            } => {
                let result =
                    match trash_cleanup::trash_cleanup_replay(&connection, &operation_id, &targets)
                    {
                        Ok(Some(result)) => Ok(result),
                        Err(error) => Err(error),
                        Ok(None) => {
                            inspect_trash_targets(&root, &connection, &mut targets);
                            let mut session = WriteSession::new(
                                &state,
                                &database_name,
                                &mut connection,
                                &mut versions,
                            );
                            trash_cleanup::cleanup_trash(&mut session, operation_id, targets)
                        }
                    };
                let _ = reply.send(result);
            }
            Command::PreviewTrashCleanup {
                root,
                mut targets,
                reply,
            } => {
                inspect_trash_targets(&root, &connection, &mut targets);
                let _ = reply.send(trash_cleanup::preview_trash_cleanup(
                    &mut connection,
                    &targets,
                ));
            }
            Command::ReadTrashCleanup {
                operation_id,
                reply,
            } => {
                let _ = reply.send(trash_cleanup::read_trash_cleanup(
                    &connection,
                    &operation_id,
                ));
            }
            Command::PermanentlyDeletedOriginalIds(reply) => {
                let _ = reply.send(permanently_deleted_original_ids(&connection));
            }
            Command::WriteProbe(reply) => {
                let result = write_transaction(&state, &database_name, &mut connection, |_| Ok(()));
                let _ = reply.send(result);
            }
            Command::Export(command) => {
                owner_export::dispatch(command, &state, &database_name, &mut connection);
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
            Command::SetPhotoVersion {
                photo_id,
                counter,
                reply,
            } => {
                versions.photo.insert(photo_id, counter);
                let _ = reply.send(Ok(()));
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
    let keys = super::metadata_store::scan_keys(
        connection,
        PERMANENT_DELETION_DELETED_ORIGINAL_PREFIX,
        |_| PersistenceError::Storage,
    )?;
    Ok(keys
        .into_iter()
        .filter_map(|key| {
            key.strip_prefix(PERMANENT_DELETION_DELETED_ORIGINAL_PREFIX)
                .map(str::to_owned)
        })
        .collect())
}
