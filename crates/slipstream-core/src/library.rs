use crate::{
    AlbumBrowseTarget, AlbumCreationResult, AlbumMembershipMutation, AlbumMembershipResult,
    AlbumMutation, AlbumMutationResult, AlbumQueryFilter, AlbumRecord, AlbumSummary,
    AppliedRelocations, CaptureFact, CheckedAlbumMutation, CheckedAlbumMutationResult,
    EditRecipeRead, EditRecipeWriteOutcome, ExplicitPhotoRemovalMutation,
    ExplicitPhotoRestoreMutation, ExplicitPhotoRestoreResult, ExportAttempt, ExportLeaseOutcome,
    ExportRecord, ExportRetryOutcome, ExportSettlement, ExportSubmission,
    ExportSubmissionResolution, ExportSubmitOutcome, ExportSweepResult, LibraryRoot,
    NativeWorkBudget, NativeWorkPermit, OriginalCapability, OriginalDeletionOutcome,
    PermanentDeletionItemState, PermanentDeletionResult, PermanentDeletionReview,
    PermanentDeletionSelection, PermanentDeletionTarget, PhotoAlbumMembership,
    PhotoOperationRemainder, PhotoQuery, PhotoQueryError, PhotoQueryProjection, PhotoRead,
    PhotoRemovalMutation, PhotoRemovalResult, PhotoRestoration, PhotoRestorationResult,
    PhotoStateBatchMutation, PhotoStateBatchResult, PhotoStateMutation, PhotoStateMutationResult,
    PreviewSeed, PreviewSeedResult, RebindEditRecipe, RecoverySurvey, RemovedPhotoRecord,
    RequestedRelocation, SaveEditRecipe, ScanLimits, ScanResult, ScanSnapshot,
};
use crate::{
    capture::capture_source_revision,
    persistence::{
        AlbumWriteError, DatabaseName, MutationError, Persistence, PersistenceError,
        StateDirectory, StateError, expand_library_binding,
    },
};
use std::{
    fmt,
    num::NonZeroUsize,
    path::PathBuf,
    sync::atomic::AtomicU64,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
};

#[cfg(test)]
use std::sync::OnceLock;

const MAX_SCAN_WAITERS: usize = 64;

const DEFAULT_SCAN_CAPACITY: usize = 1;

/// The phase of the scan currently owned by the Library scanner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScanPhase {
    /// No scan is running and none has been admitted.
    #[default]
    Idle,
    /// Walking the Library Folder for supported Original Files.
    Discovering,
    /// Inspecting Capture Time facts for discovered files.
    Inspecting,
    /// Proving relocated Original identities through content fingerprints.
    Recovering,
    /// Applying one completed scan result to the state store.
    Applying,
}

/// Truthful, measurable progress for the scan currently owned by the scanner.
/// Counters are absent where the corresponding total is not yet known.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanProgress {
    pub phase: ScanPhase,
    /// Supported files discovered so far during the current walk.
    pub discovered: u64,
    /// Originals whose Capture Time fact has been resolved so far.
    pub inspected: u64,
    /// Total originals to inspect once the walk has completed.
    pub inspect_total: Option<u64>,
    /// Fingerprint hashes completed during the recovering phase.
    pub hashed: u64,
    /// Total fingerprint hashes required by the recovering phase.
    pub hash_total: Option<u64>,
}

/// The committed outcome of the most recent completed scan, for truthful
/// status reporting. Counts are Photo counts from the committed snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanOutcome {
    pub relocated_originals: usize,
    pub fingerprinted_originals: usize,
    pub unavailable_photos: usize,
}

#[cfg(test)]
struct ScannerTestHook {
    canonical_root: PathBuf,
    entered: Mutex<usize>,
    entered_signal: Condvar,
    admitted: Mutex<usize>,
    admitted_signal: Condvar,
    release: Mutex<bool>,
    release_signal: Condvar,
}

#[cfg(test)]
static SCANNER_TEST_HOOK: OnceLock<Mutex<Option<Arc<ScannerTestHook>>>> = OnceLock::new();
#[cfg(test)]
static SCANNER_TEST_HOOK_LEASE: OnceLock<Mutex<()>> = OnceLock::new();

#[cfg(test)]
fn scanner_test_hook(root: &LibraryRoot) {
    let hook = SCANNER_TEST_HOOK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap()
        .clone();
    let Some(hook) = hook else { return };
    if hook.canonical_root != root.canonical_path() {
        return;
    }
    {
        let mut entered = hook.entered.lock().unwrap();
        *entered += 1;
        hook.entered_signal.notify_all();
    }
    let mut release = hook.release.lock().unwrap();
    while !*release {
        release = hook.release_signal.wait(release).unwrap();
    }
}

#[cfg(test)]
fn scanner_admitted(root: &LibraryRoot) {
    let hook = SCANNER_TEST_HOOK
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap()
        .clone();
    if let Some(hook) = hook {
        if hook.canonical_root != root.canonical_path() {
            return;
        }
        let mut admitted = hook.admitted.lock().unwrap();
        *admitted += 1;
        hook.admitted_signal.notify_all();
    }
}

#[derive(Clone, Debug)]
pub struct LibraryConfig {
    pub library_root: PathBuf,
    pub state_directory: PathBuf,
    pub database_basename: String,
    pub limits: ScanLimits,
    pub command_capacity: NonZeroUsize,
}

impl Default for LibraryConfig {
    fn default() -> Self {
        Self {
            library_root: PathBuf::new(),
            state_directory: PathBuf::new(),
            database_basename: "library.sqlite".to_owned(),
            limits: ScanLimits::default(),
            command_capacity: NonZeroUsize::new(64).unwrap(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum LibraryError {
    Confinement(crate::confinement::ConfinementError),
    State(StateError),
    Persistence(PersistenceError),
    Mutation(MutationError),
    AlbumWrite(AlbumWriteError),
    PhotoDecisionWrite(crate::PhotoDecisionWriteError),
    Query(PhotoQueryError),
    ScanBusy,
    Closed,
    ScannerStopped,
    UnsupportedRootEncoding,
}

impl fmt::Display for LibraryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Confinement(error) => error.fmt(formatter),
            Self::State(error) => error.fmt(formatter),
            Self::Persistence(error) => error.fmt(formatter),
            Self::Mutation(error) => error.fmt(formatter),
            Self::AlbumWrite(error) => error.fmt(formatter),
            Self::PhotoDecisionWrite(error) => error.fmt(formatter),
            Self::Query(error) => error.fmt(formatter),
            Self::ScanBusy => formatter.write_str("Photo Library scan is busy"),
            Self::Closed => formatter.write_str("Photo Library is closed"),
            Self::ScannerStopped => {
                formatter.write_str("Photo Library scanner stopped unexpectedly")
            }
            Self::UnsupportedRootEncoding => {
                formatter.write_str("Photo Library root must use UTF-8 path encoding")
            }
        }
    }
}

impl std::error::Error for LibraryError {}
impl From<crate::confinement::ConfinementError> for LibraryError {
    fn from(value: crate::confinement::ConfinementError) -> Self {
        Self::Confinement(value)
    }
}
impl From<StateError> for LibraryError {
    fn from(value: StateError) -> Self {
        Self::State(value)
    }
}
impl From<PersistenceError> for LibraryError {
    fn from(value: PersistenceError) -> Self {
        Self::Persistence(value)
    }
}
impl From<MutationError> for LibraryError {
    fn from(value: MutationError) -> Self {
        Self::Mutation(value)
    }
}
impl From<AlbumWriteError> for LibraryError {
    fn from(value: AlbumWriteError) -> Self {
        Self::AlbumWrite(value)
    }
}
impl From<crate::PhotoDecisionWriteError> for LibraryError {
    fn from(value: crate::PhotoDecisionWriteError) -> Self {
        Self::PhotoDecisionWrite(value)
    }
}
impl From<PhotoQueryError> for LibraryError {
    fn from(value: PhotoQueryError) -> Self {
        Self::Query(value)
    }
}

type ScanReply = tokio::sync::oneshot::Sender<Result<Arc<ScanSnapshot>, LibraryError>>;

enum ScanCommand {
    Scan,
    Stop,
}

struct ScanState {
    open: bool,
    in_flight: Option<Vec<ScanReply>>,
}

struct Lifecycle {
    open: bool,
}

struct Scanner {
    sender: std::sync::mpsc::SyncSender<ScanCommand>,
    state: Arc<(Mutex<ScanState>, Condvar)>,
    join: Mutex<Option<JoinHandle<()>>>,
}

pub struct Library {
    root: LibraryRoot,
    native_work: NativeWorkBudget,
    persistence: Persistence,
    scanner: Scanner,
    enrollment: Arc<(Mutex<EnrollmentState>, Condvar)>,
    enrollment_join: Mutex<Option<JoinHandle<()>>>,
    progress: Arc<Mutex<ScanProgress>>,
    outcome: Arc<Mutex<Option<ScanOutcome>>>,
    fingerprint_counts: Arc<Mutex<crate::persistence::FingerprintCounts>>,
    lifecycle: Mutex<Lifecycle>,
    shutdown: Mutex<Option<Result<(), LibraryError>>>,
}

#[derive(Default)]
struct EnrollmentState {
    stopped: bool,
    scan_running: bool,
}

pub fn expand_library(config: LibraryConfig) -> Result<(), LibraryError> {
    let root = LibraryRoot::open(&config.library_root)?;
    root.canonical_path()
        .to_str()
        .ok_or(LibraryError::UnsupportedRootEncoding)?;
    let state = StateDirectory::open_or_create(&root, &config.state_directory)?;
    let database = DatabaseName::parse(config.database_basename).map_err(LibraryError::State)?;
    expand_library_binding(&root, state, database, config.limits, false).map_err(Into::into)
}

#[cfg(test)]
fn expand_library_with_transaction_failure(config: LibraryConfig) -> Result<(), LibraryError> {
    let root = LibraryRoot::open(&config.library_root)?;
    let state = StateDirectory::open_or_create(&root, &config.state_directory)?;
    let database = DatabaseName::parse(config.database_basename).map_err(LibraryError::State)?;
    expand_library_binding(&root, state, database, config.limits, true).map_err(Into::into)
}

impl Drop for Library {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

impl Library {
    pub fn open(config: LibraryConfig) -> Result<Self, LibraryError> {
        let root = LibraryRoot::open(&config.library_root)?;
        let canonical_root = root
            .canonical_path()
            .to_str()
            .ok_or(LibraryError::UnsupportedRootEncoding)?
            .to_owned();
        let state = StateDirectory::open_or_create(&root, &config.state_directory)?;
        let database =
            DatabaseName::parse(config.database_basename).map_err(LibraryError::State)?;
        let persistence = Persistence::open_with_capacity(
            state,
            database,
            canonical_root,
            config.command_capacity,
        )?;
        let (sender, receiver) = std::sync::mpsc::sync_channel(DEFAULT_SCAN_CAPACITY);
        let state = Arc::new((
            Mutex::new(ScanState {
                open: true,
                in_flight: None,
            }),
            Condvar::new(),
        ));
        let native_work = NativeWorkBudget::new();
        let progress = Arc::new(Mutex::new(ScanProgress::default()));
        let outcome = Arc::new(Mutex::new(None::<ScanOutcome>));
        let enrollment = Arc::new((Mutex::new(EnrollmentState::default()), Condvar::new()));
        let fingerprint_counts =
            Arc::new(Mutex::new(crate::persistence::FingerprintCounts::default()));
        let worker_root = root.clone();
        let worker_native_work = native_work.clone();
        let worker_persistence = persistence.clone();
        let worker_state = state.clone();
        let worker_progress = Arc::clone(&progress);
        let worker_outcome = Arc::clone(&outcome);
        let worker_enrollment = Arc::clone(&enrollment);
        let worker_counts = Arc::clone(&fingerprint_counts);
        let join = thread::Builder::new()
            .name("slipstream-scanner".to_owned())
            .spawn(move || {
                scanner_main(
                    worker_root,
                    worker_native_work,
                    worker_persistence,
                    config.limits,
                    receiver,
                    ScannerShared {
                        state: worker_state,
                        progress: worker_progress,
                        outcome: worker_outcome,
                        enrollment: worker_enrollment,
                        fingerprint_counts: worker_counts,
                    },
                )
            })
            .map_err(|_| LibraryError::ScannerStopped)?;
        let enrollment_root = root.clone();
        let enrollment_native_work = native_work.clone();
        let enrollment_persistence = persistence.clone();
        let enrollment_shared = Arc::clone(&enrollment);
        let enrollment_counts = Arc::clone(&fingerprint_counts);
        let enrollment_join = thread::Builder::new()
            .name("slipstream-fingerprints".to_owned())
            .spawn(move || {
                enrollment_main(
                    enrollment_root,
                    enrollment_native_work,
                    enrollment_persistence,
                    enrollment_shared,
                    enrollment_counts,
                )
            })
            .map_err(|_| LibraryError::ScannerStopped)?;
        Ok(Self {
            root,
            native_work,
            persistence,
            scanner: Scanner {
                sender,
                state,
                join: Mutex::new(Some(join)),
            },
            enrollment,
            enrollment_join: Mutex::new(Some(enrollment_join)),
            progress,
            outcome,
            fingerprint_counts,
            lifecycle: Mutex::new(Lifecycle { open: true }),
            shutdown: Mutex::new(None),
        })
    }

    pub fn canonical_root(&self) -> &std::path::Path {
        self.root.canonical_path()
    }

    /// Current observable progress of the scan owned by the scanner thread.
    pub fn scan_progress(&self) -> ScanProgress {
        *self.progress.lock().unwrap()
    }

