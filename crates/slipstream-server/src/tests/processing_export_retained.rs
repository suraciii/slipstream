use super::*;

#[tokio::test]
async fn historical_publication_remains_downloadable_after_submission_owner_disconnects() {
    let (base, config) = export_fixture();
    let (application, router) = export_application(&config).await;
    let photo_id = first_photo_id(&application).await;
    let (revision, source) = save_selected_step(&application, &photo_id, "recipe-1").await;
    let artifact_id = settle_historical_export(
        &application,
        &photo_id,
        "export-disconnected",
        &revision,
        &source,
    )
    .await;
    drop(router);
    let router = create_router_with_processing(
        Arc::clone(&application),
        crate::http::open_web_root(config.web_root()),
        None,
    );
    let work = settled_export(&router, &photo_id, "export-disconnected").await;
    assert_eq!(work["state"], "succeeded");
    let response = send(
        &router,
        authenticated_request()
            .uri(format!("/api/processing-artifacts/{artifact_id}/bytes"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(
        response.into_body(),
        slipstream_core::MAXIMUM_EXPORT_BYTES as usize,
    )
    .await
    .unwrap();
    let (status, replayed) = post_processing_export(
        &router,
        &photo_id,
        &submit_body("export-disconnected", "develop-1", &revision, &source),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(replayed["replayed"], true);
    assert_eq!(replayed["artifact"]["artifactId"], artifact_id);
    assert_eq!(replayed["artifact"]["byteLength"], bytes.len());
    assert_eq!(
        replayed["artifact"]["sha256"],
        format!("{:x}", sha2::Sha256::digest(&bytes))
    );
    assert_eq!(FakePhotoEngine::at(&base).runs(), 0);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn retry_uses_failed_and_cancelled_snapshots_after_recipe_changes() {
    let (base, mut config) = export_fixture();
    // Both captured retries remain live while their durable receipts are read.
    config.export_retained_output_bytes = Some(2 * slipstream_core::MAXIMUM_EXPORT_BYTES);
    let (application, _) = export_application(&config).await;
    let photo_id = first_photo_id(&application).await;
    let (revision, source) = save_selected_step(&application, &photo_id, "recipe-1").await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    for request_id in ["export-failed-seed", "export-cancelled-seed"] {
        let outcome = application
            .library
            .submit_processing_export(
                SubmitProcessingExport {
                    photo_id: photo_id.clone(),
                    request_id: request_id.to_owned(),
                    step_id: ProcessingStepId::new("develop-1").unwrap(),
                    expected_recipe_revision: revision.clone(),
                    expected_source_revision: source.clone(),
                    bundle_id: "c".repeat(64),
                    retained_output_bytes_max: u64::MAX,
                    adapter: ProcessingExportAdapterDecision::Qualified {
                        adapter_version: DARKTABLE_ADAPTER_VERSION.to_owned(),
                        parameter_schema_version: DARKTABLE_PARAMETER_VERSION.to_owned(),
                    },
                },
                now,
            )
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            ProcessingExportSubmitOutcome::Admitted(_)
        ));
    }
    application
        .library
        .fail_processing_export("export-failed-seed", "execution_failed", now)
        .await
        .unwrap();
    application
        .library
        .cancel_processing_export("export-cancelled-seed", now)
        .await
        .unwrap();
    let current = application
        .library
        .composable_edit_recipe_read(&photo_id)
        .await
        .unwrap()
        .unwrap();
    let mut recipe = current.recipe.unwrap();
    recipe.steps[0].parameters.tree["output"] = serde_json::json!({"format":"jpeg","precisionBits":8,"colorSpace":"srgb","transferFunction":"srgb"});
    let changed = application
        .library
        .save_composable_edit_recipe(slipstream_core::SaveComposableEditRecipe {
            photo_id: photo_id.clone(),
            request_id: "recipe-changed".into(),
            expected_recipe_revision: Some(revision.clone()),
            expected_source_revision: source.clone(),
            request_intent: None,
            recipe,
            automatic_adjustment: None,
        })
        .await
        .unwrap();
    assert!(
        matches!(&changed,slipstream_core::ComposableEditRecipeWriteOutcome::Saved(r) if r.revision!=revision)
    );
    let read_router = create_router_with_processing(
        Arc::clone(&application),
        crate::http::open_web_root(config.web_root()),
        None,
    );
    for (previous, retry) in [
        ("export-failed-seed", "retry-failed"),
        ("export-cancelled-seed", "retry-cancelled"),
    ] {
        let (status, replay) = post_processing_export(
            &read_router,
            &photo_id,
            &submit_body(previous, "develop-1", &revision, &source),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(replay["error"]["code"], "export_terminal");
        assert_eq!(replay["error"]["details"]["replayed"], true);
        let (_, original) = export_status(&read_router, &photo_id, previous).await;
        let retry_uri = format!("/api/photos/{photo_id}/processing-exports/{previous}/retry");
        let outcome = application
            .library
            .retry_processing_export(
                slipstream_core::RetryProcessingExport {
                    photo_id: photo_id.clone(),
                    previous_request_id: previous.to_owned(),
                    request_id: retry.to_owned(),
                    bundle_id: "c".repeat(64),
                    retained_output_bytes_max: u64::MAX,
                    adapter: ProcessingExportAdapterDecision::Qualified {
                        adapter_version: DARKTABLE_ADAPTER_VERSION.to_owned(),
                        parameter_schema_version: DARKTABLE_PARAMETER_VERSION.to_owned(),
                    },
                },
                now,
            )
            .await
            .unwrap();
        assert!(
            matches!(outcome, ProcessingExportSubmitOutcome::Admitted(_)),
            "{outcome:?}"
        );
        let (status, accepted) = export_status(&read_router, &photo_id, retry).await;
        assert_eq!(status, StatusCode::OK);
        for field in [
            "parameters",
            "input",
            "module",
            "stepId",
            "recipeRevision",
            "sourceRevision",
            "bundleId",
        ] {
            assert_eq!(accepted[field], original[field], "captured {field}");
        }
        let response = post_json(
            &read_router,
            &retry_uri,
            serde_json::json!({"requestId": retry}),
            Some("https://camera.local"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert_eq!(response_json(response).await["outcome"], "replayed");
    }
    let (status, retained) = get_json(
        &read_router,
        &format!("/api/photos/{photo_id}/processing-exports"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let works = retained["exports"].as_array().unwrap();
    for request_id in [
        "export-failed-seed",
        "export-cancelled-seed",
        "retry-failed",
        "retry-cancelled",
    ] {
        assert!(works.iter().any(|work| work["requestId"] == request_id));
    }
    for request_id in ["retry-failed", "retry-cancelled"] {
        application
            .library
            .cancel_processing_export(request_id, now)
            .await
            .unwrap();
    }
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn swept_failed_snapshot_retry_reports_expired_without_admission() {
    let (base, config) = export_fixture();
    let (application, router) = export_application(&config).await;
    let photo_id = first_photo_id(&application).await;
    let (revision, source) = save_selected_step(&application, &photo_id, "expiry-recipe").await;
    let outcome = application
        .library
        .submit_processing_export(
            SubmitProcessingExport {
                photo_id: photo_id.clone(),
                request_id: "swept-parent".into(),
                step_id: ProcessingStepId::new("develop-1").unwrap(),
                expected_recipe_revision: revision,
                expected_source_revision: source,
                bundle_id: "c".repeat(64),
                retained_output_bytes_max: u64::MAX,
                adapter: ProcessingExportAdapterDecision::Qualified {
                    adapter_version: DARKTABLE_ADAPTER_VERSION.into(),
                    parameter_schema_version: DARKTABLE_PARAMETER_VERSION.into(),
                },
            },
            1000,
        )
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        ProcessingExportSubmitOutcome::Admitted(_)
    ));
    application
        .library
        .fail_processing_export("swept-parent", "execution_failed", 1001)
        .await
        .unwrap();
    application
        .library
        .sweep_processing_export_expiry(
            1001 + slipstream_core::processing::PROCESSING_EXPORT_RECEIPT_RETENTION_SECONDS,
        )
        .await
        .unwrap();
    assert!(
        application
            .library
            .processing_export_work("swept-parent")
            .await
            .unwrap()
            .is_none()
    );
    let read_router = create_router_with_processing(
        Arc::clone(&application),
        crate::http::open_web_root(config.web_root()),
        None,
    );
    for router in [&router, &read_router] {
        let response = post_json(
            router,
            &format!("/api/photos/{photo_id}/processing-exports/swept-parent/retry"),
            serde_json::json!({"requestId":"retry-expired-parent"}),
            Some("https://camera.local"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::GONE);
        assert_eq!(
            response_json(response).await["error"]["code"],
            "artifact_expired"
        );
        assert!(
            application
                .library
                .processing_export_work("retry-expired-parent")
                .await
                .unwrap()
                .is_none()
        );
    }
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn historical_outputs_remain_listed_and_downloadable_without_processing() {
    let (base, mut config) = export_fixture();
    config.processing = None;
    let (application, router) = export_application(&config).await;
    let photo_id = first_photo_id(&application).await;
    let export_id = "exp-retained-historical";
    let bytes = valid_development_tiff_fixture();
    let digest = format!("{:x}", sha2::Sha256::digest(&bytes));
    let recipe_digest = slipstream_core::ExportRecipePayload::capture(
        &slipstream_core::EditRecipeSettings {
            exposure_ev: 0.0,
            white_balance: slipstream_core::WhiteBalanceIntent::AsShot,
        },
        slipstream_core::ExportExposureRange {
            minimum_milli_ev: i64::MIN,
            maximum_milli_ev: i64::MAX,
        },
    )
    .unwrap()
    .digest();
    let path = application
        .export_artifacts_directory
        .join(format!("{export_id}.tiff"));
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, &bytes).unwrap();
    let connection =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    connection
        .execute(
            "INSERT INTO exports(id,photo_id,target,state,recipe_revision,exposure_ev,
           white_balance_mode,source_revision,source_profile_id,source_kind,
           recipe_digest,policy_id,bundle_id,workload,created_at,outcome,
           artifact_size,artifact_sha256,artifact_expires_at,artifact_width,
           artifact_height,artifact_profile_identity,settled_at,retain_until)
         VALUES(?1,?2,'development-tiff','succeeded','rev',0.0,'as-shot','src',
           'profile','raw',?3,?4,?5,'development-tiff',1,NULL,?6,?7,1900000000,2,1,?8,
           1800000000,1900000000)",
            rusqlite::params![
                export_id,
                photo_id,
                recipe_digest,
                "b".repeat(64),
                "c".repeat(64),
                bytes.len() as i64,
                digest,
                "e".repeat(64)
            ],
        )
        .unwrap();
    drop(connection);
    let (status, retained) = get_json(
        &router,
        &format!("/api/photos/{photo_id}/processing-exports"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let historical = retained["historicalExports"].as_array().unwrap();
    assert_eq!(historical.len(), 1);
    assert_eq!(historical[0]["exportId"], export_id);
    assert_eq!(historical[0]["state"], "succeeded");
    let response = send(
        &router,
        authenticated_request()
            .uri(format!("/api/exports/{export_id}/artifact"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "image/tiff");
    assert_eq!(response.headers()["slipstream-artifact-sha256"], digest);
    assert_eq!(
        axum::body::to_bytes(response.into_body(), 64 * 1024 * 1024)
            .await
            .unwrap()
            .as_ref(),
        bytes.as_slice()
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
