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
        unsafe { libc::utimensat(libc::AT_FDCWD, replacement_name.as_ptr(), times.as_ptr(), 0) },
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
fn resource_limited_capture_remains_retryable_for_the_same_revision() {
    let (_base, config) = fixture();
    let path = config.library_root.join("resource-limit.ARW");
    let mut excessive = b"II*\0\x08\0\0\0".to_vec();
    excessive.extend_from_slice(&1025_u16.to_le_bytes());
    fs::write(&path, excessive).unwrap();
    let root = LibraryRoot::open(&config.library_root).unwrap();
    let mut originals = root.scan(ScanLimits::default()).unwrap().originals;
    let discovery_facts = originals[0].facts;
    let expected_revision = capture_source_revision("resource-limit.ARW", discovery_facts).unwrap();

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
        crate::CaptureMetadataState::Pending
    );
    assert_eq!(
        originals[0].capture.source_revision.as_deref(),
        Some(expected_revision.as_str())
    );
}

/// An interrupted inspection is not a read verdict for the current revision:
/// the published fact stays authoritative while it binds the observed
/// revision, and a Photo without one waits for inspection instead of being
/// published as a confirmed failure.
#[test]
fn an_interrupted_inspection_keeps_the_published_fact_and_otherwise_waits() {
    let (_base, config) = fixture();
    fs::write(
        config.library_root.join("kept.ARW"),
        raw_capture_fixture("2026:02:03 04:05:06"),
    )
    .unwrap();
    fs::write(
        config.library_root.join("waiting.ARW"),
        raw_capture_fixture("2026:02:03 04:05:07"),
    )
    .unwrap();
    let root = LibraryRoot::open(&config.library_root).unwrap();
    let mut published = root.scan(ScanLimits::default()).unwrap().originals;
    inspect_capture_facts(
        &root,
        &NativeWorkBudget::new(),
        &mut published,
        &[],
        &Mutex::new(ScanProgress::default()),
    );
    let bound = published
        .iter()
        .find(|original| original.path.as_str() == "kept.ARW")
        .unwrap();
    assert_eq!(bound.capture.state, crate::CaptureMetadataState::Known);
    let previous = vec![crate::OriginalRecord {
        id: "kept-original".to_owned(),
        relative_path: bound.path.clone(),
        kind: bound.kind,
        facts: bound.facts,
        available: true,
        error_category: None,
        error_message: None,
        capture: bound.capture.clone(),
    }];
    let published_fact = bound.capture.clone();

    // Discovery sees both Originals at unchanged facts; the Library then
    // stops before either one can be read.
    let mut originals = root.scan(ScanLimits::default()).unwrap().originals;
    assert_eq!(originals.len(), 2);
    root.close();
    inspect_capture_facts(
        &root,
        &NativeWorkBudget::new(),
        &mut originals,
        &previous,
        &Mutex::new(ScanProgress::default()),
    );

    let kept = originals
        .iter()
        .find(|original| original.path.as_str() == "kept.ARW")
        .unwrap();
    assert_eq!(kept.capture, published_fact);
    assert!(kept.capture.source_revision.is_some());
    let waiting = originals
        .iter()
        .find(|original| original.path.as_str() == "waiting.ARW")
        .unwrap();
    assert_eq!(waiting.capture.state, crate::CaptureMetadataState::Pending);
    assert_eq!(waiting.capture.source_revision, None);
}

