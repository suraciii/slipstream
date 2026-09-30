use super::*;
use crate::OriginalKind;
use image::{ExtendedColorType, codecs::jpeg::JpegEncoder};
use std::{
    fs,
    sync::{
        Arc, Barrier, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

fn directories() -> (PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!(
        "slipstream-cache-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(base.join("originals")).unwrap();
    (base.join("cache"), base.join("originals"))
}

fn identity(mtime: f64) -> DerivativeIdentity {
    DerivativeIdentity {
        photo_identity: "photo-1".to_owned(),
        source: DerivativeSource::MatchingJpeg,
        source_relative_path: "春节/相机/IMG_0001.JPG".to_owned(),
        source_size: 1234567,
        source_mtime_ms: mtime,
        embedded_candidate_identity: None,
        target: DerivativeTarget::Thumbnail512,
    }
}

fn jpeg(width: u32, height: u32) -> Vec<u8> {
    let pixels = vec![127_u8; width as usize * height as usize * 3];
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, 85)
        .encode(&pixels, width, height, ExtendedColorType::Rgb8)
        .unwrap();
    bytes
}

fn scheduler(process: Option<Arc<DerivativeProcess>>) -> (DerivativeScheduler, PathBuf) {
    scheduler_with_workers(process, 1)
}

fn scheduler_with_workers(
    process: Option<Arc<DerivativeProcess>>,
    workers: usize,
) -> (DerivativeScheduler, PathBuf) {
    let (cache_path, original_path) = directories();
    let cache = CacheDirectory::open(&cache_path, &original_path).unwrap();
    let scheduler = DerivativeScheduler::with_options(cache, options(workers, process)).unwrap();
    (scheduler, cache_path)
}

fn options(workers: usize, process: Option<Arc<DerivativeProcess>>) -> DerivativeSchedulerOptions {
    DerivativeSchedulerOptions {
        workers,
        queue_capacity: 64,
        waiter_capacity: 64,
        process,
    }
}

fn wait_for_waiter_count(scheduler: &DerivativeScheduler, minimum: usize) {
    loop {
        let state = scheduler.inner.state.lock().unwrap();
        if state.waiter_count >= minimum {
            return;
        }
        drop(state);
        thread::yield_now();
    }
}

fn wait_for_queued_key(scheduler: &DerivativeScheduler, key: &str) {
    loop {
        let state = scheduler.inner.state.lock().unwrap();
        if state.queue.iter().any(|job| job.key == key) {
            return;
        }
        drop(state);
        thread::yield_now();
    }
}

#[test]
fn shared_native_budget_limits_capture_and_derivative_work_to_two() {
    let (cache_path, original_path) = directories();
    let cache = CacheDirectory::open(&cache_path, &original_path).unwrap();
    let budget = NativeWorkBudget::new();
    let holders = Arc::new((Mutex::new((0_usize, false)), Condvar::new()));
    let mut captures = Vec::new();
    for _ in 0..2 {
        let budget = budget.clone();
        let holders = Arc::clone(&holders);
        captures.push(thread::spawn(move || {
            let _capture_permit = budget.acquire();
            let (lock, signal) = &*holders;
            let mut state = lock.lock().unwrap();
            state.0 += 1;
            signal.notify_all();
            while !state.1 {
                state = signal.wait(state).unwrap();
            }
        }));
    }
    let (lock, signal) = &*holders;
    let mut state = lock.lock().unwrap();
    while state.0 != 2 {
        state = signal.wait(state).unwrap();
    }
    drop(state);

    let derivative_entered = Arc::new(AtomicU64::new(0));
    let entered = Arc::clone(&derivative_entered);
    let output = jpeg(1, 1);
    let scheduler = DerivativeScheduler::with_native_work_budget(
        cache,
        options(
            1,
            Some(Arc::new(move |_, _, _| {
                entered.fetch_add(1, Ordering::AcqRel);
                Ok(Derivative {
                    width: 1,
                    height: 1,
                    profile: DerivativeProfile::Srgb,
                    jpeg: output.clone(),
                })
            })),
        ),
        budget.clone(),
    )
    .unwrap();
    let scheduled = scheduler.clone();
    let derivative = thread::spawn(move || {
        scheduled.generate(identity(1.0), jpeg(2, 2), DerivativePriority::Current)
    });
    thread::sleep(Duration::from_millis(25));
    assert_eq!(derivative_entered.load(Ordering::Acquire), 0);
    let (lock, signal) = &*holders;
    lock.lock().unwrap().1 = true;
    signal.notify_all();
    for capture in captures {
        capture.join().unwrap();
    }
    assert!(matches!(
        derivative.join().unwrap(),
        Ok(DerivativeResult::Ready(_))
    ));
    assert!(budget.peak() <= 2);
    scheduler.shutdown().unwrap();
}

#[test]
fn cached_jpeg_validation_rejects_marker_complete_corruption_and_invalid_icc() {
    let source = jpeg(40, 20);
    let facts = jpeg_facts(&source).unwrap();
    let mut truncated = vec![
        0xff, 0xd8, 0xff, 0xc0, 0x00, 0x11, 0x08, 0x00, 0x14, 0x00, 0x28,
    ];
    truncated.extend_from_slice(&[0; 11]);
    truncated.extend_from_slice(&[0xff, 0xda, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00]);
    truncated.extend_from_slice(&[0xff, 0xd9]);
    assert!(validate_cached_jpeg(&truncated, facts, DerivativeProfileWire::Srgb,).is_none());

    let payload = b"ICC_PROFILE\0\x01\x01not-an-icc-profile";
    let length = u16::try_from(payload.len() + 2).unwrap();
    let mut invalid_icc = source[..2].to_vec();
    invalid_icc.extend_from_slice(&[0xff, 0xe2]);
    invalid_icc.extend_from_slice(&length.to_be_bytes());
    invalid_icc.extend_from_slice(payload);
    invalid_icc.extend_from_slice(&source[2..]);
    let invalid_facts = jpeg_facts(&invalid_icc).unwrap();
    assert!(
        validate_cached_jpeg(
            &invalid_icc,
            invalid_facts,
            DerivativeProfileWire::PreservedIcc,
        )
        .is_none()
    );
}

#[test]
fn cache_key_matches_updated_unicode_vector_shape() {
    let value = DerivativeIdentity {
        photo_identity: "22aabb24fc11ac401ef1989dd4f0579579b4928169040cde3546d89c4cea7255"
            .to_owned(),
        source: DerivativeSource::MatchingJpeg,
        source_relative_path: "春节/相机/IMG_0001.JPG".to_owned(),
        source_size: 1234567,
        source_mtime_ms: 1700000000123.5,
        embedded_candidate_identity: None,
        target: DerivativeTarget::Thumbnail512,
    };
    assert_eq!(
        derivative_cache_key(&value).unwrap(),
        "8b7b37540802cebab1480f9efccd688e5fcd8d1afd55b6922b9d158b59f4f45c"
    );
    assert_eq!(
        manifest_identity(&value).unwrap(),
        "8e6dd1c415faefdb79e00059d55f66ebdcc1206acaed27ed8b28effc15a75ea4"
    );
}

#[test]
fn cache_rejects_original_subtree_and_non_utf8_roots() {
    let (cache_path, original_path) = directories();
    let nested_missing = original_path.join("new").join("cache");
    assert_eq!(
        CacheDirectory::open(&nested_missing, &original_path),
        Err(CacheError::InvalidCacheDirectory)
    );
    assert!(!nested_missing.exists());
    assert_eq!(
        CacheDirectory::open(original_path.join("cache"), &original_path),
        Err(CacheError::InvalidCacheDirectory)
    );
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let bad = std::env::temp_dir().join(std::ffi::OsString::from_vec(vec![b'c', 0xff]));
        assert_eq!(
            CacheDirectory::open(bad, &original_path),
            Err(CacheError::UnsupportedEncoding)
        );
    }
    let _ = fs::remove_dir_all(cache_path.parent().unwrap());
}

#[test]
fn generates_atomic_derivative_and_manifest_with_truthful_facts() {
    let (scheduler, cache_path) = scheduler(None);
    let source = jpeg(40, 20);
    let result = scheduler
        .generate(identity(1.0), source, DerivativePriority::Current)
        .unwrap();
    let DerivativeResult::Ready(result) = result else {
        panic!("expected ready")
    };
    assert!(result.generated);
    assert!(!result.stale);
    assert_eq!((result.width, result.height), (40, 20));
    assert!(result.cache_path.is_file());
    assert!(
        cache_path
            .join(CACHE_NAMESPACE)
            .join("metadata/manifests")
            .read_dir()
            .unwrap()
            .next()
            .is_some()
    );
    assert!(
        fs::read_dir(cache_path.join(CACHE_NAMESPACE))
            .unwrap()
            .all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            })
    );
    scheduler.shutdown().unwrap();
    let _ = fs::remove_dir_all(cache_path.parent().unwrap());
}

