use serde_json::Value;
use slipstream_server::Config;
mod common;
use std::{
    fs,
    io::{ErrorKind, Read, Write},
    net::TcpListener,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread::JoinHandle,
    time::{Duration, Instant},
};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

fn fake_service(response: Value) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        for request_index in 0..2 {
            let (mut stream, _) = common::accept_tls(&listener);
            let mut request = [0_u8; 8192];
            let count = stream.read(&mut request).unwrap();
            common::assert_bearer(&request[..count]);
            let body = if request_index == 0 {
                serde_json::json!({
                    "serverVersion": "0.0.0",
                    "supportedCliContractVersions": [1],
                    "limits": {
                        "listPageMaximum": 60,
                        "mutationPhotoIdsMaximum": 100,
                        "albumReorderMembersMaximum": 100,
                        "retainedQueryIdsMaximum": 1000000,
                        "retainedQueryIdleSeconds": 900
                    }
                })
            } else {
                response.clone()
            };
            let bytes = serde_json::to_vec(&body).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            )
            .unwrap();
            stream.write_all(&bytes).unwrap();
        }
    });
    (format!("https://127.0.0.1:{}", address.port()), handle)
}

fn wait_for_exit(child: &mut Child, maximum: Duration) -> Option<std::process::ExitStatus> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return Some(status);
        }
        if started.elapsed() >= maximum {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn fixture() -> (PathBuf, Config) {
    let base = loop {
        let candidate = std::env::temp_dir().join(format!(
            "slipstream-cli-service-test-{}-{}",
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
    fs::create_dir(originals.join("trip")).unwrap();
    fs::write(web.join("index.html"), b"<main>fixture</main>").unwrap();
    for name in ["one.JPG", "two.JPG", "three.JPG", "four.JPG", "five.JPG"] {
        fs::write(originals.join("trip").join(name), format!("fixture-{name}")).unwrap();
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

async fn post_json(server: &str, path: &str, body: Value) -> reqwest::Response {
    reqwest::Client::builder()
        .add_root_certificate(common::test_certificate())
        .build()
        .unwrap()
        .post(format!("{server}{path}"))
        .bearer_auth(common::ACCESS_TOKEN)
        .json(&body)
        .send()
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

#[test]
fn executable_normalizes_parser_errors_into_the_selected_envelope() {
    let output = Command::new(env!("CARGO_BIN_EXE_slipstream"))
        .args(["photos", "list", "--rating-min", "7"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stderr.is_empty());
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["schemaVersion"], 1);
    assert_eq!(envelope["status"], "error");
    assert_eq!(envelope["error"]["code"], "invalid_input");
    assert_eq!(envelope["error"]["effect"], "none");

    for output_arguments in [["--output", "text"], ["--output=text", ""]] {
        let mut arguments = output_arguments
            .into_iter()
            .filter(|argument| !argument.is_empty())
            .collect::<Vec<_>>();
        arguments.extend(["photos", "list", "--rating-min", "7"]);
        let text = Command::new(env!("CARGO_BIN_EXE_slipstream"))
            .args(arguments)
            .output()
            .unwrap();
        assert_eq!(text.status.code(), Some(2));
        assert!(text.stderr.is_empty());
        assert!(
            String::from_utf8(text.stdout)
                .unwrap()
                .starts_with("Error: invalid_input\n")
        );
    }

    let invalid_output = Command::new(env!("CARGO_BIN_EXE_slipstream"))
        .args(["--output=invalid", "status"])
        .output()
        .unwrap();
    assert_eq!(invalid_output.status.code(), Some(2));
    assert!(invalid_output.stderr.is_empty());
    let envelope: Value = serde_json::from_slice(&invalid_output.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "invalid_input");
}

#[test]
fn executable_parser_error_output_obeys_the_recovered_deadline() {
    let invalid_rating = "7".repeat(100_000);
    let mut child = Command::new(env!("CARGO_BIN_EXE_slipstream"))
        .args([
            "--timeout",
            "1",
            "photos",
            "list",
            "--rating-min",
            &invalid_rating,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let status = wait_for_exit(&mut child, Duration::from_secs(2))
        .expect("blocked parser-error output must not outlive its recovered deadline");
    assert_eq!(status.code(), Some(6));
    let output = child.wait_with_output().unwrap();
    assert!(output.stderr.is_empty());
    assert!(output.stdout.len() < invalid_rating.len());
}

#[tokio::test]
async fn client_rejects_folder_album_and_photo_pages_over_the_requested_limit() {
    let cases = [
        (
            vec!["folders", "list", "--limit", "1"],
            serde_json::json!({
                "items": [
                    {"location":"a","name":"a","photoCount":1,"hasDescendantFolders":false},
                    {"location":"b","name":"b","photoCount":1,"hasDescendantFolders":false}
                ],
                "total": 2,
                "nextCursor": null,
                "evaluatedAt": "2026-01-01T00:00:00Z",
                "expiresAt": null,
                "publication": "p1",
                "parent": ""
            }),
            "folders-list",
        ),
        (
            vec!["albums", "list", "--limit", "1"],
            serde_json::json!({
                "items": [{"id":"a","state":"missing"},{"id":"b","state":"missing"}],
                "total": 2,
                "nextCursor": null,
                "evaluatedAt": "2026-01-01T00:00:00Z",
                "expiresAt": null
            }),
            "albums-list",
        ),
        (
            vec!["photos", "list", "--limit", "1"],
            serde_json::json!({
                "items": [{"id":"a","state":"missing"},{"id":"b","state":"missing"}],
                "total": 2,
                "nextCursor": null,
                "evaluatedAt": "2026-01-01T00:00:00Z",
                "expiresAt": null
            }),
            "photos-list",
        ),
    ];
    for (arguments, response, operation) in cases {
        let (server, handle) = fake_service(response);
        let (exit, envelope) = command(&server, &arguments).await;
        handle.join().unwrap();
        assert_eq!(exit, 6);
        assert_eq!(envelope["error"]["code"], "transport_failed");
        assert_eq!(envelope["error"]["details"]["operation"], operation);
    }
}

#[test]
fn executable_network_timeout_publishes_an_envelope_when_stdout_is_writable() {
    let (server, stalled) = common::stalled_tls_service();
    let output = common::cli_command()
        .args(["--token-file"])
        .arg(common::credential_file())
        .args(["--server", &server, "--timeout", "1", "status"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(6));
    assert!(output.stderr.is_empty());
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "transport_failed");
    stalled.join().unwrap();
}

#[test]
fn executable_output_deadline_and_interrupt_do_not_wait_for_blocked_pipes() {
    let version = "v".repeat(256 * 1024);
    let status = serde_json::json!({
        "serverVersion": version,
        "cliContractVersion": 1,
        "published": true,
        "publication": "p1",
        "photoCount": 0,
        "scan": {
            "state": "idle",
            "publication": "p1",
            "completed": 0,
            "total": 0,
            "lastRecovery": null,
            "fingerprints": null
        }
    });
    let (server, handle) = fake_service(status.clone());
    let mut timed = common::cli_command()
        .args(["--token-file"])
        .arg(common::credential_file())
        .args(["--server", &server, "--timeout", "1", "status"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let status = wait_for_exit(&mut timed, Duration::from_secs(2))
        .expect("blocked stdout/stderr must not outlive the command deadline");
    assert_eq!(status.code(), Some(6));
    let output = timed.wait_with_output().unwrap();
    assert!(output.stdout.len() <= 256 * 1024);
    handle.join().unwrap();

    let (server, handle) = fake_service(status_payload_with_version(256 * 1024));
    let mut interrupted = common::cli_command()
        .args(["--token-file"])
        .arg(common::credential_file())
        .args(["--server", &server, "--timeout", "300", "status"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(250));
    let signal = Command::new("/bin/kill")
        .args(["-INT", &interrupted.id().to_string()])
        .status()
        .unwrap();
    assert!(signal.success());
    let status = wait_for_exit(&mut interrupted, Duration::from_secs(2))
        .expect("blocked output must not delay handled interruption");
    assert_eq!(status.code(), Some(130));
    let _ = interrupted.wait_with_output().unwrap();
    handle.join().unwrap();
}

fn status_payload_with_version(length: usize) -> Value {
    serde_json::json!({
        "serverVersion": "v".repeat(length),
        "cliContractVersion": 1,
        "published": true,
        "publication": "p1",
        "photoCount": 0,
        "scan": {
            "state": "idle",
            "publication": "p1",
            "completed": 0,
            "total": 0,
            "lastRecovery": null,
            "fingerprints": null
        }
    })
}

#[tokio::test]
async fn whole_command_deadline_covers_capability_negotiation() {
    let (server, stalled) = common::stalled_tls_service();
    let started = std::time::Instant::now();
    let output = common::cli_command()
        .args(["--token-file"])
        .arg(common::credential_file())
        .args(["--server", &server, "--timeout", "1", "status"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(6));
    assert!(started.elapsed() < Duration::from_secs(3));
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "transport_failed");
    assert_eq!(envelope["error"]["details"]["operation"], "status");
    stalled.join().unwrap();
}

#[tokio::test]
async fn executable_queries_the_real_service_with_fixed_multi_page_membership() {
    let (base, config) = fixture();
    let server = common::start_authenticated_server(config).await;
    wait_until_idle(&server.url).await;

    let (exit, status) = command(&server.url, &["status"]).await;
    assert_eq!(exit, 0);
    assert_eq!(status["schemaVersion"], 1);
    assert_eq!(status["data"]["cliContractVersion"], 1);
    assert_eq!(status["data"]["photoCount"], 5);

    let (exit, folders) = command(&server.url, &["folders", "list", "--limit", "1"]).await;
    assert_eq!(exit, 0);
    assert_eq!(folders["data"]["items"][0]["location"], "trip");
    assert_eq!(folders["data"]["items"][0]["photoCount"], 5);
    assert!(folders["data"]["expiresAt"].is_null());

    let (exit, baseline) = command(
        &server.url,
        &["photos", "list", "--folder", "trip", "--limit", "60"],
    )
    .await;
    assert_eq!(exit, 0);
    let expected_ids = baseline["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(expected_ids.len(), 5);
    let second_version = baseline["data"]["items"][1]["decisionVersion"]
        .as_str()
        .unwrap()
        .to_owned();

    let (exit, first) = command(
        &server.url,
        &["photos", "list", "--folder", "trip", "--limit", "1"],
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(first["data"]["total"], 5);
    assert_eq!(first["data"]["items"][0]["id"], expected_ids[0]);
    let mut cursor = first["data"]["nextCursor"].as_str().unwrap().to_owned();
    let mut traversed_ids = vec![expected_ids[0].clone()];

    let response = post_json(
        &server.url,
        &format!("/api/photos/{}/state", expected_ids[1]),
        serde_json::json!({"field": "rating", "value": 4}),
    )
    .await;
    assert!(response.status().is_success());
    let response = post_json(
        &server.url,
        &format!("/api/photos/{}/state", expected_ids[1]),
        serde_json::json!({"field": "selectionState", "value": "selected"}),
    )
    .await;
    assert!(response.status().is_success());

    let (exit, second) = command(
        &server.url,
        &["photos", "list", "--cursor", cursor.as_str()],
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(second["data"]["items"][0]["id"], expected_ids[1]);
    assert_eq!(second["data"]["items"][0]["rating"], 4);
    assert_eq!(second["data"]["items"][0]["selectionState"], "selected");
    assert_ne!(
        second["data"]["items"][0]["decisionVersion"],
        second_version
    );
    traversed_ids.push(expected_ids[1].clone());
    cursor = second["data"]["nextCursor"].as_str().unwrap().to_owned();

    let response = post_json(&server.url, "/api/scan", serde_json::json!({})).await;
    assert!(response.status().is_success());
    wait_until_idle(&server.url).await;

    loop {
        let (exit, page) = command(
            &server.url,
            &["photos", "list", "--cursor", cursor.as_str()],
        )
        .await;
        assert_eq!(exit, 0);
        traversed_ids.push(page["data"]["items"][0]["id"].as_str().unwrap().to_owned());
        if let Some(next) = page["data"]["nextCursor"].as_str() {
            cursor = next.to_owned();
        } else {
            assert!(page["data"]["expiresAt"].is_null());
            break;
        }
    }
    assert_eq!(traversed_ids, expected_ids);
    assert_eq!(
        traversed_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        5
    );

    let (exit, filtered) = command(
        &server.url,
        &[
            "photos",
            "list",
            "--folder",
            "trip",
            "--selection",
            "selected",
            "--rating-min",
            "4",
            "--rating-max",
            "5",
            "--kind",
            "jpeg",
            "--available",
            "true",
            "--order",
            "capture-time-desc",
        ],
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(filtered["data"]["total"], 1);
    assert_eq!(filtered["data"]["items"][0]["id"], expected_ids[1]);

    let changed_id = &expected_ids[1];
    let (exit, photo) = command(&server.url, &["photos", "get", changed_id]).await;
    assert_eq!(exit, 0);
    assert_eq!(photo["data"]["id"], *changed_id);
    assert_eq!(photo["data"]["rating"], 4);
    assert!(
        photo["data"]["webUrl"]
            .as_str()
            .unwrap()
            .starts_with(&server.url)
    );
    assert!(photo["data"]["metadata"].is_object());

    let created: Value = post_json(
        &server.url,
        "/api/albums",
        serde_json::json!({"name": "CLI fixture"}),
    )
    .await
    .json()
    .await
    .unwrap();
    let album_id = created["albums"][0]["id"].as_str().unwrap().to_owned();
    let album_order = vec![
        expected_ids[4].clone(),
        expected_ids[1].clone(),
        expected_ids[3].clone(),
    ];
    let response = post_json(
        &server.url,
        &format!("/api/albums/{album_id}/members"),
        serde_json::json!({"photoIds": album_order}),
    )
    .await;
    assert!(response.status().is_success());

    let (exit, albums) = command(&server.url, &["albums", "list", "--name", "cli FIXTURE"]).await;
    assert_eq!(exit, 0);
    assert_eq!(albums["data"]["items"][0]["id"], album_id);
    assert!(
        albums["data"]["items"][0]["webUrl"]
            .as_str()
            .unwrap()
            .starts_with(&server.url)
    );

    let (exit, containing) = command(&server.url, &["albums", "list", "--photo", changed_id]).await;
    assert_eq!(exit, 0);
    assert_eq!(containing["data"]["total"], 1);
    assert_eq!(containing["data"]["items"][0]["id"], album_id);

    let (exit, album_photos) = command(
        &server.url,
        &[
            "photos",
            "list",
            "--album",
            &album_id,
            "--order",
            "album-order",
            "--limit",
            "60",
        ],
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(
        album_photos["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        album_order.iter().map(String::as_str).collect::<Vec<_>>()
    );

    let (exit, album) = command(&server.url, &["albums", "get", &album_id]).await;
    assert_eq!(exit, 0);
    assert_eq!(album["data"]["name"], "CLI fixture");
    assert_eq!(album["data"]["photoCount"], 3);
    assert!(album["data"]["albumVersion"].as_str().unwrap().len() > 1);

    let (exit, missing) = command(
        &server.url,
        &["photos", "get", "00000000-0000-4000-8000-000000000000"],
    )
    .await;
    assert_eq!(exit, 3);
    assert_eq!(missing["status"], "error");
    assert_eq!(missing["error"]["code"], "not_found");

    server.close().await.unwrap();
    fs::remove_dir_all(base).unwrap();
}
