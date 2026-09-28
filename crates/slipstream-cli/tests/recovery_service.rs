use serde_json::{Value, json};
use slipstream_server::Config;
mod common;
use std::{
    fs,
    io::{ErrorKind, Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    thread::JoinHandle,
    time::Duration,
};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

/// The complete capabilities document this CLI revision requires, including
/// the Reviewed Location Recovery bounds.
fn capabilities_body() -> Value {
    json!({
        "serverVersion": "0.0.0",
        "supportedCliContractVersions": [1],
        "limits": {
            "listPageMaximum": 60,
            "mutationPhotoIdsMaximum": 100,
            "removalPhotoIdsMaximum": 100,
            "albumReorderMembersMaximum": 100,
            "retainedQueryIdsMaximum": 1000000,
            "retainedQueryIdleSeconds": 900,
            "recoveryPageMaximum": 60,
            "recoveryMappingsMaximum": 10000,
            "recoveryApplyMaximum": 100,
            "recoveryReviewIdleSeconds": 900
        }
    })
}

/// A stub service that answers the capabilities handshake and then serves one
/// fixed status and JSON body, for refusals a small fixture cannot reach.
fn stub_service(status: u16, body: Value) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        for request_index in 0..2 {
            let (mut stream, _) = common::accept_tls(&listener);
            let mut request = [0_u8; 8192];
            let count = stream.read(&mut request).unwrap();
            common::assert_bearer(&request[..count]);
            let response = if request_index == 0 {
                (200, capabilities_body())
            } else {
                (status, body.clone())
            };
            let bytes = serde_json::to_vec(&response.1).unwrap();
            write!(
                stream,
                "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response.0,
                if response.0 == 200 { "OK" } else { "Refused" },
                bytes.len()
            )
            .unwrap();
            stream.write_all(&bytes).unwrap();
        }
    });
    (format!("https://127.0.0.1:{}", address.port()), handle)
}

fn fixture() -> (PathBuf, Config) {
    let base = loop {
        let candidate = std::env::temp_dir().join(format!(
            "slipstream-cli-recovery-test-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::create_dir(&candidate) {
            Ok(()) => break candidate,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("create fixture: {error}"),
        }
    };
    let originals = base.join("originals");
    let web = base.join("web");
    fs::create_dir(&originals).unwrap();
    fs::create_dir(&web).unwrap();
    fs::create_dir(originals.join("shoot")).unwrap();
    fs::write(web.join("index.html"), b"<main>fixture</main>").unwrap();
    for name in ["a.JPG", "b.JPG", "c.JPG"] {
        fs::write(
            originals.join("shoot").join(name),
            format!("fixture-shoot-{name}"),
        )
        .unwrap();
    }
    let config = Config {
        library_root: originals,
        state_directory: base.join("state"),
        cache_directory: base.join("cache"),
        database_basename: "library.sqlite".to_owned(),
        host: "127.0.0.1".to_owned(),
        public_origin: "https://localhost".to_owned(),
        port: 0,
        web_root: Some(web),
        processing: None,
        export_retained_output_bytes: None,
        metadata_supervisor: None,
    };
    (base, config)
}

async fn command(server: &str, arguments: &[&str]) -> (u8, Value) {
    let server = server.to_owned();
    let arguments = arguments
        .iter()
        .map(|argument| (*argument).to_owned())
        .collect::<Vec<_>>();
    tokio::task::spawn_blocking(move || {
        let output = common::cli_command()
            .arg("--token-file")
            .arg(common::credential_file())
            .arg("--server")
            .arg(server)
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.stderr.is_empty(),
            "unexpected CLI stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        (
            output.status.code().unwrap() as u8,
            serde_json::from_slice(&output.stdout).unwrap(),
        )
    })
    .await
    .unwrap()
}