#[test]
fn current_manifest_accepts_the_persisted_raw_fallback_source() {
    let (scheduler, cache_path) = scheduler(None);
    let raw_revision = 1.0;
    let raw_identity = DerivativeIdentity {
        photo_identity: "photo-1".to_owned(),
        source: DerivativeSource::EmbeddedRawJpeg,
        source_relative_path: "one.ARW".to_owned(),
        source_size: 12,
        source_mtime_ms: raw_revision,
        embedded_candidate_identity: Some("0".to_owned()),
        target: DerivativeTarget::Thumbnail512,
    };
    let result = scheduler
        .generate(
            raw_identity.clone(),
            jpeg(2, 2),
            DerivativePriority::Current,
        )
        .unwrap();
    let DerivativeResult::Ready(ready) = result else {
        panic!("expected a ready derivative")
    };
    let original = |id: &str, path: &str, kind: OriginalKind, size: u64| OriginalRecord {
        id: id.to_owned(),
        relative_path: crate::RelativeOriginalPath::parse(path).unwrap(),
        kind,
        facts: crate::OriginalFacts {
            size,
            mtime_ms: raw_revision,
            device: 0,
            inode: 0,
        },
        available: true,
        error_category: None,
        error_message: None,
        capture: crate::CaptureFact::pending(),
    };
    let originals = vec![
        original("raw-id", "one.ARW", OriginalKind::Raw, 12),
        original("jpeg-id", "one.JPG", OriginalKind::Jpeg, 9),
    ];
    let cache_key = ready.cache_key.clone();
    let photo = PhotoRecord {
        id: "photo-1".to_owned(),
        original_id: "raw-id".to_owned(),
        available: true,
        preview_state: crate::PreviewState::Ready,
        preview_source_revision: Some(source_revision("one.ARW", 12, raw_revision).unwrap()),
        preview_width: Some(2),
        preview_height: Some(2),
        cache_revision: Some(cache_key.clone()),
        sort_path: "one.ARW".to_owned(),
        selection_state: crate::SelectionState::Undecided,
        rating: 0,
        has_saved_edits: false,
        removed: false,
    };

    let manifest_path = scheduler.cache().manifest_path(&raw_identity).unwrap();
    let mut manifest = read_manifest(&manifest_path).unwrap();
    manifest.algorithm_version = "rust-vips-v1".to_owned();
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert_eq!(
        scheduler
            .cache()
            .lookup_current_key(&photo, &originals, DerivativeTarget::Thumbnail512,),
        Ok(None),
        "an old orientation algorithm must not hydrate a current URL"
    );
    assert_eq!(
        scheduler
            .cache()
            .lookup_current(&photo, &originals, DerivativeTarget::Thumbnail512,),
        Ok(None),
        "old dimensions and bytes must not hydrate as current"
    );

    manifest.algorithm_version = DERIVATIVE_ALGORITHM_VERSION.to_owned();
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert_eq!(
        scheduler
            .cache()
            .lookup_current_key(&photo, &originals, DerivativeTarget::Thumbnail512,),
        Ok(Some(cache_key))
    );
    scheduler.shutdown().unwrap();
    let _ = fs::remove_dir_all(cache_path.parent().unwrap());
}

