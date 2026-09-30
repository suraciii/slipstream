use super::*;
use clap::Parser;

#[test]
fn metadata_read_retains_unavailable_evidence_and_reason() {
    let vectors: Value = serde_json::from_str(include_str!(
        "../../../compatibility/metadata/external-metadata-read.json"
    ))
    .unwrap();
    let mut result = vectors[0]["result"].clone();
    result["evidence"]["sidecar"] = json!({
        "state": "unavailable",
        "reason": "Sidecar cannot be read without following a link"
    });
    result["saveAvailable"] = json!(false);
    result["saveUnavailableReason"] = json!("Sidecar evidence is unavailable");
    let read: MetadataReadWire = serde_json::from_value(result.clone()).unwrap();
    assert!(
        matches!(&read.evidence.sidecar, MetadataSidecarEvidenceWire::Unavailable { reason }
            if reason == "Sidecar cannot be read without following a link")
    );
    assert_eq!(serde_json::to_value(read).unwrap(), result);
}

#[test]
fn parser_accepts_metadata_commands_and_rejects_empty_ids() {
    assert!(Cli::try_parse_from(["slipstream", "photos", "metadata", "photo-1"]).is_ok());
    assert!(Cli::try_parse_from(["slipstream", "photos", "metadata", ""]).is_err());
    assert!(
        Cli::try_parse_from([
            "slipstream",
            "photos",
            "metadata-save",
            "photo-1",
            "--input",
            "save.json"
        ])
        .is_ok()
    );
    assert!(Cli::try_parse_from(["slipstream", "photos", "metadata-save", "photo-1"]).is_err());
}

#[tokio::test]
async fn metadata_save_document_validation_accepts_the_pinned_shapes() {
    let document = serde_json::json!({
        "evidence": {
            "photoId": "photo-1",
            "originalLocation": "photos/IMG_0001.CR3",
            "original": {
                "device": 2049, "inode": 42, "size": 24000000,
                "modifiedSeconds": 1790409600, "modifiedNanoseconds": 123456789
            },
            "sidecar": { "state": "absent" },
            "associationGeneration": 7,
            "instanceEpoch": "8e8c8e92-61b1-48d0-a97f-43891664c110"
        },
        "changes": {
            "photoshop:Headline": { "op": "set", "value": "New headline" },
            "xmp:Label": { "op": "clear" },
            "photoshop:Source": { "op": "remove" },
            "dc:title": {
                "op": "setLanguages",
                "languages": { "x-default": "New title", "fr": null }
            }
        }
    });
    let write = |name: &str| {
        let path = std::env::temp_dir().join(format!("slipstream-metadata-save-{name}"));
        std::fs::write(&path, serde_json::to_vec(&document).unwrap()).unwrap();
        path
    };
    let path = write("valid.json");
    let prepared = prepare_metadata_save("photo-1", path.to_str().unwrap())
        .await
        .unwrap();
    assert_eq!(prepared, document);

    let path = write("wrong-photo.json");
    let failure = prepare_metadata_save("photo-2", path.to_str().unwrap())
        .await
        .unwrap_err();
    assert_eq!(failure.exit_code, 2);
    assert_eq!(failure.payload.code, "invalid_input");

    for (name, reason, broken) in [
        (
            "extra-key",
            "The save document must contain exactly \"evidence\" and \"changes\".",
            serde_json::json!({
                "evidence": document["evidence"].clone(),
                "changes": { "xmp:Label": { "op": "clear" } },
                "force": true
            }),
        ),
        (
            "bad-op",
            "Every \"op\" must be set, clear, remove, or setLanguages.",
            serde_json::json!({
                "evidence": document["evidence"].clone(),
                "changes": { "xmp:Label": { "op": "force" } }
            }),
        ),
        (
            "set-without-value",
            "A \"set\" change must contain exactly \"op\" and \"value\".",
            serde_json::json!({
                "evidence": document["evidence"].clone(),
                "changes": { "xmp:Label": { "op": "set" } }
            }),
        ),
        (
            "set-languages-value",
            "Every language must map to text or null for removal.",
            serde_json::json!({
                "evidence": document["evidence"].clone(),
                "changes": {
                    "dc:title": {
                        "op": "setLanguages",
                        "languages": { "fr": 3 }
                    }
                }
            }),
        ),
        (
            "clear-extra",
            "A \"clear\" or \"remove\" change must contain only \"op\".",
            serde_json::json!({
                "evidence": document["evidence"].clone(),
                "changes": { "xmp:Label": { "op": "clear", "value": "" } }
            }),
        ),
    ] {
        let path = std::env::temp_dir().join(format!("slipstream-metadata-save-{name}.json"));
        std::fs::write(&path, serde_json::to_vec(&broken).unwrap()).unwrap();
        let failure = prepare_metadata_save("photo-1", path.to_str().unwrap())
            .await
            .unwrap_err();
        assert_eq!(failure.exit_code, 2, "{name}");
        assert_eq!(
            failure.payload.details["reason"].as_str(),
            Some(reason),
            "{name}"
        );
    }
}

#[test]
fn metadata_failure_exit_codes_mirror_the_shared_table() {
    let expected = [
        ("invalid_input", 2, "none"),
        ("unsupported_field", 2, "none"),
        ("photo_missing", 3, "none"),
        ("original_unavailable", 3, "none"),
        ("association_unresolved", 3, "none"),
        ("photo_removed", 3, "none"),
        ("evidence_stale", 4, "none"),
        ("metadata_malformed", 4, "none"),
        ("save_unavailable", 5, "none"),
        ("permission", 5, "none"),
        ("resource_limit", 6, "none"),
        ("storage_failure", 7, "none"),
        ("outcome_unknown", 7, "unknown"),
    ];
    for (code, exit_code, effect) in expected {
        let failure = metadata_failure(MetadataErrorWire {
            code: code.to_owned(),
            message: "message".to_owned(),
            details: serde_json::json!({}),
        });
        assert_eq!(failure.exit_code, exit_code, "{code}");
        assert_eq!(failure.payload.effect, effect, "{code}");
    }
    let failure = metadata_failure(MetadataErrorWire {
        code: "future_code".to_owned(),
        message: "message".to_owned(),
        details: serde_json::json!({}),
    });
    assert_eq!(failure.exit_code, 6);
}

