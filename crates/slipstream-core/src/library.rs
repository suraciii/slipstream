use crate::{
    LibraryRoot, OriginalCapability, PhotoSetMutation, PhotoSetMutationResult, PhotoSetRecord,
    PhotoStateMutation, PhotoStateMutationResult, PreviewSeed, PreviewSeedResult, ScanLimits,
    ScanResult, ScanSnapshot,
    persistence::{
        DatabaseName, MutationError, Persistence, PersistenceError, StateDirectory, StateError,
    },
};
use std::{
    fmt,
    num::NonZeroUsize,
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
};

#[cfg(test)]
use std::sync::OnceLock;

const MAX_SCAN_WAITERS: usize = 64;

const DEFAULT_SCAN_CAPACITY: usize = 1;

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
    persistence: Persistence,
    scanner: Scanner,
    lifecycle: Mutex<Lifecycle>,
    shutdown: Mutex<Option<Result<(), LibraryError>>>,
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
        let worker_root = root.clone();
        let worker_persistence = persistence.clone();
        let worker_state = state.clone();
        let join = thread::Builder::new()
            .name("slipstream-scanner".to_owned())
            .spawn(move || {
                scanner_main(
                    worker_root,
                    worker_persistence,
                    config.limits,
                    receiver,
                    worker_state,
                )
            })
            .map_err(|_| LibraryError::ScannerStopped)?;
        Ok(Self {
            root,
            persistence,
            scanner: Scanner {
                sender,
                state,
                join: Mutex::new(Some(join)),
            },
            lifecycle: Mutex::new(Lifecycle { open: true }),
            shutdown: Mutex::new(None),
        })
    }

    pub fn canonical_root(&self) -> &std::path::Path {
        self.root.canonical_path()
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
            let waiters = state.in_flight.get_or_insert_with(Vec::new);
            if waiters.len() >= MAX_SCAN_WAITERS {
                if waiters.is_empty() {
                    state.in_flight = None;
                }
                return Err(LibraryError::ScanBusy);
            }
            let first = waiters.is_empty();
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

    pub async fn list_photo_sets(&self) -> Result<Vec<PhotoSetRecord>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.list_photo_sets_receiver()
        }?;
        receive
            .await
            .unwrap_or(Err(PersistenceError::OwnerStopped))
            .map_err(Into::into)
    }

    pub async fn mutate_photo_set(
        &self,
        mutation: PhotoSetMutation,
    ) -> Result<PhotoSetMutationResult, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.mutate_photo_set_receiver(mutation)
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

fn scanner_main(
    root: LibraryRoot,
    persistence: Persistence,
    limits: ScanLimits,
    receiver: std::sync::mpsc::Receiver<ScanCommand>,
    state: Arc<(Mutex<ScanState>, Condvar)>,
) {
    while let Ok(command) = receiver.recv() {
        match command {
            ScanCommand::Stop => break,
            ScanCommand::Scan => {
                #[cfg(test)]
                scanner_test_hook(&root);
                let result = root
                    .scan(limits)
                    .map_err(LibraryError::from)
                    .and_then(|result: ScanResult| {
                        persistence
                            .apply_scan_blocking(result.originals, result.errors)
                            .map_err(LibraryError::from)
                    })
                    .map(Arc::new);
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
    use crate::{OriginalKind, persistence::PersistenceError};
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

    #[test]
    fn persistence_backpressure_remains_typed_at_library_boundary() {
        let error = LibraryError::Persistence(PersistenceError::Saturated);
        assert_eq!(error.to_string(), "SQLite persistence queue is saturated");
    }
}