/// Native-work admission is not an inspection outcome. A saturated budget
/// defers the attempt until capacity frees, and the retry then publishes the
/// real fact instead of a failure.
#[test]
fn a_saturated_native_work_budget_defers_inspection_without_a_failure_fact() {
    let (_base, config) = fixture();
    fs::write(
        config.library_root.join("deferred.ARW"),
        raw_capture_fixture("2026:02:03 04:05:06"),
    )
    .unwrap();
    let root = LibraryRoot::open(&config.library_root).unwrap();
    let mut originals = root.scan(ScanLimits::default()).unwrap().originals;
    let discovery_facts = originals[0].facts;

    let budget = NativeWorkBudget::new();
    let mut held = Vec::new();
    while let Some(permit) = budget.try_acquire() {
        held.push(permit);
    }
    assert!(!held.is_empty());

    let progress = Mutex::new(ScanProgress::default());
    let (done, finished) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            inspect_capture_facts(&root, &budget, &mut originals, &[], &progress);
            done.send(()).unwrap();
        });
        assert!(
            matches!(
                finished.recv_timeout(std::time::Duration::from_millis(250)),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
            ),
            "a saturated budget waits for admission instead of recording an outcome"
        );
        held.clear();
        finished
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("inspection resumes once capacity frees");
    });

    assert_eq!(originals[0].facts, discovery_facts);
    assert_eq!(
        originals[0].capture.state,
        crate::CaptureMetadataState::Known
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
        .execute_batch(include_str!(
            "../../../../compatibility/sqlite/schema-v8.sql"
        ))
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
        13
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
            .any(|item| item.id == legacy_original && item.relative_path.as_str() == "shoot/a.ARW")
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
        fs::set_permissions(&config.state_directory, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let database = config.state_directory.join("library.sqlite");
    let connection = Connection::open(&database).unwrap();
    connection
        .execute_batch(include_str!(
            "../../../../compatibility/sqlite/schema-v8.sql"
        ))
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

fn requested_relocation(
    proposal: &crate::ManualProposal,
    survey: &crate::RecoverySurvey,
    facts: crate::OriginalFacts,
    retire_photo_id: Option<&str>,
) -> crate::RequestedRelocation {
    crate::RequestedRelocation {
        original_id: proposal.original_id.clone(),
        from_location: proposal.from_location.clone(),
        to_location: proposal.to_location.clone(),
        mapping_id: proposal.mapping_id.clone(),
        fingerprint: survey
            .unavailable
            .iter()
            .find(|record| record.original_id == proposal.original_id)
            .and_then(|record| record.fingerprint.clone()),
        facts,
        retire_photo_id: retire_photo_id.map(str::to_owned),
    }
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
        .apply_relocations(vec![requested_relocation(
            &proposals[0],
            &survey,
            facts,
            None,
        )])
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
    let relocation = requested_relocation(&proposals[0], &survey, facts, None);
    assert!(library.apply_relocations(vec![relocation]).await.is_err());

    let applied = library
        .apply_relocations(vec![requested_relocation(
            &proposals[0],
            &survey,
            facts,
            Some("occupant-photo"),
        )])
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
            .apply_relocations(vec![requested_relocation(
                &proposals[0],
                &survey,
                facts,
                Some("occupant-photo"),
            )])
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
    let proposal = crate::plan_single_relocation(
        &root,
        &NativeWorkBudget::new(),
        &survey,
        &library.snapshot().await.unwrap(),
        &original_id,
        "moved/a.JPG",
    )
    .unwrap();
    // Colliding destinations reject the whole batch.
    assert!(
        library
            .apply_relocations(vec![
                requested_relocation(&proposal, &survey, facts, None),
                crate::RequestedRelocation {
                    original_id: "unknown-original".to_owned(),
                    from_location: "shoot/a.JPG".to_owned(),
                    to_location: "moved/a.JPG".to_owned(),
                    mapping_id: "unknown-mapping".to_owned(),
                    fingerprint: None,
                    facts,
                    retire_photo_id: None,
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
    let snapshot = library.snapshot().await.unwrap();
    let first_proposal = crate::plan_single_relocation(
        &root,
        &NativeWorkBudget::new(),
        &survey,
        &snapshot,
        &original_id,
        "moved/a.JPG",
    )
    .unwrap();
    let second_proposal = crate::plan_single_relocation(
        &root,
        &NativeWorkBudget::new(),
        &survey,
        &snapshot,
        &original_id,
        "moved/b.JPG",
    )
    .unwrap();
    // Two mappings for one Original File are a colliding batch: only one
    // Location could win, so the whole batch is refused with a reason.
    let error = match library
        .apply_relocations(vec![
            requested_relocation(&first_proposal, &survey, first.facts().unwrap(), None),
            requested_relocation(&second_proposal, &survey, second.facts().unwrap(), None),
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
    let raw_bytes = raw_capture_fixture("2026:02:03 04:05:06");
    fs::write(base.0.join("originals/a.ARW"), &raw_bytes).unwrap();
    let library = Library::open(initial_config).unwrap();
    let first = library.scan().await.unwrap();
    let photo_id = first.photos[0].id.clone();
    let current = library.edit_recipe(&photo_id).await.unwrap().unwrap();
    let original_source_revision = current.current_source_revision.expect("published source");
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
    seed_fingerprint(&base, "a.ARW", &raw_bytes);
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
        recovered.current_source_revision.as_deref(),
        Some(recovered_recipe.source_revision.as_str())
    );
    assert!(matches!(
        library
            .save_edit_recipe(crate::SaveEditRecipe {
                photo_id: photo_id.clone(),
                request_id: "library-stale-save".to_owned(),
                expected_recipe_version: Some(saved.revision),
                expected_source_revision: recovered
                    .current_source_revision
                    .expect("recovered source"),
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
