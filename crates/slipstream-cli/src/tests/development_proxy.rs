use super::*;
#[test]
fn development_proxy_commands_parse_their_closed_forms() {
    for arguments in [
        vec!["photos", "proxy", "get", "photo-1"],
        vec![
            "photos",
            "proxy",
            "create",
            "photo-1",
            "--input",
            "create.json",
        ],
        vec!["photos", "proxy", "remove", "photo-1"],
    ] {
        assert!(
            Cli::try_parse_from(std::iter::once("slipstream").chain(arguments.iter().copied()))
                .is_ok()
        );
    }
    for arguments in [
        vec!["photos", "proxy", "get", ""],
        vec!["photos", "proxy", "create", "photo-1"],
        vec!["photos", "proxy", "remove", ""],
        vec!["photos", "proxy", "inspect", "photo-1"],
    ] {
        assert!(
            Cli::try_parse_from(std::iter::once("slipstream").chain(arguments.iter().copied()))
                .is_err()
        );
    }
}