    /// The committed outcome of the most recent completed scan, if one has
    /// completed since this Library opened.
    pub fn scan_outcome(&self) -> Option<ScanOutcome> {
        *self.outcome.lock().unwrap()
    }

    /// Records one committed manual relocation batch in the recovery
    /// counters the scan status reports, so the review notice stays truthful
    /// between scans. Fingerprints discovered by the last scan are not part
    /// of a manual batch and report zero.
    pub fn note_manual_recovery(&self, relocated: u64, unavailable: u64) {
        let relocated = usize::try_from(relocated).unwrap_or(usize::MAX);
        let unavailable = usize::try_from(unavailable).unwrap_or(usize::MAX);
        *self.outcome.lock().unwrap() = Some(ScanOutcome {
            relocated_originals: relocated,
            fingerprinted_originals: 0,
            unavailable_photos: unavailable,
        });
    }

    /// Truthful fingerprint enrollment counters for status reporting. The
    /// enrollment worker and the scanner refresh these after every committed
    /// change, so reading them never blocks on SQLite.
    pub fn fingerprint_counts(&self) -> crate::persistence::FingerprintCounts {
        *self.fingerprint_counts.lock().unwrap()
    }

    pub(crate) fn native_work_budget(&self) -> NativeWorkBudget {
        self.native_work.clone()
    }

    /// Attempts immediate admission to this Library's shared native-work
    /// capacity. Callers must obtain the permit before scheduling blocking
    /// work and retain it until that work has completed.
    pub fn try_admit_native_work(&self) -> Option<NativeWorkPermit> {
        self.native_work.try_acquire()
    }

    pub(crate) fn snapshot_blocking(&self) -> Result<ScanSnapshot, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .snapshot_receiver()
                .map_err(LibraryError::from)?
        };
        receive
            .blocking_recv()
            .unwrap_or(Err(crate::persistence::PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    pub async fn snapshot(&self) -> Result<ScanSnapshot, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .snapshot_receiver()
                .map_err(LibraryError::from)?
        };
        receive
            .await
            .unwrap_or(Err(crate::persistence::PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    pub async fn scan(&self) -> Result<ScanSnapshot, LibraryError> {
        let (reply, receive) = tokio::sync::oneshot::channel();
        {
            let _admission = self.admit()?;
            let (lock, _) = &*self.scanner.state;
            let mut state = lock.lock().unwrap();
            if !state.open {
                return Err(LibraryError::Closed);
            }
            let first = state.in_flight.is_none();
            let waiters = state.in_flight.get_or_insert_with(Vec::new);
            // Receiver cancellation releases waiter capacity without
            // cancelling the physical scanner operation.
            waiters.retain(|waiter| !waiter.is_closed());
            if waiters.len() >= MAX_SCAN_WAITERS {
                return Err(LibraryError::ScanBusy);
            }
            waiters.push(reply);
            #[cfg(test)]
            scanner_admitted(&self.root);
            if first && self.scanner.sender.try_send(ScanCommand::Scan).is_err() {
                state.in_flight.take();
                return Err(LibraryError::ScanBusy);
            }
        }
        receive
            .await
            .unwrap_or(Err(LibraryError::ScannerStopped))
            .map(|snapshot| (*snapshot).clone())
    }

    pub fn original(
        &self,
        path: crate::RelativeOriginalPath,
    ) -> Result<OriginalCapability, LibraryError> {
        let _admission = self.admit()?;
        self.root.original(path).map_err(Into::into)
    }

    pub async fn list_albums(&self) -> Result<Vec<AlbumRecord>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.list_albums_receiver()
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// One consistent read of unavailable Photos and Album memberships for
    /// the manual recovery review entry.
    pub async fn recovery_survey(&self) -> Result<RecoverySurvey, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.recovery_survey_receiver()
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Revalidates and commits one confirmed manual relocation batch
    /// atomically. Filesystem evidence was gathered through confined
    /// descriptors before submission; the transaction rechecks every
    /// persisted precondition.
    pub async fn apply_relocations(
        &self,
        relocations: Vec<RequestedRelocation>,
    ) -> Result<AppliedRelocations, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.apply_relocations_receiver(relocations)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    pub async fn list_album_summaries(&self) -> Result<Vec<AlbumSummary>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.list_album_summaries_receiver()
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Reads one current Album summary and its mutation guard in one owner
    /// operation. `None` means the Album no longer exists.
    pub async fn album(&self, album_id: &str) -> Result<Option<AlbumSummary>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.album_receiver(album_id)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Reads current Album facts for fixed query IDs, preserving input order
    /// and returning `None` placeholders for deleted Albums.
    pub async fn albums_by_id(
        &self,
        album_ids: Vec<String>,
    ) -> Result<Vec<Option<AlbumSummary>>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.albums_by_id_receiver(album_ids)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Creates one bounded, fixed Album-ID membership for server-retained
    /// pagination. Current summaries are read separately with `albums_by_id`.
    pub async fn create_album_query(
        &self,
        filter: AlbumQueryFilter,
        maximum_results: usize,
    ) -> Result<Vec<String>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .create_album_query_receiver(filter, maximum_results)
        }?;
        receive
            .await
            .map_err(|_| LibraryError::Persistence(PersistenceError::OwnerStopped))?
            .map_err(Into::into)
    }

    /// Reads one current Photo and its decision mutation guard in one owner
    /// operation. `None` means the Photo no longer exists.
    pub async fn photo(&self, photo_id: &str) -> Result<Option<PhotoRead>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.photo_receiver(photo_id)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Reads the current saved recipe and the Library source revision in one
    /// serialized persistence-owner operation.
    pub async fn edit_recipe(
        &self,
        photo_id: &str,
    ) -> Result<Option<EditRecipeRead>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.edit_recipe_receiver(photo_id)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Saves one semantic recipe only when both the caller's recipe revision
    /// and observed Library source revision still match current persistence.
    pub async fn save_edit_recipe(
        &self,
        mutation: SaveEditRecipe,
    ) -> Result<EditRecipeWriteOutcome, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.save_edit_recipe_receiver(mutation)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Rebinds existing settings to the current observed source revision only
    /// after the caller confirms both the recipe and source revisions.
    pub async fn rebind_edit_recipe(
        &self,
        mutation: RebindEditRecipe,
    ) -> Result<EditRecipeWriteOutcome, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.rebind_edit_recipe_receiver(mutation)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Validates both expected revisions inside one serialized transaction,
    /// captures the immutable Export snapshot, reserves output capacity, and
    /// records the request-identity receipt. A repeated identity resolves to
    /// the existing Export without starting work.
    pub async fn submit_export(
        &self,
        submission: ExportSubmission,
    ) -> Result<ExportSubmitOutcome, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.submit_export_receiver(submission)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Reads one Export record. `None` means the identity is unknown or its
    /// retention window has passed.
    pub async fn export(&self, export_id: &str) -> Result<Option<ExportRecord>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.export_receiver(export_id)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Bounded most-recent list of one Photo's retained Exports. `None` means
    /// the Photo is unknown.
    pub async fn photo_exports(
        &self,
        photo_id: &str,
    ) -> Result<Option<Vec<ExportRecord>>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.photo_exports_receiver(photo_id)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Cancels one Export exactly once against its actual completion state;
    /// a settled Export is returned unchanged.
    pub async fn cancel_export(
        &self,
        export_id: &str,
    ) -> Result<Option<ExportRecord>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.cancel_export_receiver(export_id)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Applies one terminal settlement exactly once; an already settled
    /// Export is returned unchanged.
    pub async fn settle_export(
        &self,
        export_id: &str,
        settlement: ExportSettlement,
    ) -> Result<Option<ExportRecord>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .settle_export_receiver(export_id, settlement)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Persists the launcher attempt identity before any work starts. A
    /// terminal record is returned untouched so the caller aborts.
    pub async fn begin_export_attempt(
        &self,
        export_id: &str,
        attempt: ExportAttempt,
    ) -> Result<Option<ExportRecord>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .begin_export_attempt_receiver(export_id, attempt)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Records verified staged source evidence between acceptance and launch.
    pub async fn record_export_source(
        &self,
        export_id: &str,
        size: u64,
        sha256: &str,
    ) -> Result<Option<ExportRecord>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .record_export_source_receiver(export_id, size, sha256)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Re-arms a failed or cancelled Export against its retained snapshot
    /// with the caller's new request identity while its retention window
    /// remains open and the captured source and approved bundle remain
    /// available. A repeated retry identity resolves to its Export and
    /// starts no work.
    pub async fn retry_export(
        &self,
        export_id: &str,
        request_id: &str,
        expected_bundle_id: &str,
        allowance: u64,
    ) -> Result<ExportRetryOutcome, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.retry_export_receiver(
                export_id,
                request_id,
                expected_bundle_id,
                allowance,
            )
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Resolves a request identity without admission or state change: a
    /// recorded identity replays, expires, or conflicts before the submit
    /// transaction runs. `None` means the identity was never recorded.
    pub async fn resolve_export_receipt(
        &self,
        photo_id: &str,
        request_id: &str,
        payload_digest: &str,
    ) -> Result<Option<ExportSubmissionResolution>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.resolve_export_submission_receiver(
                photo_id,
                request_id,
                payload_digest,
            )
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Durably claims the publication of one export attempt before its
    /// artifact is renamed into place.
    pub async fn claim_export_publication(
        &self,
        export_id: &str,
        incarnation: &str,
        sequence: u64,
    ) -> Result<(), LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .claim_export_publication_receiver(export_id, incarnation, sequence)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(LibraryError::from)?;
        Ok(())
    }

