use super::*;

#[tokio::test]
async fn publication_survives_sweep_until_durable_settlement() {
    let (base, config) = export_fixture();
    let (application, router) = export_application(&config).await;
    let photo_id = first_photo_id(&application).await;
    let (recipe_revision, source_revision) =
        save_selected_step(&router, &photo_id, "recipe-1").await;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let manager = application.exports.as_ref().unwrap();
    for (request_id, succeed) in [("export-success", true), ("export-uncertain", false)] {
        let outcome = application
            .library
            .submit_processing_export(
                SubmitProcessingExport {
                    photo_id: photo_id.clone(),
                    request_id: request_id.to_owned(),
                    step_id: ProcessingStepId::new("develop-1").unwrap(),
                    expected_recipe_revision: recipe_revision.clone(),
                    expected_source_revision: source_revision.clone(),
                    bundle_id: config.processing.as_ref().unwrap().bundle_sha256.clone(),
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
        let ProcessingExportSubmitOutcome::Admitted(admission) = outcome else {
            panic!("qualified request must be admitted");
        };
        application
            .library
            .begin_processing_export_attempt(request_id, now)
            .await
            .unwrap();
        let executed = manager
            .run_processing_export(
                request_id,
                &photo_id,
                &source_revision,
                slipstream_processing::modules::Parameters {
                    module: admission.module.as_str().to_owned(),
                    version: admission.parameters.schema_version.clone(),
                    tree: admission.parameters.tree.clone(),
                },
            )
            .await
            .unwrap();
        let path = manager
            .artifact_path_for_workload(&executed.artifact_id, "development-tiff")
            .unwrap();
        let bytes = fs::read(&path).unwrap();
        // Execution has renamed the real validated TIFF, but no artifact
        // record exists yet. Sweep must preserve these exact bytes.
        manager.sweep_expiry().await;
        assert_eq!(fs::read(&path).unwrap(), bytes);
        if succeed {
            let mut artifact = published_artifact(&photo_id, &source_revision);
            artifact.artifact_id = ProcessingArtifactId::new(&executed.artifact_id).unwrap();
            artifact.photo_id = admission.photo_id.clone();
            artifact.step_id = admission.step_id.clone();
            artifact.module = admission.module.clone();
            artifact.adapter_schema_version = admission.adapter_schema_version.clone();
            artifact.parameters = admission.parameters.clone();
            artifact.bundle_id = admission.bundle_id.clone();
            artifact.input = ProcessingInputEvidence::new(
                admission.input.clone(),
                &executed.input_sha256,
                executed.input_size,
            )
            .unwrap();
            artifact.sha256 = executed.sha256.clone();
            artifact.byte_length = executed.size;
            artifact.output_contract.format = executed.output.format.to_owned();
            artifact.output_contract.geometry =
                ProcessingGeometry::new(executed.width, executed.height).unwrap();
            let settlement = application
                .library
                .settle_processing_export(artifact, request_id, &admission.payload_digest, now)
                .await
                .unwrap();
            assert!(
                matches!(
                    settlement,
                    slipstream_core::ProcessingExportSettlement::Settled(_)
                ),
                "publication settlement: {settlement:?}"
            );
            // A lost acknowledgement drops the guard. Durable owner reads
            // establish the claim and keep the successfully published bytes.
            drop(executed);
            manager.sweep_expiry().await;
            assert_eq!(fs::read(&path).unwrap(), bytes);
            let artifact_id = application
                .library
                .processing_export_work(request_id)
                .await
                .unwrap()
                .unwrap()
                .artifact_id
                .unwrap();
            let (status, metadata) = get_json(
                &router,
                &format!("/api/processing-artifacts/{}", artifact_id.as_str()),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            let expected_published = time::OffsetDateTime::from_unix_timestamp(now as i64)
                .unwrap()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap();
            assert_eq!(metadata["publishedAt"], expected_published);
            let work = application
                .library
                .processing_export_work(request_id)
                .await
                .unwrap()
                .unwrap();
            let expected_expiry =
                time::OffsetDateTime::from_unix_timestamp(work.retain_until.unwrap() as i64)
                    .unwrap()
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap();
            assert_eq!(metadata["expiresAt"], expected_expiry);
            assert_eq!(metadata["byteLength"], bytes.len());
            assert_eq!(
                metadata["sha256"],
                format!("{:x}", sha2::Sha256::digest(&bytes))
            );
        } else {
            // An uncertain outcome with work still executing cannot authorize
            // deleting bytes. A later durable cancellation proves no claim.
            drop(executed);
            manager.sweep_expiry().await;
            assert_eq!(fs::read(&path).unwrap(), bytes);
            application
                .library
                .cancel_processing_export(request_id, now)
                .await
                .unwrap();
            manager.sweep_expiry().await;
            assert!(!path.exists());
        }
    }
    // Startup has no live publication claims and still clears orphan leftovers.
    let orphan = manager
        .artifact_path_for_workload("pa-orphan", "development-tiff")
        .unwrap();
    fs::write(&orphan, b"unclaimed interrupted publication").unwrap();
    application.shutdown().await.unwrap();
    drop(router);
    drop(application);
    let (application, _) = export_application(&config).await;
    application.exports.as_ref().unwrap().sweep_expiry().await;
    assert!(!orphan.exists());
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
