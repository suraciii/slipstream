use super::*;
use clap::{CommandFactory, Parser};
use sha2::Digest;

fn darktable_command(arguments: &[&str]) -> darktable::DarktableArgs {
    let cli = Cli::try_parse_from([vec!["slipstream", "darktable"], arguments.to_vec()].concat())
        .unwrap();
    match cli.command {
        Command::Darktable(command) => command,
        _ => panic!("darktable command"),
    }
}
#[test]
fn namespaces_are_discoverable_and_old_edit_namespace_is_removed() {
    let mut grammar = Cli::command();
    let help = grammar.render_help().to_string();
    assert!(help.contains("workspace") && help.contains("darktable"));
    assert!(Cli::try_parse_from(["slipstream", "photos", "edit", "get", "photo-1"]).is_err());
    let darktable = grammar.find_subcommand_mut("darktable").unwrap();
    for name in [
        "modules",
        "edit",
        "resources",
        "presets",
        "styles",
        "preview",
        "export",
        "history",
    ] {
        assert!(darktable.find_subcommand(name).is_some());
    }
    let export = darktable.find_subcommand("export").unwrap();
    for name in [
        "create",
        "list",
        "status",
        "wait",
        "cancel",
        "retry",
        "download",
        "state-file",
        "state-file-download",
        "historical-download",
    ] {
        assert!(export.find_subcommand(name).is_some());
    }
}
#[test]
fn remote_selector_and_context_are_normalized_without_server_paths() {
    let command = darktable_command(&[
        "edit",
        "set",
        "photo-1",
        "instance-1",
        "exposure",
        "--value",
        "0.5",
        "--edit",
        "edit-2",
        "--request",
        "request-1",
    ]);
    let prepared = darktable::prepare(&command).unwrap();
    assert_eq!(prepared.photo.as_deref(), Some("photo-1"));
    assert_eq!(prepared.edit.as_deref(), Some("edit-2"));
    assert_eq!(
        prepared.arguments,
        json!({"instanceId":"instance-1","parameterId":"exposure","value":0.5,"requestId":"request-1"})
    );
    let command = darktable_command(&[
        "modules", "describe", "exposure", "--photo", "photo-1", "--edit", "edit-2",
    ]);
    let prepared = darktable::prepare(&command).unwrap();
    assert_eq!(prepared.arguments, json!({"moduleId":"exposure"}));
    assert_eq!(prepared.photo.as_deref(), Some("photo-1"));
    for args in [
        vec!["modules", "list", "--state", "/server/private"],
        vec!["modules", "list", "--edit", "edit-1"],
        vec!["modules", "list", "--cursor", "cursor", "--photo", "p1"],
        vec!["export", "create", "p1", "--directory", "/server/outputs"],
    ] {
        assert!(Cli::try_parse_from([vec!["slipstream", "darktable"], args].concat()).is_err());
    }
}
#[test]
fn local_semantic_validation_precedes_network_dispatch() {
    let command = darktable_command(&[
        "edit",
        "set",
        "photo-1",
        "instance-1",
        "exposure",
        "--value",
        "{\"value\":1,\"value\":2}",
    ]);
    assert!(darktable::prepare(&command).is_err());
    assert!(
        Cli::try_parse_from([
            "slipstream",
            "darktable",
            "edit",
            "apply",
            "p1",
            "--input",
            "missing.json"
        ])
        .is_err()
    );
    let command = darktable_command(&["edit", "show", "bad/id"]);
    assert!(darktable::prepare(&command).is_err());
}
#[test]
fn remote_rebind_rejects_host_paths_and_import_rejects_invalid_definition_locally() {
    let root = std::env::temp_dir().join(photo_files::identity("cli-input").unwrap());
    std::fs::create_dir(&root).unwrap();
    let input = root.join("input.json");
    std::fs::write(&input, b"{\"inputPath\":\"/private/server/input.raw\"}").unwrap();
    let command = darktable_command(&[
        "edit",
        "rebind",
        "photo-1",
        "--if-state",
        "token",
        "--input",
        input.to_str().unwrap(),
    ]);
    assert!(darktable::prepare(&command).is_err());
    std::fs::write(&input,b"{\"input\":{\"kind\":\"workspace-artifact\",\"artifactId\":\"input-1\",\"revision\":\"revision-1\"}}").unwrap();
    let command = darktable_command(&[
        "edit",
        "rebind",
        "photo-1",
        "--if-state",
        "token",
        "--input",
        input.to_str().unwrap(),
    ]);
    let prepared = darktable::prepare(&command).unwrap();
    assert_eq!(
        prepared.arguments["input"]["input"]["artifactId"],
        "input-1"
    );
    std::fs::write(
        &input,
        b"{\"schemaVersion\":1,\"kind\":\"preset\",\"record\":{\"arbitrary\":true}}",
    )
    .unwrap();
    let command = darktable_command(&["presets", "import", "--file", input.to_str().unwrap()]);
    assert!(darktable::prepare(&command).is_err());
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn workspace_continuation_and_delete_require_complete_explicit_intent() {
    for args in [
        vec!["files", "--cursor", "cursor", "--kind", "output"],
        vec!["files", "w1", "--cursor", "cursor"],
        vec!["delete", "w1", "--if-state", "token", "--request", "r1"],
    ] {
        assert!(Cli::try_parse_from([vec!["slipstream", "workspace"], args].concat()).is_err());
    }
    assert!(
        Cli::try_parse_from([
            "slipstream",
            "workspace",
            "delete",
            "w1",
            "--if-state",
            "token",
            "--request",
            "r1",
            "--confirm"
        ])
        .is_ok()
    );
}
#[test]
fn domain_refusals_preserve_conflicts_and_unknown_is_never_refusal() {
    let operation = Operation::Darktable("darktable-edit-set");
    let failure = validated_route_failure(
        ErrorPayload {
            code: "state_conflict".into(),
            message: "State changed; read it again.".into(),
            effect: "none".into(),
            details: json!({"currentStateToken":"new"}),
        },
        operation,
        "",
    )
    .unwrap();
    assert_eq!(failure.exit_code, 4);
    assert_eq!(failure.payload.details["currentStateToken"], "new");
    assert!(
        validated_route_failure(
            ErrorPayload {
                code: "outcome_unknown".into(),
                message: "May have committed".into(),
                effect: "unknown".into(),
                details: json!({})
            },
            operation,
            ""
        )
        .is_none()
    );
}

#[tokio::test]
async fn dropped_mutation_reply_is_not_retried_or_reclassified() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let peer = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let mut request = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let count = socket.read(&mut chunk).unwrap();
            if count == 0 {
                break;
            }
            request.extend_from_slice(&chunk[..count]);
            if request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                break;
            }
        }
        assert!(
            String::from_utf8_lossy(&request)
                .starts_with("POST /api/photos/photo-1/darktable/commands ")
        );
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{")
            .unwrap();
    });
    let client = ServiceClient::new(
        Url::parse(&format!("http://{address}")).unwrap(),
        "credential".into(),
        Duration::from_secs(1),
    )
    .unwrap();
    let command = darktable_command(&[
        "edit",
        "set",
        "photo-1",
        "instance-1",
        "exposure",
        "--value",
        "0.5",
        "--request",
        "request-1",
    ]);
    let prepared = darktable::prepare(&command).unwrap();
    let failure = darktable::execute(
        &client,
        &AdmissionState::default(),
        &PublicationState::default(),
        &command,
        prepared,
    )
    .await
    .unwrap_err();
    assert_eq!(failure.exit_code, 7);
    assert_eq!(failure.payload.effect, "unknown");
    peer.join().unwrap();
}

