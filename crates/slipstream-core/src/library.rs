use crate::{
    AlbumBrowseTarget, AlbumMutation, AlbumMutationResult, AlbumRecord, AlbumSummary, CaptureFact,
    LibraryRoot, NativeWorkBudget, OriginalCapability, PhotoStateMutation,
    PhotoStateMutationResult, PreviewSeed, PreviewSeedResult, ScanLimits, ScanResult, ScanSnapshot,
    capture::capture_source_revision,
    persistence::{
        DatabaseName, MutationError, Persistence, PersistenceError, StateDirectory, StateError,
        expand_library_binding,
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
    progress: Arc<Mutex<ScanProgress>>,
    lifecycle: Mutex<Lifecycle>,
    shutdown: Mutex<Option<Result<(), LibraryError>>>,
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
        let worker_root = root.clone();
        let worker_native_work = native_work.clone();
        let worker_persistence = persistence.clone();
        let worker_state = state.clone();
        let worker_progress = Arc::clone(&progress);
        let join = thread::Builder::new()
            .name("slipstream-scanner".to_owned())
            .spawn(move || {
                scanner_main(
                    worker_root,
                    worker_native_work,
                    worker_persistence,
                    config.limits,
                    receiver,
                    worker_state,
                    worker_progress,
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
            progress,
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

    pub(crate) fn native_work_budget(&self) -> NativeWorkBudget {
        self.native_work.clone()
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
                    )) => CaptureFact::failed(None),
                    Err(_) => CaptureFact::failed(Some(revision)),
                }
            }
            Err(_) => CaptureFact::failed(Some(revision)),
        };
    }
}

fn scanner_main(
    root: LibraryRoot,
    native_work: NativeWorkBudget,
    persistence: Persistence,
    limits: ScanLimits,
    receiver: std::sync::mpsc::Receiver<ScanCommand>,
    state: Arc<(Mutex<ScanState>, Condvar)>,
    progress: Arc<Mutex<ScanProgress>>,
) {
    while let Ok(command) = receiver.recv() {
        match command {
            ScanCommand::Stop => break,
            ScanCommand::Scan => {
                {
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
                        progress.lock().unwrap().phase = ScanPhase::Applying;
                        persistence
                            .apply_scan_blocking(result.originals, result.errors)
                            .map_err(LibraryError::from)
                    })
                    .map(Arc::new);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        OriginalKind,
        identity::{original_id, paired_photo_id},
        persistence::PersistenceError,
    };
    use rusqlite::{Connection, params};
    use std::{
        fs,
        os::unix::ffi::OsStringExt,
        sync::atomic::{AtomicU64, Ordering},
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
                    expected_candidate: crate::PreviewCandidate::MatchingJpeg,
                    expected_source_revision: "missing".to_owned(),
                    width: None,
                    height: None,
                    cache_revision: None,
                    actual_source: None,
                    actual_source_revision: None,
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
            .execute_batch(include_str!("../../../compatibility/sqlite/schema-v5.sql"))
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
        let pair_id = paired_photo_id(&raw_id, &jpeg_id);
        let missing_photo_id = "legacy-missing-photo";
        connection.execute(
            "INSERT INTO photos(id,raw_original_id,jpeg_original_id,ambiguous,available,preview_state,preview_candidate,preview_source,preview_source_revision,preview_width,preview_height,cache_revision,sort_path,selection_state,rating) VALUES(?,?,?,0,1,'ready','matching-jpeg','matching-jpeg','old-preview',800,600,'old-cache','a.ARW','selected',5)",
            params![pair_id,raw_id,jpeg_id],
        ).unwrap();
        connection.execute(
            "INSERT INTO photos(id,jpeg_original_id,ambiguous,available,preview_state,sort_path,selection_state,rating) VALUES(?,?,0,0,'unavailable','missing.JPG','rejected',2)",
            params![missing_photo_id,missing_id],
        ).unwrap();
        connection
            .execute("INSERT INTO albums VALUES('set','Keep',1)", [])
            .unwrap();
        connection
            .execute("INSERT INTO album_members VALUES('set',?,0)", [&pair_id])
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
        let legacy_photo = paired_photo_id(&legacy_original, &original_id("a.JPG"));

        expand_library(config.clone()).unwrap();
        let connection = Connection::open(&database).unwrap();
        assert_eq!(
            connection
                .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
                .unwrap(),
            5
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
            .find(|photo| photo.raw_original_id.as_deref() == Some(sibling.id.as_str()))
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

    #[test]
    fn persistence_backpressure_remains_typed_at_library_boundary() {
        let error = LibraryError::Persistence(PersistenceError::Saturated);
        assert_eq!(error.to_string(), "SQLite persistence queue is saturated");
    }
}