#[test]
fn duplicate_requests_coalesce_before_newer_identity_supersedes_old_work() {
    let started = Arc::new((Mutex::new(Vec::new()), Condvar::new()));
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let started_for_job = Arc::clone(&started);
    let release_for_job = Arc::clone(&release);
    let process: Arc<DerivativeProcess> = Arc::new(move |bytes, _, _| {
        started_for_job.0.lock().unwrap().push(bytes[0]);
        started_for_job.1.notify_all();
        if bytes[0] == 1 {
            let mut open = release_for_job.0.lock().unwrap();
            while !*open {
                open = release_for_job.1.wait(open).unwrap();
            }
        }
        Ok(Derivative {
            width: 8,
            height: 8,
            profile: DerivativeProfile::Srgb,
            jpeg: jpeg(8, 8),
        })
    });
    let (scheduler, cache_path) = scheduler(Some(process));
    let active = {
        let scheduler = scheduler.clone();
        std::thread::spawn(move || {
            scheduler.generate(identity(1.0), vec![1], DerivativePriority::Background)
        })
    };
    let mut guard = started.0.lock().unwrap();
    while guard.is_empty() {
        guard = started.1.wait(guard).unwrap();
    }
    drop(guard);
    let duplicate = {
        let scheduler = scheduler.clone();
        std::thread::spawn(move || {
            scheduler.generate(identity(1.0), vec![1], DerivativePriority::Current)
        })
    };
    wait_for_waiter_count(&scheduler, 2);
    let adjacent_identity = identity(2.0);
    let adjacent_key = derivative_cache_key(&adjacent_identity).unwrap();
    let adjacent = {
        let scheduler = scheduler.clone();
        std::thread::spawn(move || {
            scheduler.generate(adjacent_identity, vec![2], DerivativePriority::Adjacent)
        })
    };
    wait_for_queued_key(&scheduler, &adjacent_key);
    let background = {
        let scheduler = scheduler.clone();
        std::thread::spawn(move || {
            scheduler.generate(identity(3.0), vec![3], DerivativePriority::Background)
        })
    };
    assert_eq!(adjacent.join().unwrap(), Err(CacheError::Invalidated));
    assert_eq!(
        *started.0.lock().unwrap(),
        vec![1],
        "queued superseded work must not be processed"
    );
    *release.0.lock().unwrap() = true;
    release.1.notify_all();
    assert_eq!(active.join().unwrap(), Err(CacheError::Invalidated));
    assert_eq!(duplicate.join().unwrap(), Err(CacheError::Invalidated));
    let background_result = background.join().unwrap().unwrap();
    let DerivativeResult::Ready(ready) = background_result else {
        panic!("newer identity should publish the current derivative")
    };
    assert!(ready.generated && !ready.stale);
    assert_eq!(*started.0.lock().unwrap(), vec![1, 3]);
    scheduler.shutdown().unwrap();
    let _ = fs::remove_dir_all(cache_path.parent().unwrap());
}