#[tokio::test]
async fn verified_bytes_publish_once_and_wrong_digest_publishes_nothing() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    let root = std::env::temp_dir().join(photo_files::identity("cli-download").unwrap());
    std::fs::create_dir(&root).unwrap();
    for correct in [false, true] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let digest = if correct {
            format!("{:x}", sha2::Sha256::digest(b"verified bytes"))
        } else {
            "0".repeat(64)
        };
        let peer = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let mut received = Vec::new();
            while !received.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let count = socket.read(&mut request).unwrap();
                assert_ne!(count, 0, "request ended before headers");
                received.extend_from_slice(&request[..count]);
            }
            socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: 14\r\nX-Sha256: {digest}\r\nX-Revision: revision-1\r\nConnection: close\r\n\r\nverified bytes").as_bytes()).unwrap();
        });
        let path = root.join(if correct {
            "complete.bin"
        } else {
            "invalid.bin"
        });
        let client = ServiceClient::new(
            Url::parse(&format!("http://{address}")).unwrap(),
            "credential".into(),
            Duration::from_secs(1),
        )
        .unwrap();
        let destination = preview_download::Destination::preflight(
            preview_download::DestinationKind::Artifact,
            &path,
        )
        .unwrap();
        let result = darktable::download(
            &client,
            Operation::Workspace("workspace-download"),
            client.endpoint(&["bytes"]),
            destination,
            &PublicationState::default(),
            None,
        )
        .await;
        assert_eq!(result.is_ok(), correct);
        assert_eq!(path.exists(), correct);
        if correct {
            assert_eq!(std::fs::read(&path).unwrap(), b"verified bytes");
            assert!(
                preview_download::Destination::preflight(
                    preview_download::DestinationKind::Artifact,
                    &path
                )
                .is_err()
            );
        }
        peer.join().unwrap();
    }
    std::fs::remove_dir_all(root).unwrap();
}