#[test]
fn parser_rejects_duplicates_abbreviations_and_cursor_combinations() {
    assert!(Cli::try_parse_from(["slipstream", "library", "check"]).is_ok());
    assert!(Cli::try_parse_from(["slipstream", "library", "check", "extra"]).is_err());
    assert!(Cli::try_parse_from(["slipstream", "--time", "3", "status"]).is_err());
    assert!(
        Cli::try_parse_from(["slipstream", "--timeout", "3", "--timeout", "4", "status"]).is_err()
    );
    assert!(
        Cli::try_parse_from([
            "slipstream",
            "albums",
            "list",
            "--cursor",
            "x",
            "--limit",
            "2"
        ])
        .is_err()
    );
    assert!(Cli::try_parse_from(["slipstream", "albums", "list", "--cursor", ""]).is_err());
    assert!(Cli::try_parse_from(["slipstream", "status", "--output", "text"]).is_err());
}

#[test]
fn parser_error_preferences_recover_valid_global_options() {
    let preferences = |arguments: &[&str]| {
        parse_error_preferences(
            &arguments
                .iter()
                .map(OsString::from)
                .collect::<Vec<OsString>>(),
        )
    };
    assert_eq!(
        preferences(&[
            "slipstream",
            "--output=text",
            "--timeout=7",
            "photos",
            "list",
            "--rating-min",
            "7",
        ]),
        ParseErrorPreferences {
            output: OutputFormat::Text,
            timeout_seconds: 7,
        }
    );
    assert_eq!(
        preferences(&[
            "slipstream",
            "--output",
            "text",
            "--timeout",
            "9",
            "photos",
            "list",
            "--rating-min",
            "7",
        ]),
        ParseErrorPreferences {
            output: OutputFormat::Text,
            timeout_seconds: 9,
        }
    );
    assert_eq!(
        preferences(&[
            "slipstream",
            "--output=invalid",
            "--timeout=invalid",
            "status",
        ]),
        ParseErrorPreferences {
            output: OutputFormat::Json,
            timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
        }
    );
}

#[test]
fn parser_validates_exact_values_and_local_times() {
    assert!(Cli::try_parse_from(["slipstream", "photos", "list", "--available", "yes"]).is_err());
    assert!(
        Cli::try_parse_from([
            "slipstream",
            "photos",
            "list",
            "--captured-from",
            "2024-02-29T23:59:59"
        ])
        .is_ok()
    );
    assert!(
        Cli::try_parse_from([
            "slipstream",
            "photos",
            "list",
            "--captured-from",
            "2023-02-29T00:00:00"
        ])
        .is_err()
    );
    assert!(
        Cli::try_parse_from([
            "slipstream",
            "photos",
            "list",
            "--captured-from",
            "2024-01-01T00:00:00Z"
        ])
        .is_err()
    );
}

#[test]
fn list_validation_enforces_requested_and_global_page_bounds() {
    let list = ListData {
        items: vec![(); 2],
        total: 2,
        next_cursor: None,
        evaluated_at: "2026-01-01T00:00:00Z".to_owned(),
        expires_at: None,
    };
    assert!(!list_expiry_valid(&list, 1));
    assert!(list_expiry_valid(&list, 2));
    let continuation = ListData {
        items: vec![(); MAXIMUM_LIST_PAGE + 1],
        total: (MAXIMUM_LIST_PAGE + 1) as u64,
        next_cursor: Some("cursor".to_owned()),
        evaluated_at: "2026-01-01T00:00:00Z".to_owned(),
        expires_at: Some("2026-01-01T00:15:00Z".to_owned()),
    };
    assert!(!list_expiry_valid(&continuation, MAXIMUM_LIST_PAGE));
}

#[test]
fn connection_requires_a_clean_origin_and_obeys_precedence() {
    let explicit = Cli::try_parse_from([
        "slipstream",
        "--server",
        "https://example.test:8443",
        "status",
    ])
    .unwrap();
    assert_eq!(
        service_origin(&explicit, Some("http://ignored.test"))
            .unwrap()
            .as_str(),
        "https://example.test:8443/"
    );
    let without_server = Cli::try_parse_from(["slipstream", "status"]).unwrap();
    assert!(service_origin(&without_server, None).is_err());
    assert!(service_origin(&without_server, Some("")).is_err());
    for invalid in [
        "ftp://example.test",
        "http://127.0.0.1:3000",
        "http://user@example.test",
        "http://@example.test",
        "https://example.test/path",
        "https://example.test/?x=1",
    ] {
        assert!(
            service_origin(&without_server, Some(invalid)).is_err(),
            "accepted {invalid}"
        );
    }
}

#[test]
fn token_file_option_is_explicit_and_does_not_accept_abbreviations_or_duplicates() {
    assert!(
        Cli::try_parse_from([
            "slipstream",
            "--token-file",
            "/run/slipstream/token",
            "status"
        ])
        .is_ok()
    );
    assert!(
        Cli::try_parse_from([
            "slipstream",
            "--token-file",
            "first",
            "--token-file",
            "second",
            "status"
        ])
        .is_err()
    );
    assert!(Cli::try_parse_from(["slipstream", "--tok", "file", "status"]).is_err());
    assert!(canonical_access_token(b"A".repeat(43).as_slice()));
    assert!(!canonical_access_token(b"A".repeat(42).as_slice()));
    assert!(!canonical_access_token(
        b"A".repeat(42)
            .iter()
            .copied()
            .chain(*b"B")
            .collect::<Vec<_>>()
            .as_slice()
    ));
    assert!(!canonical_access_token(
        b"A".repeat(42)
            .iter()
            .copied()
            .chain(*b"=")
            .collect::<Vec<_>>()
            .as_slice()
    ));
}

