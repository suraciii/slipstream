use super::*;

#[tokio::test]
async fn cli_restore_all_sends_selection_and_preserves_outcomes() {
    let (base, _) = fixture_with(&[]);
    let excluded = missing_photo_id(1);
    let restored = missing_photo_id(2);
    let changed = missing_photo_id(3);
    let operation = "00000000-0000-4000-8000-000000000203";
    let input = write_input(
        &base,
        "exclude.json",
        &json!({"photoIds": [excluded]}).to_string(),
    );
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let response = json!({
        "operationId": operation,
        "counts": {"restored": 1, "alreadyActive": 0, "changedElsewhere": 1, "missing": 0},
        "results": [
            {"photoId": restored, "outcome": "restored"},
            {"photoId": changed, "outcome": "changed-elsewhere"}
        ]
    });
    let reply = response.clone();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = common::accept_tls(&listener);
        let (head, _) = common::read_http_message(&mut stream).unwrap();
        common::assert_bearer(head.as_bytes());
        write_json_response(&mut stream, 200, &capabilities_body());
        let (mut stream, _) = common::accept_tls(&listener);
        let (head, body) = common::read_http_message(&mut stream).unwrap();
        common::assert_bearer(head.as_bytes());
        assert!(head.starts_with("POST /api/trash/restore "));
        write_json_response(&mut stream, 200, &reply);
        serde_json::from_slice::<Value>(&body).unwrap()
    });
    let (exit, envelope) = command(
        &format!("https://127.0.0.1:{}", address.port()),
        &[
            "photos",
            "restore",
            operation,
            "--all",
            "--exclude-input",
            &input,
        ],
    )
    .await;
    let body = handle.join().unwrap();
    assert_eq!(
        body,
        json!({
            "operationId": operation, "all": true, "photoIds": [], "excludePhotoIds": [excluded]
        })
    );
    assert_eq!(exit, 4, "{envelope}");
    assert_eq!(envelope["error"]["code"], "conflict");
    assert_eq!(envelope["error"]["effect"], "partial");
    assert_eq!(envelope["data"], response);
    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn cli_restore_all_rejects_invalid_exclusions_before_network() {
    let (base, _) = fixture_with(&[]);
    for (name, input) in [
        ("duplicates.json", r#"["same", "same"]"#),
        ("empty-id.json", r#"{"photoIds":[""]}"#),
        ("extra.json", r#"{"photoIds":[],"extra":true}"#),
    ] {
        let input = write_input(&base, name, input);
        let (exit, envelope) = command(
            "https://127.0.0.1:9",
            &[
                "photos",
                "restore",
                "operation",
                "--all",
                "--exclude-input",
                &input,
            ],
        )
        .await;
        assert_eq!(exit, 2, "{name}: {envelope}");
        assert_eq!(envelope["error"]["code"], "invalid_input");
        assert_eq!(envelope["error"]["effect"], "none");
    }
    fs::remove_dir_all(base).unwrap();
}
#[tokio::test]
async fn cli_restore_all_reports_refusal_with_operation_identity() {
    let operation = "00000000-0000-4000-8000-000000000203";
    for (status, error) in [
        (
            400,
            json!({"code":"invalid_input","message":"Invalid operation ID", "effect":"none",
            "details":{"argument":"operationId","reason":"Invalid operation ID","operationId":operation}}),
        ),
        (
            409,
            json!({"code":"conflict","message":"Selection exceeds review bound", "effect":"none",
            "details":{"operationId":operation}}),
        ),
    ] {
        let (server, handle, _) =
            stub_service(MutationReply::Status(status, json!({"error":error})));
        let (exit, envelope) = command(&server, &["photos", "restore", operation, "--all"]).await;
        handle.join().unwrap();
        assert_ne!(exit, 0, "{envelope}");
        assert_eq!(envelope["error"]["code"], error["code"]);
        assert_eq!(envelope["error"]["effect"], "none");
        assert_eq!(envelope["error"]["details"]["operationId"], operation);
    }
}

#[tokio::test]
async fn cli_restore_all_real_server_refuses_invalid_id_and_overbound_without_mutation() {
    let (base, config) = fixture_with(&[]);
    let server = common::start_authenticated_server(config.clone()).await;
    wait_until_idle(&server.url).await;
    let db = rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    db.execute_batch("BEGIN;
        WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<5000)
        INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available)
        SELECT 'original-' || x, 'seed/' || x || '.jpg', 'jpeg', 1, 1, 1 FROM n;
        WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<5000)
        INSERT INTO photos(id,original_id,available,preview_state,sort_path,removed_at_ms,removed_operation)
        SELECT 'photo-' || x, 'original-' || x, 1, 'unavailable', 'seed/' || x, 1, 'seed' FROM n;
        COMMIT;").unwrap();
    let operation = "00000000-0000-4000-8000-000000000207";
    for (id, code) in [("invalid-id", "invalid_input"), (operation, "conflict")] {
        let response = post_json(
            &server.url,
            "/api/trash/restore",
            json!({
                "operationId": id, "all": true, "photoIds": [], "excludePhotoIds": []
            }),
        )
        .await;
        assert_eq!(
            response.status().as_u16(),
            if code == "conflict" { 409 } else { 400 }
        );
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["error"]["code"], code);
        assert_eq!(body["error"]["effect"], "none");
        assert_eq!(body["error"]["details"]["operationId"], id);
        let (exit, envelope) = command(&server.url, &["photos", "restore", id, "--all"]).await;
        assert_ne!(exit, 0, "{envelope}");
        assert_eq!(envelope["error"]["code"], code, "{envelope}");
        assert_eq!(envelope["error"]["effect"], "none");
        assert_eq!(envelope["error"]["details"]["operationId"], id);
        let count: i64 = db
            .query_row(
                "SELECT count(*) FROM photos WHERE removed_at_ms IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 5001);
    }
    drop(db);
    server.close().await.unwrap();
    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn cli_restore_all_excludes_ids_and_replays_the_captured_selection() {
    let (base, config) = fixture_with(&["one.JPG", "two.JPG"]);
    let server = common::start_authenticated_server(config).await;
    wait_until_idle(&server.url).await;
    let photos = photo_page(&server.url).await;
    let mut targets = Vec::new();
    for (photo_id, version) in &photos {
        let (exit, selected) = command(
            &server.url,
            &[
                "photos",
                "set",
                photo_id,
                "--selection",
                "rejected",
                "--if-version",
                version,
            ],
        )
        .await;
        assert_eq!(exit, 0, "{selected}");
        targets.push(json!({
            "photoId": photo_id,
            "selectionState": "rejected",
            "decisionVersion": selected["data"]["results"][0]["current"]["decisionVersion"],
            "removedAtMs": null
        }));
    }
    let input = write_input(
        &base,
        "remove-all.json",
        &json!({"photos": targets}).to_string(),
    );
    let (exit, removed) = command(
        &server.url,
        &[
            "photos",
            "remove",
            "00000000-0000-4000-8000-000000000204",
            "--input",
            &input,
        ],
    )
    .await;
    assert_eq!(exit, 0, "{removed}");
    let excluded = write_input(&base, "exclude-one.json", &json!([photos[0].0]).to_string());
    let operation = "00000000-0000-4000-8000-000000000205";
    let args = [
        "photos",
        "restore",
        operation,
        "--all",
        "--exclude-input",
        &excluded,
    ];
    let (exit, first) = command(&server.url, &args).await;
    assert_eq!(exit, 0, "{first}");
    assert_eq!(first["data"]["counts"]["restored"], 1);
    assert_eq!(first["data"]["results"][0]["photoId"], photos[1].0);
    let (exit, replay) = command(&server.url, &args).await;
    assert_eq!(exit, 0, "{replay}");
    assert_eq!(replay["data"], first["data"]);
    let (exit, inspected) = command(&server.url, &["photos", "restore-operation", operation]).await;
    assert_eq!(exit, 0, "{inspected}");
    assert_eq!(inspected["data"], first["data"]);
    let (exit, remaining) = command(
        &server.url,
        &[
            "photos",
            "restore",
            "00000000-0000-4000-8000-000000000206",
            "--all",
        ],
    )
    .await;
    assert_eq!(exit, 0, "{remaining}");
    assert_eq!(remaining["data"]["counts"]["restored"], 1);
    assert_eq!(remaining["data"]["results"][0]["photoId"], photos[0].0);
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
async fn tls_proxy_delivers_complete_response_before_upstream_closes() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let upstream = format!("http://{}", listener.local_addr().unwrap());
    let (release, wait_for_client) = std::sync::mpsc::channel();
    let service = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        common::read_http_message(&mut stream).unwrap();
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            .unwrap();
        stream.flush().unwrap();
        let _ = wait_for_client.recv_timeout(std::time::Duration::from_secs(10));
    });
    let proxy = common::tls_proxy(&upstream);
    let response = reqwest::Client::builder()
        .add_root_certificate(common::test_certificate())
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .unwrap()
        .get(proxy)
        .send()
        .await;
    let _ = release.send(());
    service.join().unwrap();
    let response = response.expect("complete response must not wait for upstream close");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(response.text().await.unwrap(), "{}");
}

