use super::*;
use clap::{CommandFactory, Parser};
use serde_json::json;

#[test]
fn help_and_version_are_offline_parser_results() {
    assert_eq!(
        Cli::try_parse_from(["slipstream", "--help"])
            .unwrap_err()
            .kind(),
        clap::error::ErrorKind::DisplayHelp
    );
    assert_eq!(
        Cli::try_parse_from(["slipstream", "--version"])
            .unwrap_err()
            .kind(),
        clap::error::ErrorKind::DisplayVersion
    );
    let photo_help = Cli::command()
        .find_subcommand_mut("photos")
        .expect("photos command")
        .render_help()
        .to_string();
    assert!(photo_help.contains("edit"));
    assert!(!photo_help.contains("processing-recipe"));
    assert_eq!(
        Cli::try_parse_from(["slipstream", "photos", "--help"])
            .unwrap_err()
            .kind(),
        clap::error::ErrorKind::DisplayHelp
    );
}

#[test]
fn stateful_edit_commands_use_primary_operation_identities() {
    for (arguments, expected) in [
        (
            vec!["slipstream", "photos", "edit", "get", "photo-1"],
            "photos-edit-get",
        ),
        (
            vec![
                "slipstream",
                "photos",
                "edit",
                "set",
                "photo-1",
                "darktable.exposure",
                "ev",
                "0.5",
                "--request",
                "request-1",
            ],
            "photos-edit-set",
        ),
        (
            vec![
                "slipstream",
                "photos",
                "edit",
                "reset",
                "photo-1",
                "darktable.exposure",
                "all",
                "--revision",
                "r1",
                "--request",
                "request-2",
            ],
            "photos-edit-reset",
        ),
        (
            vec![
                "slipstream",
                "photos",
                "edit",
                "preview",
                "photo-1",
                "--file",
                "preview.png",
            ],
            "photos-edit-preview",
        ),
        (
            vec![
                "slipstream",
                "photos",
                "edit",
                "export",
                "photo-1",
                "--revision",
                "r1",
                "--request",
                "request-3",
            ],
            "photos-edit-export",
        ),
        (
            vec![
                "slipstream",
                "photos",
                "edit",
                "export-status",
                "photo-1",
                "request-3",
            ],
            "photos-edit-export-status",
        ),
    ] {
        let cli = Cli::try_parse_from(arguments).expect("stateful Edit command");
        assert_eq!(command_operation(&cli.command).wire(), expected);
    }
}

#[test]
fn development_surface_refusals_map_onto_the_closed_exit_codes() {
    let refusal = |code: &str| ErrorPayload {
        code: code.to_owned(),
        message: "Check the request and try again.".to_owned(),
        effect: "none".to_owned(),
        details: json!({}),
    };
    let mapped = |code: &str| {
        validated_route_failure(refusal(code), Operation::PhotosProcessingExport, "")
            .unwrap_or_else(|| panic!("{code} must map to a confirmed failure"))
    };
    assert_eq!(mapped("invalid_settings").exit_code, 2);
    assert_eq!(mapped("invalid_recipe").exit_code, 2);
    assert_eq!(mapped("incompatible_input").exit_code, 2);
    assert_eq!(mapped("unsupported_photo").exit_code, 2);
    assert_eq!(mapped("unknown_photo").exit_code, 3);
    assert_eq!(mapped("unknown_export").exit_code, 3);
    assert_eq!(mapped("missing_recipe").exit_code, 3);
    assert_eq!(mapped("unknown_step").exit_code, 3);
    assert_eq!(mapped("unknown_module").exit_code, 3);
    assert_eq!(mapped("recipe_conflict").exit_code, 4);
    assert_eq!(mapped("source_changed").exit_code, 4);
    assert_eq!(mapped("requires_rebind").exit_code, 4);
    assert_eq!(mapped("request_conflict").exit_code, 4);
    assert_eq!(mapped("step_not_current").exit_code, 4);
    assert_eq!(mapped("export_conflict").exit_code, 4);
    assert_eq!(mapped("output_unavailable").exit_code, 4);
    assert_eq!(mapped("export_expired").exit_code, 6);
    assert_eq!(mapped("receipt_expired").exit_code, 4);
    assert_eq!(mapped("artifact_expired").exit_code, 6);
    let control_refusal = |code: &str| {
        validated_route_failure(
            ErrorPayload {
                code: code.to_owned(),
                message: "The control is not qualified.".to_owned(),
                effect: "none".to_owned(),
                details: json!({"target": "darktable.exposure", "control": "ev"}),
            },
            Operation::PhotosEditSet,
            "",
        )
        .unwrap_or_else(|| panic!("{code} must map to a confirmed primary Edit refusal"))
    };
    assert_eq!(control_refusal("unsupported_control").exit_code, 2);
    assert_eq!(control_refusal("invalid_value").exit_code, 2);
    assert_eq!(
        mapped("missing_revision").exit_code,
        2,
        "missing revision is a confirmed input refusal",
    );
    assert_eq!(mapped("invalid_edit").exit_code, 2);
    let edit_conflict = validated_route_failure(
        ErrorPayload {
            code: "edit_conflict".to_owned(),
            message: "The expected Edit revision is no longer current.".to_owned(),
            effect: "none".to_owned(),
            details: json!({"edit": {}}),
        },
        Operation::PhotosEditSet,
        "",
    )
    .expect("edit conflict must remain a confirmed refusal");
    assert_eq!(edit_conflict.exit_code, 4);
    assert_eq!(mapped("processing_unavailable").exit_code, 6);
    assert_eq!(mapped("module_parameters_unavailable").exit_code, 6);
    assert_eq!(mapped("source_unavailable").exit_code, 6);
    assert_eq!(mapped("resource_unavailable").exit_code, 6);
    assert_eq!(mapped("retained_output_full").exit_code, 6);
    // A possibly admitted write keeps its unknown outcome; the mapped
    // confirmed refusals keep the service's message and effect.
    assert!(
        validated_route_failure(
            refusal("outcome_unknown"),
            Operation::PhotosProcessingExport,
            "",
        )
        .is_none()
    );
    let confirmed = mapped("export_conflict");
    assert_eq!(confirmed.payload.effect, "none");
    assert_eq!(confirmed.payload.details, json!({}));
}
