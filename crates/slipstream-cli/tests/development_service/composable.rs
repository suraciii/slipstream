use super::*;
// ------------------------------------------------------ composable writes

/// The composable save submits the caller's complete document unchanged —
/// guards, step binding, and the module-owned parameter tree — and a
/// confirmed conflict is exit 4 with the service's retained recipe.
#[tokio::test]
async fn composable_recipe_save_submits_the_exact_steps_and_maps_the_conflict() {
    let base = temp_base("composable-conflict");
    let document = composable_save_document();
    let input = write_input(&base, "composable.json", &document.to_string());
    let service = fake_service(vec![Step::Capabilities, Step::ComposableRecipeConflict]);
    let (exit, envelope) = command(
        &service.url,
        &[
            "photos",
            "processing-recipe",
            "save",
            PHOTO_ID,
            "--input",
            &input,
        ],
    )
    .await;
    assert_eq!(exit, 4);
    assert_eq!(envelope["error"]["code"], "recipe_conflict");
    assert_eq!(envelope["error"]["effect"], "none");
    assert_eq!(
        envelope["error"]["details"]["revision"],
        CURRENT_RECIPE_VERSION
    );
    assert_eq!(
        envelope["error"]["details"]["sourceRevision"],
        SOURCE_REVISION
    );
    assert_eq!(
        envelope["error"]["details"]["steps"][0]["parameters"]["tree"],
        json!({"stack": []})
    );
    let (connections, requests) = service.finish();
    assert_eq!(connections, 2);
    assert_eq!(requests.len(), 2);
    let save = &requests[1];
    assert_eq!(
        save.request_line,
        format!("POST /api/photos/{PHOTO_ID}/processing-recipe HTTP/1.1")
    );
    // The exact submitted body: the CLI neither completed nor rewrote any
    // guard or module-owned parameter behind the caller's back.
    assert_eq!(
        serde_json::from_slice::<Value>(&save.body).unwrap(),
        document
    );
    fs::remove_dir_all(base).unwrap();
}

/// A composable save whose response is lost after admission is exit 7 with
/// an unknown effect, and the CLI never submits the write a second time.
#[tokio::test]
async fn a_lost_composable_recipe_save_is_an_unknown_outcome_without_retry() {
    let base = temp_base("composable-lost");
    let input = write_input(
        &base,
        "composable.json",
        &composable_save_document().to_string(),
    );
    let service = fake_service(vec![Step::Capabilities, Step::LoseAfterRead]);
    let (exit, envelope) = command(
        &service.url,
        &[
            "photos",
            "processing-recipe",
            "save",
            PHOTO_ID,
            "--input",
            &input,
        ],
    )
    .await;
    assert_eq!(exit, 7);
    assert_eq!(envelope["error"]["code"], "outcome_unknown");
    assert_eq!(envelope["error"]["effect"], "unknown");
    assert_eq!(
        envelope["error"]["details"]["operation"],
        "photos-processing-recipe-save"
    );
    assert_eq!(envelope["error"]["details"]["photoIds"], json!([PHOTO_ID]));
    let (connections, requests) = service.finish();
    assert_eq!(connections, 2, "the handshake plus exactly one submission");
    assert_eq!(requests.len(), 2, "the write is never retried");
    fs::remove_dir_all(base).unwrap();
}