#[test]
fn newer_manifest_identity_supersedes_in_flight_generation() {
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let processed = Arc::new(Mutex::new(Vec::new()));
    let entered_for_process = Arc::clone(&entered);
    let release_for_process = Arc::clone(&release);
    let processed_for_process = Arc::clone(&processed);
    let process: Arc<DerivativeProcess> = Arc::new(move |bytes, _, _| {
        processed_for_process
            .lock()
            .unwrap()
            .push(bytes.first().copied().unwrap_or_default());
        if bytes.first() == Some(&1) {
            entered_for_process.wait();
            release_for_process.wait();
            // The superseded process fails, and the failure must be
            // discarded: supersession wins over reporting the error.
            return Err(DerivativeError::Malformed);
        }
        let (width, height) = if bytes.first() == Some(&2) {
            (16, 8)
        } else {
            (8, 8)
        };
        Ok(Derivative {
            width,
            height,
            profile: DerivativeProfile::Srgb,
            jpeg: jpeg(width, height),
        })
    });
    let (scheduler, cache_path) = scheduler_with_workers(Some(process), 2);
    let old_identity = identity(11.0);
    let new_identity = identity(12.0);
    let old_key = derivative_cache_key(&old_identity).unwrap();
    let new_key = derivative_cache_key(&new_identity).unwrap();
    let old_scheduler = scheduler.clone();
    let old_value = old_identity.clone();
    let old = thread::spawn(move || {
        old_scheduler.generate(old_value, vec![1], DerivativePriority::Current)
    });
    entered.wait();

    let new_scheduler = scheduler.clone();
    let new_value = new_identity.clone();
    let new = thread::spawn(move || {
        new_scheduler.generate(new_value, vec![2], DerivativePriority::Current)
    });
    let new_result = new.join().unwrap().unwrap();
    let DerivativeResult::Ready(new_ready) = new_result else {
        panic!("new identity should publish a current derivative")
    };
    assert!(new_ready.generated && !new_ready.stale);
    assert_eq!(new_ready.cache_key, new_key);
    assert!(scheduler.cache().derivative_path(&new_key).exists());
    assert!(!scheduler.cache().derivative_path(&old_key).exists());
    let manifest_path = scheduler.cache().manifest_path(&new_identity).unwrap();
    assert_eq!(read_manifest(&manifest_path).unwrap().key, new_key);

    let duplicate_scheduler = scheduler.clone();
    let duplicate_value = old_identity.clone();
    let duplicate = thread::spawn(move || {
        duplicate_scheduler.generate(duplicate_value, vec![1], DerivativePriority::Current)
    });
    wait_for_waiter_count(&scheduler, 2);

    release.wait();
    // The old process failed with Malformed, so this also proves a
    // superseded in-flight failure is discarded rather than reported.
    assert_eq!(old.join().unwrap(), Err(CacheError::Invalidated));
    assert_eq!(duplicate.join().unwrap(), Err(CacheError::Invalidated));
    assert!(!scheduler.cache().derivative_path(&old_key).exists());
    assert_eq!(read_manifest(&manifest_path).unwrap().key, new_key);
    assert_eq!(*processed.lock().unwrap(), vec![1, 2]);
    scheduler.shutdown().unwrap();
    let _ = fs::remove_dir_all(cache_path.parent().unwrap());
}

