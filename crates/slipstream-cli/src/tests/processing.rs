use super::*;
#[test]
fn composable_recipe_conflicts_validate_the_retained_recipe_details() {
    let composable_conflict = |details: Value| ErrorPayload {
        code: "recipe_conflict".to_owned(),
        message: "The expected composable recipe revision is no longer current".to_owned(),
        effect: "none".to_owned(),
        details,
    };
    let retained = json!({
        "photoId": "p1",
        "revision": "recipe-9",
        "sourceRevision": "source-3",
        "currentStepId": "develop-1",
        "steps": [{
            "stepId": "develop-1",
            "module": "darktable",
            "input": {"kind": "original", "photoId": "p1", "sourceRevision": "source-3"},
            "parameters": {"schemaVersion": "darktable-params-1", "tree": {"stack": []}},
        }],
    });
    let failure = validated_route_failure(
        composable_conflict(retained.clone()),
        Operation::PhotosProcessingRecipeSave,
        "",
    )
    .expect("the retained recipe is a confirmed conflict");
    assert_eq!(failure.exit_code, 4);
    // The zero-step recipe a first save conflicts with is also recovery
    // evidence.
    assert!(
        validated_route_failure(
            composable_conflict(json!({
                "photoId": "p1",
                "revision": "recipe-1",
                "sourceRevision": "source-3",
                "currentStepId": null,
                "steps": [],
            })),
            Operation::PhotosProcessingRecipeSave,
            ""
        )
        .is_some()
    );
    // A current step the recipe does not carry is not recovery evidence.
    let mut stray = retained.clone();
    stray["currentStepId"] = json!("develop-2");
    assert!(
        validated_route_failure(
            composable_conflict(stray),
            Operation::PhotosProcessingRecipeSave,
            ""
        )
        .is_none()
    );
    // Unstructured details never confirm a refusal.
    assert!(
        validated_route_failure(
            composable_conflict(json!({"recipe": "opaque"})),
            Operation::PhotosProcessingRecipeSave,
            ""
        )
        .is_none()
    );
}

#[test]
fn composable_processing_commands_parse_their_closed_forms() {
    for arguments in [
        vec!["photos", "processing-recipe", "get", "photo-1"],
        vec![
            "photos",
            "processing-recipe",
            "save",
            "photo-1",
            "--input",
            "recipe.json",
        ],
        vec![
            "photos",
            "processing-preview",
            "photo-1",
            "--step",
            "develop-1",
            "--file",
            "out.jpg",
        ],
    ] {
        assert!(
            Cli::try_parse_from(std::iter::once("slipstream").chain(arguments.iter().copied()))
                .is_ok()
        );
    }
    for arguments in [
        vec!["photos", "processing-recipe", "get", ""],
        vec!["photos", "processing-recipe", "save", "photo-1"],
        vec!["photos", "processing-recipe", "inspect", "photo-1"],
        vec![
            "photos",
            "processing-preview",
            "photo-1",
            "--file",
            "out.jpg",
        ],
        vec![
            "photos",
            "processing-preview",
            "photo-1",
            "--step",
            "",
            "--file",
            "out.jpg",
        ],
    ] {
        assert!(
            Cli::try_parse_from(std::iter::once("slipstream").chain(arguments.iter().copied()))
                .is_err()
        );
    }
}
