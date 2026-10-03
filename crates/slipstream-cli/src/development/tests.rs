use super::*;

fn refuses(failure: CommandFailure, argument: &str) {
    assert_eq!(failure.exit_code, 2, "for {failure:?}");
    assert_eq!(failure.payload.code, "invalid_input");
    assert_eq!(failure.payload.details["argument"], argument);
}

fn origin() -> Url {
    Url::parse("https://slipstream.example").expect("origin parses")
}

fn identity(operation: Operation) -> MutationIdentity {
    MutationIdentity {
        operation,
        photo_ids: vec!["p1".to_owned()],
        album_id: None,
        album_name: None,
        mappings: Vec::new(),
    }
}

mod composable;
