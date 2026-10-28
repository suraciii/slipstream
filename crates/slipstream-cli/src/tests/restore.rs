use super::*;
use clap::Parser;

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
