use super::*;
use clap::Parser;

#[test]
fn trash_empty_accepts_an_optional_recovery_identity() {
    let valid = Cli::try_parse_from([
        "slipstream",
        "trash",
        "empty",
        "--operation-id",
        "00000000-0000-4000-8000-000000000777",
    ])
    .unwrap();
    assert!(validate_command(&valid.command).is_ok());
    let invalid = Cli::try_parse_from([
        "slipstream",
        "trash",
        "empty",
        "--operation-id",
        "not-an-id",
    ])
    .unwrap();
    assert!(validate_command(&invalid.command).is_err());
    assert!(Cli::try_parse_from(["slipstream", "trash", "empty"]).is_ok());
}

#[test]
fn restore_parser_supports_select_all_and_exclusions() {
    use clap::CommandFactory;

    for args in [
        vec!["slipstream", "photos", "restore", "op"],
        vec![
            "slipstream",
            "photos",
            "restore",
            "op",
            "--input",
            "markers.json",
        ],
        vec!["slipstream", "photos", "restore", "op", "--all"],
        vec![
            "slipstream",
            "photos",
            "restore",
            "op",
            "--all",
            "--exclude-input",
            "ids.json",
        ],
    ] {
        assert!(Cli::try_parse_from(args).is_ok());
    }
    for args in [
        vec![
            "slipstream",
            "photos",
            "restore",
            "op",
            "--exclude-input",
            "ids.json",
        ],
        vec![
            "slipstream",
            "photos",
            "restore",
            "op",
            "--all",
            "--input",
            "markers.json",
        ],
        vec![
            "slipstream",
            "photos",
            "restore",
            "op",
            "--all",
            "--exclude-input",
        ],
    ] {
        assert!(Cli::try_parse_from(args).is_err());
    }
    let help = Cli::command()
        .find_subcommand_mut("photos")
        .unwrap()
        .find_subcommand_mut("restore")
        .unwrap()
        .render_long_help()
        .to_string();
    assert!(help.contains("--all"));
    assert!(help.contains("--exclude-input"));
    assert!(help.contains("--input"));
}

#[test]
fn restore_all_rejects_invalid_or_excluded_results() {
    let identity = MutationIdentity::bare(Operation::PhotosRestore);
    let exclusions = vec!["excluded".to_owned()];
    for response in [
        json!({"operationId":"wrong", "counts":{"restored":0,"alreadyActive":0,"changedElsewhere":0,"missing":0}, "results":[]}),
        json!({"operationId":"op", "counts":{"restored":1,"alreadyActive":0,"changedElsewhere":0,"missing":0}, "results":[{"photoId":"excluded","outcome":"restored"}]}),
        json!({"operationId":"op", "counts":{"restored":2,"alreadyActive":0,"changedElsewhere":0,"missing":0}, "results":[{"photoId":"a","outcome":"restored"},{"photoId":"a","outcome":"restored"}]}),
        json!({"operationId":"op", "counts":{"restored":0,"alreadyActive":0,"changedElsewhere":0,"missing":0}, "results":[{"photoId":"a","outcome":"restored"}]}),
    ] {
        let wire = serde_json::from_value(response).unwrap();
        let failure =
            confirmed_restore_result(&identity, "op", None, &exclusions, wire).unwrap_err();
        assert_eq!(failure.payload.code, "outcome_unknown");
    }
}

const PHOTO: &str = "00000000-0000-4000-8000-000000000001";
const ORIGINAL: &str = "00000000-0000-4000-8000-000000000002";
const OPERATION: &str = "00000000-0000-4000-8000-000000000003";

fn item() -> Value {
    json!({"photoId": PHOTO, "removedAtMs": 42, "originalId": ORIGINAL, "originalLocation": "folder/quote\"\n雪.jpg"})
}

#[test]
fn parser_accepts_new_commands_and_retires_old_commands() {
    for args in [
        vec!["trash", "list", "--start", "5", "--limit", "2"],
        vec!["trash", "list", "--cursor", "token"],
        vec!["trash", "empty"],
        vec!["trash", "cleanup", OPERATION, "--input", "file"],
        vec!["trash", "cleanup-operation", OPERATION],
    ] {
        let mut argv = vec!["slipstream"];
        argv.extend(args);
        assert!(Cli::try_parse_from(argv).is_ok());
    }
    for old in ["review", "delete", "operation"] {
        assert!(Cli::try_parse_from(["slipstream", "trash", old]).is_err());
    }
    assert!(
        Cli::try_parse_from([
            "slipstream",
            "trash",
            "list",
            "--cursor",
            "token",
            "--start",
            "0"
        ])
        .is_err()
    );
    assert!(
        Cli::try_parse_from([
            "slipstream",
            "trash",
            "list",
            "--cursor",
            "token",
            "--limit",
            "1"
        ])
        .is_err()
    );
}

