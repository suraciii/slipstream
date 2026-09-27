use super::*;

#[tokio::test]
async fn metadata_saturation_uses_shared_admission_and_safe_capture_fallback() {
    let (base, config) = prepare_fixture();
    generated_non_tiff_raw_fixture(&config.library_root.join("native.ARW"));
    capture_metadata_fixture(
        &config.library_root.join("known.jpg"),
        "2026:01:01 10:00:00",
    );
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let by_location = photo_ids_by_location(&application, &ids).await;
    let raw_id = by_location["native.ARW"].clone();
    let known_id = by_location["known.jpg"].clone();
    let raw = application
        .library
        .snapshot()
        .await
        .unwrap()
        .originals
        .into_iter()
        .find(|original| original.relative_path.as_str() == "native.ARW")
        .unwrap();
    assert_eq!(raw.kind, slipstream_core::OriginalKind::Raw);
    assert!(raw.capture.source_revision.is_some());

    let scheduled = Arc::new(AtomicUsize::new(0));
    let scheduled_hook = Arc::clone(&scheduled);
    let _hook = crate::app::install_metadata_inspection_test_hook(move |_| {
        scheduled_hook.fetch_add(1, Ordering::AcqRel);
    });
    let first = application
        .library
        .try_admit_native_work()
        .expect("first shared native slot");
    let second = application
        .library
        .try_admit_native_work()
        .expect("second shared native slot");

    let saturated =
        tokio::time::timeout(Duration::from_secs(1), application.photo_metadata(&raw_id))
            .await
            .expect("saturation fallback must not wait")
            .unwrap();
    assert_eq!(saturated, slipstream_core::CaptureReviewMetadata::default());
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let direct = tokio::time::timeout(
        Duration::from_secs(1),
        send(
            &router,
            authenticated_request()
                .uri(format!("https://camera.local/api/photos/{known_id}"))
                .header("Slipstream-CLI-Contract", "1")
                .body(Body::empty())
                .unwrap(),
        ),
    )
    .await
    .expect("CLI fallback must not wait");
    let direct = response_json(direct).await;
    assert_eq!(direct["metadata"]["state"], "known");
    assert_eq!(
        direct["metadata"]["captureTime"],
        "2026-01-01T10:00:00.000000000"
    );
    assert_eq!(scheduled.load(Ordering::Acquire), 0);

    drop((first, second));
    assert_eq!(
        application.photo_metadata(&raw_id).await.unwrap(),
        slipstream_core::CaptureReviewMetadata::default()
    );
    assert_eq!(scheduled.load(Ordering::Acquire), 1);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn cancelled_raw_metadata_requests_retain_admission_until_native_work_finishes() {
    let (base, config) = prepare_fixture();
    for name in ["cancel-a.ARW", "cancel-b.ARW", "cancel-c.ARW"] {
        generated_non_tiff_raw_fixture(&config.library_root.join(name));
    }
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    let by_location = photo_ids_by_location(&application, &ids).await;

    // Enrollment shares native capacity. Wait for its actual completion before
    // asserting that both slots belong to these controlled metadata workers.
    let deadline = Instant::now() + Duration::from_secs(5);
    while application.library.fingerprint_counts().enrolled != 3 {
        assert!(
            Instant::now() < deadline,
            "fingerprint enrollment did not settle"
        );
        tokio::task::yield_now().await;
    }
    let gate = Arc::new((Mutex::new((0_usize, false)), Condvar::new()));
    struct ReleaseGate(Arc<(Mutex<(usize, bool)>, Condvar)>);
    impl Drop for ReleaseGate {
        fn drop(&mut self) {
            let (lock, signal) = &*self.0;
            lock.lock().unwrap().1 = true;
            signal.notify_all();
        }
    }
    // A failed assertion must also release native workers before Tokio teardown.
    let _release = ReleaseGate(Arc::clone(&gate));
    let hook_gate = gate.clone();
    let _hook = crate::app::install_metadata_inspection_test_hook(move |path| {
        if !path.as_str().starts_with("cancel-") {
            return;
        }
        let (lock, signal) = &*hook_gate;
        let mut state = lock.lock().unwrap();
        state.0 += 1;
        signal.notify_all();
        while !state.1 {
            state = signal.wait(state).unwrap();
        }
    });

    let first_application = Arc::clone(&application);
    let first_id = by_location["cancel-a.ARW"].clone();
    let first = tokio::spawn(async move { first_application.photo_metadata(&first_id).await });
    let second_application = Arc::clone(&application);
    let second_id = by_location["cancel-b.ARW"].clone();
    let second = tokio::spawn(async move { second_application.photo_metadata(&second_id).await });
    let deadline = Instant::now() + Duration::from_secs(5);
    while gate.0.lock().unwrap().0 != 2 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(gate.0.lock().unwrap().0, 2, "two metadata workers admitted");

    let third = tokio::time::timeout(
        Duration::from_secs(1),
        application.photo_metadata(&by_location["cancel-c.ARW"]),
    )
    .await
    .expect("third request must fall back instead of queueing")
    .unwrap();
    assert_eq!(third, slipstream_core::CaptureReviewMetadata::default());
    assert_eq!(gate.0.lock().unwrap().0, 2, "no third worker scheduled");

    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    let after_cancellation = tokio::time::timeout(
        Duration::from_secs(1),
        application.photo_metadata(&by_location["cancel-c.ARW"]),
    )
    .await
    .expect("cancelled waiter must not release active native work")
    .unwrap();
    assert_eq!(
        after_cancellation,
        slipstream_core::CaptureReviewMetadata::default()
    );
    assert_eq!(gate.0.lock().unwrap().0, 2);
    assert!(application.library.try_admit_native_work().is_none());

    {
        let (lock, signal) = &*gate;
        lock.lock().unwrap().1 = true;
        signal.notify_all();
    }
    second.await.unwrap().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let recovered = loop {
        if let Some(first) = application.library.try_admit_native_work() {
            if let Some(second) = application.library.try_admit_native_work() {
                break (first, second);
            }
            drop(first);
        }
        assert!(
            Instant::now() < deadline,
            "native admission was not released"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    };
    assert!(application.library.try_admit_native_work().is_none());
    drop(recovered);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn publication_preserves_facts_committed_between_scan_and_publication() {
    let (base, config) = prepare_fixture();
    for name in ["a.jpg", "b.jpg"] {
        jpeg_fixture(&config.library_root.join(name), 8, 4, [32, 64, 192]);
    }
    // First boot persists the initial scan so the second boot publishes
    // from stored state and the background rescan is the cycle under test.
    {
        let application = Application::open(&config).await.unwrap();
        wait_for_scan_settled(&application).await;
        application.shutdown().await.unwrap();
    }
    // Park the background rescan after its apply and before publication.
    let (publish_sender, publish_receiver) = tokio::sync::oneshot::channel();
    let application =
        Application::open_with_gate(&config, ScanLimits::default(), None, Some(publish_receiver))
            .await
            .unwrap();
    let router = authorized_router(Arc::clone(&application), config.web_root());
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;

    // Commit a Selection State and a Review Preview seed while the
    // completed scan is parked before publication.
    assert_eq!(
        post_json(
            &router,
            &format!("https://camera.local/api/photos/{}/state", ids[0]),
            serde_json::json!({"field": "selectionState", "value": "selected"}),
            Some("https://camera.local"),
        )
        .await
        .status(),
        StatusCode::OK
    );
    let preview = application.preview(&ids[0]).await.unwrap();
    assert_eq!(preview.state, "ready");

    // Release publication. The fresh persisted read must retain both
    // committed facts instead of reverting to the scan's apply snapshot.
    drop(publish_sender);
    wait_for_scan_settled(&application).await;
    let opened = application
        .browse_open(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::All,
            None,
        )
        .await
        .unwrap();
    let window = application
        .browse_window(&opened.token, 0, 10)
        .await
        .unwrap();
    let first = window
        .photos
        .iter()
        .find(|photo| photo.id == ids[0])
        .unwrap();
    assert_eq!(first.selection_state, "selected");
    assert_eq!(first.preview.state, "ready");
    assert_eq!(first.preview.width, Some(8));
    assert_eq!(first.preview.height, Some(4));
    application.browse_close(&opened.token);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn publication_keeps_scan_owned_invalidation_availability_and_user_state() {
    let (base, config) = prepare_fixture();
    for name in ["a.jpg", "b.jpg"] {
        jpeg_fixture(&config.library_root.join(name), 8, 4, [32, 64, 192]);
    }
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let ids = browse_photo_ids(&application, BrowseSourceRequest::Library).await;
    assert_eq!(application.preview(&ids[0]).await.unwrap().state, "ready");
    assert_eq!(
        application
            .mutate_photo_state(slipstream_core::PhotoStateMutation {
                photo_id: ids[0].clone(),
                field: slipstream_core::PhotoStateField::SelectionState,
                value: slipstream_core::PhotoStateValue::Selection(SelectionState::Selected),
                expected_current: None,
                album_id: None,
            })
            .await
            .unwrap()
            .photo_id,
        ids[0]
    );

    // A changed source revision and a removed Original are scan-owned
    // facts. The publication must keep the invalidation and availability
    // while the committed user decision survives the fresh read.
    jpeg_fixture(&config.library_root.join("a.jpg"), 9, 5, [10, 20, 30]);
    fs::remove_file(config.library_root.join("b.jpg")).unwrap();
    application.rescan().await.unwrap();

    let opened = application
        .browse_open(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::All,
            None,
        )
        .await
        .unwrap();
    let window = application
        .browse_window(&opened.token, 0, 10)
        .await
        .unwrap();
    assert_eq!(window.total, 2);
    let first = window
        .photos
        .iter()
        .find(|photo| photo.id == ids[0])
        .unwrap();
    assert_eq!(first.selection_state, "selected");
    assert_eq!(first.preview.state, "inspection-pending");
    assert_eq!(first.preview.source, None);
    assert_eq!(first.preview.width, None);
    let second = window
        .photos
        .iter()
        .find(|photo| photo.id == ids[1])
        .unwrap();
    assert!(!second.available);
    application.browse_close(&opened.token);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn fresh_service_is_healthy_while_library_initializes_then_status_reaches_published_idle() {
    let (base, config) = prepare_fixture();
    let (gate_sender, gate_receiver) = tokio::sync::oneshot::channel();
    let application =
        Application::open_with_gate(&config, ScanLimits::default(), Some(gate_receiver), None)
            .await
            .unwrap();
    let router = authorized_router(Arc::clone(&application), config.web_root());

    let health = send(
        &router,
        authenticated_request()
            .uri("https://camera.local/healthz")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(health.status(), StatusCode::OK);

    let overview: serde_json::Value = response_json(
        send(
            &router,
            authenticated_request()
                .uri("https://camera.local/api/overview")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(overview["published"], false);
    assert_eq!(overview["photoCount"], 0);
    assert_eq!(overview["scan"]["state"], "initializing");

    let status: serde_json::Value = response_json(
        send(
            &router,
            authenticated_request()
                .uri("https://camera.local/api/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(status["state"], "initializing");

    let overview: serde_json::Value = response_json(
        send(
            &router,
            authenticated_request()
                .uri("https://camera.local/api/overview")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(overview["published"], false);
    assert_eq!(overview["photoCount"], 0);

    let rejected = post_json(
        &router,
        "/api/browse",
        serde_json::json!({"source":"library"}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::SERVICE_UNAVAILABLE);

    // An Album open is no exception. `album-order` reads persisted membership
    // position, but anchor, filter membership, and counts still come from the
    // Published Library, so it fails with the same not-published response
    // instead of failing later at its first window.
    let early_album = application
        .mutate_album(slipstream_core::AlbumMutation::Create {
            name: "Early".to_owned(),
        })
        .await
        .unwrap()
        .albums
        .into_iter()
        .find(|album| album.name == "Early")
        .unwrap()
        .id;
    let rejected_album = post_json(
        &router,
        "/api/browse",
        serde_json::json!({"source":"album","albumId": early_album}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(rejected_album.status(), StatusCode::SERVICE_UNAVAILABLE);

    drop(gate_sender);
    wait_for_scan_settled(&application).await;
    let overview: serde_json::Value = response_json(
        send(
            &router,
            authenticated_request()
                .uri("https://camera.local/api/overview")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(overview["published"], true);
    assert_eq!(overview["scan"]["state"], "idle");
    let status: serde_json::Value = response_json(
        send(
            &router,
            authenticated_request()
                .uri("https://camera.local/api/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(status["state"], "idle");
    assert!(status["publication"].is_string());
    let opened = post_json(
        &router,
        "/api/browse",
        serde_json::json!({"source":"library"}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(opened.status(), StatusCode::OK);
    application.shutdown().await.unwrap();

    // A completed empty publication is durable: restart serves the empty
    // Library immediately instead of regressing to initializing.
    let (restart_gate_sender, restart_gate_receiver) = tokio::sync::oneshot::channel();
    let reopened = Application::open_with_gate(
        &config,
        ScanLimits::default(),
        Some(restart_gate_receiver),
        None,
    )
    .await
    .unwrap();
    let overview = reopened.overview().await.unwrap();
    assert!(overview.published);
    assert_eq!(overview.photo_count, 0);
    assert_eq!(overview.scan.state, "idle");
    let opened = reopened
        .browse_open(
            BrowseSourceRequest::Library,
            BrowseViewOrder::CaptureTimeAscending,
            BrowseSelectionFilter::All,
            None,
        )
        .await
        .unwrap();
    assert_eq!(opened.total, 0);
    restart_gate_sender.send(()).unwrap();
    reopened.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn persisted_library_serves_immediately_while_background_rescan_runs() {
    let (base, config) = prepare_fixture();
    capture_metadata_fixture(&config.library_root.join("a.jpg"), "2026:01:01 09:00:00");
    capture_metadata_fixture(&config.library_root.join("z.jpg"), "2026:01:01 10:00:00");
    {
        let application = Application::open(&config).await.unwrap();
        wait_for_scan_settled(&application).await;
        assert_eq!(
            browse_photo_ids(&application, BrowseSourceRequest::Library)
                .await
                .len(),
            2
        );
        application.shutdown().await.unwrap();
    }

    let (gate_sender, gate_receiver) = tokio::sync::oneshot::channel();
    let application =
        Application::open_with_gate(&config, ScanLimits::default(), Some(gate_receiver), None)
            .await
            .unwrap();
    let router = authorized_router(Arc::clone(&application), config.web_root());

    // The published Library must be served before the background rescan
    // has run at all.
    let overview: serde_json::Value = response_json(
        send(
            &router,
            authenticated_request()
                .uri("https://camera.local/api/overview")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(overview["published"], true);
    assert_eq!(overview["photoCount"], 2);
    assert_eq!(overview["scan"]["state"], "idle");

    let opened = post_json(
        &router,
        "/api/browse",
        serde_json::json!({"source":"library"}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(opened.status(), StatusCode::OK);
    let opened: serde_json::Value = response_json(opened).await;
    assert_eq!(opened["total"], 2);

    drop(gate_sender);
    wait_for_scan_settled(&application).await;
    let overview: serde_json::Value = response_json(
        send(
            &router,
            authenticated_request()
                .uri("https://camera.local/api/overview")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(overview["photoCount"], 2);
    assert_eq!(overview["scan"]["state"], "idle");
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn background_scan_failure_keeps_prior_published_library_and_reports_failed() {
    let (base, config) = prepare_fixture();
    capture_metadata_fixture(&config.library_root.join("a.jpg"), "2026:01:01 09:00:00");
    {
        let application = Application::open(&config).await.unwrap();
        wait_for_scan_settled(&application).await;
        application.shutdown().await.unwrap();
    }
    fs::write(config.library_root.join("b.jpg"), b"jpeg").unwrap();
    let application = Application::open_with_gate(
        &config,
        ScanLimits::new(100, 1, 25_000).unwrap(),
        None,
        None,
    )
    .await
    .unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());

    assert_eq!(application.scan_status().state, "failed");
    let overview: serde_json::Value = response_json(
        send(
            &router,
            authenticated_request()
                .uri("https://camera.local/api/overview")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(overview["published"], true);
    assert_eq!(overview["photoCount"], 1);
    assert_eq!(overview["scan"]["state"], "failed");
    assert_eq!(
        browse_photo_ids(&application, BrowseSourceRequest::Library)
            .await
            .len(),
        1
    );

    // An explicit rescan under the same failing limit reports the failure
    // and keeps the prior published Library browsable.
    let rescanned = send(
        &router,
        authenticated_request()
            .method("POST")
            .uri("https://camera.local/api/scan")
            .header(header::ORIGIN, "https://camera.local")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(rescanned.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(application.scan_status().state, "failed");
    assert_eq!(
        browse_photo_ids(&application, BrowseSourceRequest::Library)
            .await
            .len(),
        1
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn shutdown_drains_background_scan_before_closing() {
    let (base, config) = prepare_fixture();
    capture_metadata_fixture(&config.library_root.join("a.jpg"), "2026:01:01 09:00:00");
    let (gate_sender, gate_receiver) = tokio::sync::oneshot::channel();
    let application =
        Application::open_with_gate(&config, ScanLimits::default(), Some(gate_receiver), None)
            .await
            .unwrap();
    drop(gate_sender);
    application.shutdown().await.unwrap();
    for suffix in ["-journal", "-wal", "-shm"] {
        assert!(
            !config
                .state_directory
                .join("library.sqlite".to_owned() + suffix)
                .exists()
        );
    }
    let reopened = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&reopened).await;
    assert_eq!(reopened.published_photo_count(), 1);
    assert_eq!(reopened.scan_status().state, "idle");
    reopened.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn startup_and_explicit_waiters_share_one_scan_cycle_and_terminal_status() {
    let (base, config) = prepare_fixture();
    capture_metadata_fixture(&config.library_root.join("a.jpg"), "2026:01:01 09:00:00");
    let (gate_sender, gate_receiver) = tokio::sync::oneshot::channel();
    let application =
        Application::open_with_gate(&config, ScanLimits::default(), Some(gate_receiver), None)
            .await
            .unwrap();

    // Startup was admitted synchronously but its leader is parked. Every
    // explicit waiter must join that same application-owned cycle.
    let receivers = (0..8)
        .map(|_| application.admit_scan_cycle(None, None).unwrap())
        .collect::<Vec<_>>();
    gate_sender.send(()).unwrap();

    let mut terminal = None;
    for receiver in receivers {
        let status = receiver.await.unwrap().unwrap();
        let facts = (status.state, status.completed, status.total);
        if let Some(expected) = terminal {
            assert_eq!(facts, expected);
        } else {
            terminal = Some(facts);
        }
    }
    assert_eq!(terminal, Some(("idle", Some(1), Some(1))));
    assert_eq!(application.shared.runs_started.load(Ordering::Relaxed), 1);
    assert_eq!(application.shared.runs_completed.load(Ordering::Relaxed), 1);
    assert_eq!(application.shared.awaiting_scan.load(Ordering::Relaxed), 0);

    // A terminal cycle releases admission for one later independent cycle.
    let next = application.rescan().await.unwrap();
    assert_eq!(
        (next.state, next.completed, next.total),
        ("idle", Some(1), Some(1))
    );
    assert_eq!(application.shared.runs_started.load(Ordering::Relaxed), 2);
    assert_eq!(application.shared.runs_completed.load(Ordering::Relaxed), 2);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn dropped_scan_waiters_do_not_cancel_the_leader_or_leak_applying_status() {
    let (base, config) = prepare_fixture();
    capture_metadata_fixture(&config.library_root.join("a.jpg"), "2026:01:01 09:00:00");
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let baseline = application.shared.runs_completed.load(Ordering::Relaxed);

    let (gate_sender, gate_receiver) = tokio::sync::oneshot::channel();
    let first = application
        .admit_scan_cycle(Some(gate_receiver), None)
        .unwrap();
    let mut abandoned = vec![first];
    for _ in 1..64 {
        abandoned.push(application.admit_scan_cycle(None, None).unwrap());
    }
    // These receivers model HTTP request futures dropped after admission.
    drop(abandoned);
    // Cancellation must release bounded waiter capacity before the physical
    // cycle completes; this live caller joins the same leader.
    let live = application.admit_scan_cycle(None, None).unwrap();
    gate_sender.send(()).unwrap();
    assert_eq!(live.await.unwrap().unwrap().state, "idle");

    wait_for_scan_runs(&application, baseline + 1).await;
    assert_eq!(
        application.shared.runs_started.load(Ordering::Relaxed),
        baseline + 1
    );
    assert_eq!(application.shared.awaiting_scan.load(Ordering::Relaxed), 0);
    assert_eq!(application.scan_status().state, "idle");

    // Cleanup of the abandoned cycle must leave the next admission usable.
    let next = application.rescan().await.unwrap();
    assert_eq!(next.state, "idle");
    assert_eq!(
        application.shared.runs_completed.load(Ordering::Relaxed),
        baseline + 2
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn shutdown_returns_only_after_live_scan_waiters_receive_terminal_status() {
    let (base, config) = prepare_fixture();
    capture_metadata_fixture(&config.library_root.join("a.jpg"), "2026:01:01 09:00:00");
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;

    let (gate_sender, gate_receiver) = tokio::sync::oneshot::channel();
    let mut receive = application
        .admit_scan_cycle(Some(gate_receiver), None)
        .unwrap();
    let closing = {
        let application = Arc::clone(&application);
        tokio::spawn(async move { application.shutdown().await })
    };
    tokio::task::yield_now().await;
    gate_sender.send(()).unwrap();
    closing.await.unwrap().unwrap();

    let terminal = receive
        .try_recv()
        .expect("shutdown returned before scan waiter fan-out")
        .unwrap();
    assert_eq!(terminal.state, "idle");
    assert_eq!(application.shared.awaiting_scan.load(Ordering::Relaxed), 0);
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn persisted_forty_thousand_photo_library_serves_bounded_overview_before_rescan_completes() {
    let (base, config) = prepare_fixture();
    fs::create_dir(config.state_directory.clone()).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            config.state_directory.clone(),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
    }
    let database =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    database
        .execute_batch(include_str!(
            "../../../../compatibility/sqlite/schema-v4.sql"
        ))
        .unwrap();
    database
        .execute(
            "INSERT INTO library_metadata VALUES('canonical_root',?)",
            [config.library_root.to_str().unwrap()],
        )
        .unwrap();
    database.execute("BEGIN", []).unwrap();
    for index in 0..40_000_u32 {
        let padded = format!("{index:06}");
        let original_id = format!("{:08x}", index).repeat(8);
        let photo_id = format!("{:08x}", 1_000_000 + index).repeat(8);
        let path = format!("{padded}.jpg");
        database
                .execute(
                    "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available) VALUES(?1,?2,'jpeg',1,1.0,1)",
                    rusqlite::params![original_id, path],
                )
                .unwrap();
        database
                .execute(
                    "INSERT INTO photos(id,jpeg_original_id,ambiguous,available,preview_state,sort_path) VALUES(?1,?2,0,1,'inspection-pending',?3)",
                    rusqlite::params![photo_id, original_id, path],
                )
                .unwrap();
    }
    database.execute("COMMIT", []).unwrap();
    drop(database);

    let (gate_sender, gate_receiver) = tokio::sync::oneshot::channel();
    let application =
        Application::open_with_gate(&config, ScanLimits::default(), Some(gate_receiver), None)
            .await
            .unwrap();
    let router = authorized_router(Arc::clone(&application), config.web_root());

    // Served from the persisted Library before the background rescan runs.
    let overview_response = send(
        &router,
        authenticated_request()
            .uri("https://camera.local/api/overview")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(overview_response.status(), StatusCode::OK);
    let overview_bytes = axum::body::to_bytes(overview_response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert!(overview_bytes.len() < 20_000);
    let overview: serde_json::Value = serde_json::from_slice(&overview_bytes).unwrap();
    assert_eq!(overview["published"], true);
    assert_eq!(overview["photoCount"], 40_000);

    let opened = post_json(
        &router,
        "/api/browse",
        serde_json::json!({"source":"library"}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(opened.status(), StatusCode::OK);
    let opened: serde_json::Value = response_json(opened).await;
    let token = opened["token"].as_str().unwrap();
    let window: serde_json::Value = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/browse/{token}?start=39940&limit=60"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(window["start"], 39_940);
    assert_eq!(window["total"], 40_000);
    assert_eq!(window["photos"].as_array().unwrap().len(), 60);

    drop(gate_sender);
    wait_for_scan_settled(&application).await;
    let overview: serde_json::Value = response_json(
        send(
            &router,
            authenticated_request()
                .uri("https://camera.local/api/overview")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(overview["photoCount"], 40_000);
    assert_eq!(overview["scan"]["state"], "idle");
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// Deterministic manual-recovery HTTP fixture: one unavailable Photo with a
/// retained Rating, Selection State, and Album membership, seeded before the
/// Application opens. `fingerprint` optionally seeds a matching fingerprint
/// for the remembered bytes.
fn recovery_http_fixture(fingerprint: bool) -> (PathBuf, Config) {
    let (base, config) = prepare_fixture();
    fs::create_dir_all(&config.state_directory).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            config.state_directory.clone(),
            fs::Permissions::from_mode(0o700),
        )
        .unwrap();
    }
    let database =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    database
        .execute_batch(include_str!(
            "../../../../compatibility/sqlite/schema-v6.sql"
        ))
        .unwrap();
    database
        .execute(
            "INSERT INTO library_metadata VALUES('canonical_root',?)",
            [config.library_root.to_str().unwrap()],
        )
        .unwrap();
    database.execute(
        "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,capture_metadata_state,capture_source_revision) VALUES('aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa','shoot/a.JPG','jpeg',11,1.0,0,'missing','remembered-revision')",
        [],
    ).unwrap();
    database.execute(
        "INSERT INTO photos(id,original_id,available,preview_state,sort_path,selection_state,rating) VALUES('bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb','aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',0,'unavailable','shoot/a.JPG','selected',3)",
        [],
    ).unwrap();
    database
        .execute("INSERT INTO albums VALUES('set','Trip',1)", [])
        .unwrap();
    database
        .execute(
            "INSERT INTO album_members VALUES('set','bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb',0)",
            [],
        )
        .unwrap();
    if fingerprint {
        database.execute(
            "INSERT INTO original_fingerprints(original_id,digest,size,mtime_ms) VALUES('aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa',?,11,1.0)",
            [slipstream_core::digest_bytes(b"jpeg-bytes-a")],
        ).unwrap();
    }
    drop(database);
    (base, config)
}

async fn library_window(router: &Router) -> serde_json::Value {
    let opened = response_json(
        post_json(
            router,
            "/api/browse",
            serde_json::json!({"source":"library"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let token = opened["token"].as_str().unwrap();
    response_json(
        send(
            router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/browse/{token}?start=0&limit=60"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await
}

#[tokio::test]
async fn recovery_http_restores_unavailable_photo_without_fingerprint() {
    let (base, config) = recovery_http_fixture(false);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());

    // The moved file appears only after the settled scan, so no automatic
    // recovery can act and the persisted record stays unavailable.
    fs::create_dir_all(config.library_root.join("moved")).unwrap();
    fs::write(config.library_root.join("moved/a.JPG"), b"jpeg-bytes-a").unwrap();

    let unavailable = response_json(
        send(
            &router,
            authenticated_request()
                .uri("https://camera.local/api/recovery/unavailable")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(unavailable["unavailable"].as_array().unwrap().len(), 1);
    let record = &unavailable["unavailable"][0];
    assert_eq!(record["location"], "shoot/a.JPG");
    assert_eq!(record["kind"], "jpeg");
    assert_eq!(record["rating"], 3);
    assert_eq!(record["selectionState"], "selected");
    assert_eq!(record["fingerprintEnrolled"], false);
    assert_eq!(record["albumCount"], 1);

    let proposals = response_json(
        post_json(
            &router,
            "/api/recovery/propose",
            serde_json::json!({"oldPrefix":"shoot","newPrefix":"moved"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let proposal = &proposals["proposals"][0];
    assert_eq!(proposal["outcome"], "matched");
    assert_eq!(proposal["verified"], false);
    assert_eq!(proposal["toLocation"], "moved/a.JPG");

    let applied = response_json(
        post_json(
            &router,
            "/api/recovery/apply",
            serde_json::json!({"relocations":[{"originalId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","newLocation":"moved/a.JPG"}]}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(applied["relocatedPhotos"], 1);
    assert_eq!(applied["unavailablePhotos"], 0);

    // The restored Photo keeps its identity, decisions, and Album membership.
    let window = library_window(&router).await;
    assert_eq!(window["total"], 1);
    let photo = &window["photos"][0];
    assert_eq!(photo["id"], "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
    assert_eq!(photo["available"], true);
    assert_eq!(photo["originalFilename"], "a.JPG");
    assert_eq!(photo["rating"], 3);
    assert_eq!(photo["selectionState"], "selected");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn recovery_http_retires_discovered_destination_photo() {
    let (base, config) = recovery_http_fixture(false);
    // The moved file exists before open, so the initial scan discovers it as
    // a new default-state Photo occupying the destination.
    fs::create_dir_all(config.library_root.join("moved")).unwrap();
    fs::write(config.library_root.join("moved/a.JPG"), b"jpeg-bytes-a").unwrap();
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());

    // Both records are visible: the remembered unavailable Photo and the
    // newly discovered occupier.
    let window = library_window(&router).await;
    assert_eq!(window["total"], 2);
    let discovered = window["photos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|photo| photo["id"].as_str().unwrap().to_owned())
        .find(|id| id != "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb")
        .unwrap();

    let proposals = response_json(
        post_json(
            &router,
            "/api/recovery/propose",
            serde_json::json!({"oldPrefix":"shoot","newPrefix":"moved"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let proposal = &proposals["proposals"][0];
    assert_eq!(proposal["outcome"], "occupied");
    assert_eq!(proposal["retire"]["photoId"].as_str().unwrap(), discovered);

    // Without the explicit retire the whole batch is refused with a 409.
    let refused = post_json(
        &router,
        "/api/recovery/apply",
        serde_json::json!({"relocations":[{"originalId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","newLocation":"moved/a.JPG"}]}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);

    let applied = response_json(
        post_json(
            &router,
            "/api/recovery/apply",
            serde_json::json!({"relocations":[{"originalId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","newLocation":"moved/a.JPG","retireDestination":true}]}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(applied["relocatedPhotos"], 1);

    let window = library_window(&router).await;
    assert_eq!(window["total"], 1);
    let photo = &window["photos"][0];
    assert_eq!(photo["id"], "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb");
    assert_eq!(photo["rating"], 3);
    assert_eq!(photo["selectionState"], "selected");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn recovery_http_verifies_fingerprints_and_rejects_mismatches() {
    let (base, config) = recovery_http_fixture(true);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    fs::create_dir_all(config.library_root.join("moved")).unwrap();
    // Different content than the enrolled fingerprint.
    fs::write(config.library_root.join("moved/a.JPG"), b"other-bytes").unwrap();

    let proposals = response_json(
        post_json(
            &router,
            "/api/recovery/propose",
            serde_json::json!({"oldPrefix":"shoot","newPrefix":"moved"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(proposals["proposals"][0]["outcome"], "content-mismatch");

    let refused = post_json(
        &router,
        "/api/recovery/apply",
        serde_json::json!({"relocations":[{"originalId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","newLocation":"moved/a.JPG"}]}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    let body = response_json(refused).await;
    assert_eq!(body["rejections"][0]["reason"], "content-mismatch");

    // Matching content verifies and commits.
    fs::write(config.library_root.join("moved/a.JPG"), b"jpeg-bytes-a").unwrap();
    let proposals = response_json(
        post_json(
            &router,
            "/api/recovery/propose",
            serde_json::json!({"oldPrefix":"shoot","newPrefix":"moved"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(proposals["proposals"][0]["outcome"], "matched");
    assert_eq!(proposals["proposals"][0]["verified"], true);

    let single = response_json(
        post_json(
            &router,
            "/api/recovery/propose",
            serde_json::json!({"originalId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","newLocation":"moved/a.JPG"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(single["outcome"], "matched");
    assert_eq!(single["verified"], true);

    let applied = response_json(
        post_json(
            &router,
            "/api/recovery/apply",
            serde_json::json!({"relocations":[{"originalId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","newLocation":"moved/a.JPG"}]}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    assert_eq!(applied["relocatedPhotos"], 1);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn recovery_http_rejects_duplicate_source_mappings() {
    let (base, config) = recovery_http_fixture(false);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());
    fs::create_dir_all(config.library_root.join("moved")).unwrap();
    fs::write(config.library_root.join("moved/a.JPG"), b"jpeg-bytes-a").unwrap();
    fs::write(config.library_root.join("moved/b.JPG"), b"jpeg-bytes-b").unwrap();

    // Two mappings for one Original File are a colliding batch refused with
    // a per-mapping reason.
    let refused = post_json(
        &router,
        "/api/recovery/apply",
        serde_json::json!({"relocations":[
            {"originalId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","newLocation":"moved/a.JPG"},
            {"originalId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","newLocation":"moved/b.JPG"}
        ]}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    let body = response_json(refused).await;
    assert_eq!(body["rejections"][0]["reason"], "colliding");

    // The refusal leaves the Library untouched.
    let unavailable = response_json(
        send(
            &router,
            authenticated_request()
                .uri("https://camera.local/api/recovery/unavailable")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(unavailable["unavailable"].as_array().unwrap().len(), 1);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn recovery_http_validates_requests() {
    let (base, config) = recovery_http_fixture(false);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());

    let escape = post_json(
        &router,
        "/api/recovery/propose",
        serde_json::json!({"oldPrefix":"..","newPrefix":"moved"}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(escape.status(), StatusCode::BAD_REQUEST);

    let unknown = post_json(
        &router,
        "/api/recovery/propose",
        serde_json::json!({"originalId":"cccccccc-cccc-4ccc-8ccc-cccccccccccc","newLocation":"moved/a.JPG"}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

    let empty = post_json(
        &router,
        "/api/recovery/apply",
        serde_json::json!({"relocations":[]}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(empty.status(), StatusCode::BAD_REQUEST);

    let stale = post_json(
        &router,
        "/api/recovery/apply",
        serde_json::json!({"relocations":[{"originalId":"cccccccc-cccc-4ccc-8ccc-cccccccccccc","newLocation":"moved/a.JPG"}]}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(stale.status(), StatusCode::CONFLICT);
    let body = response_json(stale).await;
    assert_eq!(body["rejections"][0]["reason"], "stale");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn recovery_http_reports_missing_destination_without_fingerprint() {
    let (base, config) = recovery_http_fixture(false);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = authorized_router(Arc::clone(&application), config.web_root());

    // Nothing exists at the destination: the batch must not present it as a
    // usable mapping, and it must stay explicitly unverified.
    let proposals = response_json(
        post_json(
            &router,
            "/api/recovery/propose",
            serde_json::json!({"oldPrefix":"shoot","newPrefix":"moved"}),
            Some("https://camera.local"),
        )
        .await,
    )
    .await;
    let proposal = &proposals["proposals"][0];
    assert_eq!(proposal["outcome"], "missing");
    assert_eq!(proposal["verified"], false);
    assert_eq!(proposal["toLocation"], "moved/a.JPG");

    // Applying the same mapping is refused, and the refusal changes nothing.
    // The apply path judges the candidate in its own vocabulary: absent and
    // unreadable are both refusals.
    let refused = post_json(
        &router,
        "/api/recovery/apply",
        serde_json::json!({"relocations":[{"originalId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa","newLocation":"moved/a.JPG"}]}),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    let body = response_json(refused).await;
    assert_eq!(body["rejections"][0]["reason"], "unreadable");

    let unavailable = response_json(
        send(
            &router,
            authenticated_request()
                .uri("https://camera.local/api/recovery/unavailable")
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(unavailable["unavailable"].as_array().unwrap().len(), 1);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
