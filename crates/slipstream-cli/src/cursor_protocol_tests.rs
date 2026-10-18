use super::*;

#[test]
fn cursor_expiry_details_are_required_and_route_specific() {
    let refusal = |code: &str, details: Value| ErrorPayload {
        code: code.to_owned(),
        message: "Read the reported mappings and review again.".to_owned(),
        effect: "none".to_owned(),
        details,
    };
    let mapped = |code: &str, details: Value, operation: Operation| {
        validated_route_failure(refusal(code, details), operation, "")
    };

    for (kind, reason, operation) in [
        ("folder", "publication_replaced", Operation::FoldersList),
        ("album", "idle_or_evicted", Operation::AlbumsList),
        ("photo", "process_restarted", Operation::PhotosList),
        (
            "darktable",
            "idle_or_evicted",
            Operation::Darktable("darktable-edit-instances-list"),
        ),
        (
            "workspace",
            "idle_or_evicted",
            Operation::Workspace("workspace-files"),
        ),
    ] {
        let failure = mapped(
            "cursor_expired",
            json!({"cursorKind": kind, "reason": reason}),
            operation,
        )
        .expect("an expired continuation is confirmed");
        assert_eq!(failure.exit_code, 6, "for {kind}/{reason}");
    }

    for kind in [
        "folder",
        "album",
        "photo",
        "unavailable",
        "mappings",
        "workspace",
        "darktable",
    ] {
        assert!(
            mapped(
                "cursor_expired",
                json!({"cursorKind": kind, "reason": "idle_or_evicted"}),
                Operation::Darktable("darktable-edit-instances-list"),
            )
            .is_some(),
            "for {kind}"
        );
        assert!(
            mapped(
                "cursor_expired",
                json!({"cursorKind": kind}),
                Operation::Darktable("darktable-edit-instances-list"),
            )
            .is_none(),
            "an expiry without a reason is not confirmed: {kind}"
        );
        assert!(
            mapped(
                "cursor_expired",
                json!({"reason": "idle_or_evicted"}),
                Operation::Darktable("darktable-edit-instances-list"),
            )
            .is_none(),
            "an expiry without a cursor kind is not confirmed: {kind}"
        );
    }
    for reason in [
        "publication_replaced",
        "process_restarted",
        "idle_or_evicted",
    ] {
        assert!(
            mapped(
                "cursor_expired",
                json!({"cursorKind": "darktable", "reason": reason}),
                Operation::Darktable("darktable-edit-instances-list"),
            )
            .is_some(),
            "for {reason}"
        );
        assert!(
            mapped(
                "cursor_expired",
                json!({"cursorKind": "darktable", "reason": "unknown_reason"}),
                Operation::Darktable("darktable-edit-instances-list"),
            )
            .is_none(),
            "an undeclared reason is not confirmed"
        );
    }
    assert!(
        mapped(
            "cursor_expired",
            json!({}),
            Operation::Darktable("darktable-edit-instances-list"),
        )
        .is_none(),
        "generic empty details are not a confirmed expiry"
    );
}
