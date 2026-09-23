use serde_json::{Value, json};
use slipstream_server::Config;
mod common;
use std::{
    fs,
    io::{ErrorKind, Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
    thread::JoinHandle,
    time::{Duration, Instant},
};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

/// The reply a stub service gives to the one mutation request after the
/// capabilities handshake.
enum MutationReply {
    /// Accept the request, then close the connection without responding.
    Drop,
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

fn stub_service(reply: MutationReply) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        for request_index in 0..2 {
            let (mut stream, _) = common::accept_tls(&listener);
            let mut request = [0_u8; 8192];
            let count = stream.read(&mut request).unwrap();
            common::assert_bearer(&request[..count]);
            if request_index == 0 {
                write_json_response(&mut stream, 200, &capabilities_body());
            } else {
                match reply {
                    MutationReply::Drop => drop(stream),
                    MutationReply::Status(status, ref body) => {
                        write_json_response(&mut stream, status, body)
                    }
                }
            }
        }
    });
    (format!("https://127.0.0.1:{}", address.port()), handle)
}

/// Forwards requests to a real service but drops the response of the first
/// POST, so the mutation is admitted while the caller cannot learn that.
fn dropping_proxy(upstream: &str) -> String {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let upstream = upstream
        .strip_prefix("http://")
        .unwrap_or(upstream)
        .to_owned();
    std::thread::spawn(move || {
        while let Ok((stream, _)) = listener.accept() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
            let mut client = common::tls_stream(stream);
            while let Some((head, body)) = common::read_http_message(&mut client) {
                let mut forwarded = head.clone().into_bytes();
                forwarded.extend_from_slice(&body);
                let Ok(mut upstream_stream) = TcpStream::connect(&upstream) else {
                    return;
                };
                upstream_stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                if upstream_stream.write_all(&forwarded).is_err() {
                    return;
                }
                let Some((response_head, response_body)) =
                    common::read_http_message(&mut upstream_stream)
                else {
                    return;
                };
                if head.starts_with("POST ") {
                    // The mutation reached the service; its confirmed response
                    // is discarded so the client must report an unknown outcome.
                    return;
                }
                let mut response = response_head.into_bytes();
                response.extend_from_slice(&response_body);
                if client.write_all(&response).is_err() {
                    return;
                }
            }
        }
    });
    format!("https://127.0.0.1:{}", address.port())
}