#[test]
fn newer_manifest_identity_detaches_queued_older_generation() {
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let processed = Arc::new(Mutex::new(Vec::new()));
    let entered_for_process = Arc::clone(&entered);
    let release_for_process = Arc::clone(&release);
    let processed_for_process = Arc::clone(&processed);
    let process: Arc<DerivativeProcess> = Arc::new(move |bytes, _, _| {
        processed_for_process
            .lock()
            .unwrap()
            .push(bytes.first().copied().unwrap_or_default());
        if bytes.first() == Some(&1) {
            entered_for_process.wait();
            release_for_process.wait();
        }
        Ok(Derivative {
            width: 8,
            height: 8,
            profile: DerivativeProfile::Srgb,
            jpeg: jpeg(8, 8),
        })
    });
    let (scheduler, cache_path) = scheduler_with_workers(Some(process), 1);
    let active_identity = identity(21.0);
    let queued_identity = identity(22.0);
    let new_identity = identity(23.0);
    let queued_key = derivative_cache_key(&queued_identity).unwrap();
    let new_key = derivative_cache_key(&new_identity).unwrap();
    let active_scheduler = scheduler.clone();
    let active = thread::spawn(move || {
        active_scheduler.generate(active_identity, vec![1], DerivativePriority::Current)
    });
    entered.wait();

    let queued_scheduler = scheduler.clone();
    let queued = thread::spawn(move || {
        queued_scheduler.generate(queued_identity, vec![2], DerivativePriority::Current)
    });
    wait_for_queued_key(&scheduler, &queued_key);

    let new_scheduler = scheduler.clone();
    let new_value = new_identity.clone();
    let new = thread::spawn(move || {
        new_scheduler.generate(new_value, vec![3], DerivativePriority::Current)
    });
    assert_eq!(queued.join().unwrap(), Err(CacheError::Invalidated));
    assert_eq!(*processed.lock().unwrap(), vec![1]);

    release.wait();
    assert_eq!(active.join().unwrap(), Err(CacheError::Invalidated));
    let new_result = new.join().unwrap().unwrap();
    let DerivativeResult::Ready(new_ready) = new_result else {
        panic!("new identity should publish a current derivative")
    };
    assert!(new_ready.generated && !new_ready.stale);
    assert_eq!(new_ready.cache_key, new_key);
    assert_eq!(*processed.lock().unwrap(), vec![1, 3]);
    assert!(!scheduler.cache().derivative_path(&queued_key).exists());
    assert!(scheduler.cache().derivative_path(&new_key).exists());
    assert_eq!(
        read_manifest(&scheduler.cache().manifest_path(&new_identity).unwrap())
            .unwrap()
            .key,
        new_key
    );
    scheduler.shutdown().unwrap();
    let _ = fs::remove_dir_all(cache_path.parent().unwrap());
}