/// Duplicate keys, omitted nullable guards, and a current step outside the
/// submitted steps are refused with exit 2 before any network access:
/// nothing listens, so a locally valid document on the same address fails
/// as transport instead.
#[tokio::test]
async fn malformed_composable_documents_are_refused_before_any_network() {
    let base = temp_base("composable-shapes");
    let dead = "https://127.0.0.1:9";
    let cases = [
        (
            "duplicate-request-id.json".to_owned(),
            r#"{"requestId":"c-1","requestId":"c-2","expectedRecipeRevision":null,"expectedSourceRevision":"source-3","currentStepId":null,"steps":[]}"#.to_owned(),
            "input",
        ),
        (
            "unknown-key.json".to_owned(),
            json!({
                "requestId": "c-1",
                "expectedRecipeRevision": null,
                "expectedSourceRevision": "source-3",
                "currentStepId": null,
                "steps": [],
                "planner": true,
            })
            .to_string(),
            "input",
        ),
        (
            "omitted-current-step.json".to_owned(),
            json!({
                "requestId": "c-1",
                "expectedRecipeRevision": null,
                "expectedSourceRevision": "source-3",
                "steps": [],
            })
            .to_string(),
            "input",
        ),
        (
            "illformed-request-id.json".to_owned(),
            json!({
                "requestId": "c 1",
                "expectedRecipeRevision": null,
                "expectedSourceRevision": "source-3",
                "currentStepId": null,
                "steps": [],
            })
            .to_string(),
            "requestId",
        ),
        (
            "current-step-without-steps.json".to_owned(),
            json!({
                "requestId": "c-1",
                "expectedRecipeRevision": null,
                "expectedSourceRevision": "source-3",
                "currentStepId": "develop-1",
                "steps": [],
            })
            .to_string(),
            "currentStepId",
        ),
    ];
    for (name, content, argument) in cases {
        let input = write_input(&base, &name, &content);
        let (exit, envelope) = command(
            dead,
            &[
                "photos",
                "processing-recipe",
                "save",
                PHOTO_ID,
                "--input",
                &input,
            ],
        )
        .await;
        assert_eq!(exit, 2, "for {name}");
        assert_eq!(envelope["error"]["code"], "invalid_input", "for {name}");
        assert_eq!(
            envelope["error"]["details"]["argument"], argument,
            "for {name}"
        );
    }
    // A locally valid document reaches the dead transport.
    let valid = write_input(&base, "valid.json", &composable_save_document().to_string());
    let (exit, envelope) = command(
        dead,
        &[
            "photos",
            "processing-recipe",
            "save",
            PHOTO_ID,
            "--input",
            &valid,
        ],
    )
    .await;
    assert_eq!(exit, 6);
    assert_eq!(envelope["error"]["code"], "transport_failed");
    assert_eq!(
        envelope["error"]["details"]["operation"],
        "photos-processing-recipe-save"
    );
    fs::remove_dir_all(base).unwrap();
}

fn work_fixture(request_id: &str, accepted_at: u64, state: &str) -> Value {
    let terminal = matches!(state, "failed" | "cancelled" | "succeeded");
    json!({
        "photoId": PHOTO_ID, "requestId": request_id, "stepId": "develop-1",
        "module": "darktable", "recipeRevision": "recipe-9",
        "sourceRevision": format!("opaque\0{}", "s".repeat(256)),
        "adapterSchemaVersion": "darktable-adapter-1:darktable-params-1",
        "parameters": {"schemaVersion": "darktable-params-1", "tree": {"stack": []}},
        "input": {"kind": "original", "photoId": PHOTO_ID, "sourceRevision": SOURCE_REVISION},
        "bundleId": "bundle-1", "state": state, "acceptedAt": accepted_at,
        "attempt": null, "artifactId": null,
        "failureReason": if state == "failed" { Some("interrupted") } else { None },
        "terminalAt": if terminal { Some(accepted_at + 1) } else { None },
        "retainUntil": if terminal { Some(accepted_at + 604_800) } else { None },
    })
}

#[tokio::test]
async fn export_list_preserves_newest_first_work_and_rejects_a_foreign_photo() {
    let newer = work_fixture("newer", 20, "accepted");
    let older = work_fixture("older", 10, "failed");
    let body = json!({"photoId": PHOTO_ID, "exports": [newer.clone(), older.clone()], "artifacts": [], "historicalExports": []});
    let service = fake_service(vec![Step::Capabilities, Step::Json(200, body)]);
    let (exit, report) = command(
        &service.url,
        &["photos", "processing-export-list", PHOTO_ID],
    )
    .await;
    assert_eq!(exit, 0, "{report}");
    assert_eq!(report["data"]["exports"], json!([newer, older]));
    assert_eq!(report["data"]["artifacts"], json!([]));
    let (_, requests) = service.finish();
    assert_eq!(
        requests[1].request_line,
        format!("GET /api/photos/{PHOTO_ID}/processing-exports HTTP/1.1")
    );

    let mut foreign = work_fixture("foreign", 30, "accepted");
    foreign["photoId"] = json!("another-photo");
    let service = fake_service(vec![
        Step::Capabilities,
        Step::Json(
            200,
            json!({"photoId": PHOTO_ID, "exports": [foreign], "artifacts": [], "historicalExports": []}),
        ),
    ]);
    let (exit, report) = command(
        &service.url,
        &["photos", "processing-export-list", PHOTO_ID],
    )
    .await;
    assert_eq!(exit, 6, "{report}");
    assert_eq!(report["error"]["code"], "transport_failed");
    service.finish();
}

