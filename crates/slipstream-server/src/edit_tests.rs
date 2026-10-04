use super::*;
use serde_json::json;

fn original() -> ProcessingInput {
    ProcessingInput::Original {
        photo_id: "photo-1".into(),
        source_revision: "source-1".into(),
    }
}

#[test]
fn stateful_set_updates_the_current_step_without_replacing_its_input() {
    let first = build_recipe(
        "photo-1",
        "source-1",
        None,
        "darktable.exposure",
        "ev",
        Some(&json!(0.5)),
        original(),
        false,
    )
    .expect("initial state");
    let step_id = first.current_step_id.clone().expect("current step");
    let updated = build_recipe(
        "photo-1",
        "source-1",
        Some(&first),
        "darktable.exposure",
        "ev",
        Some(&json!(0.75)),
        original(),
        false,
    )
    .expect("updated state");
    assert_eq!(updated.steps.len(), 1);
    assert_eq!(updated.current_step_id, Some(step_id));
    assert_eq!(updated.steps[0].input, first.steps[0].input);
    assert_eq!(
        updated.steps[0].parameters.tree["stack"][0]["params"]["exposure"],
        json!(0.75)
    );
}

#[test]
fn reset_all_uses_the_control_reset_value() {
    let first = build_recipe(
        "photo-1",
        "source-1",
        None,
        "darktable.exposure",
        "ev",
        Some(&json!(0.75)),
        original(),
        false,
    )
    .expect("initial state");
    let reset = build_recipe(
        "photo-1",
        "source-1",
        Some(&first),
        "darktable.exposure",
        "all",
        None,
        original(),
        false,
    )
    .expect("reset state");
    assert_eq!(
        reset.steps[0].parameters.tree["stack"][0]["params"]["exposure"],
        json!(0.0)
    );
}

fn artifact_input() -> ProcessingInput {
    ProcessingInput::Artifact {
        artifact_id: ProcessingArtifactId::new("artifact-1").expect("valid artifact"),
        contract: ProcessingImageContract {
            format: "image/tiff".into(),
            precision: "float32".into(),
            color_space: "prophoto-rgb".into(),
            transfer: "linear".into(),
            geometry: slipstream_core::ProcessingGeometry::new(9504, 6336).expect("valid geometry"),
            encoding: "deflate".into(),
        },
    }
}

#[test]
fn stateful_set_rejects_implicit_engine_switch() {
    let mut existing = build_recipe(
        "photo-1",
        "source-1",
        None,
        "darktable.exposure",
        "ev",
        Some(&json!(0.5)),
        original(),
        false,
    )
    .expect("initial state");
    existing.steps[0].module = ProcessingModuleId::new("spektrafilm").expect("valid module");
    let result = build_recipe(
        "photo-1",
        "source-1",
        Some(&existing),
        "darktable.exposure",
        "ev",
        Some(&json!(0.75)),
        original(),
        false,
    );
    assert!(result.is_err());
}

#[test]
fn artifact_state_requires_rebind_after_current_source_moves() {
    let mut recipe = build_recipe(
        "photo-1",
        "source-1",
        None,
        "darktable.exposure",
        "ev",
        Some(&json!(0.5)),
        original(),
        false,
    )
    .expect("initial state");
    recipe.steps[0].input = artifact_input();
    let read = ComposableEditRecipeRead {
        recipe: Some(recipe.clone()),
        current_source_revision: Some("source-2".into()),
        source_available: true,
    };
    assert!(current_requires_rebind(&read, Some(&recipe)));
}
