use super::*;
/// The ephemeral preview render executor and the durable Export lifecycle
/// share one serialized heavy-work admission. While a durable attempt
/// holds the slot, an admitted preview-class render queues behind it
/// without starting a second engine attempt, and its own attempt runs only
/// after the durable attempt has settled and published.
#[tokio::test]
async fn preview_renders_queue_behind_a_live_export_on_one_admission_slot() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let engine = export_engine(&base);
    // One durable Export attempt starts and stays running: its engine run
    // holds the single heavy-work admission for the whole attempt.
    engine.hang_attempt(1);
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;

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
    let running = wait_for_state(&router, &export_id, "running").await;
    assert_eq!(running["terminalOutcome"], serde_json::Value::Null);

    // A preview-class render of a fresh identity is admitted while the
    // Export holds the slot. The route answers the admission immediately;
    // the render itself must queue behind the same admission instead of
    // starting a second engine attempt.
    let admitted = edit_preview_request(&router, &photo_id).await;
    assert_eq!(admitted.status(), StatusCode::ACCEPTED);
    let admission = response_json(admitted).await;
    assert!(
        admission["state"] == "queued" || admission["state"] == "running",
        "preview admission is queued or running: {admission}"
    );
    assert_eq!(admission["stage"], "develop");
    tokio::time::sleep(Duration::from_millis(500)).await;

    // The frozen engine-run count is the direct observation that the
    // preview render queued: the one run is the live Export's attempt.
    assert_eq!(
        engine.runs(),
        1,
        "a queued preview render must not start a second engine attempt"
    );

    // The held Export settles, validates, and publishes; the queued preview
    // render then acquires the slot and runs its own engine attempt.
    engine.release();
    let succeeded = wait_for_state(&router, &export_id, "succeeded").await;
    assert_eq!(succeeded["state"], "succeeded");
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
    engine.wait_for_runs(2).await;

    // Exactly one durable and one preview attempt reached the engine, and
    // the preview attempt started only after the Export's validating
    // settlement, not beside it.
    assert_eq!(
        engine.runs(),
        2,
        "one Export attempt and one coalesced preview attempt"
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