const PHOTO: &str = "00000000-0000-4000-8000-000000000001";
const ORIGINAL: &str = "00000000-0000-4000-8000-000000000002";
const OPERATION: &str = "00000000-0000-4000-8000-000000000003";

#[tokio::test]
async fn trash_listing_cleanup_and_receipt_use_exact_reviewed_evidence() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let location = "folder/space \"\n雪.jpg";
    let item = json!({"photoId": PHOTO, "removedAtMs": 42, "originalId": ORIGINAL, "originalLocation": location});
    let first_listing = json!({
        "photoId": PHOTO, "removedAtMs": 42, "originalId": ORIGINAL,
        "originalLocation": location, "originalKind": "jpeg",
        "originalSize": 10, "originalAvailable": true,
        "state": "captured", "photo": null
    });
    let second_listing = json!({
        "photoId": "00000000-0000-4000-8000-000000000004", "removedAtMs": 41,
        "originalId": "00000000-0000-4000-8000-000000000005",
        "originalLocation": "folder/b.jpg", "originalKind": "jpeg",
        "originalSize": 10, "originalAvailable": false,
        "state": "captured", "photo": null
    });
    let receipt = json!({"operationId": OPERATION, "reviewed": 1,
        "counts": {"cleaned": 1, "changed": 0, "present": 0, "blocked": 0, "unavailable": 0, "inspectionFailed": 0},
        "items": [{"photoId": PHOTO, "originalLocation": location, "originalKind": "jpeg", "state": "cleaned", "message": null, "blockers": []}]});
    let reply = receipt.clone();
    let handle = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for index in 0..4 {
            let (mut stream, _) = common::accept_tls(&listener);
            let (head, _) = common::read_http_message(&mut stream).unwrap();
            common::assert_bearer(head.as_bytes());
            write_json_response(&mut stream, 200, &capabilities_body());
            let (mut stream, _) = common::accept_tls(&listener);
            let (head, body) = common::read_http_message(&mut stream).unwrap();
            common::assert_bearer(head.as_bytes());
            let response = match index {
                0 => {
                    json!({"start":0,"limit":1,"total":2,"cleanupMaximum":5000,"evaluatedAt":"2026-01-01T00:00:00Z","expiresAt":"2026-01-01T00:15:00Z","operation":null,"nextCursor":"opaque","libraryRoot":"/service/photos","namespace":"service","photos":[first_listing.clone()]})
                }
                1 => {
                    json!({"start":1,"limit":1,"total":2,"cleanupMaximum":5000,"evaluatedAt":"2026-01-01T00:00:00Z","expiresAt":null,"operation":null,"nextCursor":null,"libraryRoot":"/service/photos","namespace":"service","photos":[second_listing.clone()]})
                }
                _ => reply.clone(),
            };
            write_json_response(&mut stream, 200, &response);
            requests.push((head, body));
        }
        requests
    });
    let server = format!("https://127.0.0.1:{}", address.port());
    let (exit, first) = command(&server, &["trash", "list", "--start", "0", "--limit", "1"]).await;
    assert_eq!(exit, 0);
    assert_eq!(first["data"]["nextCursor"], "opaque");
    let (exit, second) = command(&server, &["trash", "list", "--cursor", "opaque"]).await;
    assert_eq!(exit, 0);
    assert!(second["data"]["nextCursor"].is_null());
    let input = json!({"items": [item.clone()]}).to_string();
    let (exit, result) = common::command_with_stdin(
        &server,
        &["trash", "cleanup", OPERATION, "--input", "-"],
        &input,
    )
    .await;
    assert_eq!(exit, 0, "{result}");
    assert_eq!(result["data"], receipt);
    let (exit, result) = command(&server, &["trash", "cleanup-operation", OPERATION]).await;
    assert_eq!(exit, 0, "{result}");
    assert_eq!(result["data"], receipt);
    let requests = handle.join().unwrap();
    assert!(requests[0].0.starts_with("GET /api/trash?start=0&limit=1 "));
    assert!(requests[1].0.starts_with("GET /api/trash?cursor=opaque "));
    assert!(requests[2].0.starts_with("POST /api/trash/cleanup "));
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[2].1).unwrap(),
        json!({"operationId":OPERATION,"items":[item]})
    );
    assert!(
        requests[3]
            .0
            .starts_with(&format!("GET /api/trash/cleanup-operations/{OPERATION} "))
    );
}