fn fixture_with(photo_names: &[&str]) -> (PathBuf, Config) {
    let base = loop {
        let candidate = std::env::temp_dir().join(format!(
            "slipstream-cli-album-test-{}-{}",
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
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
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
        let exit = output.status.code().unwrap() as u8;
        let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
        (exit, envelope)
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

fn item_ids(page: &Value) -> Vec<String> {
    page["data"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_owned())
        .collect()
}

fn write_input(base: &std::path::Path, name: &str, content: &str) -> String {
    let path = base.join(name);
    fs::write(&path, content).unwrap();
    path.to_str().unwrap().to_owned()
}

fn membership_document(photo_ids: &[String]) -> String {
    serde_json::to_string(&json!({ "photoIds": photo_ids })).unwrap()
}

async fn album_mutation(
    server: &str,
    album_id: &str,
    operation: &str,
    arguments: &[&str],
    input: Option<String>,
) -> (u8, Value) {
    let mut invocation = vec![
        "albums".to_owned(),
        operation.to_owned(),
        album_id.to_owned(),
    ];
    invocation.extend(arguments.iter().map(|argument| (*argument).to_owned()));
    let invocation = invocation.iter().map(String::as_str).collect::<Vec<_>>();
    match input {
        Some(document) => command_with_stdin(server, &invocation, &document).await,
        None => command(server, &invocation).await,
    }
}

async fn assert_album_state(
    server: &str,
    album_id: &str,
    expected: &[String],
    expected_version: &str,
) {
    let (exit, page) = command(
        server,
        &[
            "photos",
            "list",
            "--album",
            album_id,
            "--order",
            "album-order",
            "--limit",
            "60",
        ],
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(item_ids(&page), expected);
    let (exit, summary) = command(server, &["albums", "get", album_id]).await;
    assert_eq!(exit, 0);
    assert_eq!(summary["data"]["albumVersion"], expected_version);
}

#[tokio::test]
async fn cli_completes_the_selected_query_to_ordered_album_workflow() {
    let (base, config) = fixture_with(&["one.JPG", "two.JPG", "three.JPG", "four.JPG", "five.JPG"]);
    let server = common::start_authenticated_server(config).await;
    wait_until_idle(&server.url).await;

    let (exit, baseline) = command(&server.url, &["photos", "list", "--limit", "60"]).await;
    assert_eq!(exit, 0);
    let all_ids = item_ids(&baseline);
    assert_eq!(all_ids.len(), 5);

    let chosen = [all_ids[0].clone(), all_ids[2].clone(), all_ids[4].clone()];
    for photo_id in &chosen {
        let response = post_json(
            &server.url,
            &format!("/api/photos/{photo_id}/state"),
            json!({"field": "rating", "value": 4}),
        )
        .await;
        assert!(response.status().is_success());
        let response = post_json(
            &server.url,
            &format!("/api/photos/{photo_id}/state"),
            json!({"field": "selectionState", "value": "selected"}),
        )
        .await;
        assert!(response.status().is_success());
    }

    let (exit, selected) = command(
        &server.url,
        &[
            "photos",
            "list",
            "--selection",
            "selected",
            "--rating-min",
            "4",
            "--limit",
            "60",
        ],
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(selected["data"]["total"], 3);
    let selected_ids = item_ids(&selected);
    assert_eq!(selected_ids, chosen);

    let (exit, created) = command(&server.url, &["albums", "create", "--name", "选片 picks"]).await;
    assert_eq!(exit, 0);
    let album = &created["data"]["album"];
    let album_id = album["id"].as_str().unwrap().to_owned();
    let version = album["albumVersion"].as_str().unwrap().to_owned();
    assert_eq!(album["name"], "选片 picks");
    assert_eq!(album["photoCount"], 0);
    assert_eq!(album["hasSavedPosition"], false);
    assert_eq!(
        album["webUrl"],
        format!("{}/?source=album&albumId={album_id}", server.url)
    );

    let (exit, added) = command_with_stdin(
        &server.url,
        &[
            "albums",
            "add",
            &album_id,
            "--input",
            "-",
            "--if-version",
            &version,
        ],
        &membership_document(&selected_ids),
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(added["data"]["addedPhotoIds"], json!(selected_ids));
    assert_eq!(added["data"]["alreadyMemberPhotoIds"], json!([]));
    assert_eq!(added["data"]["album"]["photoCount"], 3);
    let added_version = added["data"]["album"]["albumVersion"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(added_version, version);

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
    assert_eq!(item_ids(&album_photos), selected_ids);

    let (exit, summary) = command(&server.url, &["albums", "get", &album_id]).await;
    assert_eq!(exit, 0);
    assert_eq!(summary["data"]["photoCount"], 3);
    assert_eq!(summary["data"]["name"], "选片 picks");
    assert_eq!(summary["data"]["albumVersion"], added_version);
    assert_eq!(
        summary["data"]["webUrl"],
        format!("{}/?source=album&albumId={album_id}", server.url)
    );

    let (exit, conflict) =
        command(&server.url, &["albums", "create", "--name", "选片 PICKS"]).await;
    assert_eq!(exit, 4);
    assert_eq!(conflict["status"], "error");
    assert_eq!(conflict["error"]["code"], "name_conflict");
    assert_eq!(conflict["error"]["effect"], "none");
    assert_eq!(
        conflict["error"]["details"],
        json!({"name": "选片 PICKS", "albumId": album_id})
    );
    let (exit, inspected) = command(&server.url, &["albums", "list", "--name", "选片 picks"]).await;
    assert_eq!(exit, 0);
    assert_eq!(inspected["data"]["items"][0]["id"], album_id);

    server.close().await.unwrap();
    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn cli_album_mutations_use_checked_versions_and_confirmed_results() {
    let (base, config) = fixture_with(&["one.JPG", "two.JPG", "three.JPG", "four.JPG", "five.JPG"]);
    let server = common::start_authenticated_server(config).await;
    wait_until_idle(&server.url).await;

    let (exit, baseline) = command(&server.url, &["photos", "list", "--limit", "60"]).await;
    assert_eq!(exit, 0);
    let ids = item_ids(&baseline);

    let (exit, created) = command(&server.url, &["albums", "create", "--name", "Working"]).await;
    assert_eq!(exit, 0);
    let album_id = created["data"]["album"]["id"].as_str().unwrap().to_owned();
    let version = |result: &Value| {
        result["data"]["album"]["albumVersion"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let initial = version(&created);

    let album_order = |expected: Vec<String>, expected_version: String| {
        let server = server.url.clone();
        let album_id = album_id.clone();
        async move { assert_album_state(&server, &album_id, &expected, &expected_version).await }
    };

    let first = membership_document(&[ids[1].clone(), ids[0].clone()]);
    let (exit, added) = album_mutation(
        &server.url,
        &album_id,
        "add",
        &["--input", "-", "--if-version", &initial],
        Some(first),
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(
        added["data"]["addedPhotoIds"],
        json!([ids[1].clone(), ids[0].clone()])
    );
    let after_first_add = version(&added);
    album_order(
        vec![ids[1].clone(), ids[0].clone()],
        after_first_add.clone(),
    )
    .await;

    let second = membership_document(&[ids[0].clone(), ids[2].clone()]);
    let (exit, added) = album_mutation(
        &server.url,
        &album_id,
        "add",
        &["--input", "-", "--if-version", &after_first_add],
        Some(second),
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(added["data"]["addedPhotoIds"], json!([ids[2].clone()]));
    assert_eq!(
        added["data"]["alreadyMemberPhotoIds"],
        json!([ids[0].clone()])
    );
    assert_eq!(added["data"]["album"]["photoCount"], 3);
    let current = version(&added);
    album_order(
        vec![ids[1].clone(), ids[0].clone(), ids[2].clone()],
        current.clone(),
    )
    .await;

    let no_op = membership_document(&[ids[0].clone()]);
    let (exit, no_op) = album_mutation(
        &server.url,
        &album_id,
        "add",
        &["--input", "-", "--if-version", &current],
        Some(no_op),
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(no_op["data"]["addedPhotoIds"], json!([]));
    assert_eq!(
        no_op["data"]["alreadyMemberPhotoIds"],
        json!([ids[0].clone()])
    );
    assert_eq!(no_op["data"]["album"]["albumVersion"], current);

    let stale = membership_document(&[ids[0].clone()]);
    let (exit, stale) = album_mutation(
        &server.url,
        &album_id,
        "add",
        &["--input", "-", "--if-version", initial.as_str()],
        Some(stale),
    )
    .await;
    assert_eq!(exit, 4);
    assert_eq!(stale["error"]["code"], "conflict");
    assert_eq!(stale["error"]["effect"], "none");
    assert_eq!(
        stale["error"]["details"],
        json!({"resource": "album", "reference": album_id, "currentVersion": current})
    );

    let (exit, same_name) = album_mutation(
        &server.url,
        &album_id,
        "rename",
        &["--name", "Working", "--if-version", &current],
        None,
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(same_name["data"]["renamed"], false);
    assert_eq!(same_name["data"]["album"]["albumVersion"], current);

    let (exit, renamed) = album_mutation(
        &server.url,
        &album_id,
        "rename",
        &["--name", "Working set", "--if-version", &current],
        None,
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(renamed["data"]["renamed"], true);
    assert_eq!(renamed["data"]["album"]["name"], "Working set");
    let renamed_version = version(&renamed);

    let missing_id = "00000000-0000-4000-8000-00000000dead";
    let with_missing = membership_document(&[ids[4].clone(), missing_id.to_owned()]);
    let (exit, missing) = album_mutation(
        &server.url,
        &album_id,
        "add",
        &["--input", "-", "--if-version", &renamed_version],
        Some(with_missing),
    )
    .await;
    assert_eq!(exit, 3);
    assert_eq!(missing["error"]["code"], "not_found");
    assert_eq!(
        missing["error"]["details"],
        json!({"resource": "photo", "reference": missing_id})
    );
    album_order(
        vec![ids[1].clone(), ids[0].clone(), ids[2].clone()],
        renamed_version.clone(),
    )
    .await;

    let incomplete = membership_document(&[ids[1].clone()]);
    let (exit, incomplete) = album_mutation(
        &server.url,
        &album_id,
        "reorder",
        &["--input", "-", "--if-version", &renamed_version],
        Some(incomplete),
    )
    .await;
    assert_eq!(exit, 4);
    assert_eq!(incomplete["error"]["code"], "conflict");
    assert_eq!(
        incomplete["error"]["details"]["currentVersion"],
        renamed_version
    );

    let complete = membership_document(&[ids[2].clone(), ids[0].clone(), ids[1].clone()]);
    let (exit, reordered) = album_mutation(
        &server.url,
        &album_id,
        "reorder",
        &["--input", "-", "--if-version", &renamed_version],
        Some(complete),
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(
        reordered["data"]["orderedPhotoIds"],
        json!([ids[2].clone(), ids[0].clone(), ids[1].clone()])
    );
    assert_eq!(reordered["data"]["reordered"], true);
    let reordered_version = version(&reordered);
    album_order(
        vec![ids[2].clone(), ids[0].clone(), ids[1].clone()],
        reordered_version.clone(),
    )
    .await;

    let same_order = membership_document(&[ids[2].clone(), ids[0].clone(), ids[1].clone()]);
    let (exit, same_order) = album_mutation(
        &server.url,
        &album_id,
        "reorder",
        &["--input", "-", "--if-version", &reordered_version],
        Some(same_order),
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(same_order["data"]["reordered"], false);
    assert_eq!(
        same_order["data"]["album"]["albumVersion"],
        reordered_version
    );

    let response = post_json(
        &server.url,
        &format!("/api/albums/{album_id}/progress"),
        json!({"photoId": ids[2]}),
    )
    .await;
    assert!(response.status().is_success());

    let remove_saved_neighbor = membership_document(&[ids[0].clone()]);
    let (exit, removed) = album_mutation(
        &server.url,
        &album_id,
        "remove",
        &["--input", "-", "--if-version", &reordered_version],
        Some(remove_saved_neighbor),
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(removed["data"]["removedPhotoIds"], json!([ids[0].clone()]));
    assert_eq!(removed["data"]["alreadyAbsentPhotoIds"], json!([]));
    assert_eq!(removed["data"]["savedPhotoId"], ids[2].clone());
    assert_eq!(removed["data"]["album"]["hasSavedPosition"], true);
    let removed_version = version(&removed);

    let remove_saved = membership_document(&[ids[4].clone(), ids[2].clone()]);
    let (exit, removed) = album_mutation(
        &server.url,
        &album_id,
        "remove",
        &["--input", "-", "--if-version", &removed_version],
        Some(remove_saved),
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(removed["data"]["removedPhotoIds"], json!([ids[2].clone()]));
    assert_eq!(
        removed["data"]["alreadyAbsentPhotoIds"],
        json!([ids[4].clone()])
    );
    assert_eq!(removed["data"]["savedPhotoId"], Value::Null);
    assert_eq!(removed["data"]["album"]["hasSavedPosition"], false);
    assert_eq!(removed["data"]["album"]["photoCount"], 1);
    let final_version = version(&removed);

    let (exit, other) = command(&server.url, &["albums", "create", "--name", "Reserved"]).await;
    assert_eq!(exit, 0);
    let other_id = other["data"]["album"]["id"].as_str().unwrap().to_owned();
    let (exit, name_conflict) = album_mutation(
        &server.url,
        &album_id,
        "rename",
        &["--name", "reserved", "--if-version", &final_version],
        None,
    )
    .await;
    assert_eq!(exit, 4);
    assert_eq!(name_conflict["error"]["code"], "name_conflict");
    assert_eq!(
        name_conflict["error"]["details"],
        json!({"name": "reserved", "albumId": other_id})
    );

    let (exit, deleted) = album_mutation(
        &server.url,
        &album_id,
        "delete",
        &["--if-version", final_version.as_str()],
        None,
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(
        deleted["data"],
        json!({"albumId": album_id, "deleted": true, "originalFilesChanged": false})
    );
    let (exit, gone) = command(&server.url, &["albums", "get", &album_id]).await;
    assert_eq!(exit, 3);
    assert_eq!(gone["error"]["code"], "not_found");
    assert_eq!(
        gone["error"]["details"],
        json!({"resource": "album", "reference": album_id})
    );

    server.close().await.unwrap();
    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn cli_reorder_refuses_large_albums_while_membership_stays_bounded() {
    let names = (0..101)
        .map(|index| format!("p{index:03}.JPG"))
        .collect::<Vec<_>>();
    let references = names.iter().map(String::as_str).collect::<Vec<_>>();
    let (base, config) = fixture_with(&references);
    let server = common::start_authenticated_server(config).await;
    wait_until_idle(&server.url).await;

    let mut ids = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let arguments = match &cursor {
            Some(cursor) => vec!["photos", "list", "--cursor", cursor],
            None => vec!["photos", "list", "--limit", "60"],
        };
        let (exit, page) = command(&server.url, &arguments).await;
        assert_eq!(exit, 0);
        ids.extend(item_ids(&page));
        if let Some(next) = page["data"]["nextCursor"].as_str() {
            cursor = Some(next.to_owned());
        } else {
            break;
        }
    }
    assert_eq!(ids.len(), 101);

    let (exit, created) = command(&server.url, &["albums", "create", "--name", "Large"]).await;
    assert_eq!(exit, 0);
    let album_id = created["data"]["album"]["id"].as_str().unwrap().to_owned();
    let version = created["data"]["album"]["albumVersion"]
        .as_str()
        .unwrap()
        .to_owned();

    let first_batch = write_input(&base, "batch-1.json", &membership_document(&ids[..60]));
    let (exit, added) = command(
        &server.url,
        &[
            "albums",
            "add",
            &album_id,
            "--input",
            &first_batch,
            "--if-version",
            &version,
        ],
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(added["data"]["album"]["photoCount"], 60);
    let second_version = added["data"]["album"]["albumVersion"]
        .as_str()
        .unwrap()
        .to_owned();

    let second_batch = write_input(&base, "batch-2.json", &membership_document(&ids[60..]));
    let (exit, added) = command(
        &server.url,
        &[
            "albums",
            "add",
            &album_id,
            "--input",
            &second_batch,
            "--if-version",
            &second_version,
        ],
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(added["data"]["album"]["photoCount"], 101);
    let full_version = added["data"]["album"]["albumVersion"]
        .as_str()
        .unwrap()
        .to_owned();

    let mut cursor: Option<String> = None;
    let mut traversed = Vec::new();
    loop {
        let arguments = match &cursor {
            Some(cursor) => vec!["photos", "list", "--cursor", cursor],
            None => vec!["photos", "list", "--album", &album_id, "--limit", "60"],
        };
        let (exit, page) = command(&server.url, &arguments).await;
        assert_eq!(exit, 0);
        assert_eq!(page["data"]["total"], 101);
        traversed.extend(item_ids(&page));
        if let Some(next) = page["data"]["nextCursor"].as_str() {
            cursor = Some(next.to_owned());
        } else {
            break;
        }
    }
    assert_eq!(traversed, ids);

    let complete = write_input(&base, "complete.json", &membership_document(&ids));
    let (exit, refused) = command(
        &server.url,
        &[
            "albums",
            "reorder",
            &album_id,
            "--input",
            &complete,
            "--if-version",
            &full_version,
        ],
    )
    .await;
    assert_eq!(exit, 2);
    assert_eq!(refused["error"]["code"], "limit_exceeded");
    assert_eq!(refused["error"]["effect"], "none");
    assert_eq!(
        refused["error"]["details"],
        json!({"limitName": "albumReorderMembersMaximum", "limit": 100, "actual": 101})
    );
    let (exit, summary) = command(&server.url, &["albums", "get", &album_id]).await;
    assert_eq!(exit, 0);
    assert_eq!(summary["data"]["albumVersion"], full_version);
    assert_eq!(summary["data"]["photoCount"], 101);

    let removal = write_input(
        &base,
        "removal.json",
        &membership_document(&[ids[100].clone()]),
    );
    let (exit, removed) = command(
        &server.url,
        &[
            "albums",
            "remove",
            &album_id,
            "--input",
            &removal,
            "--if-version",
            &full_version,
        ],
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(
        removed["data"]["removedPhotoIds"],
        json!([ids[100].clone()])
    );
    assert_eq!(removed["data"]["album"]["photoCount"], 100);
    let reduced_version = removed["data"]["album"]["albumVersion"]
        .as_str()
        .unwrap()
        .to_owned();

    let mut reversed = ids[..100].to_vec();
    reversed.reverse();
    let order = write_input(&base, "order.json", &membership_document(&reversed));
    let (exit, reordered) = command(
        &server.url,
        &[
            "albums",
            "reorder",
            &album_id,
            "--input",
            &order,
            "--if-version",
            &reduced_version,
        ],
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(reordered["data"]["orderedPhotoIds"], json!(reversed));
    assert_eq!(reordered["data"]["reordered"], true);

    server.close().await.unwrap();
    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn cli_validates_membership_input_before_any_network_mutation() {
    let base =
        std::env::temp_dir().join(format!("slipstream-cli-input-test-{}", std::process::id()));
    fs::create_dir_all(&base).unwrap();
    // Nothing listens here; any network attempt would fail as transport.
    let dead = "https://127.0.0.1:9";
    let id = "00000000-0000-4000-8000-000000000001".to_owned();
    let other = "00000000-0000-4000-8000-000000000002".to_owned();
    let valid_ids = vec![id.clone(), other.clone()];

    let cases = [
        ("empty.json", "{\"photoIds\":[]}", "invalid_input", 2),
        ("empty-id.json", "{\"photoIds\":[\"\"]}", "invalid_input", 2),
        (
            "duplicate.json",
            "{\"photoIds\":[\"a\",\"a\"]}",
            "invalid_input",
            2,
        ),
        (
            "duplicate-key.json",
            "{\"photoIds\":[\"a\"],\"photoIds\":[\"b\"]}",
            "invalid_input",
            2,
        ),
        (
            "unknown-key.json",
            "{\"photoIds\":[\"a\"],\"extra\":1}",
            "invalid_input",
            2,
        ),
        (
            "trailing.json",
            "{\"photoIds\":[\"a\"]} trailing",
            "invalid_input",
            2,
        ),
        ("array.json", "[\"a\"]", "invalid_input", 2),
        ("string.json", "\"photoIds\"", "invalid_input", 2),
        (
            "garbage.json",
            "\u{fffd}\u{fffd}{\"photoIds\":[\"a\"]}",
            "invalid_input",
            2,
        ),
    ];
    for (name, content, code, exit) in cases {
        let input = write_input(&base, name, content);
        let (actual_exit, envelope) = command(
            dead,
            &[
                "albums",
                "add",
                "00000000-0000-4000-8000-00000000000a",
                "--input",
                &input,
                "--if-version",
                "version",
            ],
        )
        .await;
        assert_eq!(actual_exit, exit, "for {name}");
        assert_eq!(envelope["error"]["code"], code, "for {name}");
        assert_eq!(envelope["error"]["effect"], "none", "for {name}");
        assert_eq!(
            envelope["error"]["details"]["argument"], "input",
            "for {name}"
        );
    }

    let mut many = valid_ids.clone();
    for index in 2..100 {
        many.push(format!("00000000-0000-4000-8000-{index:012x}"));
    }
    assert_eq!(many.len(), 100);
    let over = membership_document(&{
        let mut overflow = many.clone();
        overflow.push("00000000-0000-4000-8000-00000000ffff".to_owned());
        overflow
    });
    let over_input = write_input(&base, "over-limit.json", &over);
    for (command_name, limit_name) in [
        ("add", "mutationPhotoIdsMaximum"),
        ("reorder", "albumReorderMembersMaximum"),
    ] {
        let (exit, envelope) = command(
            dead,
            &[
                "albums",
                command_name,
                "00000000-0000-4000-8000-00000000000a",
                "--input",
                &over_input,
                "--if-version",
                "version",
            ],
        )
        .await;
        assert_eq!(exit, 2, "for {command_name}");
        assert_eq!(envelope["error"]["code"], "limit_exceeded");
        assert_eq!(
            envelope["error"]["details"],
            json!({"limitName": limit_name, "limit": 100, "actual": 101})
        );
    }

    let oversized = write_input(
        &base,
        "oversized.json",
        &format!("{{\"photoIds\":[\"{}\"]}}", "x".repeat(70_000)),
    );
    let (exit, envelope) = command(
        dead,
        &[
            "albums",
            "add",
            "00000000-0000-4000-8000-00000000000a",
            "--input",
            &oversized,
            "--if-version",
            "version",
        ],
    )
    .await;
    assert_eq!(exit, 2);
    assert_eq!(envelope["error"]["code"], "limit_exceeded");
    assert_eq!(
        envelope["error"]["details"],
        json!({"limitName": "inputBytesMaximum", "limit": 65536, "actual": 65537})
    );

    let (exit, envelope) = command(
        dead,
        &[
            "albums",
            "add",
            "00000000-0000-4000-8000-00000000000a",
            "--input",
            "/nonexistent/slipstream-input.json",
            "--if-version",
            "version",
        ],
    )
    .await;
    assert_eq!(exit, 6);
    assert_eq!(envelope["error"]["code"], "local_io_failed");
    assert_eq!(
        envelope["error"]["details"],
        json!({
            "operation": "read-input",
            "path": "/nonexistent/slipstream-input.json",
            "fileCommitted": false
        })
    );

    let valid = write_input(&base, "valid.json", &membership_document(&valid_ids));
    let (exit, envelope) = command(
        dead,
        &[
            "albums",
            "add",
            "00000000-0000-4000-8000-00000000000a",
            "--input",
            &valid,
            "--if-version",
            "version",
        ],
    )
    .await;
    assert_eq!(exit, 6);
    assert_eq!(envelope["error"]["code"], "transport_failed");

    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn cli_reports_unknown_outcomes_for_lost_or_invalid_mutation_responses() {
    let name_conflict = json!({
        "error": {
            "code": "name_conflict",
            "message": "Inspect the existing Album before choosing a different name.",
            "effect": "none",
            "details": {"name": "Lost", "albumId": "00000000-0000-4000-8000-00000000000b"}
        }
    });
    let cases = [
        (MutationReply::Drop, "outcome_unknown", 7),
        (
            MutationReply::Status(200, json!({"album": {"id": "a"}})),
            "outcome_unknown",
            7,
        ),
        (
            MutationReply::Status(200, json!("not an object")),
            "outcome_unknown",
            7,
        ),
        (
            MutationReply::Status(
                409,
                json!({
                    "error": {
                        "code": "future_code",
                        "message": "Unrecognized by this client.",
                        "effect": "none",
                        "details": {"anything": true}
                    }
                }),
            ),
            "outcome_unknown",
            7,
        ),
        (
            MutationReply::Status(409, name_conflict),
            "name_conflict",
            4,
        ),
    ];
    for (reply, code, exit) in cases {
        let (server, handle) = stub_service(reply);
        let (actual_exit, envelope) =
            command(&server, &["albums", "create", "--name", "Lost"]).await;
        handle.join().unwrap();
        assert_eq!(actual_exit, exit);
        assert_eq!(envelope["error"]["code"], code);
        if code == "outcome_unknown" {
            assert_eq!(envelope["error"]["effect"], "unknown");
            assert_eq!(
                envelope["error"]["details"],
                json!({
                    "operation": "albums-create",
                    "photoIds": [],
                    "albumId": null,
                    "albumName": "Lost"
                })
            );
        }
    }

    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let (exit, envelope) = command(
        &format!("https://127.0.0.1:{}", address.port()),
        &["albums", "create", "--name", "Lost"],
    )
    .await;
    assert_eq!(exit, 6);
    assert_eq!(envelope["error"]["code"], "transport_failed");
    assert_eq!(envelope["error"]["effect"], "none");
}

/// Accepts the capabilities handshake, records that the mutation request
/// arrived, then never answers it.
fn stalling_after_admission(
    mutation_seen: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> String {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        for request_index in 0..2 {
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut stream = common::tls_stream(stream);
            let mut request = [0_u8; 8192];
            let count = stream.read(&mut request).unwrap_or(0);
            common::assert_bearer(&request[..count]);
            if request_index == 0 {
                write_json_response(&mut stream, 200, &capabilities_body());
            } else {
                mutation_seen.store(true, Ordering::SeqCst);
                std::thread::sleep(Duration::from_secs(10));
            }
        }
    });
    format!("https://127.0.0.1:{}", address.port())
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

/// Binds a loopback listener that counts accepted connections. Complete input
/// validation precedes network access, so a blocked or invalid input read must
/// leave the count at zero.
fn connection_recorder() -> (String, std::sync::Arc<AtomicUsize>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let connections = std::sync::Arc::new(AtomicUsize::new(0));
    let observed = std::sync::Arc::clone(&connections);
    std::thread::spawn(move || {
        for connection in listener.incoming() {
            match connection {
                Ok(_) => {
                    observed.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => return,
            }
        }
    });
    (format!("https://127.0.0.1:{}", address.port()), connections)
}

#[test]
fn cli_interruption_after_a_possible_send_reports_an_unknown_outcome() {
    let mutation_seen = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let server = stalling_after_admission(std::sync::Arc::clone(&mutation_seen));
    let child = common::cli_command()
        .arg("--token-file")
        .arg(common::credential_file())
        .args([
            "--server",
            &server,
            "--timeout",
            "300",
            "albums",
            "create",
            "--name",
            "Interrupted",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    while !mutation_seen.load(Ordering::SeqCst) {
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the mutation request never reached the service"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    let signal = Command::new("/bin/kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(signal.success());
    let output = child.wait_with_output().unwrap();
    assert!(output.stderr.is_empty());
    assert_eq!(output.status.code(), Some(130));
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["error"]["code"], "outcome_unknown");
    assert_eq!(envelope["error"]["effect"], "unknown");
    assert_eq!(
        envelope["error"]["details"],
        json!({
            "operation": "albums-create",
            "photoIds": [],
            "albumId": null,
            "albumName": "Interrupted"
        })
    );
}

#[test]
fn held_open_stdin_still_exits_at_the_whole_command_deadline() {
    let (server, connections) = connection_recorder();
    let mut child = common::cli_command()
        .arg("--token-file")
        .arg(common::credential_file())
        .args([
            "--server",
            &server,
            "--timeout",
            "1",
            "albums",
            "add",
            "probe-album",
            "--input",
            "-",
            "--if-version",
            "probe-version",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // The stdin pipe stays open with no data through the exit assertion: a
    // held-open input read must not outlive the whole-command deadline.
    let status = wait_for_exit(&mut child, Duration::from_secs(2))
        .expect("held-open stdin must not outlive the whole-command deadline");
    assert_eq!(status.code(), Some(6));
    let output = child.wait_with_output().unwrap();
    assert!(output.stderr.is_empty());
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["schemaVersion"], 1);
    assert_eq!(envelope["status"], "error");
    assert_eq!(envelope["error"]["code"], "transport_failed");
    assert_eq!(envelope["error"]["effect"], "none");
    assert_eq!(envelope["error"]["details"]["operation"], "albums-add");
    assert_eq!(
        connections.load(Ordering::SeqCst),
        0,
        "no request may be sent while the input document is unread"
    );
}

#[test]
fn held_open_stdin_interruption_exits_without_sending_a_request() {
    let (server, connections) = connection_recorder();
    let mut child = common::cli_command()
        .arg("--token-file")
        .arg(common::credential_file())
        .args([
            "--server",
            &server,
            "--timeout",
            "300",
            "albums",
            "remove",
            "probe-album",
            "--input",
            "-",
            "--if-version",
            "probe-version",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(250));
    let signal = Command::new("/bin/kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(signal.success());
    // The stdin pipe is still open and data-less when the exit is asserted.
    let status = wait_for_exit(&mut child, Duration::from_secs(2))
        .expect("handled interruption must not wait for held-open stdin");
    assert_eq!(status.code(), Some(130));
    let output = child.wait_with_output().unwrap();
    assert!(output.stderr.is_empty());
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["status"], "error");
    assert_eq!(envelope["error"]["code"], "transport_failed");
    assert_eq!(envelope["error"]["effect"], "none");
    assert_eq!(
        envelope["error"]["message"],
        "The command was interrupted. Inspect status before continuing."
    );
    assert_eq!(
        connections.load(Ordering::SeqCst),
        0,
        "an interruption before any send must not produce a request"
    );
}

#[test]
fn closed_stdin_reports_invalid_input_without_a_request() {
    let (server, connections) = connection_recorder();
    let mut child = common::cli_command()
        .arg("--token-file")
        .arg(common::credential_file())
        .args([
            "--server",
            &server,
            "--timeout",
            "1",
            "albums",
            "add",
            "probe-album",
            "--input",
            "-",
            "--if-version",
            "probe-version",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Closing the write end gives the reader EOF; an empty document is
    // invalid before any network access.
    drop(child.stdin.take());
    let status =
        wait_for_exit(&mut child, Duration::from_secs(2)).expect("closed stdin must fail promptly");
    assert_eq!(status.code(), Some(2));
    let output = child.wait_with_output().unwrap();
    assert!(output.stderr.is_empty());
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["status"], "error");
    assert_eq!(envelope["error"]["code"], "invalid_input");
    assert_eq!(envelope["error"]["effect"], "none");
    assert_eq!(envelope["error"]["details"]["argument"], "input");
    assert_eq!(
        connections.load(Ordering::SeqCst),
        0,
        "an invalid input document must not produce a request"
    );
}

#[tokio::test]
async fn cli_inspects_current_state_after_a_lost_create_response() {
    let (base, config) = fixture_with(&["one.JPG", "two.JPG"]);
    let (server, upstream) = common::start_authenticated_server_with_upstream(config).await;
    wait_until_idle(&server.url).await;

    let proxy = dropping_proxy(&upstream);
    let (exit, envelope) = command(&proxy, &["albums", "create", "--name", "Recovered"]).await;
    assert_eq!(exit, 7);
    assert_eq!(envelope["error"]["code"], "outcome_unknown");
    assert_eq!(envelope["error"]["effect"], "unknown");
    assert_eq!(
        envelope["error"]["details"],
        json!({
            "operation": "albums-create",
            "photoIds": [],
            "albumId": null,
            "albumName": "Recovered"
        })
    );

    // The inspection query reports the present Album without claiming which
    // caller produced it.
    let (exit, present) = command(&server.url, &["albums", "list", "--name", "recovered"]).await;
    assert_eq!(exit, 0);
    assert_eq!(present["data"]["total"], 1);
    let album_id = present["data"]["items"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (exit, summary) = command(&server.url, &["albums", "get", &album_id]).await;
    assert_eq!(exit, 0);
    assert_eq!(summary["data"]["name"], "Recovered");
    assert_eq!(summary["data"]["photoCount"], 0);
    let version = summary["data"]["albumVersion"].as_str().unwrap().to_owned();

    // A fresh decision can continue from the observed state.
    let (exit, baseline) = command(&server.url, &["photos", "list", "--limit", "60"]).await;
    assert_eq!(exit, 0);
    let ids = item_ids(&baseline);
    let (exit, added) = command_with_stdin(
        &server.url,
        &[
            "albums",
            "add",
            &album_id,
            "--input",
            "-",
            "--if-version",
            &version,
        ],
        &membership_document(&ids),
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(added["data"]["album"]["photoCount"], 2);

    server.close().await.unwrap();
    fs::remove_dir_all(base).unwrap();
}
