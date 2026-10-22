use super::*;
#[test]
fn recovery_commands_parse_their_closed_forms() {
    for arguments in [
        vec!["recovery", "unavailable"],
        vec!["recovery", "unavailable", "--limit", "1"],
        vec!["recovery", "unavailable", "--limit", "60"],
        vec!["recovery", "unavailable", "--cursor", "opaque"],
        vec![
            "recovery",
            "propose",
            "--old-prefix",
            "shoot",
            "--new-prefix",
            "moved",
        ],
        vec![
            "recovery",
            "propose",
            "--old-prefix",
            "",
            "--new-prefix",
            "",
        ],
        vec![
            "recovery",
            "propose",
            "--old-prefix",
            "shoot",
            "--new-prefix",
            "moved",
            "--limit",
            "2",
        ],
        vec!["recovery", "propose", "--cursor", "opaque"],
        vec![
            "recovery",
            "propose",
            "--original-id",
            "00000000-0000-4000-8000-000000000001",
            "--new-location",
            "moved/a.JPG",
        ],
        vec!["recovery", "apply", "--input", "apply.json"],
    ] {
        assert!(
            Cli::try_parse_from(std::iter::once("slipstream").chain(arguments.iter().copied()))
                .is_ok(),
            "must parse: {arguments:?}"
        );
    }
    for arguments in [
        vec!["recovery", "unavailable", "--limit", "0"],
        vec!["recovery", "unavailable", "--limit", "61"],
        vec!["recovery", "unavailable", "--limit", "abc"],
        // A continuation accepts only --cursor and the global options.
        vec![
            "recovery",
            "unavailable",
            "--cursor",
            "opaque",
            "--limit",
            "2",
        ],
        vec!["recovery", "propose", "--cursor", "opaque", "--limit", "2"],
        vec![
            "recovery",
            "propose",
            "--cursor",
            "opaque",
            "--old-prefix",
            "shoot",
            "--new-prefix",
            "moved",
        ],
        vec![
            "recovery",
            "propose",
            "--cursor",
            "opaque",
            "--original-id",
            "00000000-0000-4000-8000-000000000001",
            "--new-location",
            "moved/a.JPG",
        ],
        vec![
            "recovery",
            "propose",
            "--old-prefix",
            "shoot",
            "--new-prefix",
            "moved",
            "--original-id",
            "00000000-0000-4000-8000-000000000001",
            "--new-location",
            "moved/a.JPG",
        ],
        vec![
            "recovery",
            "propose",
            "--original-id",
            "00000000-0000-4000-8000-000000000001",
            "--new-location",
            "moved/a.JPG",
            "--limit",
            "2",
        ],
        vec!["recovery", "apply"],
    ] {
        assert!(
            Cli::try_parse_from(std::iter::once("slipstream").chain(arguments.iter().copied()))
                .is_err(),
            "must reject: {arguments:?}"
        );
    }
}

#[test]
fn recovery_propose_forms_validate_before_network_access() {
    let propose = |arguments: &[&str]| {
        let full = std::iter::once("slipstream")
            .chain(["recovery", "propose"])
            .chain(arguments.iter().copied());
        match Cli::try_parse_from(full).unwrap().command {
            Command::Recovery {
                command: RecoveryCommand::Propose(args),
            } => args,
            other => panic!("expected recovery propose, got {other:?}"),
        }
    };
    let invalid = |arguments: &[&str], argument: &str| {
        let failure = validate_command(&Command::Recovery {
            command: RecoveryCommand::Propose(propose(arguments)),
        })
        .unwrap_err();
        assert_eq!(failure.exit_code, 2, "for {arguments:?}");
        assert_eq!(failure.payload.code, "invalid_input", "for {arguments:?}");
        assert_eq!(
            failure.payload.details["argument"], argument,
            "for {arguments:?}"
        );
    };
    // Missing selector, half of one form, and both forms together.
    invalid(&[], "arguments");
    invalid(&["--old-prefix", "shoot"], "old-prefix");
    invalid(&["--new-location", "moved/a.JPG"], "original-id");
    // Malformed prefixes, ids, and locations.
    invalid(
        &["--old-prefix", "/shoot", "--new-prefix", "moved"],
        "old-prefix",
    );
    invalid(
        &["--old-prefix", "shoot/", "--new-prefix", "moved"],
        "old-prefix",
    );
    invalid(
        &[
            "--original-id",
            "not-an-id",
            "--new-location",
            "moved/a.JPG",
        ],
        "original-id",
    );
    invalid(
        &[
            "--original-id",
            "00000000-0000-4000-8000-000000000001",
            "--new-location",
            "/moved/a.JPG",
        ],
        "new-location",
    );
    // Both complete forms and the empty prefix stay valid.
    for arguments in [
        vec!["--old-prefix", "shoot", "--new-prefix", "moved"],
        vec!["--old-prefix", "", "--new-prefix", ""],
        vec![
            "--original-id",
            "00000000-0000-4000-8000-000000000001",
            "--new-location",
            "moved/a.JPG",
        ],
    ] {
        assert!(
            validate_command(&Command::Recovery {
                command: RecoveryCommand::Propose(propose(&arguments)),
            })
            .is_ok(),
            "must accept {arguments:?}"
        );
    }
}

