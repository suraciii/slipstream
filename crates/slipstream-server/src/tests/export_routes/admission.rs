use super::*;
/// The ephemeral preview render executor and the durable Export lifecycle
/// share one serialized heavy-work admission. While a durable attempt
/// holds the slot, an admitted preview-class render queues behind it
/// without touching the launcher, and its own attempt runs only after the
/// durable attempt has settled and published.
#[tokio::test]
async fn preview_renders_queue_behind_a_live_export_on_one_admission_slot() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;

    // One durable Export attempt starts and stays running: its executor
    // holds the single heavy-work admission for the whole attempt.
    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let export_id = response_json(created).await["exportId"]
        .as_str()
        .unwrap()
        .to_owned();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let started = launcher.with_script(|script| {
            script
                .ops
                .iter()
                .filter(|operation| operation.as_str() == "start")
                .count()
        });
        if started == 1 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the Export attempt must reach the launcher"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // A preview-class render of a fresh identity is admitted while the
    // Export holds the slot. The route answers the admission immediately;
    // the executor itself must queue behind the same admission instead of
    // starting a second launcher attempt.
    let admitted = edit_preview_request(&router, &photo_id).await;
    assert_eq!(admitted.status(), StatusCode::ACCEPTED);
    let admission = response_json(admitted).await;
    assert!(
        admission["state"] == "queued" || admission["state"] == "running",
        "the preview admission is answered while the Export runs: {admission}"
    );
    tokio::time::sleep(Duration::from_millis(500)).await;
    let (starts, reconciles) = launcher.with_script(|script| {
        (
            script
                .ops
                .iter()
                .filter(|operation| operation.as_str() == "start")
                .count(),
            script
                .ops
                .iter()
                .filter(|operation| operation.as_str() == "reconcile")
                .count(),
        )
    });
    assert_eq!(
        starts, 1,
        "no preview Start may pass an Export attempt that holds the admission"
    );
    // The preview executor reconciles only after it acquires the slot, so
    // the frozen reconcile count is the direct observation that it queued:
    // the two reconciles are the Export's own admission and attempt slots.
    assert_eq!(
        reconciles, 2,
        "the queued preview render must not contact the launcher"
    );

    // The held Export settles, collects, and publishes; the queued preview
    // render then acquires the slot and runs its own attempt at the next
    // launcher sequence.
    let artifact = valid_development_tiff();
    launcher.with_script(|script| {
        script.output = Some(artifact);
        script.settle_attempt(1, "completed");
        script.settle_attempt(2, "completed");
    });
    let succeeded = wait_for_state(&router, &export_id, "succeeded").await;
    assert_eq!(succeeded["state"], "succeeded");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let started = launcher.with_script(|script| {
            script
                .ops
                .iter()
                .filter(|operation| operation.as_str() == "start")
                .count()
        });
        if started == 2 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the queued preview render must run once the Export settles"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let response = edit_preview_request(&router, &photo_id).await;
        if response.status() == StatusCode::OK {
            break;
        }
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert!(
            tokio::time::Instant::now() < deadline,
            "the queued preview render did not become a rendition"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // Exactly one durable and one preview attempt crossed the real
    // descriptor protocol, and the preview attempt started only after the
    // Export's validating acknowledgement, not beside it.
    let ops = launcher.with_script(|script| script.ops.clone());
    let starts: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, operation)| operation.as_str() == "start")
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        starts.len(),
        2,
        "one Export and one preview attempt: {ops:?}"
    );
    let export_acknowledgement = ops
        .iter()
        .enumerate()
        .find(|(index, operation)| operation.as_str() == "validate" && *index < starts[1])
        .map(|(index, _)| index)
        .expect("the Export acknowledges its output before the preview starts");
    assert!(
        export_acknowledgement < starts[1],
        "the preview attempt starts only after the Export settled: {ops:?}"
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