async fn wait_until_idle(server: &str) {
    for _ in 0..400 {
        let (exit, result) = command(server, &["status"]).await;
        if exit == 0 && result["data"]["scan"]["state"] == "idle" {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("fixture Library did not become idle");
}

fn write_document(base: &std::path::Path, name: &str, document: &Value) -> String {
    let path = base.join(format!("slipstream-recovery-{name}.json"));
    fs::write(&path, serde_json::to_vec(document).unwrap()).unwrap();
    path.to_str().unwrap().to_owned()
}

/// The reviewed walk: one bounded unavailable review with its continuation,
/// both proposal forms, a committed apply batch, a confirmed batch refusal,
/// and an expired review continuation, all against the real service.
#[tokio::test]
async fn recovery_walks_review_proposal_apply_and_refusals_through_the_service() {
    let (base, config) = fixture();
    let library_root = config.library_root.clone();
    let server = common::start_authenticated_server(config).await;

    // The first scan discovers the fixture files as available Photos.
    let (exit, _) = command(&server.url, &["library", "check"]).await;
    assert_eq!(exit, 0);
    wait_until_idle(&server.url).await;

    // Each Original moves off the Library after being scanned, so the next
    // scan leaves every Photo active but unavailable.
    let staging = base.join("staging");
    fs::create_dir(&staging).unwrap();
    for name in ["a.JPG", "b.JPG", "c.JPG"] {
        fs::rename(library_root.join("shoot").join(name), staging.join(name)).unwrap();
    }
    let (exit, _) = command(&server.url, &["library", "check"]).await;
    assert_eq!(exit, 0);
    wait_until_idle(&server.url).await;

    // One bounded review of the three unavailable Photos, walked in pages.
    let (exit, first) = command(&server.url, &["recovery", "unavailable", "--limit", "2"]).await;
    assert_eq!(exit, 0, "unavailable review: {first}");
    let items = first["data"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(first["data"]["total"], 3);
    let first_cursor = first["data"]["nextCursor"].as_str().unwrap().to_owned();
    assert!(first["data"]["expiresAt"].is_string());
    assert_eq!(items[0]["state"], "unavailable");
    assert_eq!(items[0]["location"], "shoot/a.JPG");
    assert_eq!(items[0]["kind"], "jpeg");
    assert_eq!(items[0]["originalId"].as_str().unwrap().len(), 36);
    assert_eq!(items[0]["photoId"].as_str().unwrap().len(), 36);
    assert!(items[0]["webUrl"].as_str().unwrap().starts_with("https://"));
    let original_a = items[0]["originalId"].as_str().unwrap().to_owned();
    let photo_a = items[0]["photoId"].as_str().unwrap().to_owned();
    let original_b = items[1]["originalId"].as_str().unwrap().to_owned();
    assert_eq!(items[1]["location"], "shoot/b.JPG");

    let (exit, second) = command(
        &server.url,
        &["recovery", "unavailable", "--cursor", &first_cursor],
    )
    .await;
    assert_eq!(exit, 0, "unavailable continuation: {second}");
    assert_eq!(second["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(second["data"]["total"], 3);
    assert_eq!(second["data"]["items"][0]["location"], "shoot/c.JPG");
    assert!(second["data"]["nextCursor"].is_null());
    assert!(second["data"]["expiresAt"].is_null());
    let original_c = second["data"]["items"][0]["originalId"]
        .as_str()
        .unwrap()
        .to_owned();
    let photo_c = second["data"]["items"][0]["photoId"]
        .as_str()
        .unwrap()
        .to_owned();

    // A continuation outlives its retained review: opening enough fresh
    // reviews evicts the oldest, and its cursor then expires explicitly.
    let (exit, oldest) = command(&server.url, &["recovery", "unavailable", "--limit", "1"]).await;
    assert_eq!(exit, 0);
    let oldest_cursor = oldest["data"]["nextCursor"].as_str().unwrap().to_owned();
    assert!(oldest["data"]["expiresAt"].is_string());
    for _ in 0..8 {
        let (exit, _) = command(&server.url, &["recovery", "unavailable", "--limit", "1"]).await;
        assert_eq!(exit, 0);
    }
    let (exit, expired) = command(
        &server.url,
        &["recovery", "unavailable", "--cursor", &oldest_cursor],
    )
    .await;
    assert_eq!(exit, 6, "expired continuation: {expired}");
    assert_eq!(expired["error"]["code"], "cursor_expired");
    assert_eq!(expired["error"]["effect"], "none");
    assert_eq!(expired["error"]["details"]["cursorKind"], "unavailable");
    assert_eq!(expired["error"]["details"]["reason"], "idle_or_evicted");

    // The moved files reappear under new remembered Locations. No scan runs
    // after this point, so the destinations are occupied on disk only.
    fs::create_dir(library_root.join("moved")).unwrap();
    fs::rename(staging.join("a.JPG"), library_root.join("moved/a.JPG")).unwrap();
    fs::rename(staging.join("b.JPG"), library_root.join("moved/b.JPG")).unwrap();
    fs::rename(
        staging.join("c.JPG"),
        library_root.join("moved/renamed-c.JPG"),
    )
    .unwrap();

    // The Folder-prefix proposal, walked in pages.
    let (exit, proposals) = command(
        &server.url,
        &[
            "recovery",
            "propose",
            "--old-prefix",
            "shoot",
            "--new-prefix",
            "moved",
            "--limit",
            "1",
        ],
    )
    .await;
    assert_eq!(exit, 0, "prefix proposal: {proposals}");
    assert_eq!(proposals["data"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(proposals["data"]["total"], 3);
    let proposal_cursor = proposals["data"]["nextCursor"].as_str().unwrap().to_owned();
    assert!(proposals["data"]["expiresAt"].is_string());
    let mapping_a = proposals["data"]["items"][0].clone();
    assert_eq!(mapping_a["outcome"], "matched");
    assert_eq!(mapping_a["blockedReason"], Value::Null);
    assert_eq!(mapping_a["fromLocation"], "shoot/a.JPG");
    assert_eq!(mapping_a["toLocation"], "moved/a.JPG");
    assert_eq!(mapping_a["kind"], "jpeg");
    assert_eq!(mapping_a["originalId"], original_a);
    assert!(!mapping_a["mappingId"].as_str().unwrap().is_empty());
    assert!(mapping_a["verified"].is_boolean());
    assert!(mapping_a["retire"].is_null());

    let (exit, rest) = command(
        &server.url,
        &["recovery", "propose", "--cursor", &proposal_cursor],
    )
    .await;
    assert_eq!(exit, 0, "proposal continuation: {rest}");
    // The retained review keeps the opening page bound, so the remaining
    // two mappings arrive one window at a time.
    let continued = rest["data"]["items"].as_array().unwrap();
    assert_eq!(continued.len(), 1);
    assert_eq!(rest["data"]["total"], 3);
    assert_eq!(continued[0]["toLocation"], "moved/b.JPG");
    assert_eq!(continued[0]["outcome"], "matched");
    let rest_cursor = rest["data"]["nextCursor"].as_str().unwrap().to_owned();
    assert!(rest["data"]["expiresAt"].is_string());
    let (exit, tail) = command(
        &server.url,
        &["recovery", "propose", "--cursor", &rest_cursor],
    )
    .await;
    assert_eq!(exit, 0, "proposal tail: {tail}");
    let tail_items = tail["data"]["items"].as_array().unwrap();
    assert_eq!(tail_items.len(), 1);
    assert_eq!(tail["data"]["total"], 3);
    // The renamed file is not at the prefix-mapped destination, so that
    // mapping is present but blocked: absent without a fingerprint, or
    // unreadable when the remembered digest cannot be read there.
    assert_eq!(tail_items[0]["toLocation"], "moved/c.JPG");
    let blocked = tail_items[0]["outcome"].as_str().unwrap();
    assert!(
        matches!(blocked, "missing" | "unreadable"),
        "unexpected blocked outcome: {blocked}"
    );
    assert_eq!(tail_items[0]["blockedReason"], blocked);
    assert_eq!(tail_items[0]["verified"], false);
    assert!(tail["data"]["nextCursor"].is_null());
    assert!(tail["data"]["expiresAt"].is_null());

    // The single form evaluates exactly one renamed mapping without a
    // continuation.
    let (exit, single) = command(
        &server.url,
        &[
            "recovery",
            "propose",
            "--original-id",
            &original_c,
            "--new-location",
            "moved/renamed-c.JPG",
        ],
    )
    .await;
    assert_eq!(exit, 0, "single proposal: {single}");
    assert_eq!(single["data"]["total"], 1);
    assert!(single["data"]["nextCursor"].is_null());
    assert!(single["data"]["expiresAt"].is_null());
    let mapping_c = single["data"]["items"][0].clone();
    assert_eq!(mapping_c["outcome"], "matched");
    assert_eq!(mapping_c["toLocation"], "moved/renamed-c.JPG");

    // One apply batch commits both reviewed mappings atomically.
    let apply_input = write_document(
        &base,
        "apply.json",
        &json!({
            "mappings": [
                {
                    "originalId": mapping_a["originalId"],
                    "newLocation": mapping_a["toLocation"],
                    "mappingId": mapping_a["mappingId"],
                    "confirmUnverifiedContent": true
                },
                {
                    "originalId": mapping_c["originalId"],
                    "newLocation": mapping_c["toLocation"],
                    "mappingId": mapping_c["mappingId"],
                    "confirmUnverifiedContent": true
                }
            ]
        }),
    );
    let (exit, applied) =
        command(&server.url, &["recovery", "apply", "--input", &apply_input]).await;
    assert_eq!(exit, 0, "apply: {applied}");
    assert_eq!(applied["data"]["appliedMappings"], 2);
    assert_eq!(applied["data"]["refusedMappings"], 0);
    assert_eq!(applied["data"]["unavailablePhotos"], 1);
    let committed = applied["data"]["mappings"].as_array().unwrap();
    assert_eq!(committed.len(), 2);
    assert_eq!(committed[0]["originalId"], original_a);
    assert_eq!(committed[0]["photoId"], photo_a);
    assert_eq!(committed[0]["fromLocation"], "shoot/a.JPG");
    assert_eq!(committed[0]["toLocation"], "moved/a.JPG");
    assert_eq!(committed[0]["retired"], Value::Null);
    assert!(
        committed[0]["webUrl"]
            .as_str()
            .unwrap()
            .starts_with("https://")
    );
    assert_eq!(committed[1]["originalId"], original_c);
    assert_eq!(committed[1]["toLocation"], "moved/renamed-c.JPG");

    // The committed association reads back through photos get: availability
    // restored, and the renamed Location is the Photo's filename now.
    let (exit, read_a) = command(&server.url, &["photos", "get", &photo_a]).await;
    assert_eq!(exit, 0);
    assert_eq!(read_a["data"]["id"], photo_a);
    assert_eq!(read_a["data"]["originalAvailable"], true);
    assert_eq!(read_a["data"]["filename"], "a.JPG");
    let (exit, read_c) = command(&server.url, &["photos", "get", &photo_c]).await;
    assert_eq!(exit, 0);
    assert_eq!(read_c["data"]["originalAvailable"], true);
    assert_eq!(read_c["data"]["filename"], "renamed-c.JPG");

    // A batch whose reviewed identity is stale is refused completely with
    // one reason per mapping, and nothing changes.
    let stale_input = write_document(
        &base,
        "stale.json",
        &json!({
            "mappings": [
                {
                    "originalId": original_b,
                    "newLocation": "moved/b.JPG",
                    "mappingId": "not-the-reviewed-identity",
                    "confirmUnverifiedContent": true
                }
            ]
        }),
    );
    let (exit, refused) =
        command(&server.url, &["recovery", "apply", "--input", &stale_input]).await;
    assert_eq!(exit, 4, "stale apply: {refused}");
    assert_eq!(refused["error"]["code"], "recovery_conflict");
    assert_eq!(refused["error"]["effect"], "none");
    assert_eq!(refused["error"]["details"]["appliedMappings"], 0);
    assert_eq!(refused["error"]["details"]["refusedMappings"], 1);
    let rejections = refused["error"]["details"]["rejections"]
        .as_array()
        .unwrap();
    assert_eq!(rejections.len(), 1);
    assert_eq!(rejections[0]["originalId"], original_b);
    assert!(!rejections[0]["reason"].as_str().unwrap().is_empty());

    // The refused Photo is still unavailable and unchanged.
    let (exit, remaining) = command(&server.url, &["recovery", "unavailable"]).await;
    assert_eq!(exit, 0);
    assert_eq!(remaining["data"]["total"], 1);
    assert_eq!(remaining["data"]["items"][0]["location"], "shoot/b.JPG");
    assert!(remaining["data"]["nextCursor"].is_null());
    assert!(remaining["data"]["expiresAt"].is_null());

    fs::remove_dir_all(base).unwrap();
}

/// Apply documents validate locally, before any request can be admitted.
#[tokio::test]
async fn recovery_apply_input_refusals_precede_network_access() {
    let dead = "https://127.0.0.1:1";
    let base = std::env::temp_dir().join(format!(
        "slipstream-cli-recovery-local-{}",
        std::process::id()
    ));
    fs::create_dir_all(&base).unwrap();

    let valid = write_document(
        &base,
        "valid.json",
        &json!({
            "mappings": [
                {
                    "originalId": "00000000-0000-4000-8000-000000000001",
                    "newLocation": "moved/a.JPG",
                    "mappingId": "reviewed-mapping-identity"
                }
            ]
        }),
    );
    let (exit, envelope) = command(dead, &["recovery", "apply", "--input", &valid]).await;
    assert_eq!(exit, 6, "{envelope}");
    assert_eq!(envelope["error"]["code"], "transport_failed");
    assert_eq!(envelope["error"]["details"]["operation"], "recovery-apply");

    for (name, expected_reason, document) in [
        (
            "unknown-key",
            "A mapping contains an unknown key.",
            json!({"mappings": [{"originalId": "00000000-0000-4000-8000-000000000001", "newLocation": "moved/a.JPG", "mappingId": "m", "force": true}]}),
        ),
        (
            "empty",
            "The mappings array must contain at least one reviewed mapping.",
            json!({"mappings": []}),
        ),
        (
            "duplicate",
            "One Original appears in more than one mapping.",
            json!({"mappings": [
                {"originalId": "00000000-0000-4000-8000-000000000001", "newLocation": "moved/a.JPG", "mappingId": "m"},
                {"originalId": "00000000-0000-4000-8000-000000000001", "newLocation": "moved/b.JPG", "mappingId": "m"}
            ]}),
        ),
    ] {
        let path = write_document(&base, &format!("bad-{name}.json"), &document);
        let (exit, envelope) = command(dead, &["recovery", "apply", "--input", &path]).await;
        assert_eq!(exit, 2, "{name}: {envelope}");
        assert_eq!(envelope["error"]["code"], "invalid_input", "{name}");
        assert_eq!(envelope["error"]["effect"], "none", "{name}");
        assert_eq!(
            envelope["error"]["details"]["reason"], expected_reason,
            "{name}"
        );
    }

    let (exit, envelope) = command(
        dead,
        &["recovery", "apply", "--input", "/nonexistent/recovery.json"],
    )
    .await;
    assert_eq!(exit, 6);
    assert_eq!(envelope["error"]["code"], "local_io_failed");

    fs::remove_dir_all(base).unwrap();
}

/// An over-bound Folder-prefix scope is a confirmed request-limit refusal.
/// The bound exceeds anything a fixture can reach, so the stub service
/// replays the documented refusal.
#[tokio::test]
async fn recovery_scope_refusal_is_a_confirmed_request_limit() {
    let (server, handle) = stub_service(
        413,
        json!({
            "error": {
                "code": "recovery_scope_exceeded",
                "message": "Narrow the Folder prefix before reviewing this scope.",
                "effect": "none",
                "details": {"evaluated": 10_001, "limit": 10_000}
            }
        }),
    );
    let (exit, envelope) = command(
        &server,
        &[
            "recovery",
            "propose",
            "--old-prefix",
            "",
            "--new-prefix",
            "moved",
        ],
    )
    .await;
    assert_eq!(exit, 2, "{envelope}");
    assert_eq!(envelope["error"]["code"], "recovery_scope_exceeded");
    assert_eq!(envelope["error"]["effect"], "none");
    assert_eq!(
        envelope["error"]["details"],
        json!({"evaluated": 10_001, "limit": 10_000})
    );
    handle.join().unwrap();
}
