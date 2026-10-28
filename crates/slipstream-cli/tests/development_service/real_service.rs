use super::*;
// ---------------------------------------------------------------- real service

/// The two Originals of the shared fixture: JPEG sources only, with
/// processing explicitly the operator-disabled configuration, so the
/// deployment answers deterministically without an engine bundle. The
/// environment default is `auto`, which reports `bundle-unavailable` when
/// the bundle is missing — not the `disabled` condition asserted here.
fn real_service_fixture() -> (PathBuf, Config) {
    let base = temp_base("real-service");
    let originals = base.join("originals");
    let web = base.join("web");
    fs::create_dir(&originals).unwrap();
    fs::create_dir(originals.join("trip")).unwrap();
    fs::create_dir(&web).unwrap();
    fs::write(web.join("index.html"), b"<main>fixture</main>").unwrap();
    for name in ["one.JPG", "two.JPG"] {
        fs::write(originals.join("trip").join(name), jpeg_bytes()).unwrap();
    }
    let config = common::fixture_config(&base, originals, web);
    (base, config)
}

async fn wait_until_idle(server: &str) {
    let client = reqwest::Client::builder()
        .add_root_certificate(common::test_certificate())
        .build()
        .unwrap();
    for _ in 0..400 {
        let response = client
            .get(format!("{server}/api/status"))
            .header("Slipstream-CLI-Contract", "2")
            .bearer_auth(common::ACCESS_TOKEN)
            .send()
            .await
            .unwrap();
        let result: Value = response.json().await.unwrap();
        if result["scan"]["state"] == "idle" {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("fixture Library did not become idle");
}

/// Discovery reports each peer's availability and parameter schema even
/// when processing is disabled in this deployment.
#[tokio::test]
async fn the_real_service_module_discovery_preserves_peer_refusals() {
    let (base, config) = real_service_fixture();
    let server = common::start_authenticated_server(config).await;
    wait_until_idle(&server.url).await;
    let (exit, report) = command(&server.url, &["processing", "modules"]).await;
    assert_eq!(exit, 0);
    assert_eq!(
        report["data"]["contractVersion"],
        "slipstream-module-contract-1"
    );
    let modules = report["data"]["modules"].as_array().unwrap();
    assert_eq!(modules.len(), 2);
    for module in modules {
        assert_eq!(module["availability"]["state"], "unavailable");
        assert!(module["parameterSchema"].is_object());
        assert!(module["id"]["adapterVersion"].is_string());
    }
    server.close().await.unwrap();
    fs::remove_dir_all(base).unwrap();
}

/// The composable recipe reads, saves, and replays through the real
/// authenticated service: an absent recipe is a successful read, a zero-step
/// save commits the caller's exact guards, the same request identity
/// confirms its retained receipt, and the next read observes the committed
/// recipe with its server-assigned revision.
#[tokio::test]
async fn the_real_service_round_trips_the_composable_recipe() {
    let (base, config) = real_service_fixture();
    let server = common::start_authenticated_server(config).await;
    wait_until_idle(&server.url).await;
    let (exit, page) = command(&server.url, &["photos", "list", "--limit", "60"]).await;
    assert_eq!(exit, 0);
    let photo_id = page["data"]["items"][0]["id"].as_str().unwrap().to_owned();
    let (exit, workspace) = command(&server.url, &["workspace", "open", &photo_id]).await;
    assert_eq!(exit, 0, "{workspace}");

    let (exit, read) = command(
        &server.url,
        &["photos", "processing-recipe", "get", &photo_id],
    )
    .await;
    assert_eq!(exit, 0, "{read}");
    assert_eq!(read["data"]["photoId"], photo_id);
    assert_eq!(read["data"]["recipe"], Value::Null);
    let source_revision = read["data"]["sourceRevision"].as_str().unwrap().to_owned();
    assert!(!source_revision.is_empty());

    let save = json!({
        "requestId": "composable-real-1",
        "expectedRecipeRevision": Value::Null,
        "expectedSourceRevision": source_revision,
        "currentStepId": null,
        "steps": [],
    });
    let input = base.join("composable.json");
    fs::write(&input, save.to_string()).unwrap();
    let (exit, saved) = command(
        &server.url,
        &[
            "photos",
            "processing-recipe",
            "save",
            &photo_id,
            "--input",
            input.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(exit, 0, "{saved}");
    assert_eq!(saved["data"]["outcome"], "saved");
    assert_eq!(saved["data"]["requestId"], "composable-real-1");
    assert_eq!(saved["data"]["sourceRevision"], source_revision);
    let committed = saved["data"]["recipeVersion"].as_str().unwrap().to_owned();
    assert!(!committed.is_empty());
    assert_eq!(saved["data"]["recipe"]["revision"], committed.as_str());
    assert_eq!(saved["data"]["recipe"]["steps"], json!([]));

    // The same request identity replays its retained receipt.
    let (exit, replayed) = command(
        &server.url,
        &[
            "photos",
            "processing-recipe",
            "save",
            &photo_id,
            "--input",
            input.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(exit, 0, "{replayed}");
    assert_eq!(replayed["data"]["outcome"], "replayed");
    assert_eq!(replayed["data"]["recipeVersion"], committed.as_str());

    let (exit, reread) = command(
        &server.url,
        &["photos", "processing-recipe", "get", &photo_id],
    )
    .await;
    assert_eq!(exit, 0, "{reread}");
    assert_eq!(reread["data"]["recipe"]["revision"], committed.as_str());
    assert_eq!(reread["data"]["recipe"]["steps"], json!([]));
    server.close().await.unwrap();
    fs::remove_dir_all(base).unwrap();
}

/// An incorrect recipe revision remains a confirmed conflict after the
/// workspace is opened. Live source evidence is contextual for an empty recipe.
#[tokio::test]
async fn the_real_service_refuses_a_stale_composable_recipe_guard() {
    let (base, config) = real_service_fixture();
    let server = common::start_authenticated_server(config).await;
    wait_until_idle(&server.url).await;
    let (exit, page) = command(&server.url, &["photos", "list", "--limit", "60"]).await;
    assert_eq!(exit, 0);
    let photo_id = page["data"]["items"][0]["id"].as_str().unwrap().to_owned();
    let (exit, workspace) = command(&server.url, &["workspace", "open", &photo_id]).await;
    assert_eq!(exit, 0, "{workspace}");
    let (exit, read) = command(
        &server.url,
        &["photos", "processing-recipe", "get", &photo_id],
    )
    .await;
    assert_eq!(exit, 0, "{read}");
    let input = base.join("stale.json");
    fs::write(
        &input,
        json!({
            "requestId": "composable-stale",
            "expectedRecipeRevision": "stale-recipe-revision",
            "expectedSourceRevision": read["data"]["sourceRevision"],
            "currentStepId": null,
            "steps": [],
        })
        .to_string(),
    )
    .unwrap();
    let (exit, refusal) = command(
        &server.url,
        &[
            "photos",
            "processing-recipe",
            "save",
            &photo_id,
            "--input",
            input.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(exit, 4, "{refusal}");
    assert_eq!(refusal["error"]["code"], "conflict");
    assert_eq!(refusal["error"]["effect"], "none");
    server.close().await.unwrap();
    fs::remove_dir_all(base).unwrap();
}

/// New generic darktable intent is refused without creating a recipe, edit
/// state or preview. The accepted full darktable namespace owns that state.
#[tokio::test]
async fn the_real_service_refuses_new_generic_darktable_intent() {
    let (base, config) = real_service_fixture();
    let server = common::start_authenticated_server(config).await;
    wait_until_idle(&server.url).await;
    let (exit, page) = command(&server.url, &["photos", "list", "--limit", "60"]).await;
    assert_eq!(exit, 0);
    let photo_id = page["data"]["items"][0]["id"].as_str().unwrap().to_owned();
    let (exit, workspace) = command(&server.url, &["workspace", "open", &photo_id]).await;
    assert_eq!(exit, 0, "{workspace}");
    let (exit, read) = command(
        &server.url,
        &["photos", "processing-recipe", "get", &photo_id],
    )
    .await;
    assert_eq!(exit, 0);
    let source_revision = read["data"]["sourceRevision"].as_str().unwrap().to_owned();

    // Save one darktable step bound to the guarded Original and select it.
    let input = base.join("step.json");
    fs::write(
        &input,
        json!({
            "requestId": "composable-step",
            "expectedRecipeRevision": null,
            "expectedSourceRevision": source_revision,
            "currentStepId": "develop-1",
            "steps": [{
                "stepId": "develop-1",
                "module": "darktable",
                "input": {
                    "kind": "original",
                    "photoId": photo_id,
                    "sourceRevision": source_revision,
                },
                "parameters": {
                    "schemaVersion": "darktable-params-1",
                    "tree": {"stack": [], "output": {
                        "format": "tiff",
                        "precisionBits": 32,
                        "colorSpace": "prophoto-rgb",
                        "transferFunction": "linear"
                    }},
                },
            }],
        })
        .to_string(),
    )
    .unwrap();
    let (exit, saved) = command(
        &server.url,
        &[
            "photos",
            "processing-recipe",
            "save",
            &photo_id,
            "--input",
            input.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(exit, 2, "{saved}");
    assert_eq!(saved["error"]["code"], "unsupported_state");
    assert_eq!(saved["error"]["effect"], "none");
    let (exit, read) = command(
        &server.url,
        &["photos", "processing-recipe", "get", &photo_id],
    )
    .await;
    assert_eq!(exit, 0, "{read}");
    assert!(read["data"]["recipe"].is_null());
    let (exit, inspected) = command(
        &server.url,
        &[
            "workspace",
            "show",
            workspace["data"]["workspaceId"].as_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(exit, 0, "{inspected}");
    assert!(
        inspected["data"]["edits"]
            .as_array()
            .unwrap()
            .iter()
            .all(|edit| edit["tool"] != "darktable")
    );
    server.close().await.unwrap();
    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn the_real_service_lists_no_exports_or_artifacts_before_any_submission() {
    let (base, config) = real_service_fixture();
    let server = common::start_authenticated_server(config).await;
    wait_until_idle(&server.url).await;
    let (exit, page) = command(&server.url, &["photos", "list", "--limit", "60"]).await;
    assert_eq!(exit, 0, "{page}");
    let photo_id = page["data"]["items"][0]["id"].as_str().unwrap();
    let (exit, listed) =
        command(&server.url, &["photos", "processing-export-list", photo_id]).await;
    assert_eq!(exit, 0, "{listed}");
    assert_eq!(listed["data"]["photoId"], photo_id);
    assert_eq!(listed["data"]["exports"], json!([]));
    assert_eq!(listed["data"]["artifacts"], json!([]));
    assert_eq!(listed["data"]["historicalExports"], json!([]));
    server.close().await.unwrap();
    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn workspace_deletion_preserves_confirmed_inventory_and_active_use_refusals() {
    let (base, config) = real_service_fixture();
    let server = common::start_authenticated_server(config).await;
    wait_until_idle(&server.url).await;
    let (exit, page) = command(&server.url, &["photos", "list", "--limit", "60"]).await;
    assert_eq!(exit, 0);
    let photo_id = page["data"]["items"][0]["id"].as_str().unwrap();
    let (exit, opened) = command(&server.url, &["workspace", "open", photo_id]).await;
    assert_eq!(exit, 0, "{opened}");
    let workspace = opened["data"]["workspaceId"].as_str().unwrap();
    let root = PathBuf::from(opened["data"]["path"].as_str().unwrap());
    let reviewed = opened["data"]["workspaceStateToken"].as_str().unwrap();
    let notes = root.join("notes.txt");
    fs::write(&notes, b"kept independent file").unwrap();
    let (exit, stale) = command(
        &server.url,
        &[
            "workspace",
            "delete",
            workspace,
            "--if-state",
            reviewed,
            "--request",
            "delete-stale",
            "--confirm",
        ],
    )
    .await;
    assert_eq!(exit, 4, "{stale}");
    assert_eq!(stale["error"]["code"], "workspace_changed");
    assert_eq!(stale["error"]["effect"], "none");
    assert_eq!(fs::read(&notes).unwrap(), b"kept independent file");
    let (exit, current) = command(&server.url, &["workspace", "show", workspace]).await;
    assert_eq!(exit, 0, "{current}");
    let token = current["data"]["workspaceStateToken"].as_str().unwrap();
    let held = photo_files::FileLease::acquire(
        fs::File::open(&notes).unwrap(),
        photo_files::LeaseMode::Shared,
        Duration::ZERO,
    )
    .unwrap();
    let arguments = [
        "workspace",
        "delete",
        workspace,
        "--if-state",
        token,
        "--request",
        "delete-held",
        "--confirm",
    ];
    let (exit, blocked) = command(&server.url, &arguments).await;
    assert_eq!(exit, 4, "{blocked}");
    assert_eq!(blocked["error"]["code"], "workspace_referenced");
    assert_eq!(blocked["error"]["effect"], "none");
    assert_eq!(
        blocked["error"]["details"]["blockers"][0]["kind"],
        "active-use"
    );
    assert_eq!(
        blocked["error"]["details"]["blockers"][0]["name"],
        workspace
    );
    assert_eq!(fs::read(&notes).unwrap(), b"kept independent file");
    drop(held);
    let (exit, deleted) = command(&server.url, &arguments).await;
    assert_eq!(exit, 0, "{deleted}");
    assert_eq!(deleted["data"]["reclamation"], "complete");
    assert!(!root.exists());
    assert!(base.join("originals/trip/one.JPG").is_file());
    server.close().await.unwrap();
    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn malformed_workspace_refusals_preserve_unknown_mutation_identity_without_retry() {
    let valid = json!({
        "workspaceId":"workspace-1",
        "blockers":[{"kind":"active-use","id":null,"name":"independent editor","action":"Close the editor."}]
    });
    let mut cases = vec![json!({})];
    for (key, values) in [
        ("workspaceId", vec![Value::Null, json!(23), json!("")]),
        (
            "blockers",
            vec![Value::Null, json!({}), json!([]), json!([null])],
        ),
    ] {
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove(key);
        cases.push(missing);
        for value in values {
            let mut malformed = valid.clone();
            malformed[key] = value;
            cases.push(malformed);
        }
    }
    for (key, values) in [
        (
            "kind",
            vec![Value::Null, json!(23), json!(""), json!("editor")],
        ),
        ("id", vec![json!(23), json!({})]),
        ("name", vec![Value::Null, json!(23), json!("")]),
        ("action", vec![Value::Null, json!(23), json!("")]),
    ] {
        let mut missing = valid.clone();
        missing["blockers"][0].as_object_mut().unwrap().remove(key);
        cases.push(missing);
        for value in values {
            let mut malformed = valid.clone();
            malformed["blockers"][0][key] = value;
            cases.push(malformed);
        }
    }
    let mut invalid_second = valid.clone();
    invalid_second["blockers"]
        .as_array_mut()
        .unwrap()
        .push(json!({}));
    cases.push(invalid_second);
    let request = json!({"ifState":"inventory-1","requestId":"delete-1","confirm":true});
    for details in cases {
        let service = fake_service(vec![
            Step::Capabilities,
            Step::Json(
                409,
                json!({"error":{"code":"workspace_referenced","message":"Workspace is in use.","effect":"none","details":details}}),
            ),
        ]);
        let (exit, envelope) = command(
            &service.url,
            &[
                "workspace",
                "delete",
                "workspace-1",
                "--if-state",
                "inventory-1",
                "--request",
                "delete-1",
                "--confirm",
            ],
        )
        .await;
        assert_eq!(exit, 7, "{details}: {envelope}");
        assert_eq!(envelope["error"]["code"], "outcome_unknown");
        assert_eq!(envelope["error"]["effect"], "unknown");
        assert_eq!(
            envelope["error"]["details"]["operation"],
            "workspace-delete"
        );
        assert_eq!(envelope["error"]["details"]["request"], request);
        let (connections, requests) = service.finish();
        assert_eq!(
            connections, 2,
            "only the handshake and one mutation may run"
        );
        assert_eq!(requests.len(), 2, "the mutation must not be retried");
        assert_eq!(
            requests[1].request_line,
            "DELETE /api/workspaces/workspace-1 HTTP/1.1"
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&requests[1].body).unwrap(),
            request
        );
    }
}

#[tokio::test]
async fn complete_workspace_blockers_with_optional_reasons_remain_confirmed_refusals() {
    let kinds = [
        "state",
        "job",
        "output",
        "dependency",
        "active-use",
        "unverified-use",
    ];
    let blockers: Vec<Value> = kinds
        .iter()
        .enumerate()
        .map(|(index, kind)| {
            let mut blocker =
                json!({"kind":kind,"id":null,"name":"retained use","action":"Settle this use."});
            if index % 2 == 0 {
                blocker["id"] = json!(format!("use-{index}"));
                blocker["reason"] = json!("The referenced bytes are still needed.");
            }
            blocker
        })
        .collect();
    let error = json!({"code":"workspace_referenced","message":"Workspace is in use.","effect":"none","details":{"workspaceId":"workspace-1","blockers":blockers,"reason":"Retained dependencies prevent deletion."}});
    let service = fake_service(vec![
        Step::Capabilities,
        Step::Json(409, json!({"error":error})),
    ]);
    let (exit, envelope) = command(
        &service.url,
        &[
            "workspace",
            "delete",
            "workspace-1",
            "--if-state",
            "inventory-1",
            "--request",
            "delete-1",
            "--confirm",
        ],
    )
    .await;
    assert_eq!(exit, 4, "{envelope}");
    assert_eq!(envelope["error"], error);
    let (connections, requests) = service.finish();
    assert_eq!(connections, 2);
    assert_eq!(requests.len(), 2);
}

#[tokio::test]
async fn invalid_style_mapping_is_a_confirmed_refusal_with_empty_details() {
    let base = temp_base("invalid-style-mapping");
    let input = write_input(
        &base,
        "mapping.json",
        &json!({
            "mode":"replace",
            "mapping":[
                {"entryId":"entry-1","instanceId":"instance-A"},
                {"entryId":"entry-2","instanceId":"instance-A"}
            ],
            "initialization":"literal",
            "maskPolicy":"omit",
            "includeOutput":false
        })
        .to_string(),
    );
    let error = json!({"code":"invalid_mapping","message":"Style mapping repeats a target instance.","effect":"none","details":{}});
    let service = fake_service(vec![
        Step::Capabilities,
        Step::Json(400, json!({"error":error})),
    ]);
    let (exit, envelope) = command(
        &service.url,
        &[
            "darktable",
            "styles",
            "apply",
            PHOTO_ID,
            "style-1",
            "--version",
            "version-1",
            "--input",
            &input,
            "--edit",
            "edit-1",
            "--if-state",
            "state-1",
            "--request",
            "apply-1",
        ],
    )
    .await;
    assert_eq!(exit, 2, "{envelope}");
    assert_eq!(envelope["error"], error);
    let (connections, requests) = service.finish();
    assert_eq!(
        connections, 2,
        "only the handshake and one mutation may run"
    );
    assert_eq!(requests.len(), 2, "the mutation must not be retried");
    let submitted: Value = serde_json::from_slice(&requests[1].body).unwrap();
    assert_eq!(submitted["operation"], "styles apply");
    assert_eq!(submitted["editId"], "edit-1");
    assert_eq!(submitted["arguments"]["requestId"], "apply-1");
    fs::remove_dir_all(base).unwrap();
}