#[tokio::test]
async fn explicit_retry_submits_only_the_new_identity_and_accepts_an_already_terminal_receipt() {
    let base = temp_base("explicit-retry");
    let input = write_input(&base, "retry.json", r#"{"requestId":"retry-1"}"#);
    let service = fake_service(vec![
        Step::Capabilities,
        Step::Json(
            202,
            json!({
                "outcome": "accepted", "receipt": work_fixture("retry-1", 20, "failed"),
            }),
        ),
    ]);
    let (exit, report) = command(
        &service.url,
        &[
            "photos",
            "processing-export-retry",
            PHOTO_ID,
            "original-1",
            "--input",
            &input,
        ],
    )
    .await;
    assert_eq!(exit, 0, "{report}");
    assert_eq!(report["data"]["requestId"], "retry-1");
    assert_eq!(report["data"]["state"], "failed");
    let (connections, requests) = service.finish();
    assert_eq!(connections, 2);
    assert_eq!(
        requests[1].request_line,
        format!("POST /api/photos/{PHOTO_ID}/processing-exports/original-1/retry HTTP/1.1")
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[1].body).unwrap(),
        json!({"requestId": "retry-1"})
    );

    let service = fake_service(vec![Step::Capabilities, Step::LoseAfterRead]);
    let (exit, report) = command(
        &service.url,
        &[
            "photos",
            "processing-export-retry",
            PHOTO_ID,
            "original-1",
            "--input",
            &input,
        ],
    )
    .await;
    assert_eq!(exit, 7, "{report}");
    assert_eq!(report["error"]["effect"], "unknown");
    let (connections, requests) = service.finish();
    assert_eq!(
        connections, 2,
        "a lost retry response never admits another attempt"
    );
    assert_eq!(requests.len(), 2);
    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn processing_export_refuses_unsupported_input_without_admitting_more_work() {
    let base = temp_base("unsupported-export");
    let input = write_input(
        &base,
        "export.json",
        &json!({
            "requestId": "export-1", "stepId": "develop-1",
            "expectedRecipeRevision": "recipe-9", "expectedSourceRevision": SOURCE_REVISION,
        })
        .to_string(),
    );
    let refusal = json!({"error": {"code": "incompatible_input", "message": "The selected module does not support this input contract.", "effect": "none", "details": {"reason": "artifact_contract_mismatch"}}});
    let service = fake_service(vec![Step::Capabilities, Step::Json(422, refusal)]);
    let (exit, report) = command(
        &service.url,
        &["photos", "processing-export", PHOTO_ID, "--input", &input],
    )
    .await;
    assert_eq!(exit, 2, "{report}");
    assert_eq!(report["error"]["code"], "incompatible_input");
    assert_eq!(report["error"]["effect"], "none");
    let (connections, requests) = service.finish();
    assert_eq!(connections, 2);
    assert_eq!(requests.len(), 2);
    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn export_list_enforces_the_work_record_bound() {
    for count in [64, 65] {
        let exports: Vec<Value> = (0..count)
            .map(|index| {
                work_fixture(
                    &format!("request-{index}"),
                    (count - index) as u64,
                    "accepted",
                )
            })
            .collect();
        let service = fake_service(vec![
            Step::Capabilities,
            Step::Json(
                200,
                json!({"photoId": PHOTO_ID, "exports": exports, "artifacts": [], "historicalExports": []}),
            ),
        ]);
        let (exit, report) = command(
            &service.url,
            &["photos", "processing-export-list", PHOTO_ID],
        )
        .await;
        if count == 64 {
            assert_eq!(exit, 0, "{report}");
            assert_eq!(report["data"]["exports"][0]["requestId"], "request-0");
            assert_eq!(report["data"]["exports"][63]["requestId"], "request-63");
        } else {
            assert_eq!(exit, 6, "{report}");
            assert_eq!(report["error"]["code"], "transport_failed");
        }
        service.finish();
    }
}

#[tokio::test]
async fn rebind_transmits_a_long_opaque_source_revision_without_a_parameter_tree() {
    let base = temp_base("opaque-rebind");
    let document = json!({"requestId": "rebind-1", "expectedRecipeRevision": "recipe-9", "newSourceRevision": format!("opaque\0\n{}", "é".repeat(300))});
    let input = write_input(&base, "rebind.json", &document.to_string());
    let service = fake_service(vec![Step::Capabilities, Step::LoseAfterRead]);
    let (exit, report) = command(
        &service.url,
        &[
            "photos",
            "processing-recipe",
            "rebind",
            PHOTO_ID,
            "--input",
            &input,
        ],
    )
    .await;
    assert_eq!(exit, 7, "{report}");
    assert_eq!(report["error"]["effect"], "unknown");
    let (_, requests) = service.finish();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[1].request_line,
        format!("POST /api/photos/{PHOTO_ID}/processing-recipe/rebind HTTP/1.1")
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[1].body).unwrap(),
        document
    );
    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn export_list_retains_artifact_filename_and_publication_lease() {
    let artifact = json!({
        "artifactId": "artifact-1", "photoId": PHOTO_ID, "stepId": "develop-1",
        "module": "darktable", "adapterSchemaVersion": "darktable-adapter-1:darktable-params-1",
        "parameters": {"schemaVersion": "darktable-params-1", "tree": {"stack": []}},
        "input": {"binding": {"kind": "original", "photoId": PHOTO_ID, "sourceRevision": SOURCE_REVISION}, "sha256": "a".repeat(64), "byteLength": 24},
        "outputContract": {"format": "image/tiff", "precision": "float32", "colorSpace": "prophoto-rgb", "transfer": "linear", "geometry": {"width": 8, "height": 4}, "encoding": "none"},
        "bundleId": "bundle-1", "filename": "artifact-1.tif", "publishedAt": "2026-10-02T12:00:00Z",
        "expiresAt": "2026-10-09T12:00:00Z", "sha256": "b".repeat(64), "byteLength": 96,
    });
    let service = fake_service(vec![
        Step::Capabilities,
        Step::Json(
            200,
            json!({"photoId": PHOTO_ID, "exports": [], "artifacts": [artifact.clone()], "historicalExports": []}),
        ),
    ]);
    let (exit, report) = command(
        &service.url,
        &["photos", "processing-export-list", PHOTO_ID],
    )
    .await;
    assert_eq!(exit, 0, "{report}");
    assert_eq!(report["data"]["artifacts"], json!([artifact]));
    service.finish();
}

#[tokio::test]
async fn initial_export_acceptance_and_replay_report_the_same_retained_identity() {
    let base = temp_base("export-admission");
    let input = write_input(
        &base,
        "export.json",
        &json!({
            "requestId": "export-1", "stepId": "develop-1",
            "expectedRecipeRevision": "recipe-9", "expectedSourceRevision": SOURCE_REVISION,
        })
        .to_string(),
    );
    for outcome in ["accepted", "replayed"] {
        let receipt = work_fixture("export-1", 20, "cancelled");
        let service = fake_service(vec![
            Step::Capabilities,
            Step::Json(202, json!({"outcome": outcome, "receipt": receipt.clone()})),
        ]);
        let (exit, report) = command(
            &service.url,
            &["photos", "processing-export", PHOTO_ID, "--input", &input],
        )
        .await;
        assert_eq!(exit, 0, "{report}");
        assert_eq!(report["data"]["outcome"], outcome);
        assert_eq!(report["data"]["receipt"], receipt);
        assert_eq!(report["data"]["state"], "cancelled");
        let (connections, requests) = service.finish();
        assert_eq!(connections, 2);
        assert_eq!(
            requests[1].request_line,
            format!("POST /api/photos/{PHOTO_ID}/processing-exports HTTP/1.1")
        );
    }
    fs::remove_dir_all(base).unwrap();
}

fn historical_export_fixture(export_id: &str, created_at: &str) -> Value {
    json!({
        "exportId": export_id, "photoId": PHOTO_ID, "state": "failed",
        "target": "film-jpeg", "recipeVersion": "legacy-recipe-1",
        "sourceRevision": format!("legacy\0{}", "s".repeat(256)),
        "createdAt": created_at, "settledAt": created_at, "bundleId": "legacy-bundle",
        "terminalOutcome": "failed", "failureReason": "interrupted",
        "receiptExpiresAt": "2030-01-01T00:00:00Z", "artifact": null,
    })
}

#[tokio::test]
async fn export_list_preserves_historical_metadata_separately_from_processing_artifacts() {
    let newer = historical_export_fixture("old-newer", "2026-10-02T12:00:00Z");
    let older = historical_export_fixture("old-older", "2026-10-01T12:00:00Z");
    let body = json!({"photoId": PHOTO_ID, "exports": [], "artifacts": [], "historicalExports": [newer.clone(), older.clone()]});
    let service = fake_service(vec![Step::Capabilities, Step::Json(200, body)]);
    let (exit, report) = command(
        &service.url,
        &["photos", "processing-export-list", PHOTO_ID],
    )
    .await;
    assert_eq!(exit, 0, "{report}");
    assert_eq!(report["data"]["historicalExports"], json!([newer, older]));
    assert_eq!(report["data"]["artifacts"], json!([]));
    service.finish();
}

#[tokio::test]
async fn export_list_requires_bounded_newest_first_historical_records() {
    for historical in [
        Value::Null,
        json!([
            historical_export_fixture("older", "2026-10-01T12:00:00Z"),
            historical_export_fixture("newer", "2026-10-02T12:00:00Z")
        ]),
        json!(
            (0..65)
                .map(|index| historical_export_fixture(
                    &format!("old-{index}"),
                    "2026-10-02T12:00:00Z"
                ))
                .collect::<Vec<_>>()
        ),
    ] {
        let service = fake_service(vec![
            Step::Capabilities,
            Step::Json(
                200,
                json!({"photoId": PHOTO_ID, "exports": [], "artifacts": [], "historicalExports": historical}),
            ),
        ]);
        let (exit, report) = command(
            &service.url,
            &["photos", "processing-export-list", PHOTO_ID],
        )
        .await;
        assert_eq!(exit, 6, "{report}");
        assert_eq!(report["error"]["code"], "transport_failed");
        service.finish();
    }
    let service = fake_service(vec![
        Step::Capabilities,
        Step::Json(
            200,
            json!({"photoId": PHOTO_ID, "exports": [], "artifacts": []}),
        ),
    ]);
    let (exit, report) = command(
        &service.url,
        &["photos", "processing-export-list", PHOTO_ID],
    )
    .await;
    assert_eq!(exit, 6, "{report}");
    service.finish();
}

fn historical_artifact_fixture(bytes: &[u8]) -> Value {
    json!({
        "exportId": "historical-1", "target": "film-jpeg", "stage": "film",
        "contentType": "image/jpeg", "filename": "historical-1.jpg",
        "orientation": "top-left", "sampleFormat": "uint8", "colorSpace": "sRGB",
        "iccEmbedded": true, "width": 8, "height": 4, "profileIdentity": "legacy-display-profile",
        "byteLength": bytes.len(), "sha256": sha256_hex(bytes), "expiresAt": "2030-01-01T00:00:00Z",
    })
}

#[tokio::test]
async fn historical_export_download_verifies_metadata_and_bytes_before_publication() {
    let bytes = jpeg_bytes();
    for mode in ["valid", "wrong-digest", "wrong-header"] {
        let base = temp_base(mode);
        let destination = base.join("historical.jpg");
        let mut artifact = historical_artifact_fixture(&bytes);
        if mode == "wrong-digest" {
            artifact["sha256"] = json!("a".repeat(64));
        }
        let mut record = historical_export_fixture("historical-1", "2026-10-02T12:00:00Z");
        record["state"] = json!("succeeded");
        record["terminalOutcome"] = json!("succeeded");
        record["failureReason"] = Value::Null;
        record["artifact"] = artifact.clone();
        let mut headers = artifact.clone();
        if mode == "wrong-header" {
            headers["exportId"] = json!("foreign-export");
        }
        let service = fake_service(vec![
            Step::Capabilities,
            Step::Json(200, record),
            Step::HistoricalBytes(headers, bytes.clone()),
        ]);
        let (exit, report) = command(
            &service.url,
            &[
                "photos",
                "historical-export-download",
                "historical-1",
                "--file",
                destination.to_str().unwrap(),
            ],
        )
        .await;
        if mode == "valid" {
            assert_eq!(exit, 0, "{report}");
            assert_eq!(report["data"]["historical"], true);
            assert_eq!(report["data"]["artifact"], artifact);
            assert_eq!(report["data"]["fileCommitted"], true);
            assert_eq!(fs::read(&destination).unwrap(), bytes);
            assert_eq!(fs::read_dir(&base).unwrap().count(), 1);
        } else {
            assert_eq!(exit, 6, "{report}");
            assert_eq!(report["error"]["code"], "transport_failed");
            assert!(!destination.exists());
            assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
        }
        let (_, requests) = service.finish();
        assert_eq!(requests.len(), 3);
        assert_eq!(
            requests[1].request_line,
            "GET /api/exports/historical-1 HTTP/1.1"
        );
        assert_eq!(
            requests[2].request_line,
            "GET /api/exports/historical-1/artifact HTTP/1.1"
        );
        fs::remove_dir_all(base).unwrap();
    }
}

#[tokio::test]
async fn historical_export_download_never_overwrites_an_existing_destination() {
    let base = temp_base("historical-existing");
    let destination = base.join("retained.jpg");
    fs::write(&destination, b"sentinel").unwrap();
    let service = fake_service(vec![Step::Capabilities]);
    let (exit, report) = command(
        &service.url,
        &[
            "photos",
            "historical-export-download",
            "historical-1",
            "--file",
            destination.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(exit, 2, "{report}");
    assert_eq!(report["error"]["code"], "invalid_input");
    assert_eq!(fs::read(&destination).unwrap(), b"sentinel");
    let (connections, requests) = service.finish();
    assert_eq!(connections, 0);
    assert!(requests.is_empty());
    fs::remove_dir_all(base).unwrap();
}
