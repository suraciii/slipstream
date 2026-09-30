// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Export routes
// ---------------------------------------------------------------------------
use super::*;
use crate::config::ProcessingConfig;
use crate::export_manager::development_tiff_decode::{stored_zlib, write_development_tiff};
use crate::http::create_router_with_processing;
use sha2::{Digest, Sha256};
use slipstream_processing::photo::{
    self, OutputReceipt, PhotoReceipt, Request, Response as PhotoResponse, ResultBody,
};
use slipstream_processing::protocol::{Availability, ErrorCode, PHOTO_CAPABILITY, PHOTO_WORKLOAD};
use std::collections::HashMap;
use std::os::fd::AsRawFd;
use std::sync::{Arc, Mutex};

use std::time::Duration;

/// One attempt receipt the fake launcher owns, keyed by the launcher's
/// attempt identity.
#[derive(Clone)]
struct AttemptReceipt {
    state: &'static str,
    outcome: Option<String>,
}

/// Scripted behavior of one fake launcher. The transport is the real
/// production SOCK_SEQPACKET Photo protocol; Start verifies the canonical
/// manifest digest exactly as the production executor does, so the
/// service cannot pass with a divergent manifest or incarnation width.
struct LauncherScript {
    instance: String,
    policy: String,
    bundle: String,
    incarnation: String,
    next_sequence: u64,
    availability: Availability,
    attempts: HashMap<(String, u64), AttemptReceipt>,
    output: Option<Vec<u8>>,
    /// Refuse ValidateOutput like a lost acknowledgement.
    refuse_ack: bool,
    /// Refuse Output like a spent transfer claim.
    refuse_output: bool,
    /// Every served operation in arrival order.
    ops: Vec<String>,
}

impl LauncherScript {
    fn new() -> Self {
        Self {
            instance: String::new(),
            policy: String::new(),
            bundle: String::new(),
            incarnation: "ab".repeat(16),
            next_sequence: 1,
            availability: Availability::Available,
            attempts: HashMap::new(),
            output: None,
            refuse_ack: false,
            refuse_output: false,
            ops: Vec::new(),
        }
    }

    /// Reports a validated output that waits for the service's collection
    /// and acknowledgement, the phase the production launcher reports as
    /// `settling` with no outcome.
    fn ready_output(&mut self, sequence: u64) {
        let attempt = self
            .attempts
            .entry((self.incarnation.clone(), sequence))
            .or_insert(AttemptReceipt {
                state: "running",
                outcome: None,
            });
        attempt.state = "settling";
        attempt.outcome = None;
    }

    fn settle_attempt(&mut self, sequence: u64, outcome: &str) {
        let attempt = self
            .attempts
            .entry((self.incarnation.clone(), sequence))
            .or_insert(AttemptReceipt {
                state: "running",
                outcome: None,
            });
        attempt.state = "settled";
        attempt.outcome = Some(outcome.to_owned());
    }
}

struct FakeLauncher {
    socket: PathBuf,
    script: Arc<Mutex<LauncherScript>>,
}

impl FakeLauncher {
    /// Binds the launcher socket and serves the scripted behavior for the
    /// life of the test process.
    fn start(processing: &ProcessingConfig, mut script: LauncherScript) -> Self {
        script.instance = processing.instance.clone();
        script.policy = processing.policy_sha256.clone();
        script.bundle = processing.bundle_sha256.clone();
        let socket = std::env::temp_dir().join(format!(
            "slipstream-export-launcher-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let listener = photo::bind(&socket).expect("the fake launcher socket binds");
        let script = Arc::new(Mutex::new(script));
        let served = Arc::clone(&script);
        std::thread::spawn(move || {
            loop {
                let Ok(connection) = photo::accept(listener.as_raw_fd()) else {
                    break;
                };
                let _ = serve_launcher_connection(connection, &served);
            }
        });
        Self { socket, script }
    }

    fn with_script<R>(&self, apply: impl FnOnce(&mut LauncherScript) -> R) -> R {
        with_script(&self.script, apply)
    }

    /// The processing configuration the service must use to dial this
    /// launcher.
    fn processing_config(&self) -> ProcessingConfig {
        with_script(&self.script, |script| ProcessingConfig {
            instance: script.instance.clone(),
            policy_sha256: script.policy.clone(),
            bundle_sha256: script.bundle.clone(),
            socket_override: Some(self.socket.clone()),
        })
    }
}

/// Applies one mutation or read to the launcher script; the single lock
/// accessor keeps every call site free of lock-error handling.
fn with_script<R>(
    script: &Arc<std::sync::Mutex<LauncherScript>>,
    apply: impl FnOnce(&mut LauncherScript) -> R,
) -> R {
    apply(&mut script.lock().unwrap_or_else(|error| error.into_inner()))
}

fn serve_launcher_connection(
    connection: std::os::fd::OwnedFd,
    script: &Arc<std::sync::Mutex<LauncherScript>>,
) -> std::io::Result<()> {
    let (request, descriptor) = photo::receive_request(connection.as_raw_fd())?;
    // A scripted lost acknowledgement closes the connection without a
    // response, exactly like a launcher dying mid-acknowledgement.
    if matches!(request, Request::ValidateOutput { .. }) && with_script(script, |s| s.refuse_ack) {
        return Ok(());
    }
    let response = with_script(script, |script| {
        launcher_answer(&request, descriptor, script)
    });
    photo::send_response(connection.as_raw_fd(), &response)
}

fn launcher_receipt(
    script: &LauncherScript,
    export_id: &str,
    incarnation: &str,
    sequence: u64,
) -> PhotoReceipt {
    let attempt = script
        .attempts
        .get(&(incarnation.to_owned(), sequence))
        .cloned()
        .unwrap_or(AttemptReceipt {
            state: "running",
            outcome: None,
        });
    PhotoReceipt {
        export_id: export_id.to_owned(),
        incarnation: incarnation.to_owned(),
        sequence,
        workload: PHOTO_WORKLOAD.to_owned(),
        policy: script.policy.clone(),
        bundle: script.bundle.clone(),
        state: attempt.state.to_owned(),
        outcome: attempt.outcome,
    }
}

fn launcher_answer(
    request: &Request,
    descriptor: Option<std::os::fd::OwnedFd>,
    script: &mut LauncherScript,
) -> PhotoResponse {
    script.ops.push(
        match request {
            Request::Reconcile { .. } => "reconcile",
            Request::Start { .. } => "start",
            Request::Inspect { .. } => "inspect",
            Request::Cancel { .. } => "cancel",
            Request::Output { .. } => "output",
            Request::ValidateOutput { .. } => "validate",
        }
        .to_owned(),
    );
    match request {
        Request::Reconcile { instance, .. } => {
            if instance != &script.instance {
                return PhotoResponse::error(ErrorCode::WrongInstance);
            }
            PhotoResponse::result(ResultBody::Capability {
                capability: PHOTO_CAPABILITY.to_owned(),
                instance: script.instance.clone(),
                incarnation: script.incarnation.clone(),
                next_sequence: script.next_sequence,
                policy: script.policy.clone(),
                bundle: script.bundle.clone(),
                availability: script.availability,
                active: None,
            })
        }
        Request::Start {
            export_id,
            incarnation,
            sequence,
            policy,
            bundle,
            source,
            recipe,
            manifest_sha256,
            ..
        } => {
            // The production executor recomputes the canonical manifest
            // digest over every execution-relevant field, the qualified
            // source profile included, and refuses any mismatch.
            let manifest = serde_json::to_vec(&serde_json::json!({
                "bundle": bundle,
                "policy": policy,
                "recipe": [recipe.exposure_milli_ev, recipe.white_balance_mode],
                "source": {
                    "kind": source.kind,
                    "profile_id": source.profile_id,
                    "sha256": source.sha256,
                    "size": source.size,
                },
                "target": PHOTO_WORKLOAD,
                "workload": PHOTO_WORKLOAD,
            }))
            .expect("manifest serializes");
            if format!("{:x}", Sha256::digest(&manifest)) != *manifest_sha256 {
                return PhotoResponse::error(ErrorCode::InvalidRequest);
            }
            if policy != &script.policy {
                return PhotoResponse::error(ErrorCode::IncompatiblePolicy);
            }
            if bundle != &script.bundle {
                return PhotoResponse::error(ErrorCode::IncompatibleBundle);
            }
            // The receipt is created only if no scripted result already
            // exists, so a test-settled attempt keeps its outcome.
            script
                .attempts
                .entry((incarnation.clone(), *sequence))
                .or_insert(AttemptReceipt {
                    state: "running",
                    outcome: None,
                });
            script.next_sequence = sequence + 1;
            PhotoResponse::result(ResultBody::Receipt {
                receipt: launcher_receipt(script, export_id, incarnation, *sequence),
            })
        }
        Request::Inspect {
            export_id,
            incarnation,
            sequence,
            ..
        } => {
            let receipt = launcher_receipt(script, export_id, incarnation, *sequence);
            PhotoResponse::result(ResultBody::Receipt { receipt })
        }
        Request::ValidateOutput {
            export_id,
            incarnation,
            sequence,
            accepted,
            ..
        } => {
            // The production launcher settles the attempt from the
            // service's acknowledgement, so the scripted attempt does too.
            let key = (incarnation.clone(), *sequence);
            if let Some(attempt) = script.attempts.get_mut(&key)
                && attempt.state == "settling"
            {
                attempt.state = "settled";
                attempt.outcome = Some(if *accepted {
                    "completed".to_owned()
                } else {
                    "refused-output-validation".to_owned()
                });
            }
            let receipt = launcher_receipt(script, export_id, incarnation, *sequence);
            PhotoResponse::result(ResultBody::Receipt { receipt })
        }
        Request::Cancel {
            export_id,
            incarnation,
            sequence,
            ..
        } => {
            let key = (incarnation.clone(), *sequence);
            match script.attempts.get_mut(&key) {
                Some(attempt) if attempt.state != "settled" => {
                    attempt.state = "settled";
                    attempt.outcome = Some("cancelled".to_owned());
                }
                // An unknown attempt is refused like the production
                // journal; a settled attempt reports its terminal result.
                None => return PhotoResponse::error(ErrorCode::UnknownAttempt),
                _ => {}
            }
            PhotoResponse::result(ResultBody::Receipt {
                receipt: launcher_receipt(script, export_id, incarnation, *sequence),
            })
        }
        Request::Output {
            export_id,
            incarnation,
            sequence,
            ..
        } => {
            if script.refuse_output {
                return PhotoResponse::error(ErrorCode::Conflict);
            }
            let Some(output) = script.output.clone() else {
                return PhotoResponse::error(ErrorCode::Unavailable);
            };
            let Some(descriptor) = descriptor else {
                return PhotoResponse::error(ErrorCode::InvalidRequest);
            };
            use std::io::Write as _;
            let mut file = std::fs::File::from(descriptor);
            file.write_all(&output).expect("output bytes write");
            PhotoResponse::result(ResultBody::Output {
                receipt: OutputReceipt {
                    export_id: export_id.clone(),
                    incarnation: incarnation.clone(),
                    sequence: *sequence,
                    target: PHOTO_WORKLOAD.to_owned(),
                    size: output.len() as u64,
                    sha256: format!("{:x}", Sha256::digest(&output)),
                },
            })
        }
    }
}

fn tiff_bytes(tags: &[(u16, u16, Vec<u8>)]) -> Vec<u8> {
    let data_start = 8 + 2 + tags.len() * 12 + 4;
    let mut bytes = b"II*\0\x08\0\0\0".to_vec();
    bytes.extend_from_slice(&(tags.len() as u16).to_le_bytes());
    let mut data = Vec::new();
    for (tag, field_type, value) in tags {
        bytes.extend_from_slice(&tag.to_le_bytes());
        bytes.extend_from_slice(&field_type.to_le_bytes());
        let count = if *field_type == 3 || *field_type == 5 {
            1
        } else {
            value.len() as u32
        };
        bytes.extend_from_slice(&count.to_le_bytes());
        if value.len() <= 4 {
            let mut inline = [0_u8; 4];
            inline[..value.len()].copy_from_slice(value);
            bytes.extend_from_slice(&inline);
        } else {
            bytes.extend_from_slice(&((data_start + data.len()) as u32).to_le_bytes());
            data.extend_from_slice(value);
        }
    }
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&data);
    bytes
}

/// Writes one RAW fixture whose TIFF header carries the camera identity
/// so the capture inspection classifies it against the approved profile.
fn raw_fixture_with_camera(path: &std::path::Path, make: &[u8], model: &[u8]) {
    let bytes = tiff_bytes(&[(0x010f, 2, make.to_vec()), (0x0110, 2, model.to_vec())]);
    fs::write(path, bytes).unwrap();
}

fn unique_instance() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .subsec_nanos();
    format!("{nanos:08x}{:024x}", std::process::id())
}