#[test]
fn same_key_concurrent_requests_coalesce_without_duplicate_processing() {
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let runs = Arc::new(AtomicU64::new(0));
    let entered_for_process = Arc::clone(&entered);
    let release_for_process = Arc::clone(&release);
    let runs_for_process = Arc::clone(&runs);
    let process: Arc<DerivativeProcess> = Arc::new(move |_, _, _| {
        let run = runs_for_process.fetch_add(1, Ordering::AcqRel);
        if run == 0 {
            entered_for_process.wait();
            release_for_process.wait();
        }
        Ok(Derivative {
            width: 8,
            height: 8,
            profile: DerivativeProfile::Srgb,
            jpeg: jpeg(8, 8),
        })
    });
    let (scheduler, cache_path) = scheduler_with_workers(Some(process), 2);
    let value = identity(31.0);
    let first_scheduler = scheduler.clone();
    let first_value = value.clone();
    let first = thread::spawn(move || {
        first_scheduler.generate(first_value, vec![1], DerivativePriority::Background)
    });
    entered.wait();

    let duplicate_scheduler = scheduler.clone();
    let duplicate_value = value.clone();
    let duplicate = thread::spawn(move || {
        duplicate_scheduler.generate(duplicate_value, vec![1], DerivativePriority::Current)
    });
    wait_for_waiter_count(&scheduler, 2);
    release.wait();

    let first_result = first.join().unwrap().unwrap();
    let duplicate_result = duplicate.join().unwrap().unwrap();
    assert!(matches!(first_result, DerivativeResult::Ready(_)));
    assert!(matches!(duplicate_result, DerivativeResult::Ready(_)));
    assert_eq!(runs.load(Ordering::Acquire), 1);
    assert_eq!(
        first_result, duplicate_result,
        "coalesced waiters should observe the same derivative result"
    );
    assert!(
        scheduler
            .cache()
            .derivative_path(&derivative_cache_key(&value).unwrap())
            .exists()
    );
    scheduler.shutdown().unwrap();
    let _ = fs::remove_dir_all(cache_path.parent().unwrap());
}