#[tokio::test]
async fn recovery_apply_document_validates_completely_before_any_request() {
    let original = "00000000-0000-4000-8000-000000000001";
    let other = "00000000-0000-4000-8000-000000000002";
    let retire = "00000000-0000-4000-8000-000000000003";
    let mapping = "reviewed-mapping-identity";
    let write = |name: &str, document: &Value| {
        let path = std::env::temp_dir().join(format!("slipstream-recovery-apply-{name}.json"));
        std::fs::write(&path, serde_json::to_vec(document).unwrap()).unwrap();
        path.to_str().unwrap().to_owned()
    };
    let valid = json!({
        "mappings": [
            {
                "originalId": original,
                "newLocation": "moved/a.JPG",
                "mappingId": mapping,
                "confirmUnverifiedContent": true,
                "retirePhotoId": retire
            },
            {
                "originalId": other,
                "newLocation": "moved/b.JPG",
                "mappingId": "reviewed-mapping-identity-2"
            }
        ]
    });
    let prepared = read_recovery_apply(&write("valid", &valid)).await.unwrap();
    assert_eq!(
        prepared.body,
        json!({
            "mappings": [
                {
                    "originalId": original,
                    "newLocation": "moved/a.JPG",
                    "mappingId": mapping,
                    "confirmUnverifiedContent": true,
                    "retirePhotoId": retire
                },
                {
                    "originalId": other,
                    "newLocation": "moved/b.JPG",
                    "mappingId": "reviewed-mapping-identity-2"
                }
            ]
        })
    );
    assert_eq!(prepared.identities.len(), 2);
    assert_eq!(prepared.identities[0]["mappingId"], mapping);

    let oversized = json!({
        "mappings": (0..=MAXIMUM_RECOVERY_APPLY).map(|index| json!({
            "originalId": format!("00000000-0000-4000-8000-{index:012}"),
            "newLocation": format!("moved/{index}.JPG"),
            "mappingId": format!("mapping-{index}")
        })).collect::<Vec<_>>()
    });
    for (name, expected_code, document) in [
        ("not-json", "invalid_input", json!("[]")),
        ("no-mappings", "invalid_input", json!({})),
        (
            "extra-key",
            "invalid_input",
            json!({"mappings": [], "force": true}),
        ),
        ("empty", "invalid_input", json!({"mappings": []})),
        (
            "not-object",
            "invalid_input",
            json!({"mappings": [original]}),
        ),
        (
            "unknown-key",
            "invalid_input",
            json!({"mappings": [{"originalId": original, "newLocation": "moved/a.JPG", "mappingId": mapping, "force": true}]}),
        ),
        (
            "missing-identity",
            "invalid_input",
            json!({"mappings": [{"originalId": original, "newLocation": "moved/a.JPG"}]}),
        ),
        (
            "empty-mapping-id",
            "invalid_input",
            json!({"mappings": [{"originalId": original, "newLocation": "moved/a.JPG", "mappingId": ""}]}),
        ),
        (
            "malformed-id",
            "invalid_input",
            json!({"mappings": [{"originalId": "not-an-id", "newLocation": "moved/a.JPG", "mappingId": mapping}]}),
        ),
        (
            "malformed-location",
            "invalid_input",
            json!({"mappings": [{"originalId": original, "newLocation": "/moved/a.JPG", "mappingId": mapping}]}),
        ),
        (
            "dot-segment",
            "invalid_input",
            json!({"mappings": [{"originalId": original, "newLocation": "moved/../a.JPG", "mappingId": mapping}]}),
        ),
        (
            "bad-confirmation",
            "invalid_input",
            json!({"mappings": [{"originalId": original, "newLocation": "moved/a.JPG", "mappingId": mapping, "confirmUnverifiedContent": "yes"}]}),
        ),
        (
            "bad-retire",
            "invalid_input",
            json!({"mappings": [{"originalId": original, "newLocation": "moved/a.JPG", "mappingId": mapping, "retirePhotoId": "not-a-photo"}]}),
        ),
        (
            "duplicate-original",
            "invalid_input",
            json!({"mappings": [
                {"originalId": original, "newLocation": "moved/a.JPG", "mappingId": mapping},
                {"originalId": original, "newLocation": "moved/b.JPG", "mappingId": mapping}
            ]}),
        ),
        (
            "duplicate-location",
            "invalid_input",
            json!({"mappings": [
                {"originalId": original, "newLocation": "moved/a.JPG", "mappingId": mapping},
                {"originalId": other, "newLocation": "moved/a.JPG", "mappingId": mapping}
            ]}),
        ),
        ("oversized", "limit_exceeded", oversized),
    ] {
        let failure = read_recovery_apply(&write(name, &document))
            .await
            .unwrap_err();
        assert_eq!(failure.exit_code, 2, "{name}");
        assert_eq!(failure.payload.code, expected_code, "{name}");
        assert_eq!(failure.payload.effect, "none", "{name}");
    }
}