fn export_processing() -> crate::config::ProcessingConfig {
    crate::config::ProcessingConfig {
        instance: unique_instance(),
        policy_sha256: "b".repeat(64),
        bundle_sha256: "c".repeat(64),
        socket_override: None,
    }
}

/// Builds the shared Export fixture: a RAW Photo carrying an approved
/// camera identity, a JPEG-only Photo, and a configured processing
/// deployment with the given retained-output allowance.
fn export_fixture(allowance: Option<u64>) -> (PathBuf, Config) {
    let (base, mut config) = prepare_populated_fixture();
    raw_fixture_with_camera(
        &config.library_root.join("pair.ARW"),
        b"SONY\0",
        b"ILCE-7RM5\0\0\0",
    );
    config.processing = Some(export_processing());
    config.export_retained_output_bytes = allowance;
    (base, config)
}

async fn export_application(_base: &Path, config: &Config) -> (Arc<Application>, Router) {
    let application = Application::open(config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = create_router_with_processing(
        Arc::clone(&application),
        open_web_root(config.web_root()),
        config.processing.clone(),
    );
    application.access.seed_test_token();
    (application, router)
}

fn photo_id_for(config: &Config, relative_path: &str) -> String {
    let connection =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    connection
        .query_row(
            "SELECT p.id FROM photos p JOIN original_files o ON p.original_id = o.id
                 WHERE o.relative_path = ?1",
            [relative_path],
            |row| row.get::<_, String>(0),
        )
        .unwrap()
}

async fn post_export_body(router: &Router, uri: String, body: serde_json::Value) -> Response<Body> {
    send(
        router,
        authenticated_request()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

async fn submit_export_body(
    router: &Router,
    photo_id: &str,
    body: serde_json::Value,
) -> Response<Body> {
    post_export_body(
        router,
        format!("https://camera.local/api/photos/{photo_id}/exports"),
        body,
    )
    .await
}

async fn submit_export_request(
    router: &Router,
    photo_id: &str,
    request_id: &str,
    recipe_version: &str,
    source_revision: &str,
) -> Response<Body> {
    submit_export_body(
        router,
        photo_id,
        serde_json::json!({
            "requestId": request_id,
            "expectedRecipeVersion": recipe_version,
            "expectedSourceRevision": source_revision,
            "target": "development-tiff",
        }),
    )
    .await
}

async fn get_export(router: &Router, export_id: &str) -> serde_json::Value {
    response_json(
        send(
            router,
            authenticated_request()
                .uri(format!("https://camera.local/api/exports/{export_id}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await
}

async fn get_export_response(router: &Router, export_id: &str) -> Response<Body> {
    send(
        router,
        authenticated_request()
            .uri(format!("https://camera.local/api/exports/{export_id}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

async fn download_artifact(router: &Router, export_id: &str) -> Response<Body> {
    send(
        router,
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/exports/{export_id}/artifact"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

async fn current_source_revision(application: &Application, photo_id: &str) -> String {
    application
        .library
        .edit_recipe(photo_id)
        .await
        .unwrap()
        .expect("the Photo must exist")
        .current_source_revision
        .expect("settled Photo must have a published source revision")
}

async fn save_recipe(
    application: &Application,
    photo_id: &str,
    request_id: &str,
    expected_recipe_version: Option<String>,
    exposure_ev: f64,
) -> slipstream_core::EditRecipe {
    let expected_source_revision = current_source_revision(application, photo_id).await;
    let outcome = application
        .library
        .save_edit_recipe(slipstream_core::SaveEditRecipe {
            photo_id: photo_id.to_owned(),
            request_id: request_id.to_owned(),
            expected_recipe_version,
            expected_source_revision,
            settings: slipstream_core::EditRecipeSettings {
                exposure_ev,
                white_balance: slipstream_core::WhiteBalanceIntent::AsShot,
            },
        })
        .await
        .unwrap();
    match outcome {
        slipstream_core::EditRecipeWriteOutcome::Saved(recipe) => recipe,
        slipstream_core::EditRecipeWriteOutcome::Unchanged(recipe) => recipe,
        other => panic!("the recipe save must succeed, got {other:?}"),
    }
}

async fn wait_for_state(router: &Router, export_id: &str, state: &str) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let record = get_export(router, export_id).await;
        if record["state"] == state {
            return record;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the Export must reach {state}: {record}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn retry_export(router: &Router, export_id: &str, request_id: &str) -> Response<Body> {
    post_export_body(
        router,
        format!("https://camera.local/api/exports/{export_id}/retry"),
        serde_json::json!({ "requestId": request_id }),
    )
    .await
}

async fn cancel_export(router: &Router, export_id: &str) -> Response<Body> {
    send(
        router,
        authenticated_request()
            .method("POST")
            .uri(format!(
                "https://camera.local/api/exports/{export_id}/cancel"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

fn error_code(body: &serde_json::Value) -> &serde_json::Value {
    &body["error"]["code"]
}

/// One valid Development TIFF the fake launcher hands over as its output.
fn valid_development_tiff() -> Vec<u8> {
    let path = std::env::temp_dir().join(format!(
        "export-artifact-fixture-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    write_development_tiff(&path, &stored_zlib(&[0_u8; 2 * 3 * 4]));
    let bytes = fs::read(&path).unwrap();
    let _ = fs::remove_file(&path);
    bytes
}

#[tokio::test]
async fn export_submit_shape_is_refused_before_any_state_change() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;

    // A target outside the closed set is refused before acceptance.
    let wrong_target = submit_export_body(
        &router,
        &photo_id,
        serde_json::json!({
            "requestId": "request-target",
            "expectedRecipeVersion": recipe.revision,
            "expectedSourceRevision": source_revision,
            "target": "finished-jpeg",
        }),
    )
    .await;
    assert_eq!(wrong_target.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        error_code(&response_json(wrong_target).await),
        "invalid_settings"
    );

    // The refused submission created nothing: the same identity is still
    // fresh and is accepted as new work.
    let accepted = submit_export_request(
        &router,
        &photo_id,
        "request-target",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(accepted.status(), StatusCode::CREATED);

    // Unknown fields, missing fields, wrong types, and malformed request
    // identities are all refused with the one closed shape code.
    let unknown_field = submit_export_body(
        &router,
        &photo_id,
        serde_json::json!({
            "requestId": "request-unknown-field",
            "expectedRecipeVersion": recipe.revision,
            "expectedSourceRevision": source_revision,
            "target": "development-tiff",
            "extra": true,
        }),
    )
    .await;
    assert_eq!(unknown_field.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let missing_field = submit_export_body(
        &router,
        &photo_id,
        serde_json::json!({
            "requestId": "request-missing-field",
            "expectedSourceRevision": source_revision,
            "target": "development-tiff",
        }),
    )
    .await;
    assert_eq!(missing_field.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let wrong_type = submit_export_body(
        &router,
        &photo_id,
        serde_json::json!({
            "requestId": "request-wrong-type",
            "expectedRecipeVersion": 7,
            "expectedSourceRevision": source_revision,
            "target": "development-tiff",
        }),
    )
    .await;
    assert_eq!(wrong_type.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        error_code(&response_json(wrong_type).await),
        "invalid_settings"
    );

    for bad_identity in ["", "bad identity", &"x".repeat(129)] {
        let refused = submit_export_body(
            &router,
            &photo_id,
            serde_json::json!({
                "requestId": bad_identity,
                "expectedRecipeVersion": recipe.revision,
                "expectedSourceRevision": source_revision,
                "target": "development-tiff",
            }),
        )
        .await;
        assert_eq!(refused.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            error_code(&response_json(refused).await),
            "invalid_settings"
        );
    }

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn export_submit_accepts_replays_and_keeps_identity_after_later_writes() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let first = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;

    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &first.revision,
        &source_revision,
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = response_json(created).await;
    assert!(created["exportId"].as_str().unwrap().starts_with("exp-"));
    assert_eq!(created["state"], "queued");
    assert_eq!(created["target"], "development-tiff");
    assert_eq!(created["recipeVersion"], first.revision);
    assert_eq!(created["sourceRevision"], source_revision);
    // Retention runs through the active operation, so both expiries are
    // null while the Export is queued or running.
    assert!(created["receiptExpiresAt"].is_null());
    assert!(created["artifactExpiresAt"].is_null());

    // Repeating the identity and payload returns the same body with 200.
    let replayed = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &first.revision,
        &source_revision,
    )
    .await;
    assert_eq!(replayed.status(), StatusCode::OK);
    let replayed = response_json(replayed).await;
    assert_eq!(replayed["exportId"], created["exportId"]);
    assert_eq!(replayed["recipeVersion"], first.revision);

    // A later recipe write must not be overwritten by replaying the older
    // request identity: the Export keeps its original snapshot.
    let second = save_recipe(
        &application,
        &photo_id,
        "save-2",
        Some(first.revision.clone()),
        0.75,
    )
    .await;
    assert_ne!(second.revision, first.revision);
    let stale_replay = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &first.revision,
        &source_revision,
    )
    .await;
    assert_eq!(stale_replay.status(), StatusCode::OK);
    let stale_record = response_json(stale_replay).await;
    assert_eq!(stale_record["exportId"], created["exportId"]);
    assert_eq!(stale_record["recipeVersion"], first.revision);

    // A different payload under the accepted identity conflicts.
    let conflict = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &second.revision,
        &source_revision,
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(
        error_code(&response_json(conflict).await),
        "export_conflict"
    );

    // A new request identity against the current recipe starts new work.
    let fresh = submit_export_request(
        &router,
        &photo_id,
        "request-2",
        &second.revision,
        &source_revision,
    )
    .await;
    assert_eq!(fresh.status(), StatusCode::CREATED);
    let fresh_record = response_json(fresh).await;
    assert_ne!(fresh_record["exportId"], created["exportId"]);
    assert_eq!(fresh_record["recipeVersion"], second.revision);

    let listed = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{photo_id}/exports"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    let entries = listed["exports"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    // Retention order is newest first, so the Export just submitted heads
    // the list even when both share a whole-second creation time.
    assert_eq!(entries[0]["exportId"], fresh_record["exportId"]);
    assert_eq!(entries[1]["exportId"], created["exportId"]);
    for entry in entries {
        assert!(entry["exportId"].is_string());
        assert!(entry["state"].is_string());
        assert_eq!(entry["target"], "development-tiff");
    }

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn export_submit_reports_every_owner_refusal_code() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let jpeg_id = photo_id_for(&config, "pair.JPG");

    let unknown =
        submit_export_request(&router, "missing-photo", "request-unknown", "rev", "source").await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);
    assert_eq!(error_code(&response_json(unknown).await), "unknown_photo");

    // JPEG-only Photos cannot take the development-tiff workload.
    let jpeg = submit_export_request(&router, &jpeg_id, "request-jpeg", "rev", "source").await;
    assert_eq!(jpeg.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(error_code(&response_json(jpeg).await), "unsupported_photo");

    // A RAW Photo without a saved recipe cannot start an Export.
    let missing =
        submit_export_request(&router, &photo_id, "request-norecipe", "rev", "source").await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(error_code(&response_json(missing).await), "missing_recipe");

    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;

    // A stale recipe version is a conflict.
    let stale_recipe = submit_export_request(
        &router,
        &photo_id,
        "request-stale-recipe",
        "no-such-revision",
        &source_revision,
    )
    .await;
    assert_eq!(stale_recipe.status(), StatusCode::CONFLICT);
    assert_eq!(
        error_code(&response_json(stale_recipe).await),
        "recipe_conflict"
    );

    // A stale source revision is a conflict.
    let stale_source = submit_export_request(
        &router,
        &photo_id,
        "request-stale-source",
        &recipe.revision,
        "stale-source-revision",
    )
    .await;
    assert_eq!(stale_source.status(), StatusCode::CONFLICT);
    assert_eq!(
        error_code(&response_json(stale_source).await),
        "source_changed"
    );

    // Settings outside the approved range are invalid input.
    let outside = save_recipe(
        &application,
        &photo_id,
        "save-2",
        Some(recipe.revision.clone()),
        2.0,
    )
    .await;
    let invalid = submit_export_request(
        &router,
        &photo_id,
        "request-invalid",
        &outside.revision,
        &source_revision,
    )
    .await;
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        error_code(&response_json(invalid).await),
        "invalid_settings"
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn export_submit_reports_requires_rebind_for_a_stale_recipe_binding() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;

    // A new scan publishes the changed source facts and keeps the saved
    // recipe bound to the prior revision until explicit rebind.
    let original = config.library_root.join("pair.ARW");
    let mut bytes = fs::read(&original).unwrap();
    bytes.push(0);
    fs::write(&original, bytes).unwrap();
    application.rescan().await.unwrap();
    let new_source_revision = current_source_revision(&application, &photo_id).await;
    assert_ne!(new_source_revision, recipe.source_revision);

    // The expected revision is current but the stored recipe is bound to
    // the old source: only an explicit rebind may adopt it.
    let refused = submit_export_request(
        &router,
        &photo_id,
        "request-rebind",
        &recipe.revision,
        &new_source_revision,
    )
    .await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    assert_eq!(error_code(&response_json(refused).await), "requires_rebind");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn export_submit_reports_retained_output_capacity_before_acceptance() {
    let (base, config) = export_fixture(Some(slipstream_core::MAXIMUM_EXPORT_BYTES));
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;

    let first = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(first.status(), StatusCode::CREATED);

    // The in-flight Export reserves the worst-case artifact size, so the
    // allowance cannot admit a second one before acceptance.
    let second = submit_export_request(
        &router,
        &photo_id,
        "request-2",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        error_code(&response_json(second).await),
        "retained_output_full"
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A fresh Film identity is refused before any state change even while the
/// launcher is healthy and admits the Development target: the launcher
/// qualifies only Development, so a full-resolution Film Export records no
/// Export and no receipt.
#[tokio::test]
async fn unqualified_film_export_refuses_without_a_receipt() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;

    // The same healthy launcher admits the Development target, so the Film
    // refusal below is the qualification gate, not launcher availability.
    let developed = submit_export_request(
        &router,
        &photo_id,
        "develop-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(developed.status(), StatusCode::CREATED);

    let request = serde_json::json!({
        "requestId": "unqualified-film",
        "expectedRecipeVersion": recipe.revision,
        "expectedSourceRevision": source_revision,
        "target": "film-jpeg",
    });
    let refused = submit_export_body(&router, &photo_id, request.clone()).await;
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        error_code(&response_json(refused).await),
        "processing_unavailable"
    );
    // The refusal recorded no Export: only the Development admission lists.
    let listed = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{photo_id}/exports"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    let exports = listed["exports"].as_array().unwrap();
    assert_eq!(exports.len(), 1);
    assert_eq!(exports[0]["target"], "development-tiff");
    // And no receipt: the same identity is refused again, not replayed.
    let replay = submit_export_body(&router, &photo_id, request).await;
    assert_eq!(replay.status(), StatusCode::SERVICE_UNAVAILABLE);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A blocked or identity-mismatched launcher refuses the submission
/// before acceptance, so no Export, receipt, or capacity reservation is
/// consumed and the identity stays fresh.
#[tokio::test]
async fn export_submission_refuses_before_acceptance_when_admission_fails() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let processing = config.processing.clone().unwrap();
    let mut script = LauncherScript::new();
    script.availability = Availability::Blocked;
    let launcher = FakeLauncher::start(&processing, script);
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;

    let blocked = submit_export_request(
        &router,
        &photo_id,
        "request-blocked",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(blocked.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        error_code(&response_json(blocked).await),
        "processing_unavailable"
    );

    // Nothing was accepted: the listing is empty and the identity is
    // still fresh once the launcher admits work again.
    let listed = response_json(
        send(
            &router,
            authenticated_request()
                .uri(format!(
                    "https://camera.local/api/photos/{photo_id}/exports"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(listed["exports"].as_array().unwrap().len(), 0);
    launcher.with_script(|s| s.availability = Availability::Available);
    let accepted = submit_export_request(
        &router,
        &photo_id,
        "request-blocked",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(accepted.status(), StatusCode::CREATED);

    // A foreign qualified bundle is never treated as an available slot.
    launcher.with_script(|s| s.bundle = "d".repeat(64));
    let second = submit_export_request(
        &router,
        &photo_id,
        "request-foreign",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        error_code(&response_json(second).await),
        "processing_unavailable"
    );

    // A malformed launcher incarnation is refused by the persistence
    // boundary and by admission alike.
    launcher.with_script(|s| {
        s.bundle = "c".repeat(64);
        s.incarnation = "A".repeat(32);
    });
    let invalid_incarnation = submit_export_request(
        &router,
        &photo_id,
        "request-incarnation",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(
        invalid_incarnation.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn export_routes_report_processing_unavailable_without_configuration() {
    // Neither processing nor an allowance is configured.
    let (base, config) = prepare_populated_fixture();
    let (application, router) = export_application(&base, &config).await;
    let body = serde_json::json!({
        "requestId": "r",
        "expectedRecipeVersion": "rev",
        "expectedSourceRevision": "src",
        "target": "development-tiff"
    });
    let submitted = submit_export_body(&router, "whatever", body).await;
    assert_eq!(submitted.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        error_code(&response_json(submitted).await),
        "processing_unavailable"
    );
    let read = send(
        &router,
        authenticated_request()
            .uri("https://camera.local/api/exports/missing")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(read.status(), StatusCode::NOT_FOUND);
    assert_eq!(error_code(&response_json(read).await), "unknown_export");
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);

    // Processing configured without an allowance refuses the same way.
    let (base, mut config) = prepare_populated_fixture();
    raw_fixture_with_camera(
        &config.library_root.join("pair.ARW"),
        b"SONY\0",
        b"ILCE-7RM5\0\0\0",
    );
    config.processing = Some(export_processing());
    let (application, router) = export_application(&base, &config).await;
    let body = serde_json::json!({
        "requestId": "r",
        "expectedRecipeVersion": "rev",
        "expectedSourceRevision": "src",
        "target": "development-tiff"
    });
    let submitted = submit_export_body(&router, "whatever", body).await;
    assert_eq!(submitted.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        error_code(&response_json(submitted).await),
        "processing_unavailable"
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn export_unknown_identity_is_reported_on_every_route() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let (_application, router) = export_application(&base, &config).await;

    // A missing record maps to unknown_export on every route.
    let response = get_export_response(&router, "no-such-export").await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(error_code(&response_json(response).await), "unknown_export");

    let retried = retry_export(&router, "no-such-export", "retry-unknown").await;
    assert_eq!(retried.status(), StatusCode::NOT_FOUND);
    assert_eq!(error_code(&response_json(retried).await), "unknown_export");

    let cancelled = cancel_export(&router, "no-such-export").await;
    assert_eq!(cancelled.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        error_code(&response_json(cancelled).await),
        "unknown_export"
    );

    let artifact = download_artifact(&router, "no-such-export").await;
    assert_eq!(artifact.status(), StatusCode::NOT_FOUND);
    assert_eq!(error_code(&response_json(artifact).await), "unknown_export");

    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn export_cancel_settles_once_and_retry_rearms_within_retention() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = response_json(created).await;
    let export_id = created["exportId"].as_str().unwrap().to_owned();

    // The launcher-owned incarnation reaches the persistence boundary and
    // the attempt really runs against the production protocol.
    let running = wait_for_state(&router, &export_id, "running").await;
    assert_eq!(running["terminalOutcome"], serde_json::Value::Null);

    // Cancellation cancels the live launcher attempt and settles exactly
    // once against the actual completion state.
    let cancelled = cancel_export(&router, &export_id).await;
    assert_eq!(cancelled.status(), StatusCode::OK);
    let cancelled_record = response_json(cancelled).await;
    assert_eq!(cancelled_record["exportId"], created["exportId"]);
    assert_eq!(cancelled_record["state"], "cancelled");
    assert_eq!(cancelled_record["terminalOutcome"], "cancelled");
    assert_eq!(cancelled_record.as_object().unwrap().len(), 3);

    // A repeated cancellation returns the unchanged terminal record.
    let repeated = cancel_export(&router, &export_id).await;
    assert_eq!(repeated.status(), StatusCode::OK);
    assert_eq!(response_json(repeated).await, cancelled_record);

    // Replaying the original submission returns the terminal snapshot
    // with the disclosed receipt expiry.
    let replayed = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(replayed.status(), StatusCode::OK);
    let replayed = response_json(replayed).await;
    assert_eq!(replayed["state"], "cancelled");
    assert!(replayed["receiptExpiresAt"].is_string());
    assert!(replayed["artifactExpiresAt"].is_null());

    // Retry re-arms the retained snapshot under the caller's new request
    // identity and returns the admitted attempt.
    let retried = retry_export(&router, &export_id, "retry-1").await;
    assert_eq!(retried.status(), StatusCode::ACCEPTED);
    let retried_record = response_json(retried).await;
    assert_eq!(retried_record["exportId"], created["exportId"]);
    assert_eq!(retried_record["state"], "queued", "{retried_record}");
    assert_eq!(retried_record.as_object().unwrap().len(), 2);

    // Replaying the retry identity resolves to the Export without
    // starting another attempt.
    let replayed_retry = retry_export(&router, &export_id, "retry-1").await;
    assert_eq!(replayed_retry.status(), StatusCode::ACCEPTED);
    assert_eq!(response_json(replayed_retry).await["state"], "queued");

    // Without a completable launcher result the re-armed attempt fails
    // with an actionable reason and stays retriable.
    launcher.with_script(|s| s.settle_attempt(2, "engine-failed"));
    let failed = wait_for_state(&router, &export_id, "failed").await;
    let failure_reason = failed["failureReason"].as_str().unwrap().to_owned();
    assert!(failure_reason.contains("engine-failed"), "{failure_reason}");
    assert_eq!(failed["terminalOutcome"], "failed");
    assert!(failed["receiptExpiresAt"].is_string());

    let retry_failed = retry_export(&router, &export_id, "retry-2").await;
    assert_eq!(retry_failed.status(), StatusCode::ACCEPTED);
    assert_eq!(response_json(retry_failed).await["state"], "queued");

    // A retry of a queued Export is refused as a conflict.
    let conflict = retry_export(&router, &export_id, "retry-3").await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(
        error_code(&response_json(conflict).await),
        "export_conflict"
    );

    // A retry identity consumed by another Export conflicts.
    let second = submit_export_request(
        &router,
        &photo_id,
        "request-2",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(second.status(), StatusCode::CREATED);
    let second_id = response_json(second).await["exportId"]
        .as_str()
        .unwrap()
        .to_owned();
    // The second Export cannot start while the first attempt owns the
    // launcher slot, so cancel it before retrying with the shared
    // identity.
    assert_eq!(
        cancel_export(&router, &second_id).await.status(),
        StatusCode::OK
    );
    let shared = retry_export(&router, &second_id, "retry-2").await;
    assert_eq!(shared.status(), StatusCode::CONFLICT);
    assert_eq!(error_code(&response_json(shared).await), "request_conflict");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn export_cancellation_reconciles_a_completion_that_raced_it() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    let export_id = response_json(created).await["exportId"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_for_state(&router, &export_id, "running").await;

    // The launcher completed the attempt before the cancellation landed:
    // cancellation must settle to the actual terminal result and the
    // validated artifact is published, never undone.
    launcher.with_script(|s| {
        s.output = Some(valid_development_tiff());
        s.settle_attempt(1, "completed");
    });
    let cancelled = cancel_export(&router, &export_id).await;
    assert_eq!(cancelled.status(), StatusCode::OK);
    let settled = response_json(cancelled).await;
    assert_eq!(settled["state"], "succeeded");
    assert_eq!(settled["terminalOutcome"], "succeeded");

    let inspected = get_export(&router, &export_id).await;
    assert_eq!(inspected["state"], "succeeded");
    assert_eq!(inspected["artifact"]["stage"], "develop");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn export_expiry_reports_expired_identities_and_reclaims_records() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    let export_id = response_json(created).await["exportId"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        cancel_export(&router, &export_id).await.status(),
        StatusCode::OK
    );

    // A passed retention window refuses the retry with the explicit
    // expired outcome even before the sweep reclaims the record.
    let connection =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    connection
        .execute("UPDATE exports SET retain_until = 1", [])
        .unwrap();
    let expired_retry = retry_export(&router, &export_id, "retry-expired").await;
    assert_eq!(expired_retry.status(), StatusCode::GONE);
    assert_eq!(
        error_code(&response_json(expired_retry).await),
        "export_expired"
    );

    // Force the retention window to pass and run the sweep.
    drop(connection);
    let manager = Arc::clone(application.exports.as_ref().unwrap());
    manager.sweep_expiry().await;

    // The record is gone and the identity refuses new work as expired.
    let read = get_export_response(&router, &export_id).await;
    assert_eq!(read.status(), StatusCode::NOT_FOUND);
    let replayed = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(replayed.status(), StatusCode::GONE);
    assert_eq!(error_code(&response_json(replayed).await), "export_expired");
    // The swept identity still reports its expiry on retry.
    let retried = retry_export(&router, &export_id, "retry-swept").await;
    assert_eq!(retried.status(), StatusCode::GONE);
    assert_eq!(error_code(&response_json(retried).await), "export_expired");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn export_download_headers_match_the_inspect_artifact_object() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    let export_id = response_json(created).await["exportId"]
        .as_str()
        .unwrap()
        .to_owned();

    // While the attempt is live the download conflicts.
    wait_for_state(&router, &export_id, "running").await;
    let live = download_artifact(&router, &export_id).await;
    assert_eq!(live.status(), StatusCode::CONFLICT);
    assert_eq!(error_code(&response_json(live).await), "export_conflict");

    // The launcher completes with a genuinely valid Development TIFF; the
    // service validates, publishes, and settles exactly once.
    let artifact_bytes = valid_development_tiff();
    launcher.with_script(|s| {
        s.output = Some(artifact_bytes.clone());
        s.settle_attempt(1, "completed");
    });
    let settled = wait_for_state(&router, &export_id, "succeeded").await;

    let icc: &[u8] =
        include_bytes!("../../../slipstream-core/assets/prophoto-linear-g10-darktable.icc");
    let artifact = &settled["artifact"];
    assert_eq!(artifact["exportId"], settled["exportId"]);
    assert_eq!(artifact["target"], "development-tiff");
    assert_eq!(artifact["stage"], "develop");
    assert_eq!(artifact["contentType"], "image/tiff");
    assert_eq!(artifact["width"], 2);
    assert_eq!(artifact["height"], 1);
    assert_eq!(
        artifact["profileIdentity"],
        format!("{:x}", Sha256::digest(icc))
    );
    assert_eq!(artifact["byteLength"], artifact_bytes.len() as u64);
    assert_eq!(
        artifact["sha256"],
        format!("{:x}", Sha256::digest(&artifact_bytes))
    );
    assert!(artifact["expiresAt"].is_string());
    assert_eq!(settled["terminalOutcome"], "succeeded");
    assert!(settled["failureReason"].is_null());
    assert!(settled["receiptExpiresAt"].is_string());

    let download = download_artifact(&router, &export_id).await;
    assert_eq!(download.status(), StatusCode::OK);
    let headers = download.headers();
    // The closed typed metadata framing is response headers; every header
    // equals the artifact object field for field.
    assert_eq!(headers["slipstream-artifact-export-id"], export_id);
    assert_eq!(headers["slipstream-artifact-target"], "development-tiff");
    assert_eq!(headers["slipstream-artifact-stage"], "develop");
    assert_eq!(headers["slipstream-artifact-content-type"], "image/tiff");
    assert_eq!(headers["slipstream-artifact-width"], "2");
    assert_eq!(headers["slipstream-artifact-height"], "1");
    assert_eq!(
        headers["slipstream-artifact-profile-identity"],
        artifact["profileIdentity"].as_str().unwrap()
    );
    assert_eq!(
        headers["slipstream-artifact-byte-length"],
        artifact["byteLength"].as_u64().unwrap().to_string()
    );
    assert_eq!(
        headers["slipstream-artifact-sha256"],
        artifact["sha256"].as_str().unwrap()
    );
    assert_eq!(
        headers["slipstream-artifact-expires-at"],
        artifact["expiresAt"].as_str().unwrap()
    );
    assert_eq!(headers["content-type"], "image/tiff");
    assert_eq!(
        headers["content-length"],
        artifact["byteLength"].as_u64().unwrap().to_string()
    );
    let body = axum::body::to_bytes(download.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(&body[..], &artifact_bytes[..]);

    // The lease is released once the response stream settles.
    let connection =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let leases = connection
            .query_row(
                "SELECT COUNT(*) FROM export_download_leases WHERE export_id = ?1",
                [&export_id],
                |row| row.get::<_, u32>(0),
            )
            .unwrap();
        if leases == 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the download lease must be released after the stream settles"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // A row whose validated metadata is incomplete can never serve a
    // download: the artifact object and the headers would not exist.
    connection
        .execute("UPDATE exports SET artifact_width = NULL", [])
        .unwrap();
    drop(connection);
    let incomplete = download_artifact(&router, &export_id).await;
    assert_eq!(incomplete.status(), StatusCode::NOT_FOUND);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

#[tokio::test]
async fn export_without_a_retained_artifact_refuses_its_download() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    let export_id = response_json(created).await["exportId"]
        .as_str()
        .unwrap()
        .to_owned();

    // The attempt fails without an output; the download refuses with the
    // one terminal no-artifact code.
    wait_for_state(&router, &export_id, "running").await;
    launcher.with_script(|s| s.settle_attempt(1, "engine-failed"));
    let failed = wait_for_state(&router, &export_id, "failed").await;
    assert_eq!(failed["artifact"], serde_json::Value::Null);
    let missing = download_artifact(&router, &export_id).await;
    assert_eq!(missing.status(), StatusCode::CONFLICT);
    assert_eq!(
        error_code(&response_json(missing).await),
        "output_unavailable"
    );

    // After the artifact retention passes the download reports expiry.
    let connection =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE exports SET state='succeeded', artifact_size=?1, artifact_sha256=?2,
                   artifact_expires_at=1, artifact_width=2, artifact_height=1,
                   artifact_profile_identity=?3, settled_at=1, retain_until=?4 WHERE id=?5",
            rusqlite::params![22_i64, "f".repeat(64), "e".repeat(64), 2, export_id,],
        )
        .unwrap();
    drop(connection);
    let expired = download_artifact(&router, &export_id).await;
    assert_eq!(expired.status(), StatusCode::GONE);
    assert_eq!(
        error_code(&response_json(expired).await),
        "artifact_expired"
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// P1-1: the durable publication and Export commit happen before the
/// launcher acknowledgement, so a lost or refused acknowledgement can
/// never release the only valid result unpublished.
#[tokio::test]
async fn export_publication_commits_before_the_launcher_acknowledgement() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    let export_id = response_json(created).await["exportId"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_for_state(&router, &export_id, "running").await;

    // The result is valid but the acknowledgement is lost (the launcher
    // dies before answering): the Export must still settle succeeded with
    // a downloadable artifact, because publication committed first.
    launcher.with_script(|s| {
        s.output = Some(valid_development_tiff());
        s.settle_attempt(1, "completed");
        s.refuse_ack = true;
    });
    let settled = wait_for_state(&router, &export_id, "succeeded").await;
    assert_eq!(settled["terminalOutcome"], "succeeded");
    let download = download_artifact(&router, &export_id).await;
    assert_eq!(download.status(), StatusCode::OK);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// The production handshake: the launcher validates its engine artifact,
/// reports an output that waits for collection, and settles the attempt
/// from the service's acknowledgement. A service that waited for the
/// settled receipt before collecting would deadlock against it.
#[tokio::test]
async fn export_collects_the_output_the_launcher_waits_to_acknowledge() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    let export_id = response_json(created).await["exportId"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_for_state(&router, &export_id, "running").await;

    launcher.with_script(|s| {
        s.output = Some(valid_development_tiff());
        s.ready_output(1);
    });
    let settled = wait_for_state(&router, &export_id, "succeeded").await;
    assert_eq!(settled["terminalOutcome"], "succeeded");
    let download = download_artifact(&router, &export_id).await;
    assert_eq!(download.status(), StatusCode::OK);
    // The acknowledgement settled the attempt, not a poll deadline.
    let outcome = launcher.with_script(|s| {
        s.attempts
            .get(&(s.incarnation.clone(), 1))
            .and_then(|attempt| attempt.outcome.clone())
    });
    assert_eq!(outcome.as_deref(), Some("completed"));

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// P1-2: restart reconciliation validates an already-published artifact
/// from disk instead of requesting a second, impossible launcher
/// transfer, and resolves the Export from it.
#[tokio::test]
async fn export_restart_recovers_an_already_published_artifact() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let manager = Arc::clone(application.exports.as_ref().unwrap());
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    let export_id = response_json(created).await["exportId"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_for_state(&router, &export_id, "running").await;
    let ops_after_run = launcher.with_script(|s| s.ops.clone());

    // The previous process renamed the validated file into place and
    // crashed before committing: the record still looks running with its
    // attempt, the launcher receipt says completed, and the transfer
    // claim is spent.
    let artifact_bytes = valid_development_tiff();
    let path = manager.artifact_path(&export_id).unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, &artifact_bytes).unwrap();
    let incarnation = launcher.with_script(|s| s.incarnation.clone());
    launcher.with_script(|s| {
        s.output = Some(artifact_bytes.clone());
        s.settle_attempt(1, "completed");
        s.refuse_output = true;
    });
    let connection =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE exports SET state='running', attempt_incarnation=?1,
                   attempt_sequence=1 WHERE id=?2",
            rusqlite::params![incarnation, export_id],
        )
        .unwrap();
    drop(connection);
    // The crashed process had durably claimed this attempt's publication
    // just before renaming the validated file into place.
    application
        .library
        .claim_export_publication(&export_id, &incarnation, 1)
        .await
        .unwrap();

    manager.reconcile_after_restart();
    let settled = wait_for_state(&router, &export_id, "succeeded").await;
    assert_eq!(
        settled["artifact"]["byteLength"],
        artifact_bytes.len() as u64
    );
    assert_eq!(
        settled["artifact"]["sha256"],
        format!("{:x}", Sha256::digest(&artifact_bytes))
    );
    // The recovery must not have asked the launcher for another transfer.
    let ops = launcher.with_script(|s| s.ops.clone());
    assert!(
        !ops[ops_after_run.len()..].contains(&"output".to_owned()),
        "recovery must not request a second transfer: {ops:?}"
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A previously accepted Film identity must remain inspectable after Film is
/// withdrawn, even though a new Film submission can no longer be admitted.
/// The launcher is healthy, so the replay proves receipt resolution precedes
/// the qualification gate.
#[tokio::test]
async fn unqualified_film_still_replays_an_existing_receipt() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let source_revision = current_source_revision(&application, &photo_id).await;
    let recipe = save_recipe(&application, &photo_id, "save-film", None, 0.5).await;
    let identity = "previous-film-request";
    let digest = slipstream_core::export_submission_payload_digest(
        "film-jpeg",
        &recipe.revision,
        &source_revision,
    );
    let export_id = "exp-prior-film";
    let recipe_digest = slipstream_core::ExportRecipePayload::capture(
        &slipstream_core::EditRecipeSettings {
            exposure_ev: 0.5,
            white_balance: slipstream_core::WhiteBalanceIntent::AsShot,
        },
        slipstream_core::ExportExposureRange {
            minimum_milli_ev: i64::MIN,
            maximum_milli_ev: i64::MAX,
        },
    )
    .unwrap()
    .digest();
    let connection =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    connection
        .execute(
            "INSERT INTO exports(id,photo_id,target,state,recipe_revision,exposure_ev,
                   white_balance_mode,source_revision,source_profile_id,source_kind,
                   recipe_digest,policy_id,bundle_id,workload,created_at,outcome,retain_until)
             VALUES(?1,?2,'film-jpeg','failed',?3,0.5,'as-shot',?4,
                    'sony-ilce-7rm5-arw','raw',?5,?6,?7,'film-jpeg',?8,
                    'processing launcher became unreachable while the attempt ran',?9)",
            rusqlite::params![
                export_id,
                photo_id,
                recipe.revision,
                source_revision,
                recipe_digest,
                "b".repeat(64),
                "c".repeat(64),
                1_800_000_000_i64,
                1_900_000_000_i64,
            ],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO library_metadata(key,value) VALUES(?1,?2)",
            rusqlite::params![
                format!("export_receipt:{photo_id}\0{identity}"),
                serde_json::json!({
                    "payload_digest": digest,
                    "export_id": export_id,
                    "created_at": 1_800_000_000_u64,
                    "settled_at": null
                })
                .to_string(),
            ],
        )
        .unwrap();
    drop(connection);
    let body = serde_json::json!({
        "requestId": identity,
        "expectedRecipeVersion": recipe.revision,
        "expectedSourceRevision": source_revision,
        "target": "film-jpeg",
    });
    let replay = submit_export_body(&router, &photo_id, body).await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(response_json(replay).await["exportId"], export_id);
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// P1-3: a recorded submission replays with 200 even while the launcher
/// cannot admit work or after its source has become unreadable; a
/// different payload under the recorded identity still conflicts.
#[tokio::test]
async fn export_replay_resolves_without_launcher_availability() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let export_id = response_json(created).await["exportId"].clone();

    launcher.with_script(|s| s.availability = Availability::Blocked);
    let replayed = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(replayed.status(), StatusCode::OK);
    assert_eq!(response_json(replayed).await["exportId"], export_id);

    // A different payload under the recorded identity still conflicts.
    let conflict = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        "other-revision",
        &source_revision,
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(
        error_code(&response_json(conflict).await),
        "export_conflict"
    );

    // The same replay resolves after the source itself disappears:
    // receipt resolution precedes any source classification and a replay
    // never touches the source.
    fs::remove_file(config.library_root.join("pair.ARW")).unwrap();
    let unreadable = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(
        unreadable.status(),
        StatusCode::OK,
        "a recorded identity must replay even when its source is unreadable: {}",
        response_json(unreadable).await
    );
    assert_eq!(response_json(unreadable).await["exportId"], export_id);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// P2-1: Export request identities are scoped per Photo; the same
/// otherwise-valid identity on another Photo starts its own Export.
#[tokio::test]
async fn export_request_identities_are_scoped_per_photo() {
    let (base, mut config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    raw_fixture_with_camera(
        &config.library_root.join("second.ARW"),
        b"SONY\0",
        b"ILCE-7RM5\0\0\0",
    );
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let second_id = photo_id_for(&config, "second.ARW");
    let first = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let second = save_recipe(&application, &second_id, "save-2", None, 0.5).await;
    let first_source = current_source_revision(&application, &photo_id).await;
    let second_source = current_source_revision(&application, &second_id).await;

    let first_created = submit_export_request(
        &router,
        &photo_id,
        "shared-request",
        &first.revision,
        &first_source,
    )
    .await;
    assert_eq!(first_created.status(), StatusCode::CREATED);
    let second_created = submit_export_request(
        &router,
        &second_id,
        "shared-request",
        &second.revision,
        &second_source,
    )
    .await;
    assert_eq!(second_created.status(), StatusCode::CREATED);
    assert_ne!(
        response_json(first_created).await["exportId"],
        response_json(second_created).await["exportId"]
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// P1: the submission digest covers only the caller's payload, so a
/// legitimate deployment bundle change cannot turn an identical replay
/// into a conflict.
#[tokio::test]
async fn export_replay_survives_a_deployment_bundle_change() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let export_id = response_json(created).await["exportId"].clone();

    // A redeploy swaps the processing bundle; the socket and policy stay
    // as they were.
    let mut redeployed = launcher.processing_config();
    redeployed.bundle_sha256 = "d".repeat(64);
    let router_after = create_router_with_processing(
        Arc::clone(&application),
        open_web_root(config.web_root()),
        Some(redeployed),
    );
    let replayed = submit_export_request(
        &router_after,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    assert_eq!(
        replayed.status(),
        StatusCode::OK,
        "an identical caller payload must replay across a bundle change: {}",
        response_json(replayed).await
    );
    assert_eq!(response_json(replayed).await["exportId"], export_id);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// P1/P2: disk recovery must adopt a published artifact only for the
/// attempt that published it; a stale file from a superseded attempt is
/// never the live attempt's output.
#[tokio::test]
async fn export_recovery_ignores_a_file_from_a_superseded_attempt() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let manager = Arc::clone(application.exports.as_ref().unwrap());
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    let export_id = response_json(created).await["exportId"]
        .as_str()
        .unwrap()
        .to_owned();
    wait_for_state(&router, &export_id, "running").await;
    cancel_export(&router, &export_id).await;
    wait_for_state(&router, &export_id, "cancelled").await;

    // A stale file survives from the cancelled attempt, while the retry
    // runs with a spent transfer claim: the launcher refuses any new
    // Output. Only the stale file could make this Export succeed.
    let stale_bytes = valid_development_tiff();
    let stale_digest = format!("{:x}", Sha256::digest(&stale_bytes));
    let path = manager.artifact_path(&export_id).unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, &stale_bytes).unwrap();
    let incarnation = launcher.with_script(|s| s.incarnation.clone());
    launcher.with_script(|s| {
        s.settle_attempt(2, "completed");
        s.refuse_output = true;
    });
    let connection =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE exports SET state='running', attempt_incarnation=?1,
                   attempt_sequence=2 WHERE id=?2",
            rusqlite::params![incarnation, export_id],
        )
        .unwrap();
    drop(connection);

    manager.reconcile_after_restart();
    let settled = wait_for_state(&router, &export_id, "failed").await;
    let recorded_sha = settled["artifact"]["sha256"].as_str().unwrap_or("absent");
    assert_ne!(
        recorded_sha, stale_digest,
        "recovery must not adopt a superseded attempt's file as the retry's output"
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// P2: the download lease holds and keeps renewing until the response
/// body drains or is dropped, not merely until the producer reaches end
/// of file.
#[tokio::test]
async fn export_download_holds_its_lease_until_the_body_drains() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let (application, router) = export_application(&base, &config).await;
    let manager = Arc::clone(application.exports.as_ref().unwrap());
    manager.set_lease_renewal_interval(Duration::from_millis(150));
    let photo_id = photo_id_for(&config, "pair.ARW");
    let export_id = "exp-lease-drain-check".to_owned();
    let artifact_bytes = vec![9_u8; 64 * 1024];
    let digest = format!("{:x}", Sha256::digest(&artifact_bytes));
    let recipe_digest = slipstream_core::ExportRecipePayload::capture(
        &slipstream_core::EditRecipeSettings {
            exposure_ev: 0.0,
            white_balance: slipstream_core::WhiteBalanceIntent::AsShot,
        },
        slipstream_core::ExportExposureRange {
            minimum_milli_ev: i64::MIN,
            maximum_milli_ev: i64::MAX,
        },
    )
    .unwrap()
    .digest();
    let path = manager.artifact_path(&export_id).unwrap();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, &artifact_bytes).unwrap();
    let connection =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    connection
        .execute(
            "INSERT INTO exports(id,photo_id,target,state,recipe_revision,exposure_ev,
                   white_balance_mode,source_revision,source_profile_id,source_kind,
                   recipe_digest,policy_id,bundle_id,workload,created_at,outcome,
                   artifact_size,artifact_sha256,artifact_expires_at,artifact_width,
                   artifact_height,artifact_profile_identity,settled_at,retain_until)
                 VALUES(?1,?2,'development-tiff','succeeded','rev',0.0,'as-shot','src',
                   'profile','raw',?3,?4,?5,'development-tiff',1,NULL,?6,?7,?8,2,1,?9,
                   1800000000,1900000000)",
            rusqlite::params![
                export_id,
                photo_id,
                recipe_digest,
                "b".repeat(64),
                "c".repeat(64),
                artifact_bytes.len() as i64,
                digest,
                1_900_000_000_i64,
                "e".repeat(64),
            ],
        )
        .unwrap();
    drop(connection);

    let download = download_artifact(&router, &export_id).await;
    assert_eq!(download.status(), StatusCode::OK);
    // The producer has certainly reached end of file by now, but the
    // response body was never consumed: the lease must still exist.
    tokio::time::sleep(Duration::from_millis(500)).await;
    let held = {
        let connection =
            rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
        connection
            .query_row(
                "SELECT COUNT(*) FROM export_download_leases WHERE export_id = ?1",
                [&export_id],
                |row| row.get::<_, u32>(0),
            )
            .unwrap()
    };
    assert_eq!(
        held, 1,
        "an undrained response body must keep its download lease"
    );

    // A live stream keeps its lease renewed, so the staleness sweep
    // cannot reclaim it while the body is still undrained.
    let first_renewal: i64 = {
        let connection =
            rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
        connection
            .query_row(
                "SELECT created_at FROM export_download_leases WHERE export_id = ?1",
                [&export_id],
                |row| row.get(0),
            )
            .unwrap()
    };
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let renewed_at: i64 = {
        let connection =
            rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
        connection
            .query_row(
                "SELECT created_at FROM export_download_leases WHERE export_id = ?1",
                [&export_id],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert!(
        renewed_at > first_renewal,
        "a live stream must keep its lease renewed: {first_renewal} then {renewed_at}"
    );
    drop(download);

    // Dropping the body settles the stream and releases the lease.
    let connection =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let leases = connection
            .query_row(
                "SELECT COUNT(*) FROM export_download_leases WHERE export_id = ?1",
                [&export_id],
                |row| row.get::<_, u32>(0),
            )
            .unwrap();
        if leases == 0 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the download lease must be released after the body is dropped"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    drop(connection);

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

async fn edit_preview_request(router: &Router, photo_id: &str) -> Response<Body> {
    send(
        router,
        authenticated_request()
            .uri(format!(
                "http://camera.local/api/photos/{photo_id}/edit-preview/develop"
            ))
            .header("slipstream-cli-contract", "1")
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

async fn edit_preview_settings_request(
    router: &Router,
    photo_id: &str,
    settings: &str,
) -> Response<Body> {
    send(
        router,
        authenticated_request()
            .uri(format!(
                "http://camera.local/api/photos/{photo_id}/edit-preview/develop?settings={settings}"
            ))
            .header("slipstream-cli-contract", "1")
            .body(Body::empty())
            .unwrap(),
    )
    .await
}

/// The Edit Preview derives the `develop` rendition from the retained
/// Development TIFF of a succeeded Export while that Export's captured
/// identity is the current one, then admits a preview-class render after a
/// later recipe makes the retained result stale.
#[tokio::test]
async fn edit_preview_derives_from_the_retained_development_tiff_of_an_export() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.5).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    let export_id = response_json(created).await["exportId"]
        .as_str()
        .unwrap()
        .to_owned();
    let artifact_bytes = valid_development_tiff();
    launcher.with_script(|s| {
        s.output = Some(artifact_bytes.clone());
        s.settle_attempt(1, "completed");
    });
    wait_for_state(&router, &export_id, "succeeded").await;

    // The published Development TIFF is the retained Development Result of
    // exactly this identity, so the develop rendition derives from it.
    let preview = edit_preview_request(&router, &photo_id).await;
    assert_eq!(preview.status(), StatusCode::OK);
    let headers = preview.headers();
    assert_eq!(headers["slipstream-edit-preview-stage"], "develop");
    assert_eq!(
        headers["slipstream-edit-preview-recipe-version"],
        recipe.revision
    );
    assert_eq!(
        headers["slipstream-edit-preview-source-revision"],
        crate::queries::hex_encode(source_revision.as_bytes())
    );
    assert_eq!(headers["slipstream-edit-preview-width"], "2");
    assert_eq!(headers["slipstream-edit-preview-height"], "1");
    assert_eq!(headers["content-type"], "image/jpeg");
    let declared_sha256 = headers["slipstream-edit-preview-sha256"]
        .to_str()
        .unwrap()
        .to_owned();
    let body = axum::body::to_bytes(preview.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(&body[..2], &[0xff, 0xd8], "the rendition is a JPEG");
    assert_eq!(
        declared_sha256,
        format!("{:x}", Sha256::digest(&body)),
        "the rendition's content digest is the one it declares"
    );

    // A later recipe is another identity. The retained Development TIFF is
    // no longer current, so the service admits a preview-class render
    // through the same processing workload instead of refusing.
    let second = save_recipe(
        &application,
        &photo_id,
        "save-2",
        Some(recipe.revision.clone()),
        0.25,
    )
    .await;
    assert_ne!(second.revision, recipe.revision);
    // Make the background attempt settle promptly after the route has
    // observed its admission; the wire response is independent of the
    // eventual launcher outcome.
    launcher.with_script(|s| {
        s.output = Some(valid_development_tiff());
    });
    let admitted = edit_preview_request(&router, &photo_id).await;
    assert_eq!(admitted.status(), StatusCode::ACCEPTED);
    let payload = response_json(admitted).await;
    assert!(
        payload["state"] == "queued" || payload["state"] == "running",
        "preview admission state is queued or running: {payload}"
    );
    assert_eq!(payload["stage"], "develop");
    let repeated = edit_preview_request(&router, &photo_id).await;
    assert_eq!(repeated.status(), StatusCode::ACCEPTED);
    let repeated_payload = response_json(repeated).await;
    assert_eq!(repeated_payload["state"], "running");
    launcher.with_script(|s| s.settle_attempt(2, "completed"));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let response = edit_preview_request(&router, &photo_id).await;
        if response.status() == StatusCode::OK {
            break;
        }
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        assert!(
            tokio::time::Instant::now() < deadline,
            "preview-class render did not become a rendition"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let starts = launcher.with_script(|s| {
        s.ops
            .iter()
            .filter(|operation| operation.as_str() == "start")
            .count()
    });
    assert_eq!(starts, 2, "one Export and one coalesced preview attempt");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// The baseline comparison selector serves the as-shot/baseline
/// development from the retained result captured at exactly those
/// settings, independently of the saved recipe revision, and it keeps
/// serving while the saved recipe moves on.
#[tokio::test]
async fn edit_preview_serves_the_baseline_comparison_independently_of_the_recipe() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    // The saved recipe is the processing baseline itself: 0 EV, as-shot.
    let recipe = save_recipe(&application, &photo_id, "save-1", None, 0.0).await;
    let source_revision = current_source_revision(&application, &photo_id).await;
    let created = submit_export_request(
        &router,
        &photo_id,
        "request-1",
        &recipe.revision,
        &source_revision,
    )
    .await;
    let export_id = response_json(created).await["exportId"]
        .as_str()
        .unwrap()
        .to_owned();
    let artifact_bytes = valid_development_tiff();
    launcher.with_script(|s| {
        s.output = Some(artifact_bytes.clone());
        s.settle_attempt(1, "completed");
    });
    wait_for_state(&router, &export_id, "succeeded").await;

    let current = edit_preview_request(&router, &photo_id).await;
    assert_eq!(current.status(), StatusCode::OK);
    let current_sha = current.headers()["slipstream-edit-preview-sha256"]
        .to_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        current.headers()["slipstream-edit-preview-settings"],
        "current"
    );
    assert_eq!(
        current.headers()["slipstream-edit-preview-recipe-version"],
        recipe.revision
    );

    // The comparison is the baseline development: the same retained
    // result, named as the baseline rather than as the saved recipe.
    let baseline = edit_preview_settings_request(&router, &photo_id, "baseline").await;
    assert_eq!(baseline.status(), StatusCode::OK);
    assert_eq!(
        baseline.headers()["slipstream-edit-preview-settings"],
        "baseline"
    );
    assert_eq!(
        baseline.headers()["slipstream-edit-preview-recipe-version"],
        "",
        "a baseline rendition is not a saved recipe's"
    );
    assert_eq!(
        baseline.headers()["slipstream-edit-preview-sha256"],
        current_sha,
        "the baseline of a baseline recipe is the same development"
    );

    // A later edit moves the current identity. The comparison is
    // unchanged — it still resolves from the retained baseline result —
    // while the current rendition is no longer retained and is admitted
    // as its own render work.
    let second = save_recipe(
        &application,
        &photo_id,
        "save-2",
        Some(recipe.revision.clone()),
        0.5,
    )
    .await;
    assert_ne!(second.revision, recipe.revision);
    launcher.with_script(|s| {
        s.output = Some(valid_development_tiff());
    });
    let baseline = edit_preview_settings_request(&router, &photo_id, "baseline").await;
    assert_eq!(baseline.status(), StatusCode::OK);
    assert_eq!(
        baseline.headers()["slipstream-edit-preview-sha256"],
        current_sha
    );
    let admitted = edit_preview_request(&router, &photo_id).await;
    assert_eq!(admitted.status(), StatusCode::ACCEPTED);
    let payload = response_json(admitted).await;
    assert!(
        payload["state"] == "queued" || payload["state"] == "running",
        "the edited recipe is new render work: {payload}"
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A stored white-balance mode the closed execution payload cannot
/// represent is retained intent, not render work: the route refuses it
/// before admitting an attempt that could never produce a result, so a
/// polling client is told the stage is unavailable instead of being handed
/// queued work that fails and is re-admitted forever.
#[tokio::test]
async fn edit_preview_refuses_a_recipe_the_closed_payload_cannot_execute() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let source_revision = current_source_revision(&application, &photo_id).await;
    let outcome = application
        .library
        .save_edit_recipe(slipstream_core::SaveEditRecipe {
            photo_id: photo_id.clone(),
            request_id: "save-temperature-tint".to_owned(),
            expected_recipe_version: None,
            expected_source_revision: source_revision,
            settings: slipstream_core::EditRecipeSettings {
                exposure_ev: 0.25,
                white_balance: slipstream_core::WhiteBalanceIntent::TemperatureTint {
                    temperature_kelvin: 6_500,
                    tint_milli: -12,
                },
            },
        })
        .await
        .unwrap();
    assert!(
        matches!(
            outcome,
            slipstream_core::EditRecipeWriteOutcome::Saved(_)
                | slipstream_core::EditRecipeWriteOutcome::Unchanged(_)
        ),
        "an adjustable white balance is accepted as editing intent: {outcome:?}"
    );

    let refused = edit_preview_request(&router, &photo_id).await;
    assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
    let payload = response_json(refused).await;
    assert_eq!(payload["error"]["code"], "processing_unavailable");
    assert_eq!(payload["error"]["details"]["stage"], "develop");
    assert_eq!(
        payload["error"]["details"]["reason"],
        "recipe-not-representable"
    );
    let starts = launcher.with_script(|s| {
        s.ops
            .iter()
            .filter(|operation| operation.as_str() == "start")
            .count()
    });
    assert_eq!(starts, 0, "a refused identity starts no processing attempt");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A preview-class attempt that fails is released, not remembered: the
/// next request admits a new attempt instead of reporting `running` for
/// work that no longer exists, and the failed attempt's ephemeral output
/// is never served as a rendition.
#[tokio::test]
async fn edit_preview_re_admits_after_a_failed_render() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    save_recipe(&application, &photo_id, "save-1", None, 0.4).await;
    // The launcher settles the attempt as failed and transfers no output.
    launcher.with_script(|s| s.settle_attempt(1, "failed"));

    let admitted = edit_preview_request(&router, &photo_id).await;
    assert_eq!(admitted.status(), StatusCode::ACCEPTED);
    assert_eq!(response_json(admitted).await["state"], "queued");

    // The route keeps answering the admission while the attempt is live,
    // and once it has failed the next request admits a new one.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let response = edit_preview_request(&router, &photo_id).await;
        assert_eq!(response.status(), StatusCode::ACCEPTED);
        if response_json(response).await["state"] == "queued" {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "a failed render must be released for a new attempt"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let starts = launcher.with_script(|s| {
        s.ops
            .iter()
            .filter(|operation| operation.as_str() == "start")
            .count()
    });
    // The new attempt's Start reaches the launcher from a background task,
    // so the count is observed rather than assumed.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut starts = starts;
    while starts < 2 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the re-admitted attempt must reach the launcher"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
        starts = launcher.with_script(|s| {
            s.ops
                .iter()
                .filter(|operation| operation.as_str() == "start")
                .count()
        });
    }
    assert_eq!(starts, 2, "the failed attempt is re-admitted as new work");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A newer intent supersedes a live render: the launcher attempt is
/// cancelled, so the stale attempt neither occupies the serialized
/// processing slot nor publishes, and the new identity is admitted as
/// its own launcher attempt.
#[tokio::test]
async fn edit_preview_supersedes_a_live_render_with_the_newer_intent() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let first = save_recipe(&application, &photo_id, "save-1", None, 0.2).await;
    let admitted = edit_preview_request(&router, &photo_id).await;
    assert_eq!(admitted.status(), StatusCode::ACCEPTED);

    // The first attempt is live on the launcher before the newer intent
    // arrives, so the newer intent has to release it.
    wait_for_launcher_op(&launcher, "start", 1).await;

    let second = save_recipe(
        &application,
        &photo_id,
        "save-2",
        Some(first.revision.clone()),
        0.6,
    )
    .await;
    assert_ne!(second.revision, first.revision);
    let superseded = edit_preview_request(&router, &photo_id).await;
    assert_eq!(superseded.status(), StatusCode::ACCEPTED);
    assert_eq!(response_json(superseded).await["state"], "queued");

    // The superseded attempt is cancelled, and the freed slot admits the
    // newer identity as a second launcher attempt.
    wait_for_launcher_op(&launcher, "cancel", 1).await;
    wait_for_launcher_op(&launcher, "start", 2).await;

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// A preview whose validation acknowledgement is lost fails the render
/// and abandons the launcher attempt: the launcher must not keep an
/// attempt its owner has already given up on.
#[tokio::test]
async fn edit_preview_abandons_the_attempt_when_the_acknowledgement_is_lost() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    save_recipe(&application, &photo_id, "save-1", None, 0.2).await;
    launcher.with_script(|s| {
        s.output = Some(valid_development_tiff());
        s.settle_attempt(1, "completed");
        s.refuse_ack = true;
    });
    let admitted = edit_preview_request(&router, &photo_id).await;
    assert_eq!(admitted.status(), StatusCode::ACCEPTED);

    wait_for_launcher_op(&launcher, "cancel", 1).await;

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// An attempt that completed before its cancellation is released without a
/// fabricated rejection: the service holds no collected output, and the
/// launcher refuses an acknowledgement that names none.
#[tokio::test]
async fn edit_preview_releases_a_completed_attempt_it_cannot_collect() {
    let (base, config) = export_fixture(Some(64 * 1024 * 1024 * 1024));
    let processing = config.processing.clone().unwrap();
    let launcher = FakeLauncher::start(&processing, LauncherScript::new());
    let mut config = config;
    config.processing = Some(launcher.processing_config());
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    save_recipe(&application, &photo_id, "save-1", None, 0.2).await;
    launcher.with_script(|s| {
        s.output = Some(valid_development_tiff());
        s.settle_attempt(1, "completed");
        s.refuse_output = true;
    });
    let admitted = edit_preview_request(&router, &photo_id).await;
    assert_eq!(admitted.status(), StatusCode::ACCEPTED);

    wait_for_launcher_op(&launcher, "cancel", 1).await;
    let ops = launcher.with_script(|s| s.ops.clone());
    assert!(
        !ops.contains(&"validate".to_owned()),
        "a preview without a collected output must not acknowledge one: {ops:?}"
    );

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}

/// Waits until the launcher recorded `count` operations named `op`.
async fn wait_for_launcher_op(launcher: &FakeLauncher, op: &str, count: usize) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let observed =
            launcher.with_script(|s| s.ops.iter().filter(|entry| entry.as_str() == op).count());
        if observed >= count {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the launcher must record {count} {op} operations: {:?}",
            launcher.with_script(|s| s.ops.clone())
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn xmp_snapshot_download_and_replay_do_not_need_processing_or_original() {
    let (base, config) = prepare_populated_fixture();
    raw_fixture_with_camera(
        &config.library_root.join("pair.ARW"),
        b"SONY\0",
        b"ILCE-7RM5\0\0\0",
    );
    let (application, router) = export_application(&base, &config).await;
    let photo_id = photo_id_for(&config, "pair.ARW");
    let recipe = save_recipe(&application, &photo_id, "save-xmp", None, 0.5).await;
    let uri = format!("https://camera.local/api/photos/{photo_id}/edit-state-exports");
    let body = serde_json::json!({"requestId":"xmp-snapshot", "expectedRecipeVersion":recipe.revision, "expectedSourceRevision":recipe.source_revision});
    let created = post_export_body(&router, uri.clone(), body.clone()).await;
    assert_eq!(created.status(), StatusCode::CREATED);
    let created = response_json(created).await;
    let export_id = created["exportId"].as_str().unwrap().to_owned();
    let artifact_uri = format!("{uri}/{export_id}/artifact");
    let download = send(
        &router,
        authenticated_request()
            .uri(&artifact_uri)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(download.status(), StatusCode::OK);
    assert_eq!(download.headers()["content-type"], "application/rdf+xml");
    assert_eq!(
        download.headers()["slipstream-artifact-filename"],
        created["artifact"]["filename"].as_str().unwrap()
    );
    assert_eq!(
        download.headers()["slipstream-artifact-sha256"],
        created["artifact"]["sha256"].as_str().unwrap()
    );
    let bytes = axum::body::to_bytes(download.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(
        bytes.len() as u64,
        created["artifact"]["byteLength"].as_u64().unwrap()
    );
    assert_eq!(
        format!("{:x}", Sha256::digest(&bytes)),
        created["artifact"]["sha256"].as_str().unwrap()
    );
    assert!(
        std::str::from_utf8(&bytes)
            .unwrap()
            .contains("<crs:Exposure2012>0.5</crs:Exposure2012>")
    );
    save_recipe(
        &application,
        &photo_id,
        "save-xmp-later",
        Some(recipe.revision),
        1.0,
    )
    .await;
    fs::remove_file(config.library_root.join("pair.ARW")).unwrap();
    let replay = post_export_body(&router, uri.clone(), body.clone()).await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_eq!(response_json(replay).await, created);
    let listed = response_json(
        send(
            &router,
            authenticated_request()
                .uri(&uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await,
    )
    .await;
    assert_eq!(listed["exports"], serde_json::json!([created]));
    let mut conflicting = body.clone();
    conflicting["expectedRecipeVersion"] = serde_json::json!("different");
    let conflict = post_export_body(&router, uri.clone(), conflicting).await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(
        error_code(&response_json(conflict).await),
        "export_conflict"
    );
    for request_id in ["contains space", "bad/slash", "nonascii-ñ"] {
        let mut invalid = body.clone();
        invalid["requestId"] = serde_json::json!(request_id);
        assert_eq!(
            post_export_body(&router, uri.clone(), invalid)
                .await
                .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }
    let mut stale = body.clone();
    stale["requestId"] = serde_json::json!("new-request");
    assert_eq!(
        post_export_body(&router, uri.clone(), stale).await.status(),
        StatusCode::CONFLICT
    );
    let connection =
        rusqlite::Connection::open(config.state_directory.join("library.sqlite")).unwrap();
    connection
        .execute(
            "UPDATE xmp_exports SET expires_at=created_at WHERE id=?",
            [&export_id],
        )
        .unwrap();
    drop(connection);
    let expired = post_export_body(&router, uri, body).await;
    assert_eq!(expired.status(), StatusCode::GONE);
    assert_eq!(error_code(&response_json(expired).await), "export_expired");
    assert_eq!(
        send(
            &router,
            authenticated_request()
                .uri(&artifact_uri)
                .body(Body::empty())
                .unwrap()
        )
        .await
        .status(),
        StatusCode::GONE
    );
    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