#[test]
fn cleanup_input_is_closed_bounded_and_lossless() {
    let valid = json!({"items": [item()]});
    let parsed = parse_trash_cleanup_input(&serde_json::to_vec(&valid).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), valid);
    let mut cases = vec![
        json!({"items": []}),
        json!({"items": [item(), item()]}),
        json!({"items": [item()], "extra": true}),
        json!({"items": vec![item(); 5001]}),
    ];
    for (key, bad) in [
        ("removedAtMs", json!(-1)),
        ("originalLocation", json!("../outside")),
        ("photoId", json!("bad")),
        ("originalId", json!("")),
    ] {
        let mut broken = item();
        broken[key] = bad;
        cases.push(json!({"items": [broken]}));
    }
    let mut extra = item();
    extra["extra"] = json!(true);
    cases.push(json!({"items": [extra]}));
    for case in cases {
        assert!(
            parse_trash_cleanup_input(&serde_json::to_vec(&case).unwrap()).is_err(),
            "{case}"
        );
    }
    let duplicate_key = format!(
        "{{\"items\":[{{\"photoId\":\"{PHOTO}\",\"photoId\":\"{PHOTO}\",\"removedAtMs\":42,\"originalId\":\"{ORIGINAL}\",\"originalLocation\":\"a.jpg\"}}]}}"
    );
    assert!(parse_trash_cleanup_input(duplicate_key.as_bytes()).is_err());
}

#[test]
fn cleanup_receipt_accounts_for_each_reviewed_item() {
    let input = parse_trash_cleanup_input(&serde_json::to_vec(&json!({"items":[item()]})).unwrap())
        .unwrap();
    let receipt = json!({"operationId": OPERATION, "reviewed": 1,
        "counts": {"cleaned": 1, "changed": 0, "present": 0, "blocked": 0, "unavailable": 0, "inspectionFailed": 0},
        "items": [{"photoId": PHOTO, "originalLocation": item()["originalLocation"], "originalKind": "jpeg", "state": "cleaned", "message": null, "blockers": []}]});
    assert!(crate::trash::valid_cleanup_result(
        &receipt,
        OPERATION,
        Some(&input)
    ));
    let mut broken = receipt.clone();
    broken["counts"]["cleaned"] = json!(0);
    assert!(!crate::trash::valid_cleanup_result(
        &broken,
        OPERATION,
        Some(&input)
    ));
    broken = receipt.clone();
    broken["items"][0]["state"] = json!("deleted");
    assert!(!crate::trash::valid_cleanup_result(
        &broken,
        OPERATION,
        Some(&input)
    ));
    assert!(!crate::trash::valid_cleanup_result(
        &receipt, ORIGINAL, None
    ));
}

#[test]
fn text_listing_escapes_control_characters_and_preserves_json() {
    let location = "folder/quote\"\n雪.jpg";
    let data = json!({"libraryRoot":"/srv/photos", "namespace":"service", "photos":[{"photoId":PHOTO,"removedAtMs":42,"originalKind":"jpeg","originalLocation":location}]});
    let envelope = Envelope::success(data.clone());
    let text = render_text(&envelope);
    assert!(text.contains("Library Folder: /srv/photos\n"));
    assert!(text.contains("marker=42 location=\"folder/quote\\\"\\n雪.jpg\""));
    assert!(!text.contains("quote\"\n雪.jpg"));
    assert_eq!(serde_json::to_value(&envelope).unwrap()["data"], data);
}

#[test]
fn unknown_cleanup_and_conflict_keep_attempt_identity() {
    let identity = MutationIdentity {
        operation: Operation::TrashCleanup,
        photo_ids: vec![PHOTO.into()],
        album_id: None,
        album_name: None,
        mappings: vec![json!({"operationId": OPERATION})],
    };
    assert_eq!(
        CommandFailure::unknown(&identity).payload.details["operationId"],
        OPERATION
    );
    let failure = validated_route_failure(
        ErrorPayload {
            code: "conflict".into(),
            message: "Intent changed".into(),
            effect: "none".into(),
            details: json!({"operationId": OPERATION}),
        },
        Operation::TrashCleanup,
        "",
    )
    .unwrap();
    assert_eq!(failure.exit_code, 4);
    let failure = validated_route_failure(
        ErrorPayload {
            code: "outcome_unknown".into(),
            message: "Unknown".into(),
            effect: "unknown".into(),
            details: json!({"operationId": OPERATION}),
        },
        Operation::TrashCleanupReceipt,
        "",
    )
    .unwrap();
    assert_eq!(failure.exit_code, 7);
}