#[test]
fn invalidation_detaches_old_in_flight_work_before_same_key_retry() {
    let (scheduler, cache_path) = scheduler(Some(Arc::new(|bytes, _, _| {
        if bytes.first() == Some(&1) {
            std::thread::sleep(Duration::from_millis(80));
        }
        Ok(Derivative {
            width: 8,
            height: 8,
            profile: DerivativeProfile::Srgb,
            jpeg: jpeg(8, 8),
        })
    })));
    let first_scheduler = scheduler.clone();
    let first = std::thread::spawn(move || {
        first_scheduler.generate(identity(11.0), vec![1], DerivativePriority::Current)
    });
    std::thread::sleep(Duration::from_millis(10));
    scheduler.invalidate(&identity(11.0)).unwrap();
    let second = scheduler
        .generate(identity(11.0), vec![2], DerivativePriority::Current)
        .unwrap();
    assert!(matches!(
        second,
        DerivativeResult::Ready(CachedDerivative {
            generated: true,
            ..
        })
    ));
    assert_eq!(first.join().unwrap(), Err(CacheError::Invalidated));
    let key = derivative_cache_key(&identity(11.0)).unwrap();
    let manifest_path = cache_path
        .join(CACHE_NAMESPACE)
        .join("metadata/manifests")
        .join(format!(
            "{}.json",
            manifest_identity(&identity(11.0)).unwrap()
        ));
    let manifest = read_manifest(&manifest_path).unwrap();
    assert_eq!(manifest.key, key);
    scheduler.shutdown().unwrap();
    let _ = fs::remove_dir_all(cache_path.parent().unwrap());
}

#[test]
fn persistent_content_failures_survive_reconstruction_and_retry() {
    let (cache_path, original_path) = directories();
    let cache = CacheDirectory::open(&cache_path, &original_path).unwrap();
    let runs = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let runs_for_job = Arc::clone(&runs);
    let process: Arc<DerivativeProcess> = Arc::new(move |_, _, _| {
        runs_for_job.fetch_add(1, Ordering::Relaxed);
        Err(DerivativeError::Malformed)
    });
    let first = DerivativeScheduler::with_options(cache.clone(), options(1, Some(process.clone())))
        .unwrap();
    assert_eq!(
        first
            .generate(identity(4.0), vec![1], DerivativePriority::Current)
            .unwrap(),
        DerivativeResult::Failed(DerivativeFailure {
            kind: DerivativeFailureKind::Malformed
        })
    );
    first.shutdown().unwrap();
    let second = DerivativeScheduler::with_options(cache, options(1, Some(process))).unwrap();
    assert_eq!(
        second
            .generate(identity(4.0), vec![1], DerivativePriority::Current)
            .unwrap(),
        DerivativeResult::Failed(DerivativeFailure {
            kind: DerivativeFailureKind::Malformed
        })
    );
    assert_eq!(runs.load(Ordering::Relaxed), 1);
    assert_eq!(
        second
            .retry(identity(4.0), vec![1], DerivativePriority::Current)
            .unwrap(),
        DerivativeResult::Failed(DerivativeFailure {
            kind: DerivativeFailureKind::Malformed
        })
    );
    assert_eq!(runs.load(Ordering::Relaxed), 2);
    second.shutdown().unwrap();
    let _ = fs::remove_dir_all(cache_path.parent().unwrap());
}

#[test]
fn stale_fallback_reports_stored_source_facts_and_key() {
    let (scheduler, cache_path) = scheduler(None);
    let source = jpeg(80, 40);
    let initial_mtime = 1_787_845_263_523.322_5;
    let failing_mtime = 1_787_845_263_571.322_3;
    let initial = scheduler
        .generate(
            identity(initial_mtime),
            source.clone(),
            DerivativePriority::Current,
        )
        .unwrap();
    let DerivativeResult::Ready(initial) = initial else {
        panic!()
    };
    let failing: Arc<DerivativeProcess> = Arc::new(|_, _, _| Err(DerivativeError::Malformed));
    scheduler.shutdown().unwrap();
    let cache =
        CacheDirectory::open(&cache_path, cache_path.parent().unwrap().join("originals")).unwrap();
    let replacement = DerivativeScheduler::with_options(cache, options(1, Some(failing))).unwrap();
    for _ in 0..2 {
        let result = replacement
            .generate(
                identity(failing_mtime),
                source.clone(),
                DerivativePriority::Current,
            )
            .unwrap();
        let DerivativeResult::Ready(stale) = result else {
            panic!()
        };
        assert!(stale.stale);
        assert_eq!(stale.cache_key, initial.cache_key);
        assert_eq!(stale.source, DerivativeSource::MatchingJpeg);
        assert_eq!(stale.source_mtime_ms.to_bits(), initial_mtime.to_bits());
        assert_eq!((stale.width, stale.height), (80, 40));
    }
    replacement.shutdown().unwrap();
    let _ = fs::remove_dir_all(cache_path.parent().unwrap());
}

