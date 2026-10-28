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
