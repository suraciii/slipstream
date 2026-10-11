use super::*;
use crate::http::create_router_with_processing;

#[tokio::test]
async fn retained_darktable_preview_refuses_current_and_baseline_without_engine_execution() {
    let (base, mut config) = prepare_fixture();
    approved_raw_fixture(&config.library_root.join("approved.ARW"));
    let engine = FakePhotoEngine::install(&base);
    config.processing = Some(engine.processing_config());
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .remove(0);
    let source = application
        .library
        .composable_edit_recipe_read(&photo_id)
        .await
        .unwrap()
        .unwrap()
        .current_source_revision
        .expect("fixture Original source revision");
    let step_id = slipstream_core::ProcessingStepId::new("retained-darktable").unwrap();
    application
        .library
        .save_composable_edit_recipe(slipstream_core::SaveComposableEditRecipe {
            photo_id: photo_id.clone(),
            request_id: "historical-preview-seed".into(),
            expected_recipe_revision: None,
            expected_source_revision: source.clone(),
            request_intent: None,
            recipe: slipstream_core::ComposableEditRecipe {
                photo_id: photo_id.clone(),
                revision: "historical-draft".into(),
                source_revision: source.clone(),
                current_step_id: Some(step_id.clone()),
                steps: vec![slipstream_core::ProcessingStep {
                    step_id,
                    module: slipstream_core::ProcessingModuleId::new("darktable").unwrap(),
                    input: slipstream_core::ProcessingInput::Original {
                        photo_id: photo_id.clone(),
                        source_revision: source,
                    },
                    parameters: slipstream_core::ProcessingParameterSnapshot::new(
                        "darktable-params-1",
                        serde_json::json!({"stack":[]}),
                    )
                    .unwrap(),
                }],
            },
            automatic_adjustment: None,
        })
        .await
        .unwrap();
    fs::remove_file(config.library_root.join("approved.ARW")).unwrap();
    let router = create_router_with_processing(
        Arc::clone(&application),
        crate::http::open_web_root(config.web_root()),
        config.processing.clone(),
    );
    application.access.seed_test_token();
    let before = application
        .library
        .composable_edit_recipe_read(&photo_id)
        .await
        .unwrap()
        .unwrap()
        .recipe;
    for suffix in ["", "?comparison=baseline"] {
        let (status, refusal) = get_json(
            &router,
            &format!("/api/photos/{photo_id}/processing-preview/retained-darktable{suffix}"),
        )
        .await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(refusal["error"]["code"], "use_darktable_namespace");
        assert_eq!(refusal["error"]["effect"], "none");
    }
    assert_eq!(engine.runs(), 0);
    assert_eq!(
        application
            .library
            .composable_edit_recipe_read(&photo_id)
            .await
            .unwrap()
            .unwrap()
            .recipe,
        before
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