#[cfg(unix)]
#[test]
fn credential_file_reader_rejects_fifos_without_blocking() {
    use std::{
        ffi::CString,
        os::unix::ffi::OsStrExt,
        sync::{
            OnceLock,
            atomic::{AtomicU64, Ordering},
            mpsc,
        },
        thread,
    };

    static NEXT_PATH: OnceLock<AtomicU64> = OnceLock::new();
    let sequence = NEXT_PATH
        .get_or_init(|| AtomicU64::new(0))
        .fetch_add(1, Ordering::Relaxed);
    let path = env::temp_dir().join(format!(
        "slipstream-cli-token-fifo-{}-{sequence}",
        std::process::id()
    ));
    let c_path = CString::new(path.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

    let (sender, receiver) = mpsc::channel();
    let reader_path = path.clone();
    thread::spawn(move || {
        let _ = sender.send(read_access_token_file(&reader_path));
    });
    let failure = receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("opening a FIFO credential must not block");
    assert_eq!(failure.unwrap_err().payload.code, "invalid_input");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn access_boundary_errors_are_normalized_without_server_diagnostics() {
    let unauthorized = access_boundary_failure(
        StatusCode::UNAUTHORIZED,
        None,
        br#"{"error":"token leaked by a server"}"#,
        Operation::Status,
    )
    .unwrap();
    assert_eq!(unauthorized.payload.code, "authentication_required");
    assert_eq!(unauthorized.payload.details["operation"], "status");
    assert!(!unauthorized.payload.message.contains("token leaked"));

    let limited = access_boundary_failure(
        StatusCode::TOO_MANY_REQUESTS,
        Some(7),
        b"{}",
        Operation::AlbumsCreate,
    )
    .unwrap();
    assert_eq!(limited.payload.code, "server_busy");
    assert_eq!(limited.payload.details["retryAfterSeconds"], 7);

    assert!(
        access_boundary_failure(
            StatusCode::SERVICE_UNAVAILABLE,
            None,
            br#"{"error":"storage_failed"}"#,
            Operation::Status,
        )
        .is_none()
    );
    let unconfigured = access_boundary_failure(
        StatusCode::SERVICE_UNAVAILABLE,
        Some(9),
        br#"{"error":"access_unconfigured"}"#,
        Operation::Status,
    )
    .unwrap();
    assert_eq!(unconfigured.payload.code, "server_busy");
    assert_eq!(
        unconfigured.payload.details["retryAfterSeconds"],
        Value::Null
    );
}

#[test]
fn envelopes_and_explicit_text_are_deterministic() {
    let envelope = Envelope::success(json!({"value": 1}));
    assert_eq!(
        serde_json::to_string(&envelope).unwrap(),
        r#"{"schemaVersion":1,"status":"ok","data":{"value":1},"error":null}"#
    );
    assert_eq!(render_text(&envelope), "Success\n{\n  \"value\": 1\n}\n");
}

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
    assert_eq!(
        Cli::try_parse_from(["slipstream", "photos", "--help"])
            .unwrap_err()
            .kind(),
        clap::error::ErrorKind::DisplayHelp
    );
}

#[test]
fn semantic_query_validation_precedes_network_access() {
    let reversed = Cli::try_parse_from([
        "slipstream",
        "photos",
        "list",
        "--rating-min",
        "5",
        "--rating-max",
        "4",
    ])
    .unwrap();
    assert!(validate_command(&reversed.command).is_err());
    let album_order =
        Cli::try_parse_from(["slipstream", "photos", "list", "--order", "album-order"]).unwrap();
    assert!(validate_command(&album_order.command).is_err());
}

#[test]
fn album_mutation_parser_enforces_names_versions_and_input() {
    assert!(Cli::try_parse_from(["slipstream", "albums", "create", "--name", "Picks"]).is_ok());
    assert!(Cli::try_parse_from(["slipstream", "albums", "create", "--name", ""]).is_err());
    assert!(Cli::try_parse_from(["slipstream", "albums", "create", "--name", "  "]).is_err());
    let name_arguments = |name: String| {
        ["slipstream", "albums", "create", "--name"]
            .into_iter()
            .map(str::to_owned)
            .chain([name])
            .collect::<Vec<_>>()
    };
    assert!(Cli::try_parse_from(name_arguments("x".repeat(121))).is_err());
    assert!(Cli::try_parse_from(name_arguments("\u{00e9}".repeat(121))).is_err());
    assert!(Cli::try_parse_from(name_arguments("\u{00e9}".repeat(120))).is_ok());
    assert!(Cli::try_parse_from(["slipstream", "albums", "rename", "a", "--name", "N"]).is_err());
    assert!(
        Cli::try_parse_from(["slipstream", "albums", "delete", "a", "--if-version", "",]).is_err()
    );
    assert!(
        Cli::try_parse_from([
            "slipstream",
            "albums",
            "add",
            "a",
            "--input",
            "members.json",
            "--if-version",
            "v",
            "--if-version",
            "w",
        ])
        .is_err()
    );
    assert!(Cli::try_parse_from(["slipstream", "albums", "remove", "a", "--input", "-"]).is_err());
}

#[test]
fn membership_input_validates_the_complete_document() {
    let ids = |count: usize| {
        (0..count)
            .map(|index| format!("00000000-0000-4000-8000-{index:012x}"))
            .collect::<Vec<_>>()
    };
    let document =
        |photo_ids: &[String]| serde_json::to_vec(&json!({ "photoIds": photo_ids })).unwrap();
    let parsed = parse_membership_ids(document(&ids(2)), "mutationPhotoIdsMaximum").unwrap();
    assert_eq!(parsed, ids(2));
    for invalid in [
        b"".as_slice().to_vec(),
        b"{".as_slice().to_vec(),
        b"[]".as_slice().to_vec(),
        b"\"photoIds\"".as_slice().to_vec(),
        b"{\"photoIds\": []}".as_slice().to_vec(),
        b"{\"photoIds\": [\"\"]}".as_slice().to_vec(),
        b"{\"photoIds\": [\"a\", \"a\"]}".as_slice().to_vec(),
        b"{\"photoIds\": [\"a\"], \"photoIds\": [\"b\"]}"
            .as_slice()
            .to_vec(),
        b"{\"photoIds\": [\"a\"], \"extra\": 1}".as_slice().to_vec(),
        b"{\"photoIds\": [\"a\"]} trailing".as_slice().to_vec(),
        b"{\"photoIds\": [1]}".as_slice().to_vec(),
        b"\xff\xfe{\"photoIds\": [\"a\"]}".as_slice().to_vec(),
    ] {
        let failure =
            parse_membership_ids(invalid.to_vec(), "mutationPhotoIdsMaximum").unwrap_err();
        assert_eq!(failure.exit_code, 2, "for {invalid:?}");
        assert_eq!(failure.payload.code, "invalid_input");
        assert_eq!(failure.payload.details["argument"], "input");
    }
    let over_limit = parse_membership_ids(
        document(&ids(MAXIMUM_MUTATION_PHOTO_IDS + 1)),
        "albumReorderMembersMaximum",
    )
    .unwrap_err();
    assert_eq!(over_limit.exit_code, 2);
    assert_eq!(over_limit.payload.code, "limit_exceeded");
    assert_eq!(
        over_limit.payload.details,
        json!({
            "limitName": "albumReorderMembersMaximum",
            "limit": MAXIMUM_MUTATION_PHOTO_IDS,
            "actual": MAXIMUM_MUTATION_PHOTO_IDS + 1,
        })
    );
}

#[test]
fn request_order_partitions_must_cover_and_preserve_request_order() {
    let submitted = ["a", "b", "c", "d"]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();
    let strings = |values: &[&str]| {
        values
            .iter()
            .map(|value| (*value).to_owned())
            .collect::<Vec<_>>()
    };
    assert!(request_order_partition(
        &submitted,
        &strings(&["a", "c"]),
        &strings(&["b", "d"])
    ));
    assert!(request_order_partition(
        &submitted,
        &strings(&[]),
        &strings(&["a", "b", "c", "d"])
    ));
    assert!(!request_order_partition(
        &submitted,
        &strings(&["b", "a"]),
        &strings(&["c", "d"])
    ));
    assert!(!request_order_partition(
        &submitted,
        &strings(&["a", "c"]),
        &strings(&["d"])
    ));
    assert!(!request_order_partition(
        &submitted,
        &strings(&["a", "c"]),
        &strings(&["b", "d", "e"])
    ));
    assert!(!request_order_partition(
        &submitted,
        &strings(&["a", "a"]),
        &strings(&["b", "c", "d"])
    ));
    assert!(!request_order_partition(
        &submitted,
        &strings(&["a", "b", "c"]),
        &strings(&["c", "d"])
    ));
}

#[test]
fn photo_decision_parser_requires_one_complete_form() {
    let single = |arguments: &[&str]| {
        let mut invocation = vec!["slipstream", "photos", "set"];
        invocation.extend(arguments);
        Cli::try_parse_from(invocation)
    };
    assert!(single(&["p1", "--selection", "selected", "--if-version", "v"]).is_ok());
    assert!(single(&["p1", "--selection", "undecided", "--if-version", "v"]).is_ok());
    assert!(single(&["p1", "--rating", "3", "--if-version", "v"]).is_ok());
    assert!(single(&["p1", "--rating", "0", "--if-version", "v"]).is_ok());
    assert!(single(&["--input", "decisions.json"]).is_ok());
    assert!(single(&["--input", "-"]).is_ok());
    // Selection and Rating cannot change in the same command, and the
    // batch form cannot be mixed with the single-Photo options.
    assert!(
        single(&[
            "p1",
            "--selection",
            "selected",
            "--rating",
            "3",
            "--if-version",
            "v"
        ])
        .is_err()
    );
    assert!(single(&["--input", "d.json", "--if-version", "v"]).is_err());
    assert!(single(&["--input", "d.json", "p1"]).is_err());
    assert!(single(&["--input", "d.json", "--selection", "selected"]).is_err());
    // Unknown values and out-of-range ratings are parser errors.
    assert!(single(&["p1", "--selection", "all", "--if-version", "v"]).is_err());
    assert!(single(&["p1", "--rating", "6", "--if-version", "v"]).is_err());
    assert!(single(&["p1", "--rating", "-1", "--if-version", "v"]).is_err());
    assert!(single(&["--if-version", "v"]).is_err());
    // The single-Photo forms must be complete before any network access.
    let missing_field = single(&["p1", "--if-version", "v"]).expect("the shape parses");
    assert!(validate_command(&missing_field.command).is_err());
    let missing_version = single(&["p1", "--selection", "selected"]).expect("the shape parses");
    assert!(validate_command(&missing_version.command).is_err());
}

#[test]
fn decision_input_validates_the_complete_document() {
    let id = |index: usize| format!("00000000-0000-4000-8000-{index:012x}");
    let document = |field: Value, value: Value, photos: Value| {
        serde_json::to_vec(&json!({ "field": field, "value": value, "photos": photos })).unwrap()
    };
    let items = |count: usize| {
        (0..count)
            .map(|index| json!({ "photoId": id(index), "ifVersion": "v1" }))
            .collect::<Vec<_>>()
    };
    let parsed = |bytes: Vec<u8>| parse_decision_input(bytes).map(|prepared| prepared.body());
    assert_eq!(
        parsed(document(
            "selectionState".into(),
            "rejected".into(),
            json!(items(2))
        ))
        .unwrap(),
        json!({
            "field": "selectionState",
            "value": "rejected",
            "photos": [
                { "photoId": id(0), "ifVersion": "v1" },
                { "photoId": id(1), "ifVersion": "v1" }
            ]
        })
    );
    assert!(parsed(document("rating".into(), 4.into(), json!(items(1)))).is_ok());
    assert!(parsed(document("rating".into(), 0.into(), json!(items(1)))).is_ok());

    let invalid = |bytes: Vec<u8>, argument: &str| {
        let failure = parse_decision_input(bytes).unwrap_err();
        assert_eq!(failure.exit_code, 2);
        assert_eq!(failure.payload.code, "invalid_input");
        assert_eq!(failure.payload.details["argument"], argument);
    };
    for (bytes, argument) in [
            (b"".as_slice().to_vec(), "input"),
            (b"{".as_slice().to_vec(), "input"),
            (b"[]".as_slice().to_vec(), "input"),
            (b"\"x\"".as_slice().to_vec(), "input"),
            (b"\xff\xfe{}".as_slice().to_vec(), "input"),
            (b"{\"field\": \"rating\"}".as_slice().to_vec(), "input"),
            (
                b"{\"field\": \"rating\", \"value\": 4, \"photos\": [], \"extra\": 1}".as_slice().to_vec(),
                "input",
            ),
            (
                b"{\"field\": \"rating\", \"field\": \"rating\", \"value\": 4, \"photos\": []}".as_slice().to_vec(),
                "input",
            ),
            (
                b"{\"field\": \"rating\", \"value\": 4, \"photos\": [{\"photoId\": \"a\", \"ifVersion\": \"v\", \"ifVersion\": \"w\"}]}".as_slice().to_vec(),
                "input",
            ),
            (
                b"{\"field\": \"rating\", \"value\": 4, \"photos\": [{\"photoId\": 1, \"ifVersion\": \"v\"}]}".as_slice().to_vec(),
                "input",
            ),
            (
                b"{\"field\": \"rating\", \"value\": 4, \"photos\": []} trailing".as_slice().to_vec(),
                "input",
            ),
            (document("selection".into(), "selected".into(), json!(items(1))), "field"),
            (document(5.into(), "selected".into(), json!(items(1))), "input"),
            (document("rating".into(), "4".into(), json!(items(1))), "value"),
            (document("rating".into(), 4.5.into(), json!(items(1))), "value"),
            (document("rating".into(), 6.into(), json!(items(1))), "value"),
            (document("rating".into(), (-1).into(), json!(items(1))), "value"),
            (
                document("selectionState".into(), 1.into(), json!(items(1))),
                "value",
            ),
            (
                document("selectionState".into(), "picked".into(), json!(items(1))),
                "value",
            ),
            (
                document("rating".into(), 4.into(), serde_json::json!([])),
                "photos",
            ),
            (
                document(
                    "rating".into(),
                    4.into(),
                    json!([
                        { "photoId": id(0), "ifVersion": "v1" },
                        { "photoId": id(0), "ifVersion": "v2" }
                    ])
                ),
                "photos",
            ),
            (
                document("rating".into(), 4.into(), json!([{ "photoId": "", "ifVersion": "v1" }])),
                "photos",
            ),
            (
                document("rating".into(), 4.into(), json!([{ "photoId": id(0), "ifVersion": "" }])),
                "photos",
            ),
        ] {
            invalid(bytes.to_vec(), argument);
        }

    let over_limit = parse_decision_input(document(
        "rating".into(),
        4.into(),
        json!(items(MAXIMUM_MUTATION_PHOTO_IDS + 1)),
    ))
    .unwrap_err();
    assert_eq!(over_limit.exit_code, 2);
    assert_eq!(over_limit.payload.code, "limit_exceeded");
    assert_eq!(
        over_limit.payload.details,
        json!({
            "limitName": "photoIds",
            "limit": MAXIMUM_MUTATION_PHOTO_IDS,
            "actual": MAXIMUM_MUTATION_PHOTO_IDS + 1,
        })
    );
}

#[test]
fn decision_batches_partition_from_validated_results_only() {
    let identity = MutationIdentity {
        operation: Operation::PhotosSet,
        photo_ids: vec!["a".to_owned(), "b".to_owned()],
        album_id: None,
        album_name: None,
        mappings: Vec::new(),
    };
    let submitted = |ids: &[&str]| {
        ids.iter()
            .map(|id| DecisionTarget {
                photo_id: (*id).to_owned(),
                if_version: "v1".to_owned(),
            })
            .collect::<Vec<_>>()
    };
    let request = |ids: &[&str]| PreparedDecision {
        field: DecisionField::Rating,
        value: json!(4),
        photos: submitted(ids),
    };
    let facts = |selection: &str, rating: u8| PhotoDecisionFactsWire {
        selection_state: serde_json::from_value::<SelectionState>(json!(selection)).unwrap(),
        rating,
    };
    let snapshot = |selection: &str, rating: u8, version: &str| PhotoDecisionSnapshotWire {
        selection_state: serde_json::from_value::<SelectionState>(json!(selection)).unwrap(),
        rating,
        decision_version: version.to_owned(),
    };
    let changed = |id: &str, version: &str| PhotoDecisionItemWire {
        photo_id: id.to_owned(),
        outcome: "changed".to_owned(),
        prior: Some(facts("undecided", 0)),
        current: Some(snapshot("selected", 4, version)),
    };
    let unchanged = |id: &str, version: &str| PhotoDecisionItemWire {
        photo_id: id.to_owned(),
        outcome: "unchanged".to_owned(),
        prior: None,
        current: Some(snapshot("undecided", 4, version)),
    };
    let conflict = |id: &str, version: &str| PhotoDecisionItemWire {
        photo_id: id.to_owned(),
        outcome: "conflict".to_owned(),
        prior: None,
        current: Some(snapshot("rejected", 2, version)),
    };
    let missing = |id: &str| PhotoDecisionItemWire {
        photo_id: id.to_owned(),
        outcome: "missing".to_owned(),
        prior: None,
        current: None,
    };
    let wire = |results: Vec<PhotoDecisionItemWire>, counts: [usize; 4]| PhotoDecisionWire {
        results,
        counts: PhotoDecisionCountsWire {
            changed: counts[0],
            unchanged: counts[1],
            conflict: counts[2],
            missing: counts[3],
        },
    };

    // Only changed or unchanged results keep status ok and exit 0.
    let confirmed = confirmed_decision_result(
        &identity,
        &request(&["a", "b"]),
        wire(vec![changed("a", "v2"), conflict("b", "v9")], [1, 0, 1, 0]),
    )
    .unwrap_err();
    assert_eq!(confirmed.exit_code, 5);
    assert_eq!(confirmed.payload.code, "partial_result");
    assert_eq!(confirmed.payload.effect, "partial");
    assert_eq!(
        confirmed.payload.details,
        json!({ "counts": { "changed": 1, "unchanged": 0, "conflict": 1, "missing": 0 } })
    );
    assert_eq!(
        confirmed.data.as_ref().unwrap()["results"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    let (code, exit, reference) = {
        let failure = confirmed_decision_result(
            &identity,
            &request(&["a", "b"]),
            wire(vec![conflict("a", "v8"), conflict("b", "v9")], [0, 0, 2, 0]),
        )
        .unwrap_err();
        assert_eq!(failure.exit_code, 4);
        assert_eq!(failure.payload.effect, "none");
        assert_eq!(
            failure.payload.details,
            json!({ "resource": "photo", "reference": "a", "currentVersion": "v8" })
        );
        assert_eq!(failure.data.as_ref().unwrap()["counts"]["conflict"], 2);
        (
            failure.payload.code.clone(),
            failure.exit_code,
            failure.payload.details["reference"].clone(),
        )
    };
    assert_eq!(
        (code.as_str(), exit, reference.as_str().unwrap()),
        ("conflict", 4, "a")
    );

    let missing_failure = confirmed_decision_result(
        &identity,
        &request(&["a", "b"]),
        wire(vec![missing("a"), missing("b")], [0, 0, 0, 2]),
    )
    .unwrap_err();
    assert_eq!(missing_failure.exit_code, 3);
    assert_eq!(missing_failure.payload.code, "not_found");
    assert_eq!(
        missing_failure.payload.details,
        json!({ "resource": "photo", "reference": "a" })
    );

    let ok = confirmed_decision_result(
        &identity,
        &request(&["a", "b"]),
        wire(vec![changed("a", "v2"), changed("b", "v3")], [2, 0, 0, 0]),
    )
    .unwrap();
    assert_eq!(
        ok["counts"],
        json!({ "changed": 2, "unchanged": 0, "conflict": 0, "missing": 0 })
    );
    assert_eq!(
        ok["results"][0]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>(),
        ["current", "outcome", "photoId", "prior"]
    );

    let ok_mixed = confirmed_decision_result(
        &identity,
        &request(&["a", "b"]),
        wire(vec![changed("a", "v2"), unchanged("b", "v2")], [1, 1, 0, 0]),
    )
    .unwrap();
    assert_eq!(
        ok_mixed["counts"],
        json!({ "changed": 1, "unchanged": 1, "conflict": 0, "missing": 0 })
    );

    for (label, wire_result) in [
        (
            "reordered results",
            wire(vec![changed("b", "v2"), changed("a", "v3")], [2, 0, 0, 0]),
        ),
        (
            "counts do not match outcomes",
            wire(vec![changed("a", "v2"), conflict("b", "v9")], [2, 0, 0, 0]),
        ),
        (
            "short result array",
            wire(vec![changed("a", "v2")], [1, 0, 0, 0]),
        ),
        (
            "changed without prior",
            PhotoDecisionWire {
                results: vec![PhotoDecisionItemWire {
                    photo_id: "a".to_owned(),
                    outcome: "changed".to_owned(),
                    prior: None,
                    current: Some(snapshot("selected", 3, "v2")),
                }],
                counts: PhotoDecisionCountsWire {
                    changed: 1,
                    unchanged: 0,
                    conflict: 0,
                    missing: 0,
                },
            },
        ),
        (
            "changed current does not echo the requested value",
            wire(
                vec![PhotoDecisionItemWire {
                    photo_id: "a".to_owned(),
                    outcome: "changed".to_owned(),
                    prior: Some(facts("undecided", 0)),
                    current: Some(snapshot("selected", 3, "v2")),
                }],
                [1, 0, 0, 0],
            ),
        ),
        (
            "unchanged current does not echo the requested value",
            wire(
                vec![PhotoDecisionItemWire {
                    photo_id: "a".to_owned(),
                    outcome: "unchanged".to_owned(),
                    prior: None,
                    current: Some(snapshot("undecided", 2, "v2")),
                }],
                [0, 1, 0, 0],
            ),
        ),
    ] {
        let failure =
            confirmed_decision_result(&identity, &request(&["a", "b"]), wire_result).unwrap_err();
        assert_eq!(failure.exit_code, 7, "for {label}");
        assert_eq!(failure.payload.code, "outcome_unknown", "for {label}");
        assert_eq!(failure.payload.details["operation"], "photos-set");
    }
}

#[test]
fn photo_export_commands_parse_and_validate() {
    let submit = |arguments: &[&str]| {
        let mut invocation = vec!["slipstream", "photos", "export", "submit", "p1"];
        invocation.extend(arguments);
        Cli::try_parse_from(invocation)
    };
    assert!(submit(&["--target", "development-tiff", "--request-id", "r1"]).is_ok());
    assert!(submit(&["--target", "film-jpeg", "--request-id", "r1"]).is_ok());
    assert!(submit(&["--target", "film-jpeg"]).is_err());
    assert!(submit(&["--request-id", "r1"]).is_err());
    assert!(submit(&["--target", "gallery-print", "--request-id", "r1"]).is_err());
    // Only the two closed targets exist; abbreviations stay off.
    assert!(submit(&["-t", "film-jpeg", "--request-id", "r1"]).is_err());

    let export = |arguments: &[&str]| {
        let mut invocation = vec!["slipstream", "photos", "export"];
        invocation.extend(arguments);
        Cli::try_parse_from(invocation)
    };
    assert!(export(&["status", "e1"]).is_ok());
    assert!(export(&["list", "p1"]).is_ok());
    assert!(export(&["download", "e1", "--file", "out.tiff"]).is_ok());
    assert!(export(&["download", "e1"]).is_err());
    assert!(export(&["status"]).is_err());

    // The submit request identity shares the service's closed rule, and
    // it is checked before any network access.
    let valid = |request_id: &str| {
        let parsed =
            submit(&["--target", "film-jpeg", "--request-id", request_id]).expect("parses");
        validate_command(&parsed.command)
    };
    assert!(valid("r1").is_ok());
    assert!(valid("A.9_-repeat").is_ok());
    assert!(valid("has space").is_err());
    assert!(valid("slash/none").is_err());
    assert!(valid("unicode-é").is_err());
    let too_long = "a".repeat(129);
    assert!(valid(&too_long).is_err());
    let exactly_128 = "a".repeat(128);
    assert!(valid(&exactly_128).is_ok());
}

#[test]
fn export_submit_response_must_echo_the_captured_revisions() {
    let identity = MutationIdentity {
        operation: Operation::PhotosExportSubmit,
        photo_ids: vec!["p1".to_owned()],
        album_id: None,
        album_name: None,
        mappings: Vec::new(),
    };
    let confirmed = |result: ExportSubmitWire| {
        confirmed_export_submit(&identity, "film-jpeg", "recipe-2", "source-7", result)
    };
    let submit = |target: &str, recipe: &str, source: &str| ExportSubmitWire {
        export_id: "e1".to_owned(),
        state: "queued".to_owned(),
        target: target.to_owned(),
        recipe_version: recipe.to_owned(),
        source_revision: source.to_owned(),
        receipt_expires_at: None,
        artifact_expires_at: None,
    };
    let confirmed_value =
        confirmed(submit("film-jpeg", "recipe-2", "source-7")).expect("echoes submission");
    assert_eq!(confirmed_value["exportId"], "e1");
    assert_eq!(confirmed_value["target"], "film-jpeg");
    assert_eq!(confirmed_value["receiptExpiresAt"], Value::Null);
    for label in [
        ("target", submit("development-tiff", "recipe-2", "source-7")),
        ("recipe", submit("film-jpeg", "recipe-1", "source-7")),
        ("source", submit("film-jpeg", "recipe-2", "source-8")),
    ] {
        let failure = confirmed(label.1).unwrap_err();
        assert_eq!(failure.exit_code, 7, "for {}", label.0);
        assert_eq!(failure.payload.code, "outcome_unknown", "for {}", label.0);
    }
    let unknown_identity = submit("film-jpeg", "recipe-2", "source-7");
    let unknown = confirmed(ExportSubmitWire {
        export_id: "space id".to_owned(),
        ..unknown_identity
    })
    .unwrap_err();
    assert_eq!(unknown.exit_code, 7);
    let stale_time = confirmed(ExportSubmitWire {
        receipt_expires_at: Some("yesterday".to_owned()),
        ..submit("film-jpeg", "recipe-2", "source-7")
    })
    .unwrap_err();
    assert_eq!(stale_time.exit_code, 7);
}

#[test]
fn export_inspection_is_validated_against_the_closed_state_machine() {
    let artifact = ExportArtifactWire {
        export_id: "e1".to_owned(),
        target: "development-tiff".to_owned(),
        stage: "develop".to_owned(),
        content_type: "image/tiff".to_owned(),
        filename: "e1.tiff".to_owned(),
        orientation: "top-left".to_owned(),
        sample_format: "float32".to_owned(),
        color_space: "RGB".to_owned(),
        icc_embedded: false,
        width: 5542,
        height: 3696,
        profile_identity: "profile".to_owned(),
        byte_length: 4096,
        sha256: "a".repeat(64),
        expires_at: "2026-01-01T12:00:00Z".to_owned(),
    };
    let inspect = |state: &str, terminal: Option<&str>, artifact: Option<ExportArtifactWire>| {
        ExportInspectWire {
            export_id: "e1".to_owned(),
            photo_id: "p1".to_owned(),
            state: state.to_owned(),
            target: "development-tiff".to_owned(),
            recipe_version: "recipe-2".to_owned(),
            source_revision: "source-7".to_owned(),
            created_at: "2026-01-01T12:00:00Z".to_owned(),
            settled_at: terminal.map(|_| "2026-01-01T12:00:01Z".to_owned()),
            bundle_id: "bundle".to_owned(),
            terminal_outcome: terminal.map(str::to_owned),
            failure_reason: None,
            receipt_expires_at: None,
            artifact,
        }
    };
    let value = validated_export_inspect(
        inspect("running", None, None),
        "e1",
        Operation::PhotosExportStatus,
    )
    .expect("running inspection is valid");
    assert_eq!(value.state, "running");
    validated_export_inspect(
        inspect("succeeded", Some("succeeded"), Some(artifact.clone())),
        "e1",
        Operation::PhotosExportStatus,
    )
    .expect("succeeded inspection is valid");

    let transport = |data: ExportInspectWire| {
        validated_export_inspect(data, "e1", Operation::PhotosExportStatus).unwrap_err()
    };
    for label in [
        ("unknown state", inspect("expired", None, None)),
        ("wrong terminal", inspect("failed", Some("succeeded"), None)),
        (
            "terminal on active",
            inspect("queued", Some("queued"), None),
        ),
        ("unknown target", {
            let mut data = inspect("succeeded", Some("succeeded"), None);
            data.target = "gallery-print".to_owned();
            data
        }),
        ("invalid creation time", {
            let mut data = inspect("running", None, None);
            data.created_at = "yesterday".to_owned();
            data
        }),
        ("invalid settlement time", {
            let mut data = inspect("failed", Some("failed"), None);
            data.settled_at = Some("tomorrow".to_owned());
            data
        }),
    ] {
        let failure = transport(label.1);
        assert_eq!(failure.payload.code, "transport_failed", "for {}", label.0);
    }
    // The artifact object must name its own Export with the closed
    // per-target stage and media type.
    for label in [
        ("wrong export", {
            let mut artifact = artifact.clone();
            artifact.export_id = "other".to_owned();
            inspect("succeeded", Some("succeeded"), Some(artifact))
        }),
        ("jpeg stage on tiff target", {
            let mut artifact = artifact.clone();
            artifact.stage = "film".to_owned();
            inspect("succeeded", Some("succeeded"), Some(artifact))
        }),
        ("wrong media type", {
            let mut artifact = artifact.clone();
            artifact.content_type = "image/jpeg".to_owned();
            inspect("succeeded", Some("succeeded"), Some(artifact))
        }),
        ("short digest", {
            let mut artifact = artifact.clone();
            artifact.sha256 = "a".repeat(63);
            inspect("succeeded", Some("succeeded"), Some(artifact))
        }),
        ("missing filename", {
            let mut artifact = artifact.clone();
            artifact.filename.clear();
            inspect("succeeded", Some("succeeded"), Some(artifact))
        }),
        ("missing color space", {
            let mut artifact = artifact.clone();
            artifact.color_space.clear();
            inspect("succeeded", Some("succeeded"), Some(artifact))
        }),
    ] {
        let failure = transport(label.1);
        assert_eq!(failure.payload.code, "transport_failed", "for {}", label.0);
    }
}

#[test]
fn export_list_entries_carry_only_closed_states_and_targets() {
    let entry = |target: &str| ExportSummaryWire {
        export_id: "e1".to_owned(),
        state: "succeeded".to_owned(),
        target: target.to_owned(),
        recipe_version: "recipe-2".to_owned(),
        source_revision: "source-7".to_owned(),
        created_at: "2026-01-01T12:00:00Z".to_owned(),
        settled_at: Some("2026-01-01T12:00:01Z".to_owned()),
    };
    assert_eq!(
        export_list_value(
            ExportListWire {
                exports: vec![entry("film-jpeg")]
            },
            Operation::PhotosExportList
        )
        .unwrap()["exports"][0]["target"],
        "film-jpeg"
    );
    assert_eq!(
        export_list_value(
            ExportListWire {
                exports: vec![entry("development-tiff")]
            },
            Operation::PhotosExportList
        )
        .unwrap()["exports"][0]["target"],
        "development-tiff"
    );
    assert_eq!(
        export_list_value(
            ExportListWire {
                exports: Vec::new()
            },
            Operation::PhotosExportList
        )
        .unwrap()["exports"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    for target in ["gallery-print", ""] {
        let failure = export_list_value(
            ExportListWire {
                exports: vec![entry(target)],
            },
            Operation::PhotosExportList,
        )
        .unwrap_err();
        assert_eq!(failure.payload.code, "transport_failed", "for {target}");
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
        validated_route_failure(refusal(code), Operation::PhotosExportSubmit, "")
            .unwrap_or_else(|| panic!("{code} must map to a confirmed failure"))
    };
    assert_eq!(mapped("invalid_settings").exit_code, 2);
    assert_eq!(mapped("unsupported_photo").exit_code, 2);
    assert_eq!(mapped("unknown_photo").exit_code, 3);
    assert_eq!(mapped("unknown_export").exit_code, 3);
    assert_eq!(mapped("missing_recipe").exit_code, 3);
    assert_eq!(mapped("recipe_conflict").exit_code, 4);
    assert_eq!(mapped("source_changed").exit_code, 4);
    assert_eq!(mapped("requires_rebind").exit_code, 4);
    assert_eq!(mapped("request_conflict").exit_code, 4);
    assert_eq!(mapped("export_conflict").exit_code, 4);
    assert_eq!(mapped("output_unavailable").exit_code, 4);
    assert_eq!(mapped("export_expired").exit_code, 6);
    assert_eq!(mapped("receipt_expired").exit_code, 6);
    assert_eq!(mapped("artifact_expired").exit_code, 6);
    assert_eq!(mapped("processing_unavailable").exit_code, 6);
    assert_eq!(mapped("resource_unavailable").exit_code, 6);
    assert_eq!(mapped("retained_output_full").exit_code, 6);
    // A possibly admitted write keeps its unknown outcome; the mapped
    // confirmed refusals keep the service's message and effect.
    assert!(
        validated_route_failure(
            refusal("outcome_unknown"),
            Operation::PhotosExportSubmit,
            ""
        )
        .is_none()
    );
    let confirmed = mapped("export_conflict");
    assert_eq!(confirmed.payload.effect, "none");
    assert_eq!(confirmed.payload.details, json!({}));
}

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
        "selectionState": "selected",
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
            "kind": "jpeg", "rating": 3, "selectionState": "selected",
            "fingerprintEnrolled": false, "albumCount": 1, "webUrl": "/?photoId=x"
        }))
        .is_err()
    );
    assert!(
        serde_json::from_value::<RecoveryItemWire>(json!({
            "state": "unavailable", "originalId": "00000000-0000-4000-8000-000000000001",
            "photoId": "00000000-0000-4000-8000-000000000002", "location": "shoot/a.JPG",
            "kind": "tiff", "rating": 3, "selectionState": "selected",
            "fingerprintEnrolled": false, "albumCount": 1, "webUrl": "/?photoId=x"
        }))
        .is_err()
    );
    let over_rated = serde_json::from_value::<RecoveryItemWire>(json!({
        "state": "available", "originalId": "00000000-0000-4000-8000-000000000001",
        "photoId": "00000000-0000-4000-8000-000000000002", "location": "shoot/a.JPG",
        "kind": "raw", "rating": 6, "selectionState": "undecided",
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

#[test]
fn deadline_expiry_maps_library_check_to_an_unknown_scan_outcome() {
    // A `library check` deadline is never a connection failure: the scan is
    // service-owned and may have been admitted and running whether or not
    // this client handed the request over, so only `status` can report it.
    let failure = deadline_failure(
        Operation::LibraryCheck,
        &PublicationState::default(),
        &AdmissionState::default(),
    );
    assert_eq!(failure.exit_code, 7);
    assert_eq!(failure.payload.code, "outcome_unknown");
    assert_eq!(failure.payload.effect, "unknown");
    assert_eq!(failure.payload.details["operation"], "library-check");
    assert!(failure.payload.message.contains("status"));
    assert!(!failure.payload.message.contains("connection"));
}

#[test]
fn deadline_expiry_keeps_the_admitted_and_transport_mappings_elsewhere() {
    // A read that never admitted a mutation stays a transport failure.
    let transport = deadline_failure(
        Operation::Status,
        &PublicationState::default(),
        &AdmissionState::default(),
    );
    assert_eq!(transport.exit_code, 6);
    assert_eq!(transport.payload.code, "transport_failed");
    assert_eq!(transport.payload.effect, "none");
    // An admitted mutation keeps its unknown outcome and identity details.
    let admission = AdmissionState::default();
    admission.admit(MutationIdentity::bare(Operation::PhotosRecipeSave));
    let admitted = deadline_failure(
        Operation::PhotosRecipeSave,
        &PublicationState::default(),
        &admission,
    );
    assert_eq!(admitted.exit_code, 7);
    assert_eq!(admitted.payload.code, "outcome_unknown");
    assert_eq!(admitted.payload.details["operation"], "photos-recipe-save");
}

#[tokio::test]
async fn a_timed_out_library_check_reports_the_unknown_scan_outcome() {
    // The service accepts connections and never answers — the TLS handshake
    // to a silent socket cannot complete — so the deadline, not a connection
    // refusal, ends the command while the service-owned scan may be running.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let token = std::env::temp_dir().join("slipstream-cli-deadline-token");
    std::fs::write(&token, "A".repeat(43)).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(&token).unwrap().permissions();
        permissions.set_mode(0o600);
        std::fs::set_permissions(&token, permissions).unwrap();
    }
    let cli = Cli::try_parse_from([
        "slipstream",
        "--server",
        &format!("https://127.0.0.1:{port}"),
        "--token-file",
        token.to_str().unwrap(),
        "library",
        "check",
    ])
    .unwrap();
    let result = invoke_until(
        cli,
        None,
        tokio::time::Instant::now() + std::time::Duration::from_millis(500),
    )
    .await;
    assert_eq!(result.exit_code, 7);
    assert!(
        result.stdout.contains("outcome_unknown"),
        "{}",
        result.stdout
    );
    assert!(result.stdout.contains("status"), "{}", result.stdout);
    assert!(
        !result.stdout.contains("transport_failed"),
        "{}",
        result.stdout
    );
    std::fs::remove_file(&token).ok();
    drop(listener);
}

#[test]
fn source_refusals_carry_the_closed_support_reason() {
    // A save or rebind refused for the Photo's source state reports the same
    // closed `supportReason` the recipe read reports, and a preview refusal
    // names it as `reason`, so the retryable waits stay distinguishable from
    // the confirmed outcomes in the failure envelope.
    let refusal = |details: Value| ErrorPayload {
        code: "resource_unavailable".to_owned(),
        message: "Current source facts cannot be read, so no guarded write is possible.".to_owned(),
        effect: "none".to_owned(),
        details,
    };
    for reason in [
        "original-missing",
        "original-unreadable",
        "read-pending",
        "resource-unavailable",
    ] {
        let save = validated_route_failure(
            refusal(json!({"photoId": "p1", "supportReason": reason})),
            Operation::PhotosRecipeSave,
            "",
        )
        .unwrap_or_else(|| panic!("{reason} is a confirmed source refusal"));
        assert_eq!(save.exit_code, 6, "{reason}");
        assert_eq!(save.payload.details["supportReason"], json!(reason));
        let preview = validated_route_failure(
            refusal(json!({"stage": "develop", "reason": reason})),
            Operation::PhotosEditPreview,
            "",
        )
        .unwrap_or_else(|| panic!("{reason} is a confirmed preview refusal"));
        assert_eq!(preview.exit_code, 6, "{reason}");
        assert_eq!(preview.payload.details["reason"], json!(reason));
    }
    // A reported `supportReason` outside the closed set is not a confirmed
    // refusal; an internal preview reason such as `preview-superseded`
    // stays a valid deployment-condition detail.
    assert!(
        validated_route_failure(
            refusal(json!({"photoId": "p1", "supportReason": "original-rotated"})),
            Operation::PhotosRecipeSave,
            ""
        )
        .is_none()
    );
    assert!(
        validated_route_failure(
            refusal(json!({"stage": "develop", "reason": "preview-superseded"})),
            Operation::PhotosEditPreview,
            ""
        )
        .is_some()
    );
}

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
