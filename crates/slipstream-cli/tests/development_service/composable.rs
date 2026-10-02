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
