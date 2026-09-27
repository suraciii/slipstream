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
mod scanner;
#[cfg(test)]
mod tests;
#[cfg(test)]
use scanner::inspect_capture_facts;
use scanner::{ScanCommand, ScanState, Scanner, ScannerShared, scanner_main};

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

struct Lifecycle {
    open: bool,
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

    /// Current facts of the retained review memberships, in the requested
    /// order. `None` means the record no longer exists.
    pub async fn recovery_records(
        &self,
        original_ids: Vec<String>,
    ) -> Result<Vec<Option<crate::recovery::RecoveryRecord>>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.recovery_records_receiver(original_ids)
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
            self.persistence
                .apply_relocations_receiver(self.root.clone(), relocations)
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

    /// Runs metadata inspection and publication without interleaved state mutations.
    pub async fn with_metadata<R: Send + 'static>(
        &self,
        photo_id: String,
        work: impl FnOnce(
            &crate::persistence::MetadataContext<'_>,
        ) -> Result<R, crate::persistence::MetadataStoreError>
        + Send
        + 'static,
    ) -> Result<R, crate::persistence::MetadataStoreError> {
        let receive = {
            let _admission = self
                .admit()
                .map_err(|_| crate::persistence::MetadataStoreError::Storage)?;
            self.persistence.with_metadata_receiver(photo_id, work)?
        };
        tokio::task::spawn_blocking(move || {
            receive
                .blocking_recv()
                .unwrap_or(Err(crate::persistence::MetadataStoreError::Storage))
        })
        .await
        .unwrap_or(Err(crate::persistence::MetadataStoreError::Storage))
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
