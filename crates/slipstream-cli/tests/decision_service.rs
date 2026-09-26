use serde_json::{Value, json};
use slipstream_server::{Config, start_server};
mod common;
use std::{
    fs,
    io::{ErrorKind, Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

/// The reply a stub service gives to the one mutation request after the
/// capabilities handshake.
enum MutationReply {
    /// Accept the request, then close the connection without responding.
    Drop,
    /// Announce more body bytes than are sent, then close mid-body.
    Truncated,
    /// Answer with an arbitrary HTTP status and JSON body.
    Status(u16, Value),
}

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
            "retainedQueryIdleSeconds": 900
        }
    })
}

fn write_json_response(stream: &mut impl Write, status: u16, body: &Value) {
    let bytes = serde_json::to_vec(body).unwrap();
    let _ = write!(
        stream,
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status,
        if status == 200 {
            "OK"
        } else if status == 409 {
            "Conflict"
        } else {
            "Error"
        },
        bytes.len()
    );
    let _ = stream.write_all(&bytes);
}

/// A stub service that answers the capabilities handshake, records the one
/// following mutation request, and answers it with the chosen reply.
fn stub_service(reply: MutationReply) -> (String, JoinHandle<()>, Arc<AtomicUsize>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let mutations = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&mutations);
    let handle = std::thread::spawn(move || {
        for request_index in 0..2 {
            let (mut stream, _) = common::accept_tls(&listener);
            let mut request = [0_u8; 8192];
            let count = stream.read(&mut request).unwrap();
            common::assert_bearer(&request[..count]);
            if request_index == 0 {
                write_json_response(&mut stream, 200, &capabilities_body());
            } else {
                observed.fetch_add(1, Ordering::SeqCst);
                match reply {
                    MutationReply::Drop => drop(stream),
                    MutationReply::Truncated => {
                        let body = b"{\"results\":";
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len() + 64
                        );
                        let _ = stream.write_all(body);
                        drop(stream);
                    }
                    MutationReply::Status(status, ref body) => {
                        write_json_response(&mut stream, status, body)
                    }
                }
            }
        }
    });
    (
        format!("https://127.0.0.1:{}", address.port()),
        handle,
        mutations,
    )
}