#[test]
fn recovery_list_items_validate_against_their_closed_shapes() {
    let origin = Url::parse("https://library.test").unwrap();
    let item = json!({
        "state": "unavailable",
        "originalId": "00000000-0000-4000-8000-000000000001",
        "photoId": "00000000-0000-4000-8000-000000000002",
        "location": "shoot/a.JPG",
        "kind": "jpeg",
        "rating": 3,
        "selectionState": "picked",
        "fingerprintEnrolled": false,
        "albumCount": 1,
        "webUrl": "/?photoId=00000000-0000-4000-8000-000000000002"
    });
    let wire: RecoveryItemWire = serde_json::from_value(item.clone()).unwrap();
    let value = recovery_item_value(wire, &origin).unwrap();
    assert_eq!(value["state"], "unavailable");
    assert_eq!(
        value["webUrl"],
        "https://library.test/?photoId=00000000-0000-4000-8000-000000000002"
    );
    // A value outside the closed sets never deserializes into a printed
    // item, and a substantiated-field failure fails the whole read.
    assert!(
        serde_json::from_value::<RecoveryItemWire>(json!({
            "state": "vanished", "originalId": "00000000-0000-4000-8000-000000000001",
            "photoId": "00000000-0000-4000-8000-000000000002", "location": "shoot/a.JPG",
            "kind": "jpeg", "rating": 3, "selectionState": "picked",
            "fingerprintEnrolled": false, "albumCount": 1, "webUrl": "/?photoId=x"
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<RecoveryItemWire>(json!({
            "state": "unavailable", "originalId": "00000000-0000-4000-8000-000000000001",
            "photoId": "00000000-0000-4000-8000-000000000002", "location": "shoot/a.JPG",
            "kind": "tiff", "rating": 3, "selectionState": "picked",
            "fingerprintEnrolled": false, "albumCount": 1, "webUrl": "/?photoId=x"
        }))
        .is_err()
    );
    let over_rated = serde_json::from_value::<RecoveryItemWire>(json!({
        "state": "available", "originalId": "00000000-0000-4000-8000-000000000001",
        "photoId": "00000000-0000-4000-8000-000000000002", "location": "shoot/a.JPG",
        "kind": "raw", "rating": 6, "selectionState": "unflagged",
        "fingerprintEnrolled": true, "albumCount": 0, "webUrl": "/?photoId=x"
    }))
    .unwrap();
    assert!(recovery_item_value(over_rated, &origin).is_err());

    let mapping = json!({
        "mappingId": mapping_id(),
        "originalId": "00000000-0000-4000-8000-000000000001",
        "photoId": "00000000-0000-4000-8000-000000000002",
        "fromLocation": "shoot/a.JPG",
        "toLocation": "moved/a.JPG",
        "kind": "jpeg",
        "outcome": "matched",
        "verified": false,
        "blockedReason": null,
        "retire": null
    });
    let wire: RecoveryMappingWire = serde_json::from_value(mapping).unwrap();
    let value = recovery_mapping_value(wire).unwrap();
    assert_eq!(value["outcome"], "matched");
    assert_eq!(value["blockedReason"], serde_json::Value::Null);
    assert!(
        serde_json::from_value::<RecoveryMappingWire>(json!({
            "mappingId": mapping_id(), "originalId": "00000000-0000-4000-8000-000000000001",
            "photoId": "00000000-0000-4000-8000-000000000002",
            "fromLocation": "shoot/a.JPG", "toLocation": "moved/a.JPG",
            "kind": "jpeg", "outcome": "relocated", "verified": false,
            "blockedReason": null, "retire": null
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<RecoveryMappingWire>(json!({
            "mappingId": mapping_id(), "originalId": "00000000-0000-4000-8000-000000000001",
            "photoId": "00000000-0000-4000-8000-000000000002",
            "fromLocation": "shoot/a.JPG", "toLocation": "moved/a.JPG",
            "kind": "jpeg", "outcome": "matched", "verified": false,
            "blockedReason": "destination-full", "retire": null
        }))
        .is_err()
    );
    let occupied = serde_json::from_value::<RecoveryMappingWire>(json!({
        "mappingId": mapping_id(), "originalId": "00000000-0000-4000-8000-000000000001",
        "photoId": "00000000-0000-4000-8000-000000000002",
        "fromLocation": "shoot/a.JPG", "toLocation": "moved/a.JPG",
        "kind": "jpeg", "outcome": "occupied", "verified": true,
        "blockedReason": "destination-in-use",
        "retire": {
            "photoId": "00000000-0000-4000-8000-000000000003",
            "originalId": "00000000-0000-4000-8000-000000000004",
            "location": "moved/a.JPG"
        }
    }))
    .unwrap();
    let value = recovery_mapping_value(occupied).unwrap();
    assert_eq!(value["blockedReason"], "destination-in-use");
    assert_eq!(
        value["retire"]["photoId"],
        "00000000-0000-4000-8000-000000000003"
    );
}

fn mapping_id() -> String {
    "reviewed-mapping-identity".to_owned()
}

#[test]
fn recovery_apply_result_must_prove_the_committed_batch() {
    let origin = Url::parse("https://library.test").unwrap();
    let identities = vec![json!({
        "originalId": "00000000-0000-4000-8000-000000000001",
        "newLocation": "moved/a.JPG",
        "mappingId": mapping_id()
    })];
    let prepared = PreparedRecoveryApply {
        body: json!({"mappings": []}),
        identities: identities.clone(),
    };
    let identity = MutationIdentity {
        mappings: identities,
        ..MutationIdentity::bare(Operation::RecoveryApply)
    };
    let applied = RecoveryApplyData {
        applied_mappings: 1,
        refused_mappings: 0,
        unavailable_photos: 0,
        mappings: vec![
            serde_json::from_value(json!({
                "originalId": "00000000-0000-4000-8000-000000000001",
                "photoId": "00000000-0000-4000-8000-000000000002",
                "fromLocation": "shoot/a.JPG",
                "toLocation": "moved/a.JPG",
                "webUrl": "/?photoId=00000000-0000-4000-8000-000000000002",
                "retired": null
            }))
            .unwrap(),
        ],
    };
    let value = recovery_apply_value(&identity, applied, &prepared, &origin).unwrap();
    assert_eq!(value["appliedMappings"], 1);
    assert_eq!(value["refusedMappings"], 0);
    assert_eq!(
        value["mappings"][0]["webUrl"],
        "https://library.test/?photoId=00000000-0000-4000-8000-000000000002"
    );
    for (name, result) in [
        (
            "short-count",
            RecoveryApplyData {
                applied_mappings: 0,
                refused_mappings: 1,
                unavailable_photos: 1,
                mappings: Vec::new(),
            },
        ),
        (
            "wrong-order",
            RecoveryApplyData {
                applied_mappings: 1,
                refused_mappings: 0,
                unavailable_photos: 0,
                mappings: vec![
                    serde_json::from_value(json!({
                        "originalId": "00000000-0000-4000-8000-000000000009",
                        "photoId": "00000000-0000-4000-8000-000000000002",
                        "fromLocation": "shoot/a.JPG",
                        "toLocation": "moved/a.JPG",
                        "webUrl": "/?photoId=x",
                        "retired": null
                    }))
                    .unwrap(),
                ],
            },
        ),
    ] {
        let failure = recovery_apply_value(&identity, result, &prepared, &origin).unwrap_err();
        assert_eq!(failure.exit_code, 7, "{name}");
        assert_eq!(failure.payload.code, "outcome_unknown", "{name}");
        assert_eq!(failure.payload.effect, "unknown", "{name}");
        assert_eq!(
            failure.payload.details["mappings"],
            json!(prepared.identities),
            "{name}"
        );
    }
}

#[test]
fn recovery_failure_codes_validate_their_exact_details() {
    let refusal = |code: &str, details: Value| ErrorPayload {
        code: code.to_owned(),
        message: "Read the reported mappings and review again.".to_owned(),
        effect: "none".to_owned(),
        details,
    };
    let mapped = |code: &str, details: Value, operation: Operation| {
        validated_route_failure(refusal(code, details), operation, "")
    };
    let conflict = json!({
        "appliedMappings": 0,
        "refusedMappings": 2,
        "rejections": [
            {"originalId": "00000000-0000-4000-8000-000000000001", "reason": "reviewed-stale"},
            {"originalId": "00000000-0000-4000-8000-000000000002", "reason": "content-unconfirmed"}
        ]
    });
    let failure = mapped("recovery_conflict", conflict, Operation::RecoveryApply)
        .expect("a complete batch refusal is confirmed");
    assert_eq!(failure.exit_code, 4);
    assert_eq!(failure.payload.effect, "none");
    assert!(
        mapped(
            "recovery_conflict",
            json!({"appliedMappings": 1, "refusedMappings": 1, "rejections": []}),
            Operation::RecoveryApply
        )
        .is_none()
    );
    assert!(mapped(
        "recovery_conflict",
        json!({"appliedMappings": 0, "refusedMappings": 1, "rejections": [{"originalId": "", "reason": "stale"}]}),
        Operation::RecoveryApply
    )
    .is_none());

    let scope = json!({"evaluated": 10_001, "limit": 10_000});
    let failure = mapped("recovery_scope_exceeded", scope, Operation::RecoveryPropose)
        .expect("an over-bound scope refusal is confirmed");
    assert_eq!(failure.exit_code, 2);
    assert!(
        mapped(
            "recovery_scope_exceeded",
            json!({"evaluated": 10_000, "limit": 10_000}),
            Operation::RecoveryPropose
        )
        .is_none()
    );

    let missing = mapped(
        "not_found",
        json!({"resource": "original", "reference": "00000000-0000-4000-8000-000000000001"}),
        Operation::RecoveryPropose,
    )
    .expect("an unknown Original is a confirmed missing object");
    assert_eq!(missing.exit_code, 3);

    for (kind, reason, operation) in [
        (
            "unavailable",
            "process_restarted",
            Operation::RecoveryUnavailable,
        ),
        ("mappings", "idle_or_evicted", Operation::RecoveryPropose),
    ] {
        let failure = mapped(
            "cursor_expired",
            json!({"cursorKind": kind, "reason": reason}),
            operation,
        )
        .expect("an expired review continuation is confirmed");
        assert_eq!(failure.exit_code, 6);
    }

    for (details, operation, ok) in [
        (
            json!({"operation": "recovery-apply", "retryAfterSeconds": null}),
            Operation::RecoveryApply,
            true,
        ),
        (
            json!({"operation": "recovery-unavailable", "retryAfterSeconds": 30}),
            Operation::RecoveryUnavailable,
            true,
        ),
        (
            json!({"operation": "recovery-apply", "retryAfterSeconds": null}),
            Operation::RecoveryUnavailable,
            false,
        ),
        (
            json!({"operation": "recovery-propose"}),
            Operation::RecoveryPropose,
            false,
        ),
    ] {
        assert_eq!(
            mapped("server_busy", details, operation).is_some(),
            ok,
            "for {operation:?}"
        );
    }
    assert!(
        mapped(
            "storage_failed",
            json!({"operation": "recovery-apply"}),
            Operation::RecoveryApply
        )
        .is_some()
    );
}
