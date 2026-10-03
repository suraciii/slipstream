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
        vec!["processing", "modules"],
        vec!["processing", "artifact", "artifact-1"],
        vec![
            "processing",
            "artifact-download",
            "artifact-1",
            "--file",
            "out.tiff",
        ],
        vec![
            "photos",
            "processing-recipe",
            "rebind",
            "photo-1",
            "--input",
            "rebind.json",
        ],
        vec![
            "photos",
            "processing-export",
            "photo-1",
            "--input",
            "export.json",
        ],
        vec!["photos", "processing-export-status", "photo-1", "request-1"],
        vec!["photos", "processing-export-cancel", "photo-1", "request-1"],
        vec!["photos", "processing-export-list", "photo-1"],
        vec![
            "photos",
            "processing-export-retry",
            "photo-1",
            "request-1",
            "--input",
            "retry.json",
        ],
        vec![
            "photos",
            "historical-export-download",
            "export-1",
            "--file",
            "retained.jpg",
        ],
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
        vec!["processing", "capability"],
        vec!["photos", "recipe", "get", "photo-1"],
        vec![
            "photos",
            "edit-preview",
            "photo-1",
            "--stage",
            "develop",
            "--file",
            "out.jpg",
        ],
        vec![
            "photos",
            "export",
            "submit",
            "photo-1",
            "--target",
            "film-jpeg",
            "--request-id",
            "r1",
        ],
        vec!["photos", "processing-recipe", "rebind", "photo-1"],
        vec!["photos", "processing-export-list", ""],
        vec![
            "photos",
            "processing-export-list",
            "photo-1",
            "--limit",
            "64",
        ],
        vec!["photos", "processing-export-retry", "photo-1", "request-1"],
        vec![
            "photos",
            "processing-export-retry",
            "photo-1",
            "",
            "--input",
            "retry.json",
        ],
        vec![
            "photos",
            "processing-preview",
            "photo-1",
            "--stage",
            "film",
            "--file",
            "out.jpg",
        ],
        vec!["photos", "historical-export-download", "export-1"],
        vec![
            "photos",
            "historical-export-download",
            "",
            "--file",
            "retained.jpg",
        ],
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
#[test]
fn artifact_download_timeout_uses_the_bounded_cli_value() {
    let default = Cli::try_parse_from([
        "slipstream",
        "processing",
        "artifact-download",
        "artifact-1",
        "--file",
        "out.tiff",
    ])
    .unwrap();
    assert_eq!(default.timeout, DEFAULT_TIMEOUT_SECONDS);

    let configured = Cli::try_parse_from([
        "slipstream",
        "--timeout",
        "7",
        "processing",
        "artifact-download",
        "artifact-1",
        "--file",
        "out.tiff",
    ])
    .unwrap();
    assert_eq!(configured.timeout, 7);
    assert!(Cli::try_parse_from(["slipstream", "--timeout", "0", "status"]).is_err());
    assert!(Cli::try_parse_from(["slipstream", "--timeout", "301", "status"]).is_err());
}
