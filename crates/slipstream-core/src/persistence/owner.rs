use super::{DatabaseName, SchemaVersion, StateDirectory, StateError, validate_canonical_schema};
use crate::{
    CaptureFact, CaptureMetadataState, CaptureTimeField, DiscoveredOriginal, OriginalErrorCategory,
    OriginalFacts, OriginalKind, OriginalRecord, OriginalScanError, PhotoRecord, PhotoSetMember,
    PhotoSetMutation, PhotoSetMutationResult, PhotoSetRecord, PhotoStateField, PhotoStateMutation,
    PhotoStateMutationResult, PhotoStateUndo, PhotoStateValue, PreviewCandidate, PreviewSeed,
    PreviewSeedResult, PreviewState, ScanSnapshot, SelectionState,
    identity::{original_id, source_revision},
    reconcile::{preview_should_preserve, reconcile, selected_source},
};
use rusqlite::{
    Connection, ErrorCode, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use std::{
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
    NotFound,
    Conflict,
    Persistence,
    Saturated,
    Closed,
}

impl fmt::Display for MutationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
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
        | PersistenceError::Storage => MutationError::Persistence,
    }
}

fn normalize_photo_set_mutation(
    mutation: PhotoSetMutation,
) -> Result<PhotoSetMutation, MutationError> {
    let trim_name = |name: String| {
        let name = name.trim().to_owned();
        if name.is_empty() || name.chars().count() > 120 {
            Err(MutationError::Conflict)
        } else {
            Ok(name)
        }
    };
    match mutation {
        PhotoSetMutation::Create { name } => Ok(PhotoSetMutation::Create {
            name: trim_name(name)?,
        }),
        PhotoSetMutation::Rename { photo_set_id, name } => Ok(PhotoSetMutation::Rename {
            photo_set_id,
            name: trim_name(name)?,
        }),
        PhotoSetMutation::AddMembers {
            photo_set_id,
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
            Ok(PhotoSetMutation::AddMembers {
                photo_set_id,
                photo_ids,
            })
        }
        PhotoSetMutation::Delete { .. }
        | PhotoSetMutation::RemoveMember { .. }
        | PhotoSetMutation::Reorder { .. }
        | PhotoSetMutation::SetProgress { .. } => Ok(mutation),
    }
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

type Reply<T> = oneshot::Sender<Result<T, PersistenceError>>;

enum Command {
    Probe(Reply<u64>),
    Snapshot(Reply<ScanSnapshot>),
    ApplyScan {
        discovered: Vec<DiscoveredOriginal>,
        errors: Vec<OriginalScanError>,
        failure_after_first: bool,
        reply: Reply<ScanSnapshot>,
    },
    Preview(PreviewSeed, Reply<PreviewSeedResult>),
    ListPhotoSets(Reply<Vec<PhotoSetRecord>>),
    MutatePhotoSet(
        PhotoSetMutation,
        oneshot::Sender<Result<PhotoSetMutationResult, MutationError>>,
    ),
    MutatePhotoState(
        PhotoStateMutation,
        oneshot::Sender<Result<PhotoStateMutationResult, MutationError>>,
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
        let (sender, receiver) = sync_channel(capacity.get());
        let (startup_send, startup_receive) = std::sync::mpsc::channel();
        let join = thread::Builder::new()
            .name("slipstream-sqlite".to_owned())
            .spawn(move || {
                owner_main(
                    state,
                    database_name,
                    identity,
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
        self.apply_scan_inner(discovered, errors, false).await
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) async fn apply_scan_failure(
        &self,
        discovered: Vec<DiscoveredOriginal>,
        errors: Vec<OriginalScanError>,
    ) -> Result<ScanSnapshot, PersistenceError> {
        self.apply_scan_inner(discovered, errors, true).await
    }

    async fn apply_scan_inner(
        &self,
        discovered: Vec<DiscoveredOriginal>,
        errors: Vec<OriginalScanError>,
        failure_after_first: bool,
    ) -> Result<ScanSnapshot, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::ApplyScan {
            discovered,
            errors,
            failure_after_first,
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
        let (send, receive) = oneshot::channel();
        self.submit(Command::ApplyScan {
            discovered,
            errors,
            failure_after_first: false,
            reply: send,
        })?;
        receive
            .blocking_recv()
            .unwrap_or(Err(PersistenceError::OwnerStopped))
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

    pub async fn list_photo_sets(&self) -> Result<Vec<PhotoSetRecord>, PersistenceError> {
        let receive = self.list_photo_sets_receiver()?;
        receive.await.unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn list_photo_sets_receiver(
        &self,
    ) -> Result<oneshot::Receiver<Result<Vec<PhotoSetRecord>, PersistenceError>>, PersistenceError>
    {
        let (send, receive) = oneshot::channel();
        self.submit(Command::ListPhotoSets(send))?;
        Ok(receive)
    }

    pub async fn mutate_photo_set(
        &self,
        mutation: PhotoSetMutation,
    ) -> Result<PhotoSetMutationResult, MutationError> {
        let receive = self.mutate_photo_set_receiver(mutation)?;
        receive.await.unwrap_or(Err(MutationError::Persistence))
    }

    pub(crate) fn mutate_photo_set_receiver(
        &self,
        mutation: PhotoSetMutation,
    ) -> Result<oneshot::Receiver<Result<PhotoSetMutationResult, MutationError>>, MutationError>
    {
        let mutation = normalize_photo_set_mutation(mutation)?;
        let (send, receive) = oneshot::channel();
        self.submit(Command::MutatePhotoSet(mutation, send))
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
    identity: super::StateFileIdentity,
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
                failure_after_first,
                reply,
            } => {
                let result = apply_scan(
                    &state,
                    &database_name,
                    &mut connection,
                    &discovered,
                    &errors,
                    failure_after_first,
                );
                let _ = reply.send(result);
            }
            Command::Preview(preview, reply) => {
                let result = seed_preview(&state, &database_name, &mut connection, preview);
                let _ = reply.send(result);
            }
            Command::ListPhotoSets(reply) => {
                let _ = reply.send(list_photo_sets(&connection));
            }
            Command::MutatePhotoSet(mutation, reply) => {
                let result = mutate_photo_set(&state, &database_name, &mut connection, mutation);
                let _ = reply.send(result);
            }
            Command::MutatePhotoState(mutation, reply) => {
                let result = mutate_photo_state(&state, &database_name, &mut connection, mutation);
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
    let version: u32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|_| PersistenceError::Storage)?;
    if version > 3 {
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
    if version > 3 {
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
        }
        1 => {
            validate_canonical_schema(&transaction, SchemaVersion::V1)
                .map_err(|_| PersistenceError::UnsupportedSchema)?;
            migrate_v1(&transaction)?;
            migrate_v2(&transaction)?;
        }
        2 => {
            validate_canonical_schema(&transaction, SchemaVersion::V2)
                .map_err(|_| PersistenceError::UnsupportedSchema)?;
            migrate_v2(&transaction)?;
        }
        3 => validate_canonical_schema(&transaction, SchemaVersion::V3)
            .map_err(|_| PersistenceError::UnsupportedSchema)?,
        _ => unreachable!(),
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
    validate_canonical_schema(&transaction, SchemaVersion::V3)
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

fn migrate_v1(transaction: &Transaction<'_>) -> Result<(), PersistenceError> {
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
            "SELECT p.id,p.raw_original_id,p.jpeg_original_id,p.ambiguous,p.available,p.preview_state,
                    p.preview_candidate,p.preview_source,p.preview_source_revision,p.preview_width,
                    p.preview_height,p.cache_revision,p.sort_path,p.selection_state,p.rating
             FROM photos p
             LEFT JOIN original_files raw ON raw.id=p.raw_original_id
             LEFT JOIN original_files jpeg ON jpeg.id=p.jpeg_original_id
             ORDER BY CASE WHEN COALESCE(raw.capture_order_key,jpeg.capture_order_key) IS NULL THEN 1 ELSE 0 END,
                      COALESCE(raw.capture_order_key,jpeg.capture_order_key) COLLATE BINARY,
                      p.sort_path COLLATE BINARY,p.id",
        )
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], |row| {
            Ok(PhotoRecord {
                id: row.get(0)?,
                raw_original_id: row.get(1)?,
                jpeg_original_id: row.get(2)?,
                ambiguous: row.get::<_, i64>(3)? != 0,
                available: row.get::<_, i64>(4)? != 0,
                preview_state: parse_preview_state(&row.get::<_, String>(5)?)?,
                preview_candidate: parse_preview_candidate(row.get(6)?)?,
                preview_source: parse_preview_candidate(row.get(7)?)?,
                preview_source_revision: row.get(8)?,
                preview_width: parse_dimension(row.get(9)?)?,
                preview_height: parse_dimension(row.get(10)?)?,
                cache_revision: row.get(11)?,
                sort_path: row.get(12)?,
                selection_state: parse_selection_state(&row.get::<_, String>(13)?)?,
                rating: row
                    .get::<_, i64>(14)?
                    .try_into()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
            })
        })
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    Ok(ScanSnapshot {
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

fn parse_preview_candidate(value: Option<String>) -> rusqlite::Result<Option<PreviewCandidate>> {
    value
        .map(|value| match value.as_str() {
            "matching-jpeg" => Ok(PreviewCandidate::MatchingJpeg),
            "embedded-raw-jpeg" => Ok(PreviewCandidate::EmbeddedRawJpeg),
            _ => Err(rusqlite::Error::InvalidQuery),
        })
        .transpose()
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

fn candidate_name(candidate: PreviewCandidate) -> &'static str {
    match candidate {
        PreviewCandidate::MatchingJpeg => "matching-jpeg",
        PreviewCandidate::EmbeddedRawJpeg => "embedded-raw-jpeg",
    }
}

fn preview_state_name(state: PreviewState) -> &'static str {
    match state {
        PreviewState::InspectionPending => "inspection-pending",
        PreviewState::Ready => "ready",
        PreviewState::Failed => "failed",
        PreviewState::Unavailable => "unavailable",
    }
}

fn apply_scan(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    discovered: &[DiscoveredOriginal],
    errors: &[OriginalScanError],
    failure_after_first: bool,
) -> Result<ScanSnapshot, PersistenceError> {
    let before = snapshot(connection)?;
    let previous_originals = before
        .originals
        .iter()
        .map(|original| (original.relative_path.as_str().to_owned(), original.clone()))
        .collect::<std::collections::HashMap<_, _>>();
    let reconciled = reconcile(discovered, &before.photos);
    write_transaction(state, database_name, connection, |transaction| {
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
                   id=excluded.id,kind=excluded.kind,size=excluded.size,mtime_ms=excluded.mtime_ms,
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
            let id = original_id(original.path.as_str());
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
                "INSERT INTO photos(id,raw_original_id,jpeg_original_id,ambiguous,available,
                    preview_state,preview_candidate,preview_source,preview_source_revision,
                    preview_width,preview_height,cache_revision,sort_path)
                 VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)
                 ON CONFLICT(id) DO UPDATE SET
                    raw_original_id=excluded.raw_original_id,jpeg_original_id=excluded.jpeg_original_id,
                    ambiguous=excluded.ambiguous,available=excluded.available,
                    preview_state=excluded.preview_state,preview_candidate=excluded.preview_candidate,
                    preview_source=excluded.preview_source,preview_source_revision=excluded.preview_source_revision,
                    preview_width=excluded.preview_width,preview_height=excluded.preview_height,
                    cache_revision=excluded.cache_revision,sort_path=excluded.sort_path",
            )
            .map_err(|_| PersistenceError::Storage)?;
        for photo in &reconciled {
            let selected = selected_source(photo);
            let candidate = selected.map(|(_, candidate)| candidate);
            let selected_path = selected.map(|(original, _)| original.path.as_str().to_owned());
            let preserve = photo.prior.as_ref().is_some_and(|prior| {
                preview_should_preserve(prior, photo, selected, &previous_originals)
            });
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
            let prior = photo.prior.as_ref();
            upsert_photo
                .execute(params![
                    photo.id,
                    photo.raw_id,
                    photo.jpeg_id,
                    i64::from(photo.ambiguous),
                    i64::from(
                        photo
                            .raw
                            .as_ref()
                            .is_some_and(|original| original.error_category.is_none())
                            || photo
                                .jpeg
                                .as_ref()
                                .is_some_and(|original| original.error_category.is_none()),
                    ),
                    preview_state_name(preview_state),
                    candidate.map(candidate_name),
                    preserve
                        .then(|| prior.unwrap().preview_source)
                        .flatten()
                        .map(candidate_name),
                    source_revision,
                    preserve
                        .then(|| prior.unwrap().preview_width)
                        .flatten()
                        .map(i64::from),
                    preserve
                        .then(|| prior.unwrap().preview_height)
                        .flatten()
                        .map(i64::from),
                    preserve
                        .then(|| prior.unwrap().cache_revision.clone())
                        .flatten(),
                    if photo.sort_path.is_empty() {
                        selected_path.unwrap_or_default()
                    } else {
                        photo.sort_path.clone()
                    },
                ])
                .map_err(|_| PersistenceError::Storage)?;
        }
        Ok(())
    })?;
    let mut result = snapshot(connection)?;
    result.errors = errors.to_vec();
    Ok(result)
}

fn source_revision_for_id(
    transaction: &Transaction<'_>,
    id: Option<&str>,
) -> Result<Option<String>, PersistenceError> {
    let Some(id) = id else {
        return Ok(None);
    };
    let row = transaction
        .query_row(
            "SELECT relative_path,size,mtime_ms,available,error_category FROM original_files WHERE id=?",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, i64>(3)? != 0,
                    row.get::<_, Option<String>>(4)?,
                ))
            },
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    let Some((path, size, mtime_ms, available, error_category)) = row else {
        return Ok(None);
    };
    if !available || error_category.is_some() {
        return Ok(None);
    }
    source_revision(
        &path,
        size.try_into().map_err(|_| PersistenceError::Storage)?,
        mtime_ms,
    )
    .map(Some)
    .map_err(|_| PersistenceError::Storage)
}

fn seed_preview(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    preview: PreviewSeed,
) -> Result<PreviewSeedResult, PersistenceError> {
    write_transaction(state, database_name, connection, |transaction| {
        let row = transaction
            .query_row(
                "SELECT preview_candidate,raw_original_id,jpeg_original_id FROM photos WHERE id=?",
                [&preview.photo_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|_| PersistenceError::Storage)?;
        let Some((candidate, raw_id, jpeg_id)) = row else {
            return Ok(PreviewSeedResult::StaleIgnored);
        };
        if candidate.as_deref() != Some(candidate_name(preview.expected_candidate)) {
            return Ok(PreviewSeedResult::StaleIgnored);
        }
        let expected_id = match preview.expected_candidate {
            PreviewCandidate::MatchingJpeg => jpeg_id.as_deref(),
            PreviewCandidate::EmbeddedRawJpeg => raw_id.as_deref(),
        };
        let expected_revision = source_revision_for_id(transaction, expected_id)?;
        if expected_revision.as_deref() != Some(preview.expected_source_revision.as_str()) {
            return Ok(PreviewSeedResult::StaleIgnored);
        }

        let actual_source = preview.actual_source.unwrap_or(preview.expected_candidate);
        if actual_source != preview.expected_candidate
            && (preview.expected_candidate != PreviewCandidate::MatchingJpeg
                || actual_source != PreviewCandidate::EmbeddedRawJpeg)
        {
            return Ok(PreviewSeedResult::StaleIgnored);
        }
        let actual_revision = if actual_source == preview.expected_candidate {
            expected_revision.clone()
        } else {
            source_revision_for_id(transaction, raw_id.as_deref())?
        };
        let expected_actual_revision = if actual_source == preview.expected_candidate {
            Some(preview.expected_source_revision.as_str())
        } else {
            preview.actual_source_revision.as_deref()
        };
        if actual_revision.is_none() || actual_revision.as_deref() != expected_actual_revision {
            return Ok(PreviewSeedResult::StaleIgnored);
        }
        let changed = transaction.execute(
            "UPDATE photos SET preview_state=?,preview_source=?,preview_source_revision=?,preview_width=?,preview_height=?,cache_revision=? WHERE id=? AND preview_candidate=?",
            params![preview_state_name(preview.state), candidate_name(actual_source), actual_revision, preview.width.map(i64::from), preview.height.map(i64::from), preview.cache_revision, preview.photo_id, candidate_name(preview.expected_candidate)],
        ).map_err(|_| PersistenceError::Storage)?;
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

fn uuid_v4() -> Result<String, MutationError> {
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
            return Err(MutationError::Persistence);
        }
        if result == 0 {
            return Err(MutationError::Persistence);
        }
        offset += usize::try_from(result).map_err(|_| MutationError::Persistence)?;
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

fn unix_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| i64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

fn list_photo_sets(connection: &Connection) -> Result<Vec<PhotoSetRecord>, PersistenceError> {
    let sets = connection
        .prepare("SELECT id,name FROM photo_sets ORDER BY created_at,id")
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    let mut result = Vec::with_capacity(sets.len());
    for (id, name) in sets {
        let members = connection
            .prepare(
                "SELECT m.photo_id,m.position,p.available,p.selection_state,p.rating
                 FROM photo_set_members m JOIN photos p ON p.id=m.photo_id
                 WHERE m.photo_set_id=? ORDER BY m.position",
            )
            .map_err(|_| PersistenceError::Storage)?
            .query_map([id.as_str()], |row| {
                Ok(PhotoSetMember {
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
                "SELECT photo_id FROM review_progress WHERE photo_set_id=?",
                [id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| PersistenceError::Storage)?;
        result.push(PhotoSetRecord {
            id,
            name,
            last_reviewed_photo_id,
            members,
        });
    }
    Ok(result)
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

fn require_photo_set(
    transaction: &Transaction<'_>,
    photo_set_id: &str,
) -> Result<(), MutationError> {
    transaction
        .query_row(
            "SELECT 1 FROM photo_sets WHERE id=?",
            [photo_set_id],
            |_| Ok(()),
        )
        .optional()
        .map_err(mutation_error_from_sqlite)?
        .ok_or(MutationError::NotFound)
}

fn mutate_photo_set(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    mutation: PhotoSetMutation,
) -> Result<PhotoSetMutationResult, MutationError> {
    mutation_transaction(state, database_name, connection, |transaction| {
        let photo_set_id = match mutation {
            PhotoSetMutation::Create { name } => {
                let id = uuid_v4()?;
                transaction
                    .execute(
                        "INSERT INTO photo_sets(id,name,created_at) VALUES(?,?,?)",
                        params![id, name, unix_millis()],
                    )
                    .map_err(mutation_error_from_sqlite)?;
                id
            }
            PhotoSetMutation::Rename { photo_set_id, name } => {
                require_photo_set(transaction, &photo_set_id)?;
                transaction
                    .execute(
                        "UPDATE photo_sets SET name=? WHERE id=?",
                        params![name, photo_set_id],
                    )
                    .map_err(mutation_error_from_sqlite)?;
                photo_set_id
            }
            PhotoSetMutation::Delete { photo_set_id } => {
                require_photo_set(transaction, &photo_set_id)?;
                transaction
                    .execute("DELETE FROM photo_sets WHERE id=?", [&photo_set_id])
                    .map_err(mutation_error_from_sqlite)?;
                photo_set_id
            }
            PhotoSetMutation::AddMembers {
                photo_set_id,
                photo_ids,
            } => {
                require_photo_set(transaction, &photo_set_id)?;
                for photo_id in &photo_ids {
                    transaction
                        .query_row("SELECT 1 FROM photos WHERE id=?", [photo_id], |_| Ok(()))
                        .optional()
                        .map_err(mutation_error_from_sqlite)?
                        .ok_or(MutationError::NotFound)?;
                }
                let mut position: i64 = transaction
                    .query_row(
                        "SELECT COALESCE(MAX(position)+1,0) FROM photo_set_members WHERE photo_set_id=?",
                        [&photo_set_id],
                        |row| row.get(0),
                    )
                    .map_err(mutation_error_from_sqlite)?;
                for photo_id in photo_ids {
                    transaction
                        .execute(
                            "INSERT INTO photo_set_members(photo_set_id,photo_id,position) VALUES(?,?,?)",
                            params![photo_set_id, photo_id, position],
                        )
                        .map_err(mutation_error_from_sqlite)?;
                    position = position.checked_add(1).ok_or(MutationError::Conflict)?;
                }
                photo_set_id
            }
            PhotoSetMutation::RemoveMember {
                photo_set_id,
                photo_id,
            } => {
                let position: i64 = transaction
                    .query_row(
                        "SELECT position FROM photo_set_members WHERE photo_set_id=? AND photo_id=?",
                        params![photo_set_id, photo_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(mutation_error_from_sqlite)?
                    .ok_or(MutationError::NotFound)?;
                transaction
                    .execute(
                        "DELETE FROM photo_set_members WHERE photo_set_id=? AND photo_id=?",
                        params![photo_set_id, photo_id],
                    )
                    .map_err(mutation_error_from_sqlite)?;
                transaction
                    .execute(
                        "UPDATE photo_set_members SET position=position-1 WHERE photo_set_id=? AND position>?",
                        params![photo_set_id, position],
                    )
                    .map_err(mutation_error_from_sqlite)?;
                photo_set_id
            }
            PhotoSetMutation::Reorder {
                photo_set_id,
                photo_ids,
            } => {
                require_photo_set(transaction, &photo_set_id)?;
                let current = transaction
                    .prepare(
                        "SELECT photo_id,position FROM photo_set_members WHERE photo_set_id=? ORDER BY position",
                    )
                    .map_err(mutation_error_from_sqlite)?
                    .query_map([&photo_set_id], |row| {
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
                        "UPDATE photo_set_members SET position=position+? WHERE photo_set_id=?",
                        params![temporary_offset, photo_set_id],
                    )
                    .map_err(mutation_error_from_sqlite)?;
                for (position, photo_id) in photo_ids.iter().enumerate() {
                    transaction
                        .execute(
                            "UPDATE photo_set_members SET position=? WHERE photo_set_id=? AND photo_id=?",
                            params![position as i64, photo_set_id, photo_id],
                        )
                        .map_err(mutation_error_from_sqlite)?;
                }
                photo_set_id
            }
            PhotoSetMutation::SetProgress {
                photo_set_id,
                photo_id,
            } => {
                transaction
                    .query_row(
                        "SELECT 1 FROM photo_set_members WHERE photo_set_id=? AND photo_id=?",
                        params![photo_set_id, photo_id],
                        |_| Ok(()),
                    )
                    .optional()
                    .map_err(mutation_error_from_sqlite)?
                    .ok_or(MutationError::NotFound)?;
                transaction
                    .execute(
                        "INSERT INTO review_progress(photo_set_id,photo_id) VALUES(?,?)
                         ON CONFLICT(photo_set_id) DO UPDATE SET photo_id=excluded.photo_id",
                        params![photo_set_id, photo_id],
                    )
                    .map_err(mutation_error_from_sqlite)?;
                photo_set_id
            }
        };
        Ok(PhotoSetMutationResult { photo_set_id })
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
        if let Some(photo_set_id) = mutation.photo_set_id.as_deref() {
            transaction
                .query_row(
                    "SELECT 1 FROM photo_set_members WHERE photo_set_id=? AND photo_id=?",
                    params![photo_set_id, mutation.photo_id],
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
                        params![
                            match value {
                                SelectionState::Undecided => "undecided",
                                SelectionState::Selected => "selected",
                                SelectionState::Rejected => "rejected",
                            },
                            mutation.photo_id
                        ],
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
        if let Some(photo_set_id) = mutation.photo_set_id.as_deref() {
            transaction
                .execute(
                    "INSERT INTO review_progress(photo_set_id,photo_id) VALUES(?,?)
                     ON CONFLICT(photo_set_id) DO UPDATE SET photo_id=excluded.photo_id",
                    params![photo_set_id, mutation.photo_id],
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
    use crate::LibraryRoot;
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
    async fn initializes_exact_v3_and_runs_fifo_writes() {
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
        validate_canonical_schema(&connection, SchemaVersion::V3).unwrap();
    }

    #[tokio::test]
    async fn migrates_shared_v0_and_v1_to_v3_and_rejects_malformed_v2() {
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
            validate_canonical_schema(&connection, SchemaVersion::V3).unwrap();
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

    #[tokio::test]
    async fn v2_migration_preserves_identity_membership_decisions_preview_and_progress() {
        let (_base, library, state, name, path) = fixture();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v2.sql"),
        );
        let original_id = original_id("shoot/A.JPG");
        let photo_id = "photo-preserved";
        let set_id = "00000000-0000-4000-8000-000000000027";
        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "INSERT INTO original_files VALUES(?,?,?,?,?,?,?,?)",
                params![
                    original_id,
                    "shoot/A.JPG",
                    "jpeg",
                    12_i64,
                    1_000.0_f64,
                    1_i64,
                    Option::<String>::None,
                    Option::<String>::None
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO photos(id,jpeg_original_id,ambiguous,available,preview_state,preview_candidate,preview_source,preview_source_revision,preview_width,preview_height,cache_revision,sort_path,selection_state,rating) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
                params![photo_id, original_id, 0_i64, 1_i64, "ready", "matching-jpeg", "matching-jpeg", "preview-revision", 8_i64, 4_i64, "cache-revision", "shoot/A.JPG", "selected", 5_i64],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO photo_sets(id,name,created_at) VALUES(?,?,?)",
                params![set_id, "Preserved", 1_i64],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO photo_set_members(photo_set_id,photo_id,position) VALUES(?,?,?)",
                params![set_id, photo_id, 0_i64],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO review_progress(photo_set_id,photo_id) VALUES(?,?)",
                params![set_id, photo_id],
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
        assert_eq!(photo.raw_original_id, None);
        assert_eq!(
            photo.jpeg_original_id.as_deref(),
            Some(original_id.as_str())
        );
        assert!(!photo.ambiguous);
        assert!(photo.available);
        assert_eq!(photo.preview_state, PreviewState::Ready);
        assert_eq!(
            photo.preview_candidate,
            Some(PreviewCandidate::MatchingJpeg)
        );
        assert_eq!(photo.preview_source, Some(PreviewCandidate::MatchingJpeg));
        assert_eq!(
            photo.preview_source_revision.as_deref(),
            Some("preview-revision")
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
        assert_eq!(original.capture, CaptureFact::pending());
        let set = persistence.list_photo_sets().await.unwrap().remove(0);
        assert_eq!(set.id, set_id);
        assert_eq!(set.name, "Preserved");
        assert_eq!(set.members.len(), 1);
        assert_eq!(set.members[0].photo_id, photo_id);
        assert_eq!(set.members[0].position, 0);
        assert!(set.members[0].available);
        assert_eq!(set.members[0].selection_state, SelectionState::Selected);
        assert_eq!(set.members[0].rating, 5);
        assert_eq!(set.last_reviewed_photo_id.as_deref(), Some(photo_id));
        persistence.shutdown().unwrap();
        let connection = Connection::open(path).unwrap();
        validate_canonical_schema(&connection, SchemaVersion::V3).unwrap();
    }

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
                    .mutate_photo_set(PhotoSetMutation::Create {
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
    async fn capture_order_uses_raw_authority_ties_paths_and_retains_unavailable_facts() {
        let (_base, library, state, name, _path) = fixture();
        let vectors = capture_order_vectors();
        let disagreement = vectors
            .iter()
            .find(|vector| vector.name == "raw-jpeg-disagreement-uses-raw")
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
        assert_eq!(first.photos.len(), 1);
        let pair = &first.photos[0];
        assert!(pair.available);
        assert!(!pair.ambiguous);
        assert_eq!(pair.preview_state, PreviewState::InspectionPending);
        assert_eq!(pair.preview_candidate, Some(PreviewCandidate::MatchingJpeg));
        let pair_id = pair.id.clone();
        let raw_id = pair.raw_original_id.clone().unwrap();
        let jpeg_id = pair.jpeg_original_id.clone().unwrap();

        let unavailable = persistence
            .apply_scan(Vec::new(), Vec::new())
            .await
            .unwrap();
        let missing = &unavailable.photos[0];
        assert_eq!(missing.id, pair_id);
        assert_eq!(missing.raw_original_id.as_deref(), Some(raw_id.as_str()));
        assert_eq!(missing.jpeg_original_id.as_deref(), Some(jpeg_id.as_str()));
        assert!(!missing.available);
        assert_eq!(missing.preview_state, PreviewState::Unavailable);

        let restored = persistence.apply_scan(vec![raw], Vec::new()).await.unwrap();
        let restored_photo = &restored.photos[0];
        assert_eq!(restored_photo.id, pair_id);
        assert!(restored_photo.available);
        assert_eq!(
            restored_photo.raw_original_id.as_deref(),
            Some(raw_id.as_str())
        );
        assert_eq!(
            restored_photo.jpeg_original_id.as_deref(),
            Some(jpeg_id.as_str())
        );
        assert_eq!(
            restored_photo.preview_state,
            PreviewState::InspectionPending
        );
        persistence.shutdown().unwrap();
    }

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
                    "SELECT photo_id FROM review_progress WHERE photo_set_id=?",
                    ["00000000-0000-4000-8000-000000000021"],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "stable-photo"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT count(*) FROM photo_set_members WHERE photo_set_id=?",
                    ["00000000-0000-4000-8000-000000000021"],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn preserves_preview_facts_only_for_unchanged_selected_source_and_uses_cas() {
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
        let photo_id = first.photos[0].id.clone();
        let revision = source_revision("one.JPG", 4, 1000.0).unwrap();
        assert_eq!(
            persistence
                .seed_preview(PreviewSeed {
                    photo_id: photo_id.clone(),
                    state: PreviewState::Ready,
                    expected_candidate: PreviewCandidate::MatchingJpeg,
                    expected_source_revision: revision.clone(),
                    width: Some(100),
                    height: Some(50),
                    cache_revision: Some("cache-v1".to_owned()),
                    actual_source: None,
                    actual_source_revision: None,
                })
                .await
                .unwrap(),
            PreviewSeedResult::Applied
        );
        let unchanged = persistence
            .apply_scan(vec![raw.clone(), jpeg.clone()], Vec::new())
            .await
            .unwrap();
        assert_eq!(unchanged.photos[0].preview_state, PreviewState::Ready);
        assert_eq!(
            unchanged.photos[0].preview_source,
            Some(PreviewCandidate::MatchingJpeg)
        );
        assert_eq!(
            unchanged.photos[0].cache_revision.as_deref(),
            Some("cache-v1")
        );

        let changed_jpeg = discovered("one.JPG", OriginalKind::Jpeg, 5, 1001.0);
        let changed = persistence
            .apply_scan(vec![raw, changed_jpeg], Vec::new())
            .await
            .unwrap();
        assert_eq!(
            changed.photos[0].preview_state,
            PreviewState::InspectionPending
        );
        assert_eq!(changed.photos[0].preview_source, None);
        assert_eq!(
            persistence
                .seed_preview(PreviewSeed {
                    photo_id,
                    state: PreviewState::Ready,
                    expected_candidate: PreviewCandidate::MatchingJpeg,
                    expected_source_revision: revision,
                    width: Some(100),
                    height: Some(50),
                    cache_revision: Some("stale".to_owned()),
                    actual_source: None,
                    actual_source_revision: None,
                })
                .await
                .unwrap(),
            PreviewSeedResult::StaleIgnored
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn preview_cas_accepts_only_matching_jpeg_to_raw_fallback_with_both_revisions() {
        let (_base, library, state, name, _path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let raw = discovered("one.ARW", OriginalKind::Raw, 3, 1000.0);
        let jpeg = discovered("one.JPG", OriginalKind::Jpeg, 4, 1000.0);
        let initial = persistence
            .apply_scan(vec![raw, jpeg], Vec::new())
            .await
            .unwrap();
        let photo_id = initial.photos[0].id.clone();
        let jpeg_revision = source_revision("one.JPG", 4, 1000.0).unwrap();
        let raw_revision = source_revision("one.ARW", 3, 1000.0).unwrap();
        assert_eq!(
            persistence
                .seed_preview(PreviewSeed {
                    photo_id: photo_id.clone(),
                    state: PreviewState::Ready,
                    expected_candidate: PreviewCandidate::MatchingJpeg,
                    expected_source_revision: jpeg_revision,
                    width: Some(512),
                    height: Some(341),
                    cache_revision: Some("fallback".to_owned()),
                    actual_source: Some(PreviewCandidate::EmbeddedRawJpeg),
                    actual_source_revision: Some(raw_revision.clone()),
                })
                .await
                .unwrap(),
            PreviewSeedResult::Applied
        );
        let preserved = persistence
            .apply_scan(
                vec![
                    discovered("one.ARW", OriginalKind::Raw, 3, 1000.0),
                    discovered("one.JPG", OriginalKind::Jpeg, 4, 1000.0),
                ],
                Vec::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            preserved.photos[0].preview_source,
            Some(PreviewCandidate::EmbeddedRawJpeg)
        );
        assert_eq!(
            preserved.photos[0].preview_source_revision.as_deref(),
            Some(raw_revision.as_str())
        );
        assert_eq!(
            persistence
                .seed_preview(PreviewSeed {
                    photo_id: photo_id.clone(),
                    state: PreviewState::Ready,
                    expected_candidate: PreviewCandidate::EmbeddedRawJpeg,
                    expected_source_revision: raw_revision.clone(),
                    width: Some(512),
                    height: Some(341),
                    cache_revision: Some("inverse".to_owned()),
                    actual_source: Some(PreviewCandidate::MatchingJpeg),
                    actual_source_revision: Some(source_revision("one.JPG", 4, 1000.0).unwrap()),
                })
                .await
                .unwrap(),
            PreviewSeedResult::StaleIgnored
        );
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
                    expected_candidate: PreviewCandidate::EmbeddedRawJpeg,
                    expected_source_revision: raw_revision.clone(),
                    width: Some(512),
                    height: Some(341),
                    cache_revision: Some("raw-cache".to_owned()),
                    actual_source: None,
                    actual_source_revision: None,
                })
                .await
                .unwrap(),
            PreviewSeedResult::Applied
        );
        let jpeg = discovered("one.JPG", OriginalKind::Jpeg, 4, 1000.0);
        let updated = first.apply_scan(vec![raw, jpeg], Vec::new()).await.unwrap();
        assert_eq!(
            updated.photos[0].preview_candidate,
            Some(PreviewCandidate::MatchingJpeg)
        );
        assert_eq!(
            first
                .seed_preview(PreviewSeed {
                    photo_id,
                    state: PreviewState::Ready,
                    expected_candidate: PreviewCandidate::EmbeddedRawJpeg,
                    expected_source_revision: raw_revision,
                    width: Some(512),
                    height: Some(341),
                    cache_revision: Some("stale".to_owned()),
                    actual_source: None,
                    actual_source_revision: None,
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

    #[tokio::test]
    async fn photo_set_crud_normalizes_names_and_preserves_rows_across_restart() {
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
            .mutate_photo_set(PhotoSetMutation::Create {
                name: "  Picks  ".to_owned(),
            })
            .await
            .unwrap();
        let set_id = created.photo_set_id;
        persistence
            .mutate_photo_set(PhotoSetMutation::AddMembers {
                photo_set_id: set_id.clone(),
                photo_ids: vec![photo_id.clone()],
            })
            .await
            .unwrap();
        assert_eq!(
            persistence
                .mutate_photo_set(PhotoSetMutation::Create {
                    name: "pIcKs".to_owned(),
                })
                .await,
            Err(MutationError::Conflict)
        );
        persistence
            .mutate_photo_set(PhotoSetMutation::Rename {
                photo_set_id: set_id.clone(),
                name: " Renamed ".to_owned(),
            })
            .await
            .unwrap();
        let sets = persistence.list_photo_sets().await.unwrap();
        assert_eq!(sets[0].name, "Renamed");
        persistence.shutdown().unwrap();

        let state = StateDirectory::open_or_create(&library, path.parent().unwrap()).unwrap();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let sets = persistence.list_photo_sets().await.unwrap();
        assert_eq!(sets[0].id, set_id);
        assert_eq!(sets[0].members[0].photo_id, photo_id);
        persistence
            .mutate_photo_set(PhotoSetMutation::Delete {
                photo_set_id: set_id,
            })
            .await
            .unwrap();
        assert!(persistence.list_photo_sets().await.unwrap().is_empty());
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
        let set_id = persistence
            .mutate_photo_set(PhotoSetMutation::Create {
                name: "Order".to_owned(),
            })
            .await
            .unwrap()
            .photo_set_id;
        assert_eq!(
            persistence
                .mutate_photo_set(PhotoSetMutation::AddMembers {
                    photo_set_id: set_id.clone(),
                    photo_ids: vec![ids[0].clone(), ids[0].clone()],
                })
                .await,
            Err(MutationError::Conflict)
        );
        assert_eq!(
            persistence.list_photo_sets().await.unwrap()[0]
                .members
                .len(),
            0
        );
        assert_eq!(
            persistence
                .mutate_photo_set(PhotoSetMutation::AddMembers {
                    photo_set_id: set_id.clone(),
                    photo_ids: vec![ids[0].clone(), "unknown".to_owned()],
                })
                .await,
            Err(MutationError::NotFound)
        );
        assert_eq!(
            persistence.list_photo_sets().await.unwrap()[0]
                .members
                .len(),
            0
        );
        assert_eq!(
            persistence
                .mutate_photo_set(PhotoSetMutation::AddMembers {
                    photo_set_id: set_id.clone(),
                    photo_ids: (0..=100).map(|index| format!("unknown-{index}")).collect(),
                })
                .await,
            Err(MutationError::Conflict)
        );
        persistence
            .mutate_photo_set(PhotoSetMutation::AddMembers {
                photo_set_id: set_id.clone(),
                photo_ids: ids.clone(),
            })
            .await
            .unwrap();
        assert_eq!(
            persistence
                .mutate_photo_set(PhotoSetMutation::Reorder {
                    photo_set_id: set_id.clone(),
                    photo_ids: vec![ids[2].clone(), ids[0].clone(), ids[1].clone()],
                })
                .await
                .unwrap()
                .photo_set_id,
            set_id
        );
        assert_eq!(
            persistence.list_photo_sets().await.unwrap()[0]
                .members
                .iter()
                .map(|member| member.photo_id.clone())
                .collect::<Vec<_>>(),
            vec![ids[2].clone(), ids[0].clone(), ids[1].clone()]
        );
        assert_eq!(
            persistence
                .mutate_photo_set(PhotoSetMutation::Reorder {
                    photo_set_id: set_id.clone(),
                    photo_ids: vec![ids[0].clone(), ids[0].clone(), ids[1].clone()],
                })
                .await,
            Err(MutationError::Conflict)
        );
        persistence
            .mutate_photo_set(PhotoSetMutation::RemoveMember {
                photo_set_id: set_id.clone(),
                photo_id: ids[0].clone(),
            })
            .await
            .unwrap();
        let members = &persistence.list_photo_sets().await.unwrap()[0].members;
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
                "UPDATE photo_set_members SET position=9 WHERE photo_set_id=? AND photo_id=?",
                params![set_id, ids[1]],
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
                .mutate_photo_set(PhotoSetMutation::Reorder {
                    photo_set_id: set_id.clone(),
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
        let set_a = persistence
            .mutate_photo_set(PhotoSetMutation::Create {
                name: "A".to_owned(),
            })
            .await
            .unwrap()
            .photo_set_id;
        let set_b = persistence
            .mutate_photo_set(PhotoSetMutation::Create {
                name: "B".to_owned(),
            })
            .await
            .unwrap()
            .photo_set_id;
        for set_id in [&set_a, &set_b] {
            persistence
                .mutate_photo_set(PhotoSetMutation::AddMembers {
                    photo_set_id: set_id.clone(),
                    photo_ids: ids.clone(),
                })
                .await
                .unwrap();
        }
        persistence
            .mutate_photo_set(PhotoSetMutation::SetProgress {
                photo_set_id: set_a.clone(),
                photo_id: ids[0].clone(),
            })
            .await
            .unwrap();
        let sets = persistence.list_photo_sets().await.unwrap();
        assert_eq!(
            sets.iter()
                .find(|set| set.id == set_a)
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
                photo_set_id: Some(set_b.clone()),
            })
            .await
            .unwrap();
        assert_eq!(
            result.undo.prior_value,
            PhotoStateValue::Selection(SelectionState::Undecided)
        );
        let sets = persistence.list_photo_sets().await.unwrap();
        let current_a = sets.iter().find(|set| set.id == set_a).unwrap();
        let current_b = sets.iter().find(|set| set.id == set_b).unwrap();
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
                    photo_set_id: None,
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
                photo_set_id: None,
            })
            .await
            .unwrap();
        persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: ids[1].clone(),
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(5),
                expected_current: Some(PhotoStateValue::Rating(0)),
                photo_set_id: Some(set_a.clone()),
            })
            .await
            .unwrap();
        let sets = persistence.list_photo_sets().await.unwrap();
        let current_a = sets.iter().find(|set| set.id == set_a).unwrap();
        let current_b = sets.iter().find(|set| set.id == set_b).unwrap();
        assert_eq!(current_a.members[1].rating, 5);
        assert_eq!(current_b.members[1].rating, 5);
        assert_eq!(
            current_a.last_reviewed_photo_id.as_deref(),
            Some(ids[1].as_str())
        );
        persistence
            .mutate_photo_set(PhotoSetMutation::RemoveMember {
                photo_set_id: set_a.clone(),
                photo_id: ids[1].clone(),
            })
            .await
            .unwrap();
        let sets = persistence.list_photo_sets().await.unwrap();
        assert_eq!(
            sets.iter()
                .find(|set| set.id == set_a)
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
                photo_set_id: None,
            })
            .await
            .unwrap();
        let set_id = persistence
            .mutate_photo_set(PhotoSetMutation::Create {
                name: "Keep".to_owned(),
            })
            .await
            .unwrap()
            .photo_set_id;
        persistence
            .mutate_photo_set(PhotoSetMutation::AddMembers {
                photo_set_id: set_id.clone(),
                photo_ids: vec![photo_id.clone()],
            })
            .await
            .unwrap();
        persistence
            .apply_scan(Vec::new(), Vec::new())
            .await
            .unwrap();
        let member = &persistence.list_photo_sets().await.unwrap()[0].members[0];
        assert!(!member.available);
        assert_eq!(member.rating, 4);
        let sidecar = path.with_file_name("library.sqlite-journal");
        fs::write(&sidecar, b"operator recovery data").unwrap();
        fs::set_permissions(&sidecar, fs::Permissions::from_mode(0o600)).unwrap();
        let before = fs::read(&path).unwrap();
        assert_eq!(
            persistence
                .mutate_photo_set(PhotoSetMutation::Create {
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
            library.list_photo_sets().await,
            Err(crate::LibraryError::Closed)
        ));
        assert!(matches!(
            library
                .mutate_photo_set(PhotoSetMutation::Create {
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