    /// Reads an export's durable publication claim, if any: the attempt
    /// whose validated artifact is (about to be) published.
    pub async fn export_publication_claim(
        &self,
        export_id: &str,
    ) -> Result<Option<(String, u64)>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .export_publication_claim_receiver(export_id)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Refreshes a download lease's liveness anchor while its stream runs.
    /// `false` means the lease is gone and the stream must stop renewing.
    pub async fn renew_export_lease(&self, lease_id: &str, now: u64) -> Result<bool, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.renew_export_lease_receiver(lease_id, now)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Removes expired artifacts, records, and stale leases. The caller
    /// removes the named artifact files after the deletions commit.
    pub async fn sweep_export_expiry(&self, now: u64) -> Result<ExportSweepResult, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.sweep_export_expiry_receiver(now)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Lists unfinished Exports for restart reconciliation.
    pub async fn unfinished_exports(&self) -> Result<Vec<ExportRecord>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.unfinished_exports_receiver()
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Holds one Export's artifact and snapshot against expiry cleanup until
    /// the matching download stream settles.
    pub async fn acquire_export_lease(
        &self,
        export_id: &str,
        now: u64,
    ) -> Result<ExportLeaseOutcome, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .acquire_export_lease_receiver(export_id, now)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Releases a download lease once its response stream has settled.
    pub async fn release_export_lease(&self, lease_id: &str) -> Result<bool, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.release_export_lease_receiver(lease_id)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Reads current decision facts and versions for fixed Published Photo
    /// facts, preserving input order and returning `None` placeholders for
    /// Photos absent from that publication or current persistence.
    pub async fn photos_by_id(
        &self,
        photo_ids: Vec<String>,
        projection: Arc<PhotoQueryProjection>,
    ) -> Result<Vec<Option<PhotoRead>>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .photos_by_id_receiver(photo_ids, projection)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    /// Evaluates source, filters, and ordering in the serialized owner and
    /// returns only a bounded fixed ID membership. The caller retains and
    /// pages these IDs, then asks `photos_by_id` for current facts.
    pub async fn create_photo_query(
        &self,
        query: PhotoQuery,
        projection: Arc<PhotoQueryProjection>,
        maximum_results: usize,
    ) -> Result<Vec<String>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .create_photo_query_receiver(query, projection, maximum_results)
        }?;
        receive
            .await
            .map_err(|_| LibraryError::Persistence(PersistenceError::OwnerStopped))?
            .map_err(Into::into)
    }

    /// Bounded per-Photo Album membership for the Photo View membership
    /// query. `None` means the Photo is unknown to the persisted Library.
    pub async fn photo_albums(
        &self,
        photo_id: &str,
    ) -> Result<Option<Vec<PhotoAlbumMembership>>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.photo_albums_receiver(photo_id)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    pub async fn album_browse_target(
        &self,
        album_id: &str,
    ) -> Result<Option<AlbumBrowseTarget>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.album_browse_target_receiver(album_id)
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    pub async fn mutate_album(
        &self,
        mutation: AlbumMutation,
    ) -> Result<AlbumMutationResult, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.mutate_album_receiver(mutation)
        }
        .map_err(LibraryError::from)?;
        receive
            .await
            .unwrap_or(Err(MutationError::Persistence))
            .map_err(Into::into)
    }

    pub async fn mutate_album_membership(
        &self,
        mutation: AlbumMembershipMutation,
    ) -> Result<AlbumMembershipResult, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.mutate_album_membership_receiver(mutation)
        }
        .map_err(LibraryError::from)?;
        receive
            .await
            .unwrap_or(Err(MutationError::Persistence))
            .map_err(Into::into)
    }

    /// Creates an Album and returns its first process-epoch mutation version.
    pub async fn create_album_checked(
        &self,
        name: String,
    ) -> Result<AlbumCreationResult, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.create_album_checked_receiver(name)
        }
        .map_err(LibraryError::from)?;
        receive
            .await
            .unwrap_or(Err(AlbumWriteError::Persistence))
            .map_err(Into::into)
    }

    /// Applies one atomic, version-checked Album mutation and returns the
    /// identity-bearing facts needed by a machine client response.
    pub async fn mutate_album_checked(
        &self,
        mutation: CheckedAlbumMutation,
    ) -> Result<CheckedAlbumMutationResult, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.mutate_album_checked_receiver(mutation)
        }
        .map_err(LibraryError::from)?;
        receive
            .await
            .unwrap_or(Err(AlbumWriteError::Persistence))
            .map_err(Into::into)
    }

    /// Applies one atomic, version-checked Photo decision batch and returns
    /// the per-Photo facts a machine client response needs. The write changes
    /// only the requested decision field and never an Album saved position.
    pub async fn mutate_photo_decision_checked(
        &self,
        mutation: crate::CheckedPhotoDecisionMutation,
    ) -> Result<crate::CheckedPhotoDecisionResult, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .mutate_photo_decision_checked_receiver(mutation)
        }
        .map_err(LibraryError::from)?;
        receive
            .await
            .unwrap_or(Err(crate::PhotoDecisionWriteError::Persistence))
            .map_err(Into::into)
    }

    pub async fn mutate_photo_state(
        &self,
        mutation: PhotoStateMutation,
    ) -> Result<PhotoStateMutationResult, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.mutate_photo_state_receiver(mutation)
        }
        .map_err(LibraryError::from)?;
        receive
            .await
            .unwrap_or(Err(MutationError::Persistence))
            .map_err(Into::into)
    }

    pub async fn mutate_photo_state_batch(
        &self,
        mutation: PhotoStateBatchMutation,
    ) -> Result<PhotoStateBatchResult, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.mutate_photo_state_batch_receiver(mutation)
        }
        .map_err(LibraryError::from)?;
        receive
            .await
            .unwrap_or(Err(MutationError::Persistence))
            .map_err(Into::into)
    }

    /// Removes one reviewed result's Photos from the Library in one
    /// transaction. Each requested Photo reports exactly one outcome, and the
    /// operation id groups what Undo restores.
    pub async fn remove_photos(
        &self,
        mutation: PhotoRemovalMutation,
    ) -> Result<PhotoRemovalResult, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.remove_photos_receiver(mutation)
        }
        .map_err(LibraryError::from)?;
        receive
            .await
            .unwrap_or(Err(MutationError::Persistence))
            .map_err(Into::into)
    }
    /// Removes an explicit, caller-reviewed Photo set with decision evidence.
    pub async fn remove_photos_explicit(
        &self,
        mutation: ExplicitPhotoRemovalMutation,
    ) -> Result<PhotoRemovalResult, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.remove_photos_explicit_receiver(mutation)
        }
        .map_err(LibraryError::from)?;
        receive
            .await
            .unwrap_or(Err(MutationError::Persistence))
            .map_err(Into::into)
    }
    /// Reads the durable historical result of one removal operation without
    /// changing Library state.
    pub async fn photo_removal_operation(
        &self,
        operation_id: String,
    ) -> Result<Option<PhotoRemovalResult>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .photo_removal_operation_receiver(operation_id)
        }
        .map_err(LibraryError::from)?;
        receive
            .await
            .unwrap_or(Err(MutationError::Persistence))
            .map_err(Into::into)
    }
    /// Restores an explicit, caller-reviewed Photo set and retains its
    /// operation result for replay and read-only outcome inspection.
    pub async fn restore_photos_explicit(
        &self,
        mutation: ExplicitPhotoRestoreMutation,
    ) -> Result<ExplicitPhotoRestoreResult, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.restore_photos_explicit_receiver(mutation)
        }
        .map_err(LibraryError::from)?;
        receive
            .await
            .unwrap_or(Err(MutationError::Persistence))
            .map_err(Into::into)
    }

    /// Reads the durable historical result of one explicit restore attempt
    /// without changing Library state.
    pub async fn photo_restore_operation(
        &self,
        operation_id: String,
    ) -> Result<Option<ExplicitPhotoRestoreResult>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .photo_restore_operation_receiver(operation_id)
        }
        .map_err(LibraryError::from)?;
        receive
            .await
            .unwrap_or(Err(MutationError::Persistence))
            .map_err(Into::into)
    }

    /// Restores every Photo one operation still owns, or an explicit set,
    /// through the same compare-and-set rule.
    pub async fn restore_photos(
        &self,
        restoration: PhotoRestoration,
    ) -> Result<PhotoRestorationResult, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.restore_photos_receiver(restoration)
        }
        .map_err(LibraryError::from)?;
        receive
            .await
            .unwrap_or(Err(MutationError::Persistence))
            .map_err(Into::into)
    }

    /// One bounded page of removed Photos, newest removal first.
    pub async fn removed_photos(
        &self,
        start: usize,
        limit: usize,
    ) -> Result<
        (
            Vec<RemovedPhotoRecord>,
            usize,
            Option<PhotoOperationRemainder>,
        ),
        LibraryError,
    > {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.removed_photos_receiver(start, limit)?
        };
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }
    /// Captures a fixed Permanent Deletion review from the current Trash
    /// selection. Filesystem facts are read before the owner persists the
    /// receipt, so the review and later unlink share one exact identity.
    pub async fn prepare_permanent_deletion(
        &self,
        operation_id: String,
        selection: PermanentDeletionSelection,
    ) -> Result<PermanentDeletionReview, LibraryError> {
        let requested_ids = match &selection {
            PermanentDeletionSelection::Photos(photo_ids) => photo_ids.clone(),
            PermanentDeletionSelection::All { .. } => Vec::new(),
        };
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .trash_candidates_receiver(selection)
                .map_err(LibraryError::from)?
        };
        let candidates = receive
            .await
            .unwrap_or(Err(MutationError::Persistence))
            .map_err(LibraryError::from)?;
        let mut targets = Vec::with_capacity(
            candidates
                .len()
                .saturating_add(requested_ids.len().saturating_sub(candidates.len())),
        );
        let mut seen = std::collections::HashSet::with_capacity(candidates.len());
        for candidate in candidates {
            let facts = self
                .root
                .original(candidate.relative_path.clone())
                .ok()
                .and_then(|original| original.facts_if_present().ok().flatten());
            seen.insert(candidate.photo_id.clone());
            targets.push(PermanentDeletionTarget {
                photo_id: candidate.photo_id,
                removed_at_ms: candidate.removed_at_ms,
                facts,
            });
        }
        for photo_id in requested_ids {
            if seen.insert(photo_id.clone()) {
                targets.push(PermanentDeletionTarget {
                    photo_id,
                    removed_at_ms: 0,
                    facts: None,
                });
            }
        }
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .prepare_permanent_deletion_receiver(operation_id, targets)
                .map_err(LibraryError::from)?
        };
        receive
            .await
            .unwrap_or(Err(MutationError::Persistence))
            .map_err(LibraryError::from)
    }

    /// Applies a previously reviewed Permanent Deletion one Original at a
    /// time. A durable `deleting` state makes a crash recoverable as
    /// `uncertain` rather than silently retrying an unlink.
    pub async fn permanently_delete(
        &self,
        operation_id: String,
    ) -> Result<PermanentDeletionResult, LibraryError> {
        let retry_unresolved = self
            .read_permanent_deletion(operation_id.clone())
            .await?
            .items
            .iter()
            .any(|item| {
                matches!(
                    item.state,
                    PermanentDeletionItemState::Failed | PermanentDeletionItemState::Uncertain
                )
            });
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .permanent_deletion_work_receiver(operation_id.clone(), retry_unresolved)
                .map_err(LibraryError::from)?
        };
        let work = receive
            .await
            .unwrap_or(Err(MutationError::Persistence))
            .map_err(LibraryError::from)?;
        for item in work {
            // The durable mark names the unresolved item to every other
            // surface before the confined unlink is attempted.
            let receive = {
                let _admission = self.admit()?;
                self.persistence
                    .mark_permanent_deletion_deleting_receiver(
                        operation_id.clone(),
                        item.photo_id.clone(),
                    )
                    .map_err(LibraryError::from)?
            };
            match receive.await.unwrap_or(Err(MutationError::Persistence)) {
                Ok(()) => {}
                // Another confirmation settled this item first; its own result
                // is authoritative and this attempt changes nothing.
                Err(MutationError::Conflict) => continue,
                Err(error) => return Err(LibraryError::from(error)),
            }
            let (state, message) = match self.root.original(item.relative_path.clone()) {
                Err(error) => (PermanentDeletionItemState::Failed, Some(error.to_string())),
                Ok(original) => match original.delete_if_unchanged(item.facts) {
                    Ok(OriginalDeletionOutcome::Deleted) => {
                        (PermanentDeletionItemState::Deleted, None)
                    }
                    Ok(OriginalDeletionOutcome::Missing) => (
                        if item.state == PermanentDeletionItemState::Deleting {
                            PermanentDeletionItemState::Uncertain
                        } else {
                            PermanentDeletionItemState::Missing
                        },
                        Some("Original was missing before deletion completed".to_owned()),
                    ),
                    Ok(OriginalDeletionOutcome::Changed) => (
                        PermanentDeletionItemState::Changed,
                        Some("Original facts changed after review".to_owned()),
                    ),
                    Ok(OriginalDeletionOutcome::Failed(error)) => {
                        (PermanentDeletionItemState::Failed, Some(error))
                    }
                    Err(error) => (PermanentDeletionItemState::Failed, Some(error.to_string())),
                },
            };
            let receive = {
                let _admission = self.admit()?;
                self.persistence
                    .settle_permanent_deletion_receiver(
                        operation_id.clone(),
                        item.photo_id,
                        state,
                        message,
                    )
                    .map_err(LibraryError::from)?
            };
            receive
                .await
                .unwrap_or(Err(MutationError::Persistence))
                .map_err(LibraryError::from)?;
        }
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .read_permanent_deletion_receiver(operation_id)
                .map_err(LibraryError::from)?
        };
        receive
            .await
            .unwrap_or(Err(MutationError::Persistence))
            .map_err(LibraryError::from)
    }

    pub async fn read_permanent_deletion(
        &self,
        operation_id: String,
    ) -> Result<PermanentDeletionResult, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .read_permanent_deletion_receiver(operation_id)
                .map_err(LibraryError::from)?
        };
        receive
            .await
            .unwrap_or(Err(MutationError::Persistence))
            .map_err(LibraryError::from)
    }

    pub(crate) fn seed_preview_blocking(
        &self,
        preview: PreviewSeed,
    ) -> Result<PreviewSeedResult, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .seed_preview_receiver(preview)
                .map_err(LibraryError::from)?
        };
        receive
            .blocking_recv()
            .unwrap_or(Err(crate::persistence::PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    pub async fn seed_preview(
        &self,
        preview: PreviewSeed,
    ) -> Result<PreviewSeedResult, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .seed_preview_receiver(preview)
                .map_err(LibraryError::from)?
        };
        receive
            .await
            .unwrap_or(Err(crate::persistence::PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    fn admit(&self) -> Result<std::sync::MutexGuard<'_, Lifecycle>, LibraryError> {
        let lifecycle = self.lifecycle.lock().unwrap();
        if lifecycle.open {
            Ok(lifecycle)
        } else {
            Err(LibraryError::Closed)
        }
    }

    pub fn shutdown(&self) -> Result<(), LibraryError> {
        let mut shutdown = self.shutdown.lock().unwrap();
        if let Some(result) = shutdown.clone() {
            return result;
        }
        let result = self.shutdown_inner();
        *shutdown = Some(result.clone());
        result
    }

    fn shutdown_inner(&self) -> Result<(), LibraryError> {
        {
            let mut lifecycle = self.lifecycle.lock().unwrap();
            lifecycle.open = false;
        }
        {
            let (lock, _) = &*self.scanner.state;
            let mut state = lock.lock().unwrap();
            state.open = false;
        }

        let mut first_error = None;
        // A queued scan is already admitted and must drain before shutdown.
        if self.scanner.sender.send(ScanCommand::Stop).is_err() {
            first_error = Some(LibraryError::ScannerStopped);
        }
        if let Some(join) = self.scanner.join.lock().unwrap().take()
            && join.join().is_err()
        {
            first_error.get_or_insert(LibraryError::ScannerStopped);
            let (lock, _) = &*self.scanner.state;
            let mut state = lock.lock().unwrap();
            if let Some(waiters) = state.in_flight.take() {
                for waiter in waiters {
                    let _ = waiter.send(Err(LibraryError::ScannerStopped));
                }
            }
        }
        {
            let mut enrollment_state = self.enrollment.0.lock().unwrap();
            enrollment_state.stopped = true;
        }
        self.enrollment.1.notify_all();
        if let Some(join) = self.enrollment_join.lock().unwrap().take() {
            let _ = join.join();
        }
        self.root.close();
        if let Err(error) = self.persistence.shutdown() {
            first_error.get_or_insert(error.into());
        }
        first_error.map_or(Ok(()), Err)
    }
}

fn inspect_capture_facts(
    root: &LibraryRoot,
    native_work: &NativeWorkBudget,
    originals: &mut [crate::DiscoveredOriginal],
    previous: &[crate::OriginalRecord],
    progress: &Mutex<ScanProgress>,
) {
    let previous = previous
        .iter()
        .map(|original| (original.relative_path.as_str(), original))
        .collect::<std::collections::HashMap<_, _>>();
    for (index, original) in originals.iter_mut().enumerate() {
        progress.lock().unwrap().inspected = u64::try_from(index + 1).unwrap_or(u64::MAX);
        let prior = previous.get(original.path.as_str()).copied();
        if original.error_category.is_some() {
            if let Some(prior) = prior {
                original.capture = prior.capture.clone();
            }
            continue;
        }
        let revision = capture_source_revision(original.path.as_str(), original.facts);
        let Ok(revision) = revision else {
            original.capture = CaptureFact::failed(None);
            continue;
        };
        if let Some(prior) = prior
            && prior.capture.is_reusable_for(&revision)
        {
            original.capture = prior.capture.clone();
            continue;
        }
        original.capture = match root.original(original.path.clone()) {
            Ok(capability) => {
                let _permit = native_work.acquire();
                match crate::capture::inspect_capture(&capability, original.kind, original.facts) {
                    Ok(capture) => capture,
                    Err(crate::CaptureInspectionError::Confinement(
                        crate::confinement::ConfinementError::Changed,
                    )) => match crate::capture::inspect_capture_fresh(&capability, original.kind) {
                        Ok(observation) => {
                            original.facts = observation.facts;
                            observation.capture
                        }
                        Err(_) => CaptureFact::failed(None),
                    },
                    Err(_) => CaptureFact::failed(Some(revision)),
                }
            }
            Err(_) => CaptureFact::failed(Some(revision)),
        };
    }
}

/// Shared handles the scanner thread owns for the lifetime of the Library.
struct ScannerShared {
    state: Arc<(Mutex<ScanState>, Condvar)>,
    progress: Arc<Mutex<ScanProgress>>,
    outcome: Arc<Mutex<Option<ScanOutcome>>>,
    enrollment: Arc<(Mutex<EnrollmentState>, Condvar)>,
    fingerprint_counts: Arc<Mutex<crate::persistence::FingerprintCounts>>,
}

fn scanner_main(
    root: LibraryRoot,
    native_work: NativeWorkBudget,
    persistence: Persistence,
    limits: ScanLimits,
    receiver: std::sync::mpsc::Receiver<ScanCommand>,
    shared: ScannerShared,
) {
    let ScannerShared {
        state,
        progress,
        outcome,
        enrollment,
        fingerprint_counts,
    } = shared;
    while let Ok(command) = receiver.recv() {
        match command {
            ScanCommand::Stop => break,
            ScanCommand::Scan => {
                {
                    enrollment.0.lock().unwrap().scan_running = true;
                    let mut progress = progress.lock().unwrap();
                    *progress = ScanProgress {
                        phase: ScanPhase::Discovering,
                        ..ScanProgress::default()
                    };
                }
                #[cfg(test)]
                scanner_test_hook(&root);
                let discovered = AtomicU64::new(0);
                let result = root
                    .scan_with_progress(limits, &discovered)
                    .map_err(LibraryError::from)
                    .and_then(|mut result: ScanResult| {
                        let previous = persistence
                            .snapshot_blocking()
                            .map_err(LibraryError::from)?;
                        {
                            let mut progress = progress.lock().unwrap();
                            progress.discovered =
                                discovered.load(std::sync::atomic::Ordering::Relaxed);
                            progress.inspected = 0;
                            progress.inspect_total =
                                Some(u64::try_from(result.originals.len()).unwrap_or(u64::MAX));
                            progress.phase = ScanPhase::Inspecting;
                        }
                        inspect_capture_facts(
                            &root,
                            &native_work,
                            &mut result.originals,
                            &previous.originals,
                            &progress,
                        );
                        let mut evidence_ids =
                            crate::recovery::evidence_original_ids(&result.originals, &previous);
                        // A permanently deleted Original keeps no relocation
                        // or identity evidence: its bytes are gone, and a file
                        // at its reviewed Location is a new Original.
                        let deleted_originals = persistence
                            .permanently_deleted_original_ids_blocking()
                            .map_err(LibraryError::from)?
                            .into_iter()
                            .collect::<std::collections::HashSet<_>>();
                        evidence_ids.retain(|id| !deleted_originals.contains(id));
                        let fingerprints = persistence
                            .recovery_facts_blocking(evidence_ids)
                            .map_err(LibraryError::from)?;
                        let mut recovery_progress = crate::recovery::RecoveryProgress::default();
                        {
                            let mut progress = progress.lock().unwrap();
                            progress.phase = ScanPhase::Recovering;
                        }
                        let recovery = crate::recovery::plan_recovery(
                            &root,
                            &native_work,
                            &result.originals,
                            &previous,
                            &fingerprints,
                            &deleted_originals,
                            &mut recovery_progress,
                        );
                        {
                            let mut progress = progress.lock().unwrap();
                            progress.hashed = recovery_progress.hashed;
                            progress.hash_total = Some(recovery_progress.hash_total);
                        }
                        progress.lock().unwrap().phase = ScanPhase::Applying;
                        let applied = persistence
                            .apply_scan_recovered_blocking(
                                result.originals,
                                result.errors,
                                recovery,
                            )
                            .map_err(LibraryError::from)?;
                        let unavailable = applied
                            .snapshot
                            .photos
                            .iter()
                            .filter(|photo| !photo.available)
                            .count();
                        *outcome.lock().unwrap() = Some(ScanOutcome {
                            relocated_originals: applied.relocated_originals,
                            fingerprinted_originals: applied.fingerprinted_originals,
                            unavailable_photos: unavailable,
                        });
                        if let Ok(counts) = persistence.fingerprint_counts_blocking() {
                            *fingerprint_counts.lock().unwrap() = counts;
                        }
                        Ok(applied.snapshot)
                    })
                    .map(Arc::new);
                {
                    let mut enrollment_state = enrollment.0.lock().unwrap();
                    enrollment_state.scan_running = false;
                }
                enrollment.1.notify_all();
                progress.lock().unwrap().phase = ScanPhase::Idle;
                let (lock, signal) = &*state;
                let mut guard = lock.lock().unwrap();
                if let Some(waiters) = guard.in_flight.take() {
                    for waiter in waiters {
                        let _ = waiter.send(result.clone());
                    }
                }
                signal.notify_all();
            }
        }
    }
    let (lock, signal) = &*state;
    let mut guard = lock.lock().unwrap();
    if let Some(waiters) = guard.in_flight.take() {
        for waiter in waiters {
            let _ = waiter.send(Err(LibraryError::ScannerStopped));
        }
    }
    signal.notify_all();
}

/// Background enrollment of content fingerprints for available Originals.
/// One bounded worker reads each Original once; scans pause it so candidate
/// hashing and enrollment never compete for storage.
fn enrollment_main(
    root: LibraryRoot,
    native_work: NativeWorkBudget,
    persistence: Persistence,
    enrollment: Arc<(Mutex<EnrollmentState>, Condvar)>,
    fingerprint_counts: Arc<Mutex<crate::persistence::FingerprintCounts>>,
) {
    let refresh_counts = |persistence: &Persistence| {
        if let Ok(counts) = persistence.fingerprint_counts_blocking() {
            *fingerprint_counts.lock().unwrap() = counts;
        }
    };
    refresh_counts(&persistence);
    let mut deferred: std::collections::HashMap<String, std::time::Instant> =
        std::collections::HashMap::new();
    loop {
        {
            let mut state = enrollment.0.lock().unwrap();
            if state.stopped {
                return;
            }
            while state.scan_running && !state.stopped {
                state = enrollment
                    .1
                    .wait_timeout(state, std::time::Duration::from_secs(5))
                    .unwrap()
                    .0;
            }
            if state.stopped {
                return;
            }
        }
        let target = match persistence.next_fingerprint_target_blocking() {
            Ok(target) => target,
            Err(_) => {
                thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
        };
        let Some(target) = target else {
            let state = enrollment.0.lock().unwrap();
            if state.stopped {
                return;
            }
            drop(state);
            let (lock, signal) = &*enrollment;
            let state = lock.lock().unwrap();
            if state.stopped {
                return;
            }
            let _unused = signal
                .wait_timeout(state, std::time::Duration::from_secs(5))
                .unwrap()
                .0;
            continue;
        };
        if let Some(deferred_at) = deferred.get(&target.original_id)
            && deferred_at.elapsed() < std::time::Duration::from_secs(60)
        {
            thread::sleep(std::time::Duration::from_millis(250));
            continue;
        }
        let relative = match crate::RelativeOriginalPath::parse(target.relative_path.clone()) {
            Ok(relative) => relative,
            Err(_) => {
                deferred.insert(target.original_id.clone(), std::time::Instant::now());
                continue;
            }
        };
        let permit = native_work.acquire();
        let digest = root
            .original(relative)
            .and_then(|capability| capability.digest_file());
        drop(permit);
        match digest {
            Ok(checked)
                if checked.facts.size == target.size
                    && checked.facts.mtime_ms == target.mtime_ms =>
            {
                let fingerprint = crate::OriginalFingerprint {
                    original_id: target.original_id.clone(),
                    digest: checked.digest,
                    size: checked.facts.size,
                    mtime_ms: checked.facts.mtime_ms,
                };
                if persistence.store_fingerprint_blocking(fingerprint).is_ok() {
                    deferred.remove(&target.original_id);
                    refresh_counts(&persistence);
                } else {
                    deferred.insert(target.original_id.clone(), std::time::Instant::now());
                }
            }
            _ => {
                deferred.insert(target.original_id.clone(), std::time::Instant::now());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OriginalKind, identity::original_id, persistence::PersistenceError};
    use rusqlite::{Connection, params};
    use std::{
        ffi::CString,
        fs,
        os::unix::{
            ffi::{OsStrExt, OsStringExt},
            fs::MetadataExt,
        },
        sync::atomic::{AtomicU64, AtomicUsize, Ordering},
    };

    static NEXT_TEMP_TREE: AtomicU64 = AtomicU64::new(0);

    struct TempTree(PathBuf);

    impl TempTree {
        fn new() -> Self {
            loop {
                let nonce = NEXT_TEMP_TREE.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir()
                    .join(format!("slipstream-library-{}-{nonce}", std::process::id()));
                match fs::create_dir(&path) {
                    Ok(()) => return Self(path),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("temporary Library fixture could not be created: {error}"),
                }
            }
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn config(base: &TempTree) -> LibraryConfig {
        LibraryConfig {
            library_root: base.0.join("originals"),
            state_directory: base.0.join("state"),
            database_basename: "library.sqlite".to_owned(),
            limits: ScanLimits::default(),
            command_capacity: NonZeroUsize::new(64).unwrap(),
        }
    }

    fn fixture() -> (TempTree, LibraryConfig) {
        let base = TempTree::new();
        fs::create_dir(base.0.join("originals")).unwrap();
        let config = config(&base);
        (base, config)
    }

    #[test]
    fn library_native_admission_shares_the_owner_budget() {
        let (_base, config) = fixture();
        let library = Library::open(config).unwrap();
        let first = library.try_admit_native_work().expect("first slot");
        let second = library.try_admit_native_work().expect("second slot");
        assert!(library.try_admit_native_work().is_none());
        assert!(library.native_work_budget().try_acquire().is_none());
        drop(first);
        let shared = library
            .native_work_budget()
            .try_acquire()
            .expect("owner slot released");
        assert!(library.try_admit_native_work().is_none());
        drop((second, shared));
        library.shutdown().unwrap();
    }

    fn raw_capture_fixture(value: &str) -> Vec<u8> {
        let value = format!("{value}\0").into_bytes();
        let value_offset = 8 + 2 + 12 + 4;
        let mut bytes = b"II*\0\x08\0\0\0".to_vec();
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&0x9003_u16.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&(value.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(value_offset as u32).to_le_bytes());
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&value);
        bytes
    }

    fn jpeg_capture_fixture(value: &str) -> Vec<u8> {
        let tiff = raw_capture_fixture(value);
        let mut payload = b"Exif\0\0".to_vec();
        payload.extend_from_slice(&tiff);
        let mut bytes = vec![0xff, 0xd8, 0xff, 0xe1];
        bytes.extend_from_slice(&u16::try_from(payload.len() + 2).unwrap().to_be_bytes());
        bytes.extend_from_slice(&payload);
        bytes.extend_from_slice(&[0xff, 0xd9]);
        bytes
    }

    fn replace_with_preserved_mtime(path: &std::path::Path, bytes: &[u8]) {
        let metadata = fs::metadata(path).unwrap();
        let replacement = path.with_extension("replacement");
        fs::write(&replacement, bytes).unwrap();
        let replacement_name = CString::new(replacement.as_os_str().as_bytes()).unwrap();
        let times = [
            libc::timespec {
                tv_sec: 0,
                tv_nsec: libc::UTIME_OMIT,
            },
            libc::timespec {
                tv_sec: metadata.mtime(),
                tv_nsec: metadata.mtime_nsec(),
            },
        ];
        assert_eq!(
            unsafe {
                libc::utimensat(libc::AT_FDCWD, replacement_name.as_ptr(), times.as_ptr(), 0)
            },
            0
        );
        fs::rename(replacement, path).unwrap();
    }

    #[test]
    fn capture_inspection_reobserves_stale_jpeg_discovery() {
        let (_base, config) = fixture();
        let path = config.library_root.join("current.JPG");
        fs::write(&path, jpeg_capture_fixture("2026:02:03 04:05:06")).unwrap();
        let root = LibraryRoot::open(&config.library_root).unwrap();
        let mut originals = root.scan(ScanLimits::default()).unwrap().originals;
        fs::write(&path, jpeg_capture_fixture("2026:02:03 05:05:06")).unwrap();
        let current_facts = root
            .original(originals[0].path.clone())
            .unwrap()
            .facts()
            .unwrap();

        inspect_capture_facts(
            &root,
            &NativeWorkBudget::new(),
            &mut originals,
            &[],
            &Mutex::new(ScanProgress::default()),
        );

        assert_eq!(originals[0].facts, current_facts);
        assert_eq!(
            originals[0].capture.order_key.as_deref(),
            Some("2026-02-03T05:05:06.000000000")
        );
        assert_eq!(
            originals[0].capture.source_revision,
            Some(capture_source_revision("current.JPG", current_facts).unwrap())
        );
    }

    #[test]
    fn capture_inspection_accepts_same_size_same_mtime_raw_replacement_as_fresh() {
        let (_base, config) = fixture();
        let path = config.library_root.join("current.ARW");
        let initial = raw_capture_fixture("2026:02:03 04:05:06");
        let replacement = raw_capture_fixture("2026:02:03 05:05:06");
        assert_eq!(initial.len(), replacement.len());
        fs::write(&path, initial).unwrap();
        let root = LibraryRoot::open(&config.library_root).unwrap();
        let mut originals = root.scan(ScanLimits::default()).unwrap().originals;
        let stale_facts = originals[0].facts;
        replace_with_preserved_mtime(&path, &replacement);
        let current_facts = root
            .original(originals[0].path.clone())
            .unwrap()
            .facts()
            .unwrap();
        assert_eq!(current_facts.size, stale_facts.size);
        assert_eq!(current_facts.mtime_ms, stale_facts.mtime_ms);
        assert_eq!(current_facts.device, stale_facts.device);
        assert_ne!(current_facts.inode, stale_facts.inode);

        inspect_capture_facts(
            &root,
            &NativeWorkBudget::new(),
            &mut originals,
            &[],
            &Mutex::new(ScanProgress::default()),
        );

        assert_eq!(originals[0].facts, current_facts);
        assert_eq!(
            originals[0].capture.order_key.as_deref(),
            Some("2026-02-03T05:05:06.000000000")
        );
        assert_eq!(
            originals[0].capture.source_revision,
            Some(capture_source_revision("current.ARW", current_facts).unwrap())
        );
    }

    #[test]
    fn fresh_capture_mid_read_change_fails_without_third_attempt_or_stale_fact_adoption() {
        let (_base, config) = fixture();
        let path = config.library_root.join("bounded-fresh.ARW");
        let initial = raw_capture_fixture("2026:02:03 04:05:06");
        fs::write(&path, initial).unwrap();
        let root = LibraryRoot::open(&config.library_root).unwrap();
        let mut initial_originals = root.scan(ScanLimits::default()).unwrap().originals;
        inspect_capture_facts(
            &root,
            &NativeWorkBudget::new(),
            &mut initial_originals,
            &[],
            &Mutex::new(ScanProgress::default()),
        );
        let remembered_key = initial_originals[0].capture.order_key.clone();
        let previous = vec![crate::OriginalRecord {
            id: "remembered-original".to_owned(),
            relative_path: initial_originals[0].path.clone(),
            kind: initial_originals[0].kind,
            facts: initial_originals[0].facts,
            available: true,
            error_category: None,
            error_message: None,
            capture: initial_originals[0].capture.clone(),
        }];

        let mut intermediate = raw_capture_fixture("2026:02:03 05:05:06");
        intermediate.push(0);
        fs::write(&path, intermediate).unwrap();
        let mut originals = root.scan(ScanLimits::default()).unwrap().originals;
        let discovery_facts = originals[0].facts;
        let mut current = raw_capture_fixture("2026:02:03 06:05:06");
        current.push(0);
        replace_with_preserved_mtime(&path, &current);

        let opens = Arc::new(AtomicUsize::new(0));
        let hook_opens = opens.clone();
        let hook_path = path.clone();
        let _hook = crate::capture::install_capture_inspection_test_hook(move |relative, point| {
            if relative.as_str() != "bounded-fresh.ARW" {
                return;
            }
            match point {
                crate::capture::CaptureInspectionTestPoint::BeforeOpen => {
                    hook_opens.fetch_add(1, Ordering::SeqCst);
                }
                crate::capture::CaptureInspectionTestPoint::BeforeVerification
                    if hook_opens.load(Ordering::SeqCst) == 2 =>
                {
                    let mut bytes = fs::read(&hook_path).unwrap();
                    bytes.push(0);
                    fs::write(&hook_path, bytes).unwrap();
                }
                crate::capture::CaptureInspectionTestPoint::BeforeVerification => {}
            }
        });

        inspect_capture_facts(
            &root,
            &NativeWorkBudget::new(),
            &mut originals,
            &previous,
            &Mutex::new(ScanProgress::default()),
        );

        assert_eq!(opens.load(Ordering::SeqCst), 2);
        assert_eq!(originals[0].facts, discovery_facts);
        assert!(remembered_key.is_some());
        assert_eq!(
            originals[0].capture.state,
            crate::CaptureMetadataState::Failed
        );
        assert_eq!(originals[0].capture.source_revision, None);
        assert_eq!(originals[0].capture.order_key, None);
    }

    #[test]
    fn stable_non_revision_capture_failure_is_not_retried() {
        let (_base, config) = fixture();
        let path = config.library_root.join("resource-limit.ARW");
        let mut excessive = b"II*\0\x08\0\0\0".to_vec();
        excessive.extend_from_slice(&1025_u16.to_le_bytes());
        fs::write(&path, excessive).unwrap();
        let root = LibraryRoot::open(&config.library_root).unwrap();
        let mut originals = root.scan(ScanLimits::default()).unwrap().originals;
        let discovery_facts = originals[0].facts;
        let expected_revision =
            capture_source_revision("resource-limit.ARW", discovery_facts).unwrap();

        let opens = Arc::new(AtomicUsize::new(0));
        let hook_opens = opens.clone();
        let _hook = crate::capture::install_capture_inspection_test_hook(move |relative, point| {
            if relative.as_str() == "resource-limit.ARW"
                && point == crate::capture::CaptureInspectionTestPoint::BeforeOpen
            {
                hook_opens.fetch_add(1, Ordering::SeqCst);
            }
        });

        inspect_capture_facts(
            &root,
            &NativeWorkBudget::new(),
            &mut originals,
            &[],
            &Mutex::new(ScanProgress::default()),
        );

        assert_eq!(opens.load(Ordering::SeqCst), 1);
        assert_eq!(originals[0].facts, discovery_facts);
        assert_eq!(
            originals[0].capture.state,
            crate::CaptureMetadataState::Failed
        );
        assert_eq!(
            originals[0].capture.source_revision.as_deref(),
            Some(expected_revision.as_str())
        );
    }

    #[tokio::test]
    async fn fresh_capture_publication_preserves_identity_decisions_album_order_and_resume() {
        let (_base, config) = fixture();
        let target_path = config.library_root.join("preserved.JPG");
        let sibling_path = config.library_root.join("sibling.ARW");
        fs::write(&target_path, jpeg_capture_fixture("2026:02:03 04:05:06")).unwrap();
        fs::write(&sibling_path, raw_capture_fixture("2026:02:03 06:05:06")).unwrap();
        let library = Library::open(config.clone()).unwrap();
        let initial = library.scan().await.unwrap();
        let target_original = initial
            .originals
            .iter()
            .find(|original| original.relative_path.as_str() == "preserved.JPG")
            .unwrap()
            .clone();
        let target_photo = initial
            .photos
            .iter()
            .find(|photo| photo.original_id == target_original.id)
            .unwrap()
            .clone();
        let sibling_photo = initial
            .photos
            .iter()
            .find(|photo| photo.original_id != target_original.id)
            .unwrap()
            .clone();
        library
            .mutate_photo_state(crate::PhotoStateMutation {
                photo_id: target_photo.id.clone(),
                field: crate::PhotoStateField::SelectionState,
                value: crate::PhotoStateValue::Selection(crate::SelectionState::Selected),
                expected_current: Some(crate::PhotoStateValue::Selection(
                    crate::SelectionState::Undecided,
                )),
                album_id: None,
            })
            .await
            .unwrap();
        library
            .mutate_photo_state(crate::PhotoStateMutation {
                photo_id: target_photo.id.clone(),
                field: crate::PhotoStateField::Rating,
                value: crate::PhotoStateValue::Rating(4),
                expected_current: Some(crate::PhotoStateValue::Rating(0)),
                album_id: None,
            })
            .await
            .unwrap();
        let album_id = library
            .mutate_album(AlbumMutation::Create {
                name: "Preserved order".to_owned(),
            })
            .await
            .unwrap()
            .album_id;
        let album_order = vec![sibling_photo.id.clone(), target_photo.id.clone()];
        library
            .mutate_album(AlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: album_order.clone(),
            })
            .await
            .unwrap();
        library
            .mutate_album(AlbumMutation::SetProgress {
                album_id: album_id.clone(),
                photo_id: target_photo.id.clone(),
            })
            .await
            .unwrap();

        // Make discovery observe an intermediate revision. The test hook then
        // completes a second replacement immediately before the first Capture
        // open, forcing the bounded fresh observation through the real scanner.
        fs::write(&target_path, jpeg_capture_fixture("2026:02:03 05:05:06")).unwrap();
        let final_bytes = jpeg_capture_fixture("2026:02:03 07:05:06");
        let replacement_path = config.library_root.join("preserved.replacement");
        fs::write(&replacement_path, final_bytes).unwrap();
        let replacements = Arc::new(AtomicUsize::new(0));
        let hook_replacements = replacements.clone();
        let hook_target = target_path.clone();
        let _hook = crate::capture::install_capture_inspection_test_hook(move |relative, point| {
            if relative.as_str() == "preserved.JPG"
                && point == crate::capture::CaptureInspectionTestPoint::BeforeOpen
                && hook_replacements.fetch_add(1, Ordering::SeqCst) == 0
            {
                fs::rename(&replacement_path, &hook_target).unwrap();
            }
        });

        let current = library.scan().await.unwrap();
        assert_eq!(replacements.load(Ordering::SeqCst), 2);
        let current_original = current
            .originals
            .iter()
            .find(|original| original.relative_path.as_str() == "preserved.JPG")
            .unwrap();
        let current_photo = current
            .photos
            .iter()
            .find(|photo| photo.original_id == current_original.id)
            .unwrap();
        assert_eq!(current_original.id, target_original.id);
        assert_eq!(
            current_original.relative_path,
            target_original.relative_path
        );
        assert_eq!(current_photo.id, target_photo.id);
        assert_eq!(
            current_photo.selection_state,
            crate::SelectionState::Selected
        );
        assert_eq!(current_photo.rating, 4);
        assert_eq!(
            current_original.capture.order_key.as_deref(),
            Some("2026-02-03T07:05:06.000000000")
        );
        let current_filesystem_facts = library
            .original(current_original.relative_path.clone())
            .unwrap()
            .facts()
            .unwrap();
        assert_eq!(current_original.facts.size, current_filesystem_facts.size);
        assert_eq!(
            current_original.facts.mtime_ms,
            current_filesystem_facts.mtime_ms
        );
        assert_eq!(
            current_original.capture.source_revision,
            Some(capture_source_revision("preserved.JPG", current_filesystem_facts).unwrap())
        );
        let album = library
            .list_albums()
            .await
            .unwrap()
            .into_iter()
            .find(|album| album.id == album_id)
            .unwrap();
        assert_eq!(
            album
                .members
                .iter()
                .map(|member| member.photo_id.clone())
                .collect::<Vec<_>>(),
            album_order
        );
        assert_eq!(
            album.last_reviewed_photo_id.as_deref(),
            Some(target_photo.id.as_str())
        );
        library.shutdown().unwrap();
    }

    #[test]
    fn rejects_non_utf8_canonical_root_before_creating_state() {
        for suffix in [0x80, 0x81] {
            let base = TempTree::new();
            let root = base.0.join(std::ffi::OsString::from_vec(vec![
                b'r', b'o', b'o', b't', suffix,
            ]));
            fs::create_dir(&root).unwrap();
            let state = base.0.join("missing/state");
            let result = Library::open(LibraryConfig {
                library_root: root,
                state_directory: state.clone(),
                ..LibraryConfig::default()
            });
            assert!(matches!(result, Err(LibraryError::UnsupportedRootEncoding)));
            assert!(!state.exists());
        }
    }

    struct ScannerHookGuard {
        hook: Arc<ScannerTestHook>,
        _lease: std::sync::MutexGuard<'static, ()>,
    }

    impl ScannerHookGuard {
        fn install(canonical_root: PathBuf) -> Self {
            let lease = SCANNER_TEST_HOOK_LEASE
                .get_or_init(|| Mutex::new(()))
                .lock()
                .unwrap();
            let hook = Arc::new(ScannerTestHook {
                canonical_root,
                entered: Mutex::new(0),
                entered_signal: Condvar::new(),
                admitted: Mutex::new(0),
                admitted_signal: Condvar::new(),
                release: Mutex::new(false),
                release_signal: Condvar::new(),
            });
            *SCANNER_TEST_HOOK
                .get_or_init(|| Mutex::new(None))
                .lock()
                .unwrap() = Some(hook.clone());
            Self {
                hook,
                _lease: lease,
            }
        }

        fn wait_for_entries(&self, expected: usize) {
            let mut entered = self.hook.entered.lock().unwrap();
            while *entered < expected {
                entered = self.hook.entered_signal.wait(entered).unwrap();
            }
        }

        fn wait_for_admissions(&self, expected: usize) {
            let mut admitted = self.hook.admitted.lock().unwrap();
            while *admitted < expected {
                admitted = self.hook.admitted_signal.wait(admitted).unwrap();
            }
        }

        fn release(&self) {
            *self.hook.release.lock().unwrap() = true;
            self.hook.release_signal.notify_all();
        }
    }

    impl Drop for ScannerHookGuard {
        fn drop(&mut self) {
            self.release();
            *SCANNER_TEST_HOOK
                .get_or_init(|| Mutex::new(None))
                .lock()
                .unwrap() = None;
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn coalesces_concurrent_scans_into_one_scanner_operation() {
        let (base, config) = fixture();
        fs::write(base.0.join("originals/one.JPG"), b"jpeg").unwrap();
        let library = Arc::new(Library::open(config).unwrap());
        let hook = ScannerHookGuard::install(library.canonical_root().to_owned());
        let first_library = library.clone();
        let first = tokio::spawn(async move { first_library.scan().await });
        hook.wait_for_entries(1);
        let second_library = library.clone();
        let second = tokio::spawn(async move { second_library.scan().await });
        hook.wait_for_admissions(2);
        assert_eq!(*hook.hook.entered.lock().unwrap(), 1);
        hook.release();
        let first = first.await.unwrap().unwrap();
        let second = second.await.unwrap().unwrap();
        assert_eq!(first, second);
        assert_eq!(first.originals.len(), 1);
        assert_eq!(first.originals[0].kind, OriginalKind::Jpeg);
        library.shutdown().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn scanner_shutdown_waits_for_an_admitted_in_flight_scan() {
        let (base, config) = fixture();
        fs::write(base.0.join("originals/one.JPG"), b"jpeg").unwrap();
        let library = Arc::new(Library::open(config).unwrap());
        let hook = ScannerHookGuard::install(library.canonical_root().to_owned());
        let scan_library = library.clone();
        let scan = tokio::spawn(async move { scan_library.scan().await });
        hook.wait_for_entries(1);
        let shutdown_library = library.clone();
        let (started_send, started_receive) = std::sync::mpsc::channel();
        let (finished_send, finished_receive) = std::sync::mpsc::channel();
        let shutdown = tokio::task::spawn_blocking(move || {
            started_send.send(()).unwrap();
            let result = shutdown_library.shutdown();
            finished_send.send(result.clone()).unwrap();
            result
        });
        started_receive.recv().unwrap();
        assert!(finished_receive.try_recv().is_err());
        hook.release();
        assert!(scan.await.unwrap().is_ok());
        assert!(shutdown.await.unwrap().is_ok());
        assert!(matches!(library.scan().await, Err(LibraryError::Closed)));
    }

    #[tokio::test]
    async fn lifecycle_rejects_operations_after_shutdown() {
        let (_base, config) = fixture();
        let library = Library::open(config).unwrap();
        library.shutdown().unwrap();
        assert!(matches!(
            library.snapshot().await,
            Err(LibraryError::Closed)
        ));
        assert!(matches!(library.scan().await, Err(LibraryError::Closed)));
        assert!(matches!(
            library.original(crate::RelativeOriginalPath::parse("missing.JPG").unwrap()),
            Err(LibraryError::Closed)
        ));
        assert!(matches!(
            library
                .seed_preview(PreviewSeed {
                    photo_id: "missing".to_owned(),
                    state: crate::PreviewState::Failed,
                    source: crate::PreviewSource::JpegOriginal,
                    expected_source_revision: "missing".to_owned(),
                    width: None,
                    height: None,
                    cache_revision: None,
                })
                .await,
            Err(LibraryError::Closed)
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropped_scan_waiters_release_capacity_before_completion() {
        let (_base, config) = fixture();
        let library = Arc::new(Library::open(config).unwrap());
        let hook = ScannerHookGuard::install(library.canonical_root().to_owned());
        let first_library = library.clone();
        let first = tokio::spawn(async move { first_library.scan().await });
        hook.wait_for_entries(1);
        let mut abandoned = Vec::new();
        for _ in 1..MAX_SCAN_WAITERS {
            let library = library.clone();
            abandoned.push(tokio::spawn(async move { library.scan().await }));
        }
        hook.wait_for_admissions(MAX_SCAN_WAITERS);
        for waiter in abandoned {
            waiter.abort();
        }
        tokio::task::yield_now().await;
        let live_library = library.clone();
        let live = tokio::spawn(async move { live_library.scan().await });
        hook.wait_for_admissions(MAX_SCAN_WAITERS + 1);
        hook.release();
        assert!(live.await.unwrap().is_ok());
        assert!(first.await.unwrap().is_ok());
        library.shutdown().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_scan_waiters_are_bounded_deterministically() {
        let (_base, config) = fixture();
        let library = Arc::new(Library::open(config).unwrap());
        let hook = ScannerHookGuard::install(library.canonical_root().to_owned());
        let first_library = library.clone();
        let first = tokio::spawn(async move { first_library.scan().await });
        hook.wait_for_entries(1);
        let barrier = Arc::new(tokio::sync::Barrier::new(MAX_SCAN_WAITERS));
        let mut tasks = Vec::new();
        for _ in 0..MAX_SCAN_WAITERS {
            let library = library.clone();
            let barrier = barrier.clone();
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                library.scan().await
            }));
        }
        hook.wait_for_admissions(MAX_SCAN_WAITERS);
        hook.release();
        let mut busy = 0;
        let mut completed = 0;
        for task in tasks {
            match task.await.unwrap() {
                Err(LibraryError::ScanBusy) => busy += 1,
                Ok(_) => completed += 1,
                Err(error) => panic!("unexpected scan result: {error}"),
            }
        }
        assert_eq!(busy, 1);
        assert_eq!(completed, MAX_SCAN_WAITERS - 1);
        assert!(first.await.unwrap().is_ok());
        library.shutdown().unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn scan_progress_reports_truthful_phases_and_counters() {
        let (base, config) = fixture();
        fs::write(base.0.join("originals/one.JPG"), b"jpeg").unwrap();
        fs::write(base.0.join("originals/two.JPG"), b"jpeg").unwrap();
        let library = Arc::new(Library::open(config).unwrap());
        assert_eq!(
            library.scan_progress(),
            ScanProgress {
                phase: ScanPhase::Idle,
                ..ScanProgress::default()
            }
        );
        let hook = ScannerHookGuard::install(library.canonical_root().to_owned());
        let scan_library = library.clone();
        let scan = tokio::spawn(async move { scan_library.scan().await });
        hook.wait_for_entries(1);
        assert_eq!(
            library.scan_progress().phase,
            ScanPhase::Discovering,
            "the admitted scan must report discovery before publication"
        );
        hook.release();
        let snapshot = scan.await.unwrap().unwrap();
        assert_eq!(snapshot.originals.len(), 2);
        let progress = library.scan_progress();
        assert_eq!(progress.phase, ScanPhase::Idle);
        assert_eq!(progress.discovered, 2);
        assert_eq!(progress.inspect_total, Some(2));
        assert_eq!(progress.inspected, 2);
        library.shutdown().unwrap();
    }

    #[tokio::test]
    async fn scan_failure_does_not_replace_the_previous_persisted_snapshot() {
        let (base, initial_config) = fixture();
        fs::write(base.0.join("originals/one.JPG"), b"jpeg").unwrap();
        let library = Library::open(initial_config).unwrap();
        let initial = library.scan().await.unwrap();
        fs::remove_file(base.0.join("originals/one.JPG")).unwrap();
        fs::write(base.0.join("originals/two.JPG"), b"jpeg").unwrap();
        fs::write(base.0.join("originals/three.JPG"), b"jpeg").unwrap();
        let mut failing = config(&base);
        failing.limits = ScanLimits::new(100, 1, 25_000).unwrap();
        library.shutdown().unwrap();
        let failed_library = Library::open(failing).unwrap();
        let result = failed_library.scan().await;
        assert!(matches!(result, Err(LibraryError::Confinement(_))));
        assert_eq!(failed_library.snapshot().await.unwrap(), initial);
        failed_library.shutdown().unwrap();
    }

    fn expansion_fixture() -> (TempTree, LibraryConfig, PathBuf) {
        let base = TempTree::new();
        let proposed = base.0.join("originals");
        let old = proposed.join("shoot");
        let state = base.0.join("state");
        fs::create_dir_all(&old).unwrap();
        fs::create_dir(&state).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(old.join("a.ARW"), b"raw-original").unwrap();
        fs::write(old.join("a.JPG"), b"jpeg-original").unwrap();
        let database = state.join("library.sqlite");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(include_str!("../../../compatibility/sqlite/schema-v8.sql"))
            .unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata VALUES('canonical_root',?)",
                [old.to_str().unwrap()],
            )
            .unwrap();
        let raw_id = original_id("a.ARW");
        let jpeg_id = original_id("a.JPG");
        let missing_id = original_id("missing.JPG");
        for (
            id,
            path,
            kind,
            size,
            available,
            capture_state,
            capture_key,
            capture_field,
            capture_revision,
        ) in [
            (
                &raw_id,
                "a.ARW",
                "raw",
                12_i64,
                1_i64,
                "known",
                Some("2026-01-01T10:00:00.000000000"),
                Some("date-time-original"),
                Some("old-capture"),
            ),
            (
                &jpeg_id,
                "a.JPG",
                "jpeg",
                13_i64,
                1_i64,
                "missing",
                None,
                None,
                Some("old-jpeg-capture"),
            ),
            (
                &missing_id,
                "missing.JPG",
                "jpeg",
                7_i64,
                0_i64,
                "failed",
                None,
                None,
                Some("retained-failure"),
            ),
        ] {
            connection.execute(
                "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,capture_metadata_state,capture_order_key,capture_time_field,capture_source_revision) VALUES(?,?,?,?,?,?,?,?,?,?)",
                params![id,path,kind,size,1.0_f64,available,capture_state,capture_key,capture_field,capture_revision],
            ).unwrap();
        }
        let legacy_photo = "legacy-raw-photo";
        let legacy_jpeg_photo = "legacy-jpeg-photo";
        let missing_photo_id = "legacy-missing-photo";
        connection.execute(
            "INSERT INTO photos(id,original_id,available,preview_state,preview_source_revision,preview_width,preview_height,cache_revision,sort_path,selection_state,rating) VALUES(?, ?,1,'ready','old-preview',800,600,'old-cache','a.ARW','selected',5)",
            params![legacy_photo, raw_id],
        ).unwrap();
        connection.execute(
            "INSERT INTO photos(id,original_id,available,preview_state,sort_path,selection_state,rating) VALUES(?, ?,1,'inspection-pending','a.JPG','undecided',0)",
            params![legacy_jpeg_photo, jpeg_id],
        ).unwrap();
        connection.execute(
            "INSERT INTO photos(id,original_id,available,preview_state,sort_path,selection_state,rating) VALUES(?, ?,0,'unavailable','missing.JPG','rejected',2)",
            params![missing_photo_id, missing_id],
        ).unwrap();
        connection
            .execute("INSERT INTO albums VALUES('set','Keep',1)", [])
            .unwrap();
        connection
            .execute(
                "INSERT INTO album_members VALUES('set',?,0)",
                [legacy_photo],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO album_members VALUES('set',?,1)",
                [missing_photo_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO album_progress VALUES('set',?)",
                [missing_photo_id],
            )
            .unwrap();
        drop(connection);
        (
            base,
            LibraryConfig {
                library_root: proposed,
                state_directory: state,
                database_basename: "library.sqlite".to_owned(),
                ..LibraryConfig::default()
            },
            database,
        )
    }

    #[tokio::test]
    async fn expansion_preserves_legacy_identity_and_user_state_then_discovers_sibling() {
        let (base, config, database) = expansion_fixture();
        let old_raw = fs::read(config.library_root.join("shoot/a.ARW")).unwrap();
        let old_jpeg = fs::read(config.library_root.join("shoot/a.JPG")).unwrap();
        fs::write(config.library_root.join("a.ARW"), b"sibling-raw").unwrap();
        let legacy_original = original_id("a.ARW");
        let legacy_photo = "legacy-raw-photo".to_owned();

        expand_library(config.clone()).unwrap();
        let connection = Connection::open(&database).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
                .unwrap(),
            9
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT value FROM library_metadata WHERE key='canonical_root'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            config.library_root.to_str().unwrap()
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT relative_path FROM original_files WHERE id=?",
                    [&legacy_original],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "shoot/a.ARW"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT capture_metadata_state FROM original_files WHERE id=?",
                    [&legacy_original],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "pending"
        );
        let photo: (String, String, i64, String, i64) = connection.query_row(
            "SELECT sort_path,preview_state,rating,selection_state,(SELECT position FROM album_members WHERE album_id='set' AND photo_id=photos.id) FROM photos WHERE id=?",
            [&legacy_photo], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?)),
        ).unwrap();
        assert_eq!(
            photo,
            (
                "shoot/a.ARW".to_owned(),
                "inspection-pending".to_owned(),
                5,
                "selected".to_owned(),
                0
            )
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT photo_id FROM album_progress WHERE album_id='set'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "legacy-missing-photo"
        );
        drop(connection);

        let library = Library::open(config.clone()).unwrap();
        let snapshot = library.scan().await.unwrap();
        assert!(
            snapshot
                .originals
                .iter()
                .any(|item| item.id == legacy_original
                    && item.relative_path.as_str() == "shoot/a.ARW")
        );
        assert!(snapshot.photos.iter().any(|item| item.id == legacy_photo));
        let sibling = snapshot
            .originals
            .iter()
            .find(|item| item.relative_path.as_str() == "a.ARW")
            .unwrap();
        assert_ne!(sibling.id, legacy_original);
        assert_eq!(sibling.id.len(), 36);
        let sibling_photo = snapshot
            .photos
            .iter()
            .find(|photo| photo.original_id == sibling.id)
            .unwrap();
        assert_ne!(sibling_photo.id, legacy_photo);
        assert_eq!(sibling_photo.id.len(), 36);
        let albums = library.list_albums().await.unwrap();
        assert_eq!(
            albums[0]
                .members
                .iter()
                .map(|member| (member.photo_id.as_str(), member.position))
                .collect::<Vec<_>>(),
            [(legacy_photo.as_str(), 0), ("legacy-missing-photo", 1)]
        );
        assert_eq!(
            albums[0].last_reviewed_photo_id.as_deref(),
            Some("legacy-missing-photo")
        );
        library.shutdown().unwrap();
        assert_eq!(
            fs::read(config.library_root.join("shoot/a.ARW")).unwrap(),
            old_raw
        );
        assert_eq!(
            fs::read(config.library_root.join("shoot/a.JPG")).unwrap(),
            old_jpeg
        );
        drop(base);
    }

    #[test]
    fn expansion_failures_leave_binding_and_locations_unchanged() {
        for case in [
            "transaction",
            "scan-limit",
            "sidecar",
            "non-ancestor",
            "running-service",
            "invalid-location",
            "schema",
        ] {
            let (base, mut config, database) = expansion_fixture();
            let old_root = config.library_root.join("shoot");
            let result = match case {
                "transaction" => expand_library_with_transaction_failure(config.clone()),
                "scan-limit" => {
                    config.limits = ScanLimits::new(1, 1, 1).unwrap();
                    expand_library(config.clone())
                }
                "sidecar" => {
                    fs::write(database.with_file_name("library.sqlite-wal"), b"recovery").unwrap();
                    expand_library(config.clone())
                }
                "non-ancestor" => {
                    let unrelated = base.0.join("unrelated");
                    fs::create_dir(&unrelated).unwrap();
                    config.library_root = unrelated;
                    expand_library(config.clone())
                }
                "running-service" => {
                    let running = Library::open(LibraryConfig {
                        library_root: old_root.clone(),
                        ..config.clone()
                    })
                    .unwrap();
                    let result = expand_library(config.clone());
                    running.shutdown().unwrap();
                    result
                }
                "invalid-location" => {
                    let connection = Connection::open(&database).unwrap();
                    connection
                        .execute(
                            "UPDATE original_files SET relative_path='unsupported.txt' WHERE id=?",
                            [original_id("a.ARW")],
                        )
                        .unwrap();
                    drop(connection);
                    expand_library(config.clone())
                }
                "schema" => {
                    let connection = Connection::open(&database).unwrap();
                    connection.pragma_update(None, "user_version", 3).unwrap();
                    drop(connection);
                    expand_library(config.clone())
                }
                _ => unreachable!(),
            };
            assert!(result.is_err(), "{case}");
            let connection = Connection::open(&database).unwrap();
            assert_eq!(
                connection
                    .query_row(
                        "SELECT value FROM library_metadata WHERE key='canonical_root'",
                        [],
                        |row| row.get::<_, String>(0)
                    )
                    .unwrap(),
                old_root.to_str().unwrap(),
                "{case}"
            );
            let expected_path = if case == "invalid-location" {
                "unsupported.txt"
            } else {
                "a.ARW"
            };
            assert_eq!(
                connection
                    .query_row(
                        "SELECT relative_path FROM original_files WHERE id=?",
                        [original_id("a.ARW")],
                        |row| row.get::<_, String>(0)
                    )
                    .unwrap(),
                expected_path,
                "{case}"
            );
        }
    }

    /// Deterministic manual-recovery fixture: one unavailable Photo with
    /// retained decisions and Album membership, its file relocated on disk,
    /// and optionally an occupying destination record.
    fn manual_recovery_fixture(
        occupant_state: Option<(&'static str, u8)>,
        fingerprint: Option<bool>,
    ) -> (TempTree, LibraryConfig) {
        manual_recovery_fixture_with_candidate(occupant_state, fingerprint, CandidateFile::Readable)
    }

    /// What the proposed destination Location holds on disk.
    #[derive(Clone, Copy)]
    enum CandidateFile {
        /// One generated readable JPEG, the ordinary recovered candidate.
        Readable,
        /// No entry at all, as for a destination that was never written.
        Absent,
        /// A generated directory carrying the candidate filename, which no
        /// Original read can use.
        NotRegular,
    }

    /// [`manual_recovery_fixture`] with explicit control over the destination
    /// entry, for candidate states the scanner would not have discovered.
    fn manual_recovery_fixture_with_candidate(
        occupant_state: Option<(&'static str, u8)>,
        fingerprint: Option<bool>,
        candidate: CandidateFile,
    ) -> (TempTree, LibraryConfig) {
        let (base, config) = fixture();
        let root = &config.library_root;
        fs::create_dir_all(root.join("moved")).unwrap();
        match candidate {
            CandidateFile::Readable => {
                fs::write(root.join("moved/a.JPG"), b"jpeg-bytes-a").unwrap();
            }
            CandidateFile::Absent => {}
            CandidateFile::NotRegular => {
                fs::create_dir(root.join("moved/a.JPG")).unwrap();
            }
        }
        fs::create_dir_all(&config.state_directory).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&config.state_directory, fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let database = config.state_directory.join("library.sqlite");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(include_str!("../../../compatibility/sqlite/schema-v8.sql"))
            .unwrap();
        connection
            .execute(
                "INSERT INTO library_metadata VALUES('canonical_root',?)",
                [root.to_str().unwrap()],
            )
            .unwrap();
        let missing_id = original_id("shoot/a.JPG");
        connection
            .execute(
                "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,capture_metadata_state,capture_source_revision) VALUES(?,'shoot/a.JPG','jpeg',11,1.0,0,'missing','remembered-revision')",
                params![missing_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO photos(id,original_id,available,preview_state,sort_path,selection_state,rating) VALUES('missing-photo',?,0,'unavailable','shoot/a.JPG','selected',3)",
                params![missing_id],
            )
            .unwrap();
        connection
            .execute("INSERT INTO albums VALUES('set','Trip',1)", [])
            .unwrap();
        connection
            .execute(
                "INSERT INTO album_members VALUES('set','missing-photo',0)",
                [],
            )
            .unwrap();
        if let Some((occupant_path, rating)) = occupant_state {
            let occupant_original = original_id(occupant_path);
            connection
                .execute(
                    "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,capture_metadata_state) VALUES(?,?,'jpeg',9,1.0,1,'pending')",
                    params![occupant_original, occupant_path],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO photos(id,original_id,available,preview_state,sort_path,selection_state,rating) VALUES('occupant-photo',?,1,'inspection-pending',?,'undecided',?)",
                    params![occupant_original, occupant_path, i64::from(rating)],
                )
                .unwrap();
        }
        if fingerprint == Some(true) {
            connection
                .execute(
                    "INSERT INTO original_fingerprints(original_id,digest,size,mtime_ms) VALUES(?,?,11,1.0)",
                    params![missing_id, crate::digest_bytes(b"jpeg-bytes-a")],
                )
                .unwrap();
        }
        drop(connection);
        (base, config)
    }

    #[tokio::test]
    async fn manual_recovery_restores_unavailable_photo_without_fingerprint() {
        let (base, config) = manual_recovery_fixture(None, Some(false));
        let library = Library::open(config.clone()).unwrap();
        let survey = library.recovery_survey().await.unwrap();
        assert_eq!(survey.unavailable.len(), 1);
        let record = &survey.unavailable[0];
        assert_eq!(record.relative_path, "shoot/a.JPG");
        assert_eq!(record.rating, 3);
        assert!(record.fingerprint.is_none());
        assert_eq!(record.album_count, 1);

        let root = LibraryRoot::open(config.library_root.clone()).unwrap();
        let budget = NativeWorkBudget::new();
        let snapshot = library.snapshot().await.unwrap();
        let proposals =
            crate::plan_manual_relocations(&root, &budget, &survey, &snapshot, "shoot", "moved")
                .unwrap();
        assert_eq!(proposals.len(), 1);
        assert!(matches!(
            proposals[0].outcome,
            crate::ManualOutcome::Matched
        ));
        assert!(!proposals[0].verified);
        assert_eq!(proposals[0].to_location, "moved/a.JPG");

        let capability = root
            .original(crate::RelativeOriginalPath::parse("moved/a.JPG").unwrap())
            .unwrap();
        let facts = capability.facts().unwrap();
        let applied = library
            .apply_relocations(vec![crate::RequestedRelocation {
                original_id: record.original_id.clone(),
                to_location: "moved/a.JPG".to_owned(),
                facts,
                retire_destination: false,
            }])
            .await
            .unwrap();
        assert_eq!(applied.relocated_photos, 1);
        assert_eq!(applied.unavailable_photos, 0);

        let snapshot = library.snapshot().await.unwrap();
        let photo = snapshot
            .photos
            .iter()
            .find(|photo| photo.id == "missing-photo")
            .unwrap();
        assert!(photo.available);
        assert_eq!(photo.rating, 3);
        assert_eq!(photo.selection_state, crate::SelectionState::Selected);
        let original = snapshot
            .originals
            .iter()
            .find(|original| original.relative_path.as_str() == "moved/a.JPG")
            .unwrap();
        assert!(original.available);
        // The remembered Location is gone.
        assert!(
            !snapshot
                .originals
                .iter()
                .any(|original| original.relative_path.as_str() == "shoot/a.JPG")
        );
        library.shutdown().unwrap();
        drop(base);
    }

    #[tokio::test]
    async fn manual_recovery_verifies_fingerprinted_originals() {
        let (base, config) = manual_recovery_fixture(None, Some(true));
        let library = Library::open(config.clone()).unwrap();
        let survey = library.recovery_survey().await.unwrap();
        assert!(survey.unavailable[0].fingerprint.is_some());
        let root = LibraryRoot::open(config.library_root.clone()).unwrap();
        let snapshot = library.snapshot().await.unwrap();
        let proposals = crate::plan_manual_relocations(
            &root,
            &NativeWorkBudget::new(),
            &survey,
            &snapshot,
            "shoot",
            "moved",
        )
        .unwrap();
        assert_eq!(proposals.len(), 1);
        assert!(matches!(
            proposals[0].outcome,
            crate::ManualOutcome::Matched
        ));
        assert!(proposals[0].verified);

        // Different content at the candidate fails verification instead of
        // silently rebinding identity.
        fs::write(config.library_root.join("moved/a.JPG"), b"other-bytes").unwrap();
        let proposals = crate::plan_manual_relocations(
            &root,
            &NativeWorkBudget::new(),
            &survey,
            &snapshot,
            "shoot",
            "moved",
        )
        .unwrap();
        assert!(matches!(
            proposals[0].outcome,
            crate::ManualOutcome::ContentMismatch
        ));
        library.shutdown().unwrap();
        drop(base);
    }

    #[tokio::test]
    async fn manual_recovery_retires_only_unreferenced_default_destination() {
        let (base, config) = manual_recovery_fixture(Some(("moved/a.JPG", 0)), Some(false));
        let library = Library::open(config.clone()).unwrap();
        let survey = library.recovery_survey().await.unwrap();
        let root = LibraryRoot::open(config.library_root.clone()).unwrap();
        let snapshot = library.snapshot().await.unwrap();
        let proposals = crate::plan_manual_relocations(
            &root,
            &NativeWorkBudget::new(),
            &survey,
            &snapshot,
            "shoot",
            "moved",
        )
        .unwrap();
        assert!(matches!(
            proposals[0].outcome,
            crate::ManualOutcome::Occupied { retire: Some(_) }
        ));
        let capability = root
            .original(crate::RelativeOriginalPath::parse("moved/a.JPG").unwrap())
            .unwrap();
        let facts = capability.facts().unwrap();
        let relocation = crate::RequestedRelocation {
            original_id: survey.unavailable[0].original_id.clone(),
            to_location: "moved/a.JPG".to_owned(),
            facts,
            retire_destination: false,
        };
        assert!(
            library
                .apply_relocations(vec![relocation.clone()])
                .await
                .is_err()
        );

        let applied = library
            .apply_relocations(vec![crate::RequestedRelocation {
                retire_destination: true,
                ..relocation
            }])
            .await
            .unwrap();
        assert_eq!(applied.relocated_photos, 1);
        let snapshot = library.snapshot().await.unwrap();
        // The occupier's Photo and Original rows are retired; the relocated
        // Photo keeps its identity and decisions.
        assert!(
            !snapshot
                .photos
                .iter()
                .any(|photo| photo.id == "occupant-photo")
        );
        let photo = snapshot
            .photos
            .iter()
            .find(|photo| photo.id == "missing-photo")
            .unwrap();
        assert!(photo.available);
        assert_eq!(photo.rating, 3);
        library.shutdown().unwrap();
        drop(base);
    }

    #[tokio::test]
    async fn manual_recovery_refuses_retire_with_user_state() {
        let (base, config) = manual_recovery_fixture(Some(("moved/a.JPG", 4)), Some(false));
        let library = Library::open(config.clone()).unwrap();
        let survey = library.recovery_survey().await.unwrap();
        let root = LibraryRoot::open(config.library_root.clone()).unwrap();
        let snapshot = library.snapshot().await.unwrap();
        let proposals = crate::plan_manual_relocations(
            &root,
            &NativeWorkBudget::new(),
            &survey,
            &snapshot,
            "shoot",
            "moved",
        )
        .unwrap();
        assert!(matches!(
            proposals[0].outcome,
            crate::ManualOutcome::Occupied { retire: None }
        ));
        let capability = root
            .original(crate::RelativeOriginalPath::parse("moved/a.JPG").unwrap())
            .unwrap();
        let facts = capability.facts().unwrap();
        assert!(
            library
                .apply_relocations(vec![crate::RequestedRelocation {
                    original_id: survey.unavailable[0].original_id.clone(),
                    to_location: "moved/a.JPG".to_owned(),
                    facts,
                    retire_destination: true,
                }])
                .await
                .is_err()
        );
        // Both records survive the refusal.
        let snapshot = library.snapshot().await.unwrap();
        assert!(
            snapshot
                .photos
                .iter()
                .any(|photo| photo.id == "occupant-photo")
        );
        assert!(
            snapshot
                .photos
                .iter()
                .any(|photo| photo.id == "missing-photo")
        );
        library.shutdown().unwrap();
        drop(base);
    }

    #[tokio::test]
    async fn manual_recovery_rejects_stale_and_colliding_batches() {
        let (base, config) = manual_recovery_fixture(None, Some(false));
        let library = Library::open(config.clone()).unwrap();
        let survey = library.recovery_survey().await.unwrap();
        let root = LibraryRoot::open(config.library_root.clone()).unwrap();
        let capability = root
            .original(crate::RelativeOriginalPath::parse("moved/a.JPG").unwrap())
            .unwrap();
        let facts = capability.facts().unwrap();
        let original_id = survey.unavailable[0].original_id.clone();
        // Colliding destinations reject the whole batch.
        assert!(
            library
                .apply_relocations(vec![
                    crate::RequestedRelocation {
                        original_id: original_id.clone(),
                        to_location: "moved/a.JPG".to_owned(),
                        facts,
                        retire_destination: false,
                    },
                    crate::RequestedRelocation {
                        original_id: "unknown-original".to_owned(),
                        to_location: "moved/a.JPG".to_owned(),
                        facts,
                        retire_destination: false,
                    },
                ])
                .await
                .is_err()
        );
        // An empty batch is not a recovery.
        assert!(library.apply_relocations(vec![]).await.is_err());
        library.shutdown().unwrap();
        drop(base);
    }

    #[tokio::test]
    async fn manual_recovery_rejects_duplicate_source_mappings() {
        let (base, config) = manual_recovery_fixture(None, Some(false));
        fs::write(base.0.join("originals/moved/b.JPG"), b"jpeg-bytes-b").unwrap();
        let library = Library::open(config.clone()).unwrap();
        let survey = library.recovery_survey().await.unwrap();
        let root = LibraryRoot::open(config.library_root.clone()).unwrap();
        let first = root
            .original(crate::RelativeOriginalPath::parse("moved/a.JPG").unwrap())
            .unwrap();
        let second = root
            .original(crate::RelativeOriginalPath::parse("moved/b.JPG").unwrap())
            .unwrap();
        let original_id = survey.unavailable[0].original_id.clone();
        // Two mappings for one Original File are a colliding batch: only one
        // Location could win, so the whole batch is refused with a reason.
        let error = match library
            .apply_relocations(vec![
                crate::RequestedRelocation {
                    original_id: original_id.clone(),
                    to_location: "moved/a.JPG".to_owned(),
                    facts: first.facts().unwrap(),
                    retire_destination: false,
                },
                crate::RequestedRelocation {
                    original_id,
                    to_location: "moved/b.JPG".to_owned(),
                    facts: second.facts().unwrap(),
                    retire_destination: false,
                },
            ])
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("duplicate source mappings must be rejected"),
        };
        assert!(matches!(
            error,
            LibraryError::Persistence(PersistenceError::InvalidRecoveryMapping {
                reason: "colliding",
                ..
            })
        ));
        // The refusal leaves the Library untouched.
        let snapshot = library.snapshot().await.unwrap();
        let photo = snapshot
            .photos
            .iter()
            .find(|photo| photo.id == "missing-photo")
            .unwrap();
        assert!(!photo.available);
        library.shutdown().unwrap();
        drop(base);
    }

    #[tokio::test]
    async fn manual_recovery_reports_verified_content_behind_an_occupied_destination() {
        let (base, config) = manual_recovery_fixture(Some(("moved/a.JPG", 4)), Some(true));
        let library = Library::open(config.clone()).unwrap();
        let survey = library.recovery_survey().await.unwrap();
        let snapshot = library.snapshot().await.unwrap();
        let proposals = crate::plan_manual_relocations(
            &LibraryRoot::open(config.library_root.clone()).unwrap(),
            &NativeWorkBudget::new(),
            &survey,
            &snapshot,
            "shoot",
            "moved",
        )
        .unwrap();
        assert!(matches!(
            proposals[0].outcome,
            crate::ManualOutcome::Occupied { retire: None }
        ));
        // The destination content matched the persisted fingerprint, so the
        // proposal reports verification even without a permitted retire.
        assert!(proposals[0].verified);
        library.shutdown().unwrap();
        drop(base);
    }

    /// A fingerprint-less proposal cannot fall back on remembered content, so
    /// it must establish that the destination really holds a readable
    /// Original. An absent unowned Location is not a match.
    #[tokio::test]
    async fn manual_recovery_reports_missing_destination_without_fingerprint() {
        let (base, config) =
            manual_recovery_fixture_with_candidate(None, Some(false), CandidateFile::Absent);
        let library = Library::open(config.clone()).unwrap();
        let survey = library.recovery_survey().await.unwrap();
        assert!(survey.unavailable[0].fingerprint.is_none());
        let root = LibraryRoot::open(config.library_root.clone()).unwrap();
        let snapshot = library.snapshot().await.unwrap();
        let proposals = crate::plan_manual_relocations(
            &root,
            &NativeWorkBudget::new(),
            &survey,
            &snapshot,
            "shoot",
            "moved",
        )
        .unwrap();
        assert_eq!(proposals.len(), 1);
        assert!(matches!(
            proposals[0].outcome,
            crate::ManualOutcome::Missing
        ));
        assert!(!proposals[0].verified);

        // The single-mapping path reads the same Location and reports the
        // same truth.
        let single = crate::plan_single_relocation(
            &root,
            &NativeWorkBudget::new(),
            &survey,
            &snapshot,
            &survey.unavailable[0].original_id,
            "moved/a.JPG",
        )
        .unwrap();
        assert!(matches!(single.outcome, crate::ManualOutcome::Missing));
        assert!(!single.verified);
        library.shutdown().unwrap();
        drop(base);
    }

    /// A remembered destination owner cannot turn an absent Location into a
    /// retireable occupant: nothing occupies a Location with no file.
    #[tokio::test]
    async fn manual_recovery_reports_missing_occupied_destination_without_retirement() {
        let (base, config) = manual_recovery_fixture_with_candidate(
            Some(("moved/a.JPG", 0)),
            Some(false),
            CandidateFile::Absent,
        );
        let library = Library::open(config.clone()).unwrap();
        let survey = library.recovery_survey().await.unwrap();
        let snapshot = library.snapshot().await.unwrap();
        let root = LibraryRoot::open(config.library_root.clone()).unwrap();
        let proposals = crate::plan_manual_relocations(
            &root,
            &NativeWorkBudget::new(),
            &survey,
            &snapshot,
            "shoot",
            "moved",
        )
        .unwrap();
        assert_eq!(proposals.len(), 1);
        // The occupant is remembered with default decisions, so a readable
        // candidate here would offer retirement. With no file at the
        // Location the proposal refuses instead.
        assert!(matches!(
            proposals[0].outcome,
            crate::ManualOutcome::Missing
        ));
        assert!(!proposals[0].verified);
        library.shutdown().unwrap();
        drop(base);
    }

    /// An inaccessible candidate cannot be judged, so it is not a match and
    /// offers no retirement even when a remembered occupant qualifies.
    #[tokio::test]
    async fn manual_recovery_reports_inaccessible_destination_without_fingerprint() {
        let (base, config) = manual_recovery_fixture_with_candidate(
            Some(("moved/a.JPG", 0)),
            Some(false),
            CandidateFile::Readable,
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                config.library_root.join("moved/a.JPG"),
                fs::Permissions::from_mode(0o000),
            )
            .unwrap();
        }
        let library = Library::open(config.clone()).unwrap();
        let survey = library.recovery_survey().await.unwrap();
        let snapshot = library.snapshot().await.unwrap();
        let proposals = crate::plan_manual_relocations(
            &LibraryRoot::open(config.library_root.clone()).unwrap(),
            &NativeWorkBudget::new(),
            &survey,
            &snapshot,
            "shoot",
            "moved",
        )
        .unwrap();
        assert!(matches!(
            proposals[0].outcome,
            crate::ManualOutcome::Unreadable
        ));
        assert!(!proposals[0].verified);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(
                config.library_root.join("moved/a.JPG"),
                fs::Permissions::from_mode(0o644),
            );
        }
        library.shutdown().unwrap();
        drop(base);
    }

    /// A non-regular entry at the destination is not a usable Original, so
    /// the proposal refuses it instead of offering a match.
    #[tokio::test]
    async fn manual_recovery_reports_non_regular_destination_without_fingerprint() {
        let (base, config) =
            manual_recovery_fixture_with_candidate(None, Some(false), CandidateFile::NotRegular);
        let library = Library::open(config.clone()).unwrap();
        let survey = library.recovery_survey().await.unwrap();
        let snapshot = library.snapshot().await.unwrap();
        let proposals = crate::plan_manual_relocations(
            &LibraryRoot::open(config.library_root.clone()).unwrap(),
            &NativeWorkBudget::new(),
            &survey,
            &snapshot,
            "shoot",
            "moved",
        )
        .unwrap();
        assert!(matches!(
            proposals[0].outcome,
            crate::ManualOutcome::Unreadable
        ));
        assert!(!proposals[0].verified);
        library.shutdown().unwrap();
        drop(base);
    }

    fn seed_fingerprint(base: &TempTree, relative_path: &str, bytes: &[u8]) {
        let connection = Connection::open(base.0.join("state").join("library.sqlite")).unwrap();
        let original_id: String = connection
            .query_row(
                "SELECT id FROM original_files WHERE relative_path=?",
                [relative_path],
                |row| row.get(0),
            )
            .unwrap();
        connection
            .execute(
                "INSERT OR REPLACE INTO original_fingerprints(original_id,digest,size,mtime_ms) \
                 VALUES(?,?,?,1.0)",
                params![
                    original_id,
                    crate::digest_bytes(bytes),
                    i64::try_from(bytes.len()).unwrap()
                ],
            )
            .unwrap();
    }

    #[tokio::test]
    async fn scan_recovery_follows_a_unique_exact_candidate() {
        let (base, initial_config) = fixture();
        let raw_bytes = b"raw-bytes-a";
        fs::write(base.0.join("originals/a.ARW"), raw_bytes).unwrap();
        let library = Library::open(initial_config).unwrap();
        let first = library.scan().await.unwrap();
        let photo_id = first.photos[0].id.clone();
        let current = library.edit_recipe(&photo_id).await.unwrap().unwrap();
        let original_source_revision = current.current_source_revision;
        let saved = library
            .save_edit_recipe(crate::SaveEditRecipe {
                photo_id: photo_id.clone(),
                request_id: "library-first-save".to_owned(),
                expected_recipe_version: None,
                expected_source_revision: original_source_revision,
                settings: crate::EditRecipeSettings {
                    exposure_ev: 0.0,
                    white_balance: crate::WhiteBalanceIntent::AsShot,
                },
            })
            .await
            .unwrap();
        let saved = match saved {
            crate::EditRecipeWriteOutcome::Saved(recipe) => recipe,
            outcome => panic!("first recipe save should succeed, got {outcome:?}"),
        };
        library.shutdown().unwrap();
        seed_fingerprint(&base, "a.ARW", raw_bytes);
        fs::create_dir(base.0.join("originals/moved")).unwrap();
        fs::rename(
            base.0.join("originals/a.ARW"),
            base.0.join("originals/moved/a.ARW"),
        )
        .unwrap();
        let library = Library::open(config(&base)).unwrap();
        let second = library.scan().await.unwrap();
        assert_eq!(second.photos.len(), 1);
        assert_eq!(second.photos[0].id, photo_id);
        assert!(second.photos[0].available);
        assert!(second.photos[0].has_saved_edits);
        assert!(
            library
                .photo(&photo_id)
                .await
                .unwrap()
                .unwrap()
                .has_saved_edits
        );
        assert!(
            second
                .originals
                .iter()
                .any(|original| original.relative_path.as_str() == "moved/a.ARW")
        );
        let recovered = library.edit_recipe(&photo_id).await.unwrap().unwrap();
        let recovered_recipe = recovered.recipe.unwrap();
        assert_eq!(recovered_recipe.revision, saved.revision);
        assert_eq!(recovered_recipe.settings, saved.settings);
        assert_ne!(
            recovered.current_source_revision,
            recovered_recipe.source_revision
        );
        assert!(matches!(
            library
                .save_edit_recipe(crate::SaveEditRecipe {
                    photo_id: photo_id.clone(),
                    request_id: "library-stale-save".to_owned(),
                    expected_recipe_version: Some(saved.revision),
                    expected_source_revision: recovered.current_source_revision,
                    settings: crate::EditRecipeSettings {
                        exposure_ev: 1.0,
                        white_balance: crate::WhiteBalanceIntent::AsShot,
                    },
                })
                .await
                .unwrap(),
            crate::EditRecipeWriteOutcome::RequiresRebind(_)
        ));
        library.shutdown().unwrap();
        drop(base);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn scan_recovery_treats_unreadable_same_kind_candidates_as_ambiguous() {
        use std::os::unix::fs::PermissionsExt;
        let (base, initial_config) = fixture();
        fs::write(base.0.join("originals/a.JPG"), b"jpeg-bytes-a").unwrap();
        let library = Library::open(initial_config).unwrap();
        let first = library.scan().await.unwrap();
        let photo_id = first.photos[0].id.clone();
        library.shutdown().unwrap();
        seed_fingerprint(&base, "a.JPG", b"jpeg-bytes-a");
        fs::create_dir_all(base.0.join("originals/moved")).unwrap();
        fs::rename(
            base.0.join("originals/a.JPG"),
            base.0.join("originals/moved/a.JPG"),
        )
        .unwrap();
        let locked = base.0.join("originals/locked/b.JPG");
        fs::create_dir_all(base.0.join("originals/locked")).unwrap();
        fs::write(&locked, b"jpeg-bytes-a").unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
        let library = Library::open(config(&base)).unwrap();
        let second = library.scan().await.unwrap();
        let photo = second
            .photos
            .iter()
            .find(|photo| photo.id == photo_id)
            .unwrap();
        // The unreadable same-kind candidate could hold the same content,
        // so the scan must not treat uniqueness as proven.
        assert!(!photo.available);
        assert!(
            second
                .originals
                .iter()
                .any(|original| original.relative_path.as_str() == "a.JPG" && !original.available)
        );
        library.shutdown().unwrap();
        let _ = fs::set_permissions(&locked, fs::Permissions::from_mode(0o644));
        drop(base);
    }
}