fn fixture_with(photo_names: &[&str]) -> (PathBuf, Config) {
    let base = loop {
        let candidate = std::env::temp_dir().join(format!(
            "slipstream-cli-decision-test-{}-{}",
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
    for name in photo_names {
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
        processing: None,
        export_retained_output_bytes: None,
    };
    (base, config)
}

async fn command(server: &str, arguments: &[&str]) -> (u8, Value) {
    command_with_stdin(server, arguments, "").await
}

async fn command_with_stdin(server: &str, arguments: &[&str], stdin: &str) -> (u8, Value) {
    let server = server.to_owned();
    let arguments = arguments
        .iter()
        .map(|argument| (*argument).to_owned())
        .collect::<Vec<_>>();
    let stdin = stdin.to_owned();
    tokio::task::spawn_blocking(move || {
        let mut child = common::cli_command()
            .arg("--token-file")
            .arg(common::credential_file())
            .arg("--server")
            .arg(server)
            .args(arguments)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        // A failed write only means the command exited before reading stdin.
        let _ = child.stdin.as_mut().unwrap().write_all(stdin.as_bytes());
        let output = child.wait_with_output().unwrap();
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

async fn photo_page(server: &str) -> Vec<(String, String)> {
    let (exit, page) = command(server, &["photos", "list", "--limit", "60"]).await;
    assert_eq!(exit, 0);
    page["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["id"].as_str().unwrap().to_owned(),
                item["decisionVersion"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn decision_document(field: &str, value: Value, photos: &[(String, String)]) -> String {
    serde_json::to_string(&json!({
        "field": field,
        "value": value,
        "photos": photos
            .iter()
            .map(|(photo_id, if_version)| json!({
                "photoId": photo_id,
                "ifVersion": if_version,
            }))
            .collect::<Vec<_>>()
    }))
    .unwrap()
}

fn sorted_keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    keys
}

fn write_input(base: &std::path::Path, name: &str, content: &str) -> String {
    let path = base.join(name);
    fs::write(&path, content).unwrap();
    path.to_str().unwrap().to_owned()
}

fn missing_photo_id(index: usize) -> String {
    format!("00000000-0000-4000-8000-{index:012x}")
}

#[tokio::test]
async fn cli_photo_decision_forms_apply_checked_decisions_and_isolate_fields() {
    let (base, config) = fixture_with(&["one.JPG", "two.JPG", "three.JPG"]);
    let server = common::start_authenticated_server(config).await;
    wait_until_idle(&server.url).await;
    let photos = photo_page(&server.url).await;
    assert_eq!(photos.len(), 3);

    // The single Selection State form is a one-item batch with prior and
    // current decisions in the exact reference shape.
    let (exit, set) = command(
        &server.url,
        &[
            "photos",
            "set",
            &photos[0].0,
            "--selection",
            "selected",
            "--if-version",
            &photos[0].1,
        ],
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(set["schemaVersion"], 1);
    assert_eq!(set["status"], "ok");
    assert_eq!(set["error"], Value::Null);
    assert_eq!(
        set["data"]["counts"],
        json!({"changed": 1, "unchanged": 0, "conflict": 0, "missing": 0})
    );
    let changed = &set["data"]["results"][0];
    assert_eq!(
        sorted_keys(changed),
        ["current", "outcome", "photoId", "prior"]
    );
    assert_eq!(changed["photoId"], photos[0].0);
    assert_eq!(changed["outcome"], "changed");
    assert_eq!(
        changed["prior"],
        json!({"selectionState": "undecided", "rating": 0})
    );
    assert_eq!(changed["current"]["selectionState"], "selected");
    assert_eq!(changed["current"]["rating"], 0);
    let selected_version = changed["current"]["decisionVersion"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(!selected_version.is_empty());
    assert_ne!(selected_version, photos[0].1);

    // A no-op write reports unchanged and keeps the observed version.
    let (exit, unchanged) = command(
        &server.url,
        &[
            "photos",
            "set",
            &photos[0].0,
            "--selection",
            "selected",
            "--if-version",
            &selected_version,
        ],
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(
        unchanged["data"]["counts"],
        json!({"changed": 0, "unchanged": 1, "conflict": 0, "missing": 0})
    );
    let unchanged_item = &unchanged["data"]["results"][0];
    assert_eq!(
        sorted_keys(unchanged_item),
        ["current", "outcome", "photoId"]
    );
    assert_eq!(unchanged_item["outcome"], "unchanged");
    assert_eq!(
        unchanged_item["current"]["decisionVersion"],
        selected_version
    );

    // A Rating write changes only Rating, never the other decision field.
    let (exit, rated) = command(
        &server.url,
        &[
            "photos",
            "set",
            &photos[0].0,
            "--rating",
            "4",
            "--if-version",
            &selected_version,
        ],
    )
    .await;
    assert_eq!(exit, 0);
    let rated_item = &rated["data"]["results"][0];
    assert_eq!(rated_item["outcome"], "changed");
    assert_eq!(
        rated_item["prior"],
        json!({"selectionState": "selected", "rating": 0})
    );
    assert_eq!(rated_item["current"]["selectionState"], "selected");
    assert_eq!(rated_item["current"]["rating"], 4);
    let rated_version = rated_item["current"]["decisionVersion"]
        .as_str()
        .unwrap()
        .to_owned();

    // Clearing Rating means zero.
    let (exit, cleared) = command(
        &server.url,
        &[
            "photos",
            "set",
            &photos[0].0,
            "--rating",
            "0",
            "--if-version",
            &rated_version,
        ],
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(cleared["data"]["results"][0]["current"]["rating"], 0);
    assert_eq!(
        cleared["data"]["results"][0]["current"]["selectionState"],
        "selected"
    );

    // The batch form applies one field to every named Photo in order.
    let batch = [
        (photos[1].0.clone(), photos[1].1.clone()),
        (photos[2].0.clone(), photos[2].1.clone()),
    ];
    let (exit, batched) = command_with_stdin(
        &server.url,
        &["photos", "set", "--input", "-"],
        &decision_document("rating", json!(2), &batch),
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(
        batched["data"]["counts"],
        json!({"changed": 2, "unchanged": 0, "conflict": 0, "missing": 0})
    );
    assert_eq!(
        batched["data"]["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["photoId"].as_str().unwrap())
            .collect::<Vec<_>>(),
        batch.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>()
    );

    // A CLI decision changes neither the Album version nor its saved
    // browsing position.
    let (exit, created) =
        command(&server.url, &["albums", "create", "--name", "Resume check"]).await;
    assert_eq!(exit, 0);
    let album_id = created["data"]["album"]["id"].as_str().unwrap().to_owned();
    let album_version = created["data"]["album"]["albumVersion"]
        .as_str()
        .unwrap()
        .to_owned();
    let (exit, added) = command_with_stdin(
        &server.url,
        &[
            "albums",
            "add",
            &album_id,
            "--input",
            "-",
            "--if-version",
            &album_version,
        ],
        &serde_json::to_string(&json!({"photoIds": [photos[0].0]})).unwrap(),
    )
    .await;
    assert_eq!(exit, 0);
    let member_version = added["data"]["album"]["albumVersion"]
        .as_str()
        .unwrap()
        .to_owned();
    let response = post_json(
        &server.url,
        &format!("/api/albums/{album_id}/progress"),
        json!({"photoId": photos[0].0}),
    )
    .await;
    assert!(response.status().is_success());

    let fresh = photo_page(&server.url).await;
    let (exit, decided) = command(
        &server.url,
        &[
            "photos",
            "set",
            &photos[0].0,
            "--rating",
            "5",
            "--if-version",
            &fresh[0].1,
        ],
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(decided["data"]["results"][0]["outcome"], "changed");
    let (exit, summary) = command(&server.url, &["albums", "get", &album_id]).await;
    assert_eq!(exit, 0);
    assert_eq!(summary["data"]["albumVersion"], member_version);
    assert_eq!(summary["data"]["hasSavedPosition"], true);

    server.close().await.unwrap();
    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn cli_photo_batches_report_truthful_partials_and_all_failed_partitions() {
    let (base, config) = fixture_with(&["one.JPG", "two.JPG", "three.JPG"]);
    let server = common::start_authenticated_server(config).await;
    wait_until_idle(&server.url).await;
    let photos = photo_page(&server.url).await;

    // The Photographer edits one queried Photo in the Web between the CLI
    // read and write.
    let response = post_json(
        &server.url,
        &format!("/api/photos/{}/state", photos[1].0),
        json!({"field": "rating", "value": 3}),
    )
    .await;
    assert!(response.status().is_success());

    let mixed = [
        (photos[0].0.clone(), photos[0].1.clone()),
        (photos[1].0.clone(), photos[1].1.clone()),
        (photos[2].0.clone(), photos[2].1.clone()),
    ];
    let (exit, partial) = command_with_stdin(
        &server.url,
        &["photos", "set", "--input", "-"],
        &decision_document("selectionState", json!("selected"), &mixed),
    )
    .await;
    assert_eq!(exit, 5);
    assert_eq!(partial["schemaVersion"], 1);
    assert_eq!(partial["status"], "partial");
    assert_eq!(partial["error"]["code"], "partial_result");
    assert_eq!(partial["error"]["effect"], "partial");
    assert_eq!(
        partial["error"]["details"]["counts"],
        json!({"changed": 2, "unchanged": 0, "conflict": 1, "missing": 0})
    );
    assert_eq!(
        partial["data"]["results"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["photoId"].as_str().unwrap())
            .collect::<Vec<_>>(),
        mixed.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>()
    );
    assert_eq!(partial["data"]["results"][0]["outcome"], "changed");
    let conflicted = &partial["data"]["results"][1];
    assert_eq!(conflicted["outcome"], "conflict");
    assert_eq!(sorted_keys(conflicted), ["current", "outcome", "photoId"]);
    assert_eq!(conflicted["current"]["rating"], 3);
    let web_version = conflicted["current"]["decisionVersion"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(web_version, photos[1].1);
    assert_eq!(
        partial["data"]["counts"],
        json!({"changed": 2, "unchanged": 0, "conflict": 1, "missing": 0})
    );

    // A mixed batch whose only successful outcome is unchanged is still a
    // confirmed partial result.
    let fresh = photo_page(&server.url).await;
    let no_op_and_stale = [
        (photos[0].0.clone(), fresh[0].1.clone()),
        (photos[1].0.clone(), photos[1].1.clone()),
    ];
    let (exit, unchanged_partial) = command_with_stdin(
        &server.url,
        &["photos", "set", "--input", "-"],
        &decision_document("selectionState", json!("selected"), &no_op_and_stale),
    )
    .await;
    assert_eq!(exit, 5);
    assert_eq!(unchanged_partial["error"]["code"], "partial_result");
    assert_eq!(
        unchanged_partial["error"]["details"]["counts"],
        json!({"changed": 0, "unchanged": 1, "conflict": 1, "missing": 0})
    );
    assert_eq!(
        unchanged_partial["data"]["results"][0]["outcome"],
        "unchanged"
    );

    // An all-conflict batch is a confirmed conflict naming the first
    // conflicting Photo in request order, with the complete array retained.
    let all_stale = [
        (photos[0].0.clone(), photos[0].1.clone()),
        (photos[1].0.clone(), photos[1].1.clone()),
    ];
    let (exit, conflict) = command_with_stdin(
        &server.url,
        &["photos", "set", "--input", "-"],
        &decision_document("selectionState", json!("rejected"), &all_stale),
    )
    .await;
    assert_eq!(exit, 4);
    assert_eq!(conflict["status"], "error");
    assert_eq!(conflict["error"]["code"], "conflict");
    assert_eq!(conflict["error"]["effect"], "none");
    assert_eq!(conflict["error"]["details"]["resource"], "photo");
    assert_eq!(conflict["error"]["details"]["reference"], all_stale[0].0);
    assert_ne!(
        conflict["error"]["details"]["currentVersion"],
        all_stale[0].1
    );
    assert_eq!(
        conflict["data"]["counts"],
        json!({"changed": 0, "unchanged": 0, "conflict": 2, "missing": 0})
    );

    // An all-missing batch is a confirmed missing result naming the first
    // missing Photo in request order.
    let missing = [
        (missing_photo_id(1), "v1".to_owned()),
        (missing_photo_id(2), "v2".to_owned()),
    ];
    let (exit, not_found) = command_with_stdin(
        &server.url,
        &["photos", "set", "--input", "-"],
        &decision_document("rating", json!(5), &missing),
    )
    .await;
    assert_eq!(exit, 3);
    assert_eq!(not_found["error"]["code"], "not_found");
    assert_eq!(not_found["error"]["effect"], "none");
    assert_eq!(
        not_found["error"]["details"],
        json!({"resource": "photo", "reference": missing_photo_id(1)})
    );
    assert_eq!(
        not_found["data"]["counts"],
        json!({"changed": 0, "unchanged": 0, "conflict": 0, "missing": 2})
    );
    for item in not_found["data"]["results"].as_array().unwrap() {
        assert_eq!(sorted_keys(item), ["outcome", "photoId"]);
        assert_eq!(item["outcome"], "missing");
    }

    // Conflicting and missing Photos were not overwritten: the newer Web
    // decision survives every refused write above.
    let (exit, current) = command(&server.url, &["photos", "get", &photos[1].0]).await;
    assert_eq!(exit, 0);
    assert_eq!(current["data"]["rating"], 3);

    server.close().await.unwrap();
    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn cli_photo_decisions_conflict_with_web_interleaving_and_restart() {
    let (base, config) = fixture_with(&["one.JPG", "two.JPG"]);
    let server = common::start_authenticated_server(config.clone()).await;
    wait_until_idle(&server.url).await;
    let photos = photo_page(&server.url).await;

    // A change away and back to the original value still conflicts with the
    // version read before those edits.
    for rating in [2, 0] {
        let response = post_json(
            &server.url,
            &format!("/api/photos/{}/state", photos[0].0),
            json!({"field": "rating", "value": rating}),
        )
        .await;
        assert!(response.status().is_success());
    }
    let (exit, away_and_back) = command(
        &server.url,
        &[
            "photos",
            "set",
            &photos[0].0,
            "--selection",
            "selected",
            "--if-version",
            &photos[0].1,
        ],
    )
    .await;
    assert_eq!(exit, 4);
    assert_eq!(away_and_back["error"]["code"], "conflict");
    assert_eq!(away_and_back["error"]["effect"], "none");
    assert_eq!(away_and_back["error"]["details"]["resource"], "photo");
    assert_eq!(away_and_back["error"]["details"]["reference"], photos[0].0);
    let restored = &away_and_back["data"]["results"][0];
    assert_eq!(restored["outcome"], "conflict");
    assert_eq!(restored["current"]["rating"], 0);
    assert_eq!(restored["current"]["selectionState"], "undecided");
    assert_ne!(restored["current"]["decisionVersion"], photos[0].1);
    let (exit, facts) = command(&server.url, &["photos", "get", &photos[0].0]).await;
    assert_eq!(exit, 0);
    assert_eq!(facts["data"]["selectionState"], "undecided");
    assert_eq!(facts["data"]["rating"], 0);
    let pre_restart_version = facts["data"]["decisionVersion"]
        .as_str()
        .unwrap()
        .to_owned();

    // A restart invalidates every earlier token; a fresh read allows one
    // explicit next write. The persisted access record still authenticates
    // the configured token, so the restart reuses it without re-seeding.
    server.close().await.unwrap();
    let mut server = start_server(config).await.unwrap();
    server.url = common::tls_proxy(&server.url);
    wait_until_idle(&server.url).await;

    let (exit, stale_epoch) = command(
        &server.url,
        &[
            "photos",
            "set",
            &photos[0].0,
            "--selection",
            "selected",
            "--if-version",
            &pre_restart_version,
        ],
    )
    .await;
    assert_eq!(exit, 4);
    assert_eq!(stale_epoch["error"]["code"], "conflict");
    assert_eq!(stale_epoch["error"]["details"]["reference"], photos[0].0);

    let fresh = photo_page(&server.url).await;
    assert_ne!(fresh[0].1, pre_restart_version);
    let (exit, next_write) = command(
        &server.url,
        &[
            "photos",
            "set",
            &photos[0].0,
            "--selection",
            "selected",
            "--if-version",
            &fresh[0].1,
        ],
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(next_write["data"]["results"][0]["outcome"], "changed");
    assert_eq!(
        next_write["data"]["results"][0]["current"]["selectionState"],
        "selected"
    );

    server.close().await.unwrap();
    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn cli_explicit_removal_restore_inspect_survives_restart() {
    let (base, config) = fixture_with(&["one.JPG", "two.JPG"]);
    let server = common::start_authenticated_server(config.clone()).await;
    wait_until_idle(&server.url).await;
    let (exit, page) = command(&server.url, &["photos", "list", "--limit", "60"]).await;
    assert_eq!(exit, 0);
    let items = page["data"]["items"].as_array().unwrap();
    let first = items[0]["id"].as_str().unwrap().to_owned();
    let second = items[1]["id"].as_str().unwrap().to_owned();
    for item in items {
        let (exit, result) = command(
            &server.url,
            &[
                "photos",
                "set",
                item["id"].as_str().unwrap(),
                "--selection",
                "rejected",
                "--if-version",
                item["decisionVersion"].as_str().unwrap(),
            ],
        )
        .await;
        assert_eq!(exit, 0, "{result}");
    }
    let (exit, rejected) =
        command(&server.url, &["photos", "list", "--selection", "rejected"]).await;
    assert_eq!(exit, 0);
    let first_facts = rejected["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["id"] == first)
        .unwrap();
    assert_eq!(first_facts["removedAtMs"], Value::Null);
    let input = write_input(
        &base,
        "remove.json",
        &serde_json::to_string(&json!({
            "photos": [{
                "photoId": first,
                "selectionState": "rejected",
                "decisionVersion": first_facts["decisionVersion"],
                "removedAtMs": null
            }]
        }))
        .unwrap(),
    );
    let removal_operation = "00000000-0000-4000-8000-000000000201";
    let (exit, removed) = command(
        &server.url,
        &["photos", "remove", removal_operation, "--input", &input],
    )
    .await;
    assert_eq!(exit, 0, "{removed}");
    assert_eq!(removed["data"]["counts"]["removed"], 1);
    assert_eq!(removed["data"]["results"][0]["photoId"], first);
    assert!(
        removed["data"]["results"][0]["removedAtMs"]
            .as_i64()
            .is_some()
    );

    server.close().await.unwrap();
    let mut server = start_server(config).await.unwrap();
    server.url = common::tls_proxy(&server.url);
    wait_until_idle(&server.url).await;

    let (exit, inspected) = command(
        &server.url,
        &["photos", "removal-operation", removal_operation],
    )
    .await;
    assert_eq!(exit, 0, "{inspected}");
    assert_eq!(inspected["data"], removed["data"]);
    let (exit, trash) = command(&server.url, &["trash", "list"]).await;
    assert_eq!(exit, 0);
    let marker = trash["data"]["photos"][0]["removedAtMs"].as_i64().unwrap();
    assert_eq!(trash["data"]["photos"][0]["photo"]["id"], first);
    assert_eq!(
        trash["data"]["photos"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|photo| photo["photo"]["id"] == second)
            .count(),
        0
    );

    let restore_input = write_input(
        &base,
        "restore.json",
        &serde_json::to_string(&json!({
            "photos": [{"photoId": first, "removedAtMs": marker}]
        }))
        .unwrap(),
    );
    let restore_operation = "00000000-0000-4000-8000-000000000202";
    let (exit, restored) = command(
        &server.url,
        &[
            "photos",
            "restore",
            restore_operation,
            "--input",
            &restore_input,
        ],
    )
    .await;
    assert_eq!(exit, 0, "{restored}");
    assert_eq!(restored["data"]["counts"]["restored"], 1);
    let (exit, restore_inspected) = command(
        &server.url,
        &["photos", "restore-operation", restore_operation],
    )
    .await;
    assert_eq!(exit, 0, "{restore_inspected}");
    assert_eq!(restore_inspected["data"], restored["data"]);
    let (exit, after) = command(&server.url, &["photos", "list", "--selection", "rejected"]).await;
    assert_eq!(exit, 0);
    let after_ids = after["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(after_ids.contains(first.as_str()));
    assert!(after_ids.contains(second.as_str()));
    assert!(
        after["data"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["removedAtMs"] == Value::Null)
    );

    server.close().await.unwrap();
    fs::remove_dir_all(base).unwrap();
}
#[tokio::test]
async fn cli_photo_decisions_validate_input_before_any_network_mutation() {
    let base = std::env::temp_dir().join(format!(
        "slipstream-cli-decision-input-{}",
        std::process::id()
    ));
    fs::create_dir_all(&base).unwrap();
    // Nothing listens here; any network attempt would fail as transport.
    // The CLI accepts only an HTTPS service origin.
    let dead = "https://127.0.0.1:9";
    let first = missing_photo_id(1);
    let second = missing_photo_id(2);

    let cases = [
        (
            "trailing.json",
            "{\"field\": \"rating\", \"value\": 4, \"photos\": []} trailing",
            "input",
        ),
        ("garbage.json", "\u{fffd}\u{fffd}{}", "input"),
        (
            "bad-field.json",
            "{\"field\": \"selection\", \"value\": \"selected\", \"photos\": [{\"photoId\": \"a\", \"ifVersion\": \"v\"}]}",
            "field",
        ),
        (
            "over-rating.json",
            "{\"field\": \"rating\", \"value\": 6, \"photos\": [{\"photoId\": \"a\", \"ifVersion\": \"v\"}]}",
            "value",
        ),
        (
            "empty-photos.json",
            "{\"field\": \"rating\", \"value\": 4, \"photos\": []}",
            "photos",
        ),
    ];
    for (name, content, argument) in cases {
        let input = write_input(&base, name, content);
        let (exit, envelope) = command(dead, &["photos", "set", "--input", &input]).await;
        assert_eq!(exit, 2, "for {name}");
        assert_eq!(envelope["error"]["code"], "invalid_input", "for {name}");
        assert_eq!(envelope["error"]["effect"], "none", "for {name}");
        assert_eq!(
            envelope["error"]["details"]["argument"], argument,
            "for {name}"
        );
    }

    let mut over = Vec::new();
    for index in 0..101 {
        over.push(json!({"photoId": missing_photo_id(index), "ifVersion": "v"}));
    }
    let over_limit = write_input(
        &base,
        "over-limit.json",
        &serde_json::to_string(&json!({"field": "rating", "value": 4, "photos": over})).unwrap(),
    );
    let (exit, envelope) = command(dead, &["photos", "set", "--input", &over_limit]).await;
    assert_eq!(exit, 2);
    assert_eq!(envelope["error"]["code"], "limit_exceeded");
    assert_eq!(
        envelope["error"]["details"],
        json!({"limitName": "photoIds", "limit": 100, "actual": 101})
    );

    let oversized = write_input(
        &base,
        "oversized.json",
        &format!(
            "{{\"field\": \"rating\", \"value\": 4, \"photos\": [{{\"photoId\": \"{}\", \"ifVersion\": \"v\"}}]}}",
            "x".repeat(70_000)
        ),
    );
    let (exit, envelope) = command(dead, &["photos", "set", "--input", &oversized]).await;
    assert_eq!(exit, 2);
    assert_eq!(envelope["error"]["code"], "limit_exceeded");
    assert_eq!(
        envelope["error"]["details"],
        json!({"limitName": "inputBytesMaximum", "limit": 65536, "actual": 65537})
    );

    let (exit, envelope) = command(
        dead,
        &[
            "photos",
            "set",
            "--input",
            "/nonexistent/slipstream-decisions.json",
        ],
    )
    .await;
    assert_eq!(exit, 6);
    assert_eq!(envelope["error"]["code"], "local_io_failed");
    assert_eq!(
        envelope["error"]["details"],
        json!({
            "operation": "read-input",
            "path": "/nonexistent/slipstream-decisions.json",
            "fileCommitted": false
        })
    );

    // Incomplete single-Photo commands are refused the same way.
    for arguments in [
        vec!["photos", "set", "p1", "--selection", "selected"],
        vec!["photos", "set", "p1", "--if-version", "v"],
        vec![
            "photos",
            "set",
            "p1",
            "--selection",
            "selected",
            "--rating",
            "1",
            "--if-version",
            "v",
        ],
        vec!["photos", "set", "--input", "-", "--if-version", "v"],
    ] {
        let (exit, envelope) = command(dead, &arguments).await;
        assert_eq!(exit, 2, "for {arguments:?}");
        assert_eq!(envelope["error"]["code"], "invalid_input");
        assert_eq!(envelope["error"]["effect"], "none");
    }

    // A locally valid document reaches the transport, which is unreachable.
    let valid = decision_document(
        "rating",
        json!(4),
        &[(first, "v1".to_owned()), (second, "v2".to_owned())],
    );
    let (exit, envelope) =
        command_with_stdin(dead, &["photos", "set", "--input", "-"], &valid).await;
    assert_eq!(exit, 6);
    assert_eq!(envelope["error"]["code"], "transport_failed");
    assert_eq!(envelope["error"]["details"]["operation"], "photos-set");

    fs::remove_dir_all(base).unwrap();
}

fn decision_reply(results: Value, counts: Value) -> Value {
    json!({ "results": results, "counts": counts })
}

#[tokio::test]
async fn cli_photo_decisions_report_unknown_outcomes_without_retry() {
    let first = missing_photo_id(1);
    let second = missing_photo_id(2);
    let document = decision_document(
        "rating",
        json!(4),
        &[
            (first.clone(), "v1".to_owned()),
            (second.clone(), "v2".to_owned()),
        ],
    );
    let invoke = |server: &str, document: &str| {
        let server = server.to_owned();
        let document = document.to_owned();
        async move { command_with_stdin(&server, &["photos", "set", "--input", "-"], &document).await }
    };

    let changed_item = |id: &str, selection: &str, rating: u8| {
        json!({
            "photoId": id,
            "outcome": "changed",
            "prior": {"selectionState": "undecided", "rating": 0},
            "current": {"selectionState": selection, "rating": rating, "decisionVersion": "v9"}
        })
    };
    let unchanged_item = |id: &str, rating: u8| {
        json!({
            "photoId": id,
            "outcome": "unchanged",
            "current": {
                "selectionState": "undecided",
                "rating": rating,
                "decisionVersion": "v1"
            }
        })
    };
    let conflict_item = |id: &str| {
        json!({
            "photoId": id,
            "outcome": "conflict",
            "current": {
                "selectionState": "selected",
                "rating": 5,
                "decisionVersion": "v3"
            }
        })
    };
    let missing_item = |id: &str| json!({"photoId": id, "outcome": "missing"});
    let valid_counts = json!({"changed": 2, "unchanged": 0, "conflict": 0, "missing": 0});
    let selection_document = decision_document(
        "selectionState",
        json!("selected"),
        &[
            (first.clone(), "v1".to_owned()),
            (second.clone(), "v2".to_owned()),
        ],
    );

    // A lost or unusable response after a possible send is an unknown
    // outcome that names every submitted Photo in request order. A changed
    // or unchanged result is confirmed only when its current decision
    // repeats the requested value: a structurally valid batch whose items
    // report any other current value claims an effect the checked-decision
    // contract cannot produce, so it also settles as an unknown outcome.
    for (label, document, reply) in [
        ("dropped reply", document.clone(), MutationReply::Drop),
        (
            "truncated reply",
            document.clone(),
            MutationReply::Truncated,
        ),
        (
            "non-object reply",
            document.clone(),
            MutationReply::Status(200, json!("not an object")),
        ),
        (
            "counts do not match the results",
            document.clone(),
            MutationReply::Status(
                200,
                decision_reply(
                    json!([
                        changed_item(&first, "undecided", 4),
                        changed_item(&second, "undecided", 4)
                    ]),
                    json!({"changed": 1, "unchanged": 1, "conflict": 0, "missing": 0}),
                ),
            ),
        ),
        (
            "results in reverse order",
            document.clone(),
            MutationReply::Status(
                200,
                decision_reply(
                    json!([
                        {"photoId": second, "outcome": "missing"},
                        {"photoId": first, "outcome": "missing"}
                    ]),
                    json!({"changed": 0, "unchanged": 0, "conflict": 0, "missing": 2}),
                ),
            ),
        ),
        (
            "unexpected field on a changed item",
            document.clone(),
            MutationReply::Status(
                200,
                decision_reply(
                    json!([{
                        "photoId": first,
                        "outcome": "changed",
                        "prior": {"selectionState": "undecided", "rating": 0},
                        "current": {"selectionState": "undecided", "rating": 4, "decisionVersion": "v9"},
                        "unexpected": true
                    }]),
                    valid_counts.clone(),
                ),
            ),
        ),
        (
            "one changed item missing from the results",
            document.clone(),
            MutationReply::Status(
                200,
                decision_reply(
                    json!([changed_item(&first, "undecided", 4)]),
                    valid_counts.clone(),
                ),
            ),
        ),
        (
            "changed current rating is not the requested rating",
            document.clone(),
            MutationReply::Status(
                200,
                decision_reply(
                    json!([changed_item(&first, "undecided", 3), missing_item(&second)]),
                    json!({"changed": 1, "unchanged": 0, "conflict": 0, "missing": 1}),
                ),
            ),
        ),
        (
            "unchanged current rating is not the requested rating",
            document.clone(),
            MutationReply::Status(
                200,
                decision_reply(
                    json!([unchanged_item(&first, 2), conflict_item(&second)]),
                    json!({"changed": 0, "unchanged": 1, "conflict": 1, "missing": 0}),
                ),
            ),
        ),
        (
            "one non-echoing changed item in an all-success batch",
            document.clone(),
            MutationReply::Status(
                200,
                decision_reply(
                    json!([
                        changed_item(&first, "undecided", 4),
                        changed_item(&second, "undecided", 3)
                    ]),
                    json!({"changed": 2, "unchanged": 0, "conflict": 0, "missing": 0}),
                ),
            ),
        ),
        (
            "changed current selection is not the requested selection",
            selection_document.clone(),
            MutationReply::Status(
                200,
                decision_reply(
                    json!([changed_item(&first, "rejected", 4), missing_item(&second)]),
                    json!({"changed": 1, "unchanged": 0, "conflict": 0, "missing": 1}),
                ),
            ),
        ),
    ] {
        let (server, handle, mutations) = stub_service(reply);
        let (exit, envelope) = invoke(&server, &document).await;
        handle.join().unwrap();
        assert_eq!(exit, 7, "for {label}");
        assert_eq!(envelope["error"]["code"], "outcome_unknown", "for {label}");
        assert_eq!(envelope["error"]["effect"], "unknown", "for {label}");
        assert_eq!(
            envelope["error"]["details"],
            json!({
                "operation": "photos-set",
                "photoIds": [first.clone(), second.clone()],
                "albumId": null,
                "albumName": null
            }),
            "for {label}"
        );
        assert_eq!(envelope["data"], Value::Null, "for {label}");
        assert_eq!(
            mutations.load(Ordering::SeqCst),
            1,
            "the client must not automatically retry a write, for {label}"
        );
    }

    // A complete, valid service error is a confirmed failure instead.
    let storage_failed = json!({
        "error": {
            "code": "storage_failed",
            "message": "Inspect server health and the current Photo decisions before trying again.",
            "effect": "none",
            "details": {"operation": "photos-set"}
        }
    });
    let (server, handle, mutations) = stub_service(MutationReply::Status(503, storage_failed));
    let (exit, envelope) = invoke(&server, &document).await;
    handle.join().unwrap();
    assert_eq!(exit, 6);
    assert_eq!(envelope["error"]["code"], "storage_failed");
    assert_eq!(envelope["error"]["effect"], "none");
    assert_eq!(envelope["error"]["details"]["operation"], "photos-set");
    assert_eq!(mutations.load(Ordering::SeqCst), 1);

    let invalid_value = json!({
        "error": {
            "code": "invalid_input",
            "message": "Correct the request and try again.",
            "effect": "none",
            "details": {"argument": "value", "reason": "The decision value must match the field's type and range."}
        }
    });
    let (server, handle, mutations) = stub_service(MutationReply::Status(400, invalid_value));
    let (exit, envelope) = invoke(&server, &document).await;
    handle.join().unwrap();
    assert_eq!(exit, 2);
    assert_eq!(envelope["error"]["code"], "invalid_input");
    assert_eq!(envelope["error"]["details"]["argument"], "value");
    assert_eq!(mutations.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cli_photo_decision_failures_redact_attached_data() {
    let first = missing_photo_id(1);
    let second = missing_photo_id(2);
    let document = decision_document(
        "rating",
        json!(4),
        &[
            (first.clone(), "v1".to_owned()),
            (second.clone(), "v2".to_owned()),
        ],
    );
    // A hostile service echoes the client credential inside a validated
    // result field of a confirmed partial batch.
    let reply = MutationReply::Status(
        200,
        decision_reply(
            json!([
                {
                    "photoId": first,
                    "outcome": "changed",
                    "prior": {"selectionState": "undecided", "rating": 0},
                    "current": {
                        "selectionState": "undecided",
                        "rating": 4,
                        "decisionVersion": format!("{}-v9", common::ACCESS_TOKEN)
                    }
                },
                {"photoId": second, "outcome": "missing"}
            ]),
            json!({"changed": 1, "unchanged": 0, "conflict": 0, "missing": 1}),
        ),
    );
    let (server, handle, mutations) = stub_service(reply);
    let (exit, envelope) =
        command_with_stdin(&server, &["photos", "set", "--input", "-"], &document).await;
    handle.join().unwrap();
    assert_eq!(exit, 5);
    assert_eq!(envelope["error"]["code"], "partial_result");
    assert_eq!(envelope["error"]["effect"], "partial");
    assert_eq!(mutations.load(Ordering::SeqCst), 1);
    let rendered = serde_json::to_string(&envelope).unwrap();
    assert!(
        !rendered.contains(common::ACCESS_TOKEN),
        "the credential must not reach stdout: {rendered}"
    );
    assert!(rendered.contains("[redacted]"));
    assert_eq!(
        envelope["data"]["results"][0]["current"]["decisionVersion"],
        "[redacted]-v9"
    );
}