#[tokio::test]
async fn malformed_or_oversized_cleanup_input_fails_before_network() {
    let bad = json!({"items":[{"photoId":PHOTO,"originalId":ORIGINAL,"removedAtMs":42,"originalLocation":"../outside"}]}).to_string();
    let (exit, response) = common::command_with_stdin(
        "https://127.0.0.1:9",
        &["trash", "cleanup", OPERATION, "--input", "-"],
        &bad,
    )
    .await;
    assert_eq!(exit, 2, "{response}");
    assert_eq!(response["error"]["code"], "invalid_input");
    let oversized = " ".repeat(1024 * 1024 + 1);
    let (exit, response) = common::command_with_stdin(
        "https://127.0.0.1:9",
        &["trash", "cleanup", OPERATION, "--input", "-"],
        &oversized,
    )
    .await;
    assert_eq!(exit, 2, "{response}");
    assert_eq!(response["error"]["code"], "limit_exceeded");
    assert_eq!(response["error"]["details"]["limit"], 1024 * 1024);
}

#[tokio::test]
async fn lost_cleanup_response_is_recovered_with_the_same_operation() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let receipt = json!({"operationId":OPERATION,"reviewed":1,
        "counts":{"cleaned":0,"changed":0,"present":1,"blocked":0,"unavailable":0,"inspectionFailed":0},
        "items":[{"photoId":PHOTO,"originalLocation":"a.jpg","originalKind":"jpeg","state":"present","message":null,"blockers":[]}]});
    let reply = receipt.clone();
    let handle = std::thread::spawn(move || {
        for index in 0..2 {
            let (mut stream, _) = common::accept_tls(&listener);
            let _ = common::read_http_message(&mut stream).unwrap();
            write_json_response(&mut stream, 200, &capabilities_body());
            let (mut stream, _) = common::accept_tls(&listener);
            let (head, _) = common::read_http_message(&mut stream).unwrap();
            if index == 0 {
                assert!(head.starts_with("POST /api/trash/cleanup "));
                drop(stream);
            } else {
                assert!(
                    head.starts_with(&format!("GET /api/trash/cleanup-operations/{OPERATION} "))
                );
                write_json_response(&mut stream, 200, &reply);
            }
        }
    });
    let server = format!("https://127.0.0.1:{}", address.port());
    let input = json!({"items":[{"photoId":PHOTO,"originalId":ORIGINAL,"removedAtMs":42,"originalLocation":"a.jpg"}]}).to_string();
    let (exit, unknown) = common::command_with_stdin(
        &server,
        &["trash", "cleanup", OPERATION, "--input", "-"],
        &input,
    )
    .await;
    assert_eq!(exit, 7, "{unknown}");
    assert_eq!(unknown["error"]["code"], "outcome_unknown");
    assert_eq!(unknown["error"]["details"]["operationId"], OPERATION);
    let (exit, recovered) = command(&server, &["trash", "cleanup-operation", OPERATION]).await;
    assert_eq!(exit, 4, "{recovered}");
    assert_eq!(recovered["error"]["code"], "conflict");
    assert_eq!(recovered["data"], receipt);
    handle.join().unwrap();
}