#[test]
fn malformed_oversized_and_wrong_key_metadata_is_ignored() {
    let (scheduler, cache_path) = scheduler(None);
    let value = identity(9.0);
    let manifest_path = cache_path
        .join(CACHE_NAMESPACE)
        .join("metadata/manifests")
        .join(format!("{}.json", manifest_identity(&value).unwrap()));
    let failure_path = cache_path
        .join(CACHE_NAMESPACE)
        .join("metadata/failures")
        .join(format!("{}.json", derivative_cache_key(&value).unwrap()));
    fs::write(&manifest_path, b"{}").unwrap();
    fs::write(&failure_path, b"not-json").unwrap();
    assert!(read_manifest(&manifest_path).is_err());
    assert!(
        read_failure(
            &failure_path,
            &value,
            &derivative_cache_key(&value).unwrap()
        )
        .is_none()
    );
    let valid = Manifest {
        schema_version: CACHE_RECORD_SCHEMA_VERSION,
        algorithm_version: DERIVATIVE_ALGORITHM_VERSION.to_owned(),
        photo_identity: value.photo_identity.clone(),
        target_long_edge: value.target.long_edge(),
        key: "0".repeat(64),
        source: value.source,
        source_relative_path: value.source_relative_path.clone(),
        source_size: value.source_size,
        source_mtime_ms: value.source_mtime_ms,
        source_mtime_bits: value.source_mtime_ms.to_bits(),
        embedded_candidate_identity: value.embedded_candidate_identity.clone(),
        width: 8,
        height: 8,
        color_profile: DerivativeProfileWire::Srgb,
    };
    fs::write(&manifest_path, serde_json::to_vec(&valid).unwrap()).unwrap();
    let decoded = read_manifest(&manifest_path).unwrap();
    assert!(!manifest_matches_identity(&decoded, &value));
    fs::write(
        &manifest_path,
        vec![b'x'; MAXIMUM_METADATA_BYTES as usize + 1],
    )
    .unwrap();
    assert!(matches!(
        read_manifest(&manifest_path),
        Err(CacheError::InvalidCachedDerivative)
    ));
    scheduler.shutdown().unwrap();
    let _ = fs::remove_dir_all(cache_path.parent().unwrap());
}

#[test]
fn shutdown_rejects_admission_and_does_not_publish_queued_work() {
    let (scheduler, cache_path) = scheduler(Some(Arc::new(|_, _, _| {
        std::thread::sleep(Duration::from_millis(30));
        Ok(Derivative {
            width: 8,
            height: 8,
            profile: DerivativeProfile::Srgb,
            jpeg: jpeg(8, 8),
        })
    })));
    let running = {
        let scheduler = scheduler.clone();
        std::thread::spawn(move || {
            scheduler.generate(identity(7.0), vec![1], DerivativePriority::Current)
        })
    };
    std::thread::sleep(Duration::from_millis(5));
    scheduler.shutdown().unwrap();
    assert_eq!(
        scheduler.generate(identity(8.0), vec![1], DerivativePriority::Current),
        Err(CacheError::Closed)
    );
    assert!(matches!(running.join().unwrap(), Err(CacheError::Closed)));
    assert!(
        fs::read_dir(cache_path.join(CACHE_NAMESPACE))
            .unwrap()
            .all(|entry| {
                !entry
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".jpg")
            })
    );
    let _ = fs::remove_dir_all(cache_path.parent().unwrap());
}