#[tokio::test]
async fn trash_rejects_a_service_without_cleanup_capability() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = common::accept_tls(&listener);
        let _ = common::read_http_message(&mut stream).unwrap();
        let mut capabilities = capabilities_body();
        capabilities["limits"]
            .as_object_mut()
            .unwrap()
            .remove("trashCleanupMaximum");
        write_json_response(&mut stream, 200, &capabilities);
    });
    let server = format!("https://127.0.0.1:{}", address.port());
    let (exit, response) = command(&server, &["trash", "list"]).await;
    assert_eq!(exit, 6, "{response}");
    assert_eq!(response["error"]["code"], "incompatible_server");
    assert_eq!(
        response["error"]["details"]["field"],
        "limits.trashCleanupMaximum"
    );
    handle.join().unwrap();
}

#[tokio::test]
async fn mixed_cleanup_reports_partial_with_the_complete_receipt() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let second_photo = "00000000-0000-4000-8000-000000000004";
    let second_original = "00000000-0000-4000-8000-000000000005";
    let first = json!({"photoId": PHOTO, "removedAtMs": 42, "originalId": ORIGINAL, "originalLocation": "a.jpg"});
    let second = json!({"photoId": second_photo, "removedAtMs": 41, "originalId": second_original, "originalLocation": "b.jpg"});
    let receipt = json!({
        "operationId": OPERATION, "reviewed": 2,
        "counts": {"cleaned": 1, "changed": 0, "present": 1, "blocked": 0, "unavailable": 0, "inspectionFailed": 0},
        "items": [
            {"photoId": PHOTO, "originalLocation": "a.jpg", "originalKind": "jpeg", "state": "cleaned", "message": null, "blockers": []},
            {"photoId": second_photo, "originalLocation": "b.jpg", "originalKind": "jpeg", "state": "present", "message": null, "blockers": []}
        ]
    });
    let reply = receipt.clone();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = common::accept_tls(&listener);
        let _ = common::read_http_message(&mut stream).unwrap();
        write_json_response(&mut stream, 200, &capabilities_body());
        let (mut stream, _) = common::accept_tls(&listener);
        let (head, body) = common::read_http_message(&mut stream).unwrap();
        assert!(head.starts_with("POST /api/trash/cleanup "));
        write_json_response(&mut stream, 200, &reply);
        serde_json::from_slice::<Value>(&body).unwrap()
    });
    let server = format!("https://127.0.0.1:{}", address.port());
    let input = json!({"items": [first, second]}).to_string();
    let (exit, result) = common::command_with_stdin(
        &server,
        &["trash", "cleanup", OPERATION, "--input", "-"],
        &input,
    )
    .await;
    assert_eq!(exit, 5, "{result}");
    assert_eq!(result["status"], "partial");
    assert_eq!(result["error"]["code"], "trash_cleanup_partial");
    assert_eq!(result["error"]["effect"], "partial");
    assert_eq!(result["data"], receipt);
    let request = handle.join().unwrap();
    assert_eq!(request["items"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn trash_empty_captures_every_record_and_submits_one_cleanup() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let second_photo = "00000000-0000-4000-8000-000000000004";
    let second_original = "00000000-0000-4000-8000-000000000005";
    let first = json!({"photoId": PHOTO, "removedAtMs": 42, "originalId": ORIGINAL, "originalLocation": "a.jpg", "originalKind": "jpeg", "originalSize": 10, "originalAvailable": false, "state": "captured", "photo": null});
    let second = json!({"photoId": second_photo, "removedAtMs": 41, "originalId": second_original, "originalLocation": "b.jpg", "originalKind": "jpeg", "originalSize": 10, "originalAvailable": false, "state": "captured", "photo": null});
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = common::accept_tls(&listener);
        let _ = common::read_http_message(&mut stream).unwrap();
        write_json_response(&mut stream, 200, &capabilities_body());
        let (mut stream, _) = common::accept_tls(&listener);
        let (head, _) = common::read_http_message(&mut stream).unwrap();
        assert!(head.starts_with("GET /api/trash?start=0&limit=60 "));
        write_json_response(
            &mut stream,
            200,
            &json!({
                "start": 0, "limit": 60, "total": 2, "cleanupMaximum": 5000,
                "evaluatedAt": "2026-01-01T00:00:00Z", "expiresAt": null,
                "nextCursor": null, "libraryRoot": "/service/photos",
                "namespace": "service", "operation": null, "photos": [first, second]
            }),
        );
        let (mut stream, _) = common::accept_tls(&listener);
        let (head, body) = common::read_http_message(&mut stream).unwrap();
        assert!(head.starts_with("POST /api/trash/cleanup "));
        let request: Value = serde_json::from_slice(&body).unwrap();
        let operation_id = request["operationId"].as_str().unwrap().to_owned();
        let items: Vec<_> = request["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| {
                json!({
                    "photoId": item["photoId"],
                    "originalLocation": item["originalLocation"],
                    "originalKind": "jpeg",
                    "state": "cleaned",
                    "message": null,
                    "blockers": []
                })
            })
            .collect();
        let receipt = json!({
            "operationId": operation_id,
            "reviewed": 2,
            "counts": {"cleaned": 2, "changed": 0, "present": 0, "blocked": 0, "unavailable": 0, "inspectionFailed": 0},
            "items": items
        });
        write_json_response(&mut stream, 200, &receipt);
        (request, receipt)
    });
    let server = format!("https://127.0.0.1:{}", address.port());
    let (exit, result) = command(&server, &["trash", "empty"]).await;
    assert_eq!(exit, 0, "{result}");
    assert_eq!(result["data"]["counts"]["cleaned"], 2);
    assert_eq!(result["data"]["reviewed"], 2);
    let (request, receipt) = handle.join().unwrap();
    assert_eq!(request["items"].as_array().unwrap().len(), 2);
    assert_eq!(result["data"], receipt);
    let operation_id = result["data"]["operationId"].as_str().unwrap();
    assert!((36..=64).contains(&operation_id.len()));
    assert!(
        operation_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte) || byte == b'-')
    );
}

#[tokio::test]
async fn trash_empty_reports_an_empty_recycle_bin_without_a_cleanup_request() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = common::accept_tls(&listener);
        let _ = common::read_http_message(&mut stream).unwrap();
        write_json_response(&mut stream, 200, &capabilities_body());
        let (mut stream, _) = common::accept_tls(&listener);
        let (head, _) = common::read_http_message(&mut stream).unwrap();
        assert!(head.starts_with("GET /api/trash?start=0&limit=60 "));
        write_json_response(
            &mut stream,
            200,
            &json!({
                "start": 0, "limit": 60, "total": 0, "cleanupMaximum": 5000,
                "evaluatedAt": "2026-01-01T00:00:00Z", "expiresAt": null,
                "nextCursor": null, "libraryRoot": "/service/photos",
                "namespace": "service", "operation": null, "photos": []
            }),
        );
    });
    let server = format!("https://127.0.0.1:{}", address.port());
    let (exit, result) = command(&server, &["trash", "empty"]).await;
    assert_eq!(exit, 0, "{result}");
    assert_eq!(result["data"]["reviewed"], 0);
    assert!(result["data"]["operationId"].is_null());
    handle.join().unwrap();
}
