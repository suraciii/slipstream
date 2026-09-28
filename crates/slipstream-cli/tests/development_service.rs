//! Executable tests of the Photo Development surface: Edit Recipe guarded
//! writes and Edit Preview downloads against a compact scripted TLS service,
//! plus the two deployment facts the real service answers deterministically
//! without a processing launcher (the disabled capability report and the
//! unsupported JPEG source class). Every scenario runs the actual CLI
//! binary, so the exit codes, envelopes, and local file effects are the
//! consumer-visible contract.

use serde_json::{Value, json};
use slipstream_server::Config;
mod common;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{ErrorKind, Read, Write},
    net::TcpListener,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

const PHOTO_ID: &str = "00000000-0000-4000-8000-000000000001";
const SOURCE_REVISION: &str = "source-3";
const CURRENT_RECIPE_VERSION: &str = "recipe-9";
const EXPIRES_AT: &str = "2030-01-01T00:00:00Z";

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------- helpers

fn temp_base(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "slipstream-cli-development-{name}-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&base).unwrap();
    base
}

fn write_input(base: &std::path::Path, name: &str, content: &str) -> String {
    let path = base.join(name);
    fs::write(&path, content).unwrap();
    path.to_str().unwrap().to_owned()
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
            .arg(&server)
            .args(&arguments)
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

/// A deterministic 8x4 JPEG, the smallest complete rendition the publish
/// path can verify header for header, by digest, and by JPEG structure.
fn jpeg_bytes() -> Vec<u8> {
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
        .encode(&[50; 8 * 4 * 3], 8, 4, image::ExtendedColorType::Rgb8)
        .unwrap();
    jpeg
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The service hex encodes the opaque source revision for the response
/// headers; reproduce the encoding the CLI must decode.
fn hex_encode(value: &str) -> String {
    value.bytes().map(|byte| format!("{byte:02x}")).collect()
}

fn save_document() -> Value {
    json!({
        "requestId": "save-001",
        "expectedRecipeVersion": "recipe-7",
        "expectedSourceRevision": SOURCE_REVISION,
        "settings": {"exposureEv": 0.25, "whiteBalance": {"mode": "as-shot"}},
    })
}

// ---------------------------------------------------------------- fake service

/// One scripted reply of the compact TLS service. Every CLI invocation makes
/// one connection per request, so connection N receives reply N.
enum Step {
    /// Answer the capabilities handshake with the closed body.
    Capabilities,
    /// Answer the handshake without `limits.removalPhotoIdsMaximum`.
    CapabilitiesWithoutRemovalLimit,
    /// Read one mutation request, then close without responding.
    LoseAfterRead,
    /// Answer the save with the service's `recipe_conflict` facts.
    RecipeConflict,
    /// Answer the Edit Preview read with the pending admission.
    AdmitPreview,
    IndeterminatePreview,
    /// Serve a complete, correctly described ready Develop rendition.
    ServeRendition,
    /// Serve a complete, correctly described ready Film rendition.
    ServeFilmRendition,
    /// Serve the rendition with a digest that names other bytes.
    ServeWrongDigest,
    /// Announce more body bytes than the transfer sends, then close.
    ServeShortTransfer,
}

#[derive(Clone, Debug)]
struct RecordedRequest {
    request_line: String,
    body: Vec<u8>,
}

struct FakeService {
    url: String,
    stop: Arc<AtomicBool>,
    connections: Arc<AtomicUsize>,
    recorded: mpsc::Receiver<RecordedRequest>,
    handle: JoinHandle<()>,
}

impl FakeService {
    /// Stops serving, drains briefly so any late or retried connection is
    /// still counted, and reports every connection and full request seen.
    fn finish(self) -> (usize, Vec<RecordedRequest>) {
        self.stop.store(true, Ordering::SeqCst);
        self.handle.join().expect("fake service thread completed");
        let mut requests = Vec::new();
        while let Ok(request) = self.recorded.try_recv() {
            requests.push(request);
        }
        (self.connections.load(Ordering::SeqCst), requests)
    }
}

fn fake_service(steps: Vec<Step>) -> FakeService {
    fake_service_with(steps, Arc::new(jpeg_bytes()))
}

fn fake_service_with(steps: Vec<Step>, rendition: Arc<Vec<u8>>) -> FakeService {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!(
        "https://127.0.0.1:{}",
        listener.local_addr().unwrap().port()
    );
    let stop = Arc::new(AtomicBool::new(false));
    let connections = Arc::new(AtomicUsize::new(0));
    let (sender, recorded) = mpsc::channel::<RecordedRequest>();
    let handle = {
        let stop = Arc::clone(&stop);
        let connections = Arc::clone(&connections);
        std::thread::spawn(move || {
            let mut next_step = 0usize;
            loop {
                if stop.load(Ordering::SeqCst) {
                    // The CLI process has exited; any connection it opened is
                    // already queued here, so a short window sees it all.
                    let drain_until = Instant::now() + Duration::from_millis(300);
                    while Instant::now() < drain_until {
                        match listener.accept() {
                            Ok((stream, _)) => {
                                connections.fetch_add(1, Ordering::SeqCst);
                                drop(stream);
                            }
                            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                                std::thread::sleep(Duration::from_millis(2))
                            }
                            Err(_) => break,
                        }
                    }
                    return;
                }
                match listener.accept() {
                    Ok((stream, _)) => {
                        connections.fetch_add(1, Ordering::SeqCst);
                        stream.set_nonblocking(false).unwrap();
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
                        let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
                        let mut stream = common::tls_stream(stream);
                        let (head, body) = read_request(&mut stream);
                        let wire = format!("{head}\n{}", String::from_utf8_lossy(&body));
                        common::assert_bearer(wire.as_bytes());
                        assert!(
                            wire.to_ascii_lowercase()
                                .contains("slipstream-cli-contract: 1"),
                            "CLI request did not frame the contract header"
                        );
                        sender
                            .send(RecordedRequest {
                                request_line: head.lines().next().unwrap_or_default().to_owned(),
                                body,
                            })
                            .expect("the test holds the recording receiver");
                        if let Some(step) = steps.get(next_step) {
                            answer(&mut stream, step, &rendition);
                        }
                        next_step += 1;
                        drop(stream);
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2))
                    }
                    Err(_) => return,
                }
            }
        })
    };
    FakeService {
        url,
        stop,
        connections,
        recorded,
        handle,
    }
}

/// Reads one complete HTTP request: the header block and exactly the
/// `Content-Length` body bytes that follow it.
fn read_request(stream: &mut impl Read) -> (String, Vec<u8>) {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];
    let end = loop {
        if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
        let count = stream.read(&mut chunk).expect("read CLI request head");
        assert!(
            count > 0,
            "CLI closed the connection before sending a request"
        );
        buffer.extend_from_slice(&chunk[..count]);
    };
    let head = String::from_utf8_lossy(&buffer[..end]).into_owned();
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    while buffer.len() < end + 4 + length {
        let count = stream.read(&mut chunk).expect("read CLI request body");
        assert!(count > 0, "CLI closed the connection mid-request");
        buffer.extend_from_slice(&chunk[..count]);
    }
    (head, buffer[end + 4..end + 4 + length].to_vec())
}

fn answer(stream: &mut impl Write, step: &Step, rendition: &[u8]) {
    match step {
        Step::Capabilities => write_json_response(stream, 200, "OK", &capabilities_body()),
        Step::CapabilitiesWithoutRemovalLimit => {
            write_json_response(stream, 200, "OK", &handshake_without_removal_limit())
        }
        // The request was read; dropping the stream loses the response.
        Step::LoseAfterRead => {}
        Step::RecipeConflict => write_json_response(stream, 409, "Conflict", &conflict_body()),
        Step::AdmitPreview => write_json_response(
            stream,
            202,
            "Accepted",
            &json!({"state": "queued", "stage": "develop"}),
        ),
        Step::IndeterminatePreview => write_json_response(
            stream,
            500,
            "Internal Server Error",
            &json!({"error":{"code":"outcome_unknown", "message":"The render admission outcome is unknown; request the preview again.", "effect":"none", "details":{"stage":"develop"}}}),
        ),
        Step::ServeRendition => write_rendition(
            stream,
            "develop",
            &sha256_hex(rendition),
            rendition.len(),
            rendition,
        ),
        Step::ServeFilmRendition => write_rendition(
            stream,
            "film",
            &sha256_hex(rendition),
            rendition.len(),
            rendition,
        ),
        Step::ServeWrongDigest => write_rendition(
            stream,
            "develop",
            &sha256_hex(b"other bytes"),
            rendition.len(),
            rendition,
        ),
        Step::ServeShortTransfer => write_rendition(
            stream,
            "develop",
            &sha256_hex(rendition),
            rendition.len() + 24,
            &rendition[..rendition.len() / 2],
        ),
    }
}

fn write_json_response(stream: &mut impl Write, status: u16, reason: &str, body: &Value) {
    let bytes = serde_json::to_vec(body).unwrap();
    let _ = write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        bytes.len()
    );
    let _ = stream.write_all(&bytes);
}

/// The ready-rendition response in the server's own header naming, with the
/// declared digest, length, and body chosen by the caller.
fn write_rendition(
    stream: &mut impl Write,
    stage: &str,
    digest: &str,
    declared: usize,
    sent: &[u8],
) {
    let display_transform = if stage == "film" {
        "display-transform-v1"
    } else {
        "sRGB"
    };
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {declared}\r\nx-content-type-options: nosniff\r\nslipstream-edit-preview-photo-id: {PHOTO_ID}\r\nslipstream-edit-preview-stage: {stage}\r\nslipstream-edit-preview-settings: current\r\nslipstream-edit-preview-width: 8\r\nslipstream-edit-preview-height: 4\r\nslipstream-edit-preview-sha256: {digest}\r\nslipstream-edit-preview-source-revision: {}\r\nslipstream-edit-preview-recipe-version: recipe-7\r\nslipstream-edit-preview-display-transform: {display_transform}\r\nslipstream-edit-preview-expires-at: {EXPIRES_AT}\r\nConnection: close\r\n\r\n",
        hex_encode(SOURCE_REVISION)
    );
    let _ = stream.write_all(sent);
}

/// The closed handshake body the service itself serves.
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

fn handshake_without_removal_limit() -> Value {
    let mut body = capabilities_body();
    body["limits"]
        .as_object_mut()
        .unwrap()
        .remove("removalPhotoIdsMaximum");
    body
}

/// The service's `recipe_conflict` refusal carrying the current facts the
/// caller needs to recover without a second read.
fn conflict_body() -> Value {
    json!({"error": {
        "code": "recipe_conflict",
        "message": "The expected recipe revision is no longer current; decide again from the carried facts.",
        "effect": "none",
        "details": {
            "currentSourceRevision": SOURCE_REVISION,
            "currentRecipeVersion": CURRENT_RECIPE_VERSION
        }
    }})
}

// ---------------------------------------------------------------- writes

/// A capability report that omits one closed limit is an incomplete report:
/// the CLI keeps the versions the service did publish, names the missing
/// field, and never reaches the operational write.
#[tokio::test]
async fn recipe_save_writes_nothing_when_the_capability_report_is_incomplete() {
    let base = temp_base("capability-shape");
    let input = write_input(&base, "save.json", &save_document().to_string());
    let service = fake_service(vec![Step::CapabilitiesWithoutRemovalLimit]);
    let (exit, envelope) = command(
        &service.url,
        &["photos", "recipe", "save", PHOTO_ID, "--input", &input],
    )
    .await;
    assert_eq!(exit, 6);
    assert_eq!(envelope["error"]["code"], "incompatible_server");
    assert_eq!(
        envelope["error"]["details"]["supportedContractVersions"],
        json!([1])
    );
    assert_eq!(
        envelope["error"]["details"]["field"],
        "limits.removalPhotoIdsMaximum"
    );
    assert_eq!(
        envelope["error"]["details"]["reason"],
        "missing-or-invalid-field"
    );
    let (connections, requests) = service.finish();
    assert_eq!(connections, 1, "only the handshake may run");
    assert_eq!(requests.len(), 1, "no operational write may follow");
    assert!(
        requests[0]
            .request_line
            .starts_with("GET /api/capabilities")
    );
    assert!(requests[0].body.is_empty());
    fs::remove_dir_all(base).unwrap();
}

/// The save submits the caller's complete guarded document unchanged, and a
/// confirmed conflict is exit 4 with the service's current facts.
#[tokio::test]
async fn recipe_save_submits_the_callers_exact_guards_and_maps_the_conflict() {
    let base = temp_base("recipe-conflict");
    let document = save_document();
    let input = write_input(&base, "save.json", &document.to_string());
    let service = fake_service(vec![Step::Capabilities, Step::RecipeConflict]);
    let (exit, envelope) = command(
        &service.url,
        &["photos", "recipe", "save", PHOTO_ID, "--input", &input],
    )
    .await;
    assert_eq!(exit, 4);
    assert_eq!(envelope["error"]["code"], "recipe_conflict");
    assert_eq!(envelope["error"]["effect"], "none");
    assert_eq!(
        envelope["error"]["details"]["currentRecipeVersion"],
        CURRENT_RECIPE_VERSION
    );
    assert_eq!(
        envelope["error"]["details"]["currentSourceRevision"],
        SOURCE_REVISION
    );
    let (connections, requests) = service.finish();
    assert_eq!(connections, 2);
    assert_eq!(requests.len(), 2);
    let save = &requests[1];
    assert_eq!(
        save.request_line,
        format!("POST /api/photos/{PHOTO_ID}/edit-recipe HTTP/1.1")
    );
    // The exact submitted body: the CLI neither completed nor rewrote any
    // guard behind the caller's back.
    assert_eq!(
        serde_json::from_slice::<Value>(&save.body).unwrap(),
        document
    );
    fs::remove_dir_all(base).unwrap();
}

/// Duplicate, unknown, and nested-malformed documents are refused with exit 2
/// before any network access: nothing listens, so a locally valid document
/// on the same address fails as transport instead.
#[tokio::test]
async fn malformed_recipe_documents_are_refused_before_any_network() {
    let base = temp_base("input-shapes");
    // Nothing listens here; any network attempt would fail as transport.
    let dead = "https://127.0.0.1:9";
    let valid = save_document();
    let as_shot = json!({"mode": "as-shot"}).to_string();
    let with = |request_id: &str, version: Value, source: &str, white_balance: &str| {
        json!({
            "requestId": request_id,
            "expectedRecipeVersion": version,
            "expectedSourceRevision": source,
            "settings": {"exposureEv": 0.25, "whiteBalance": serde_json::from_str::<Value>(white_balance).unwrap()},
        })
        .to_string()
    };
    let cases = [
        // A duplicated key anywhere, including inside whiteBalance, is a
        // decoder refusal; the raw text keeps the duplicate the json!
        // macro would collapse.
        (
            "duplicate-request-id.json",
            r#"{"requestId":"save-001","requestId":"save-002","expectedRecipeVersion":null,"expectedSourceRevision":"source-3","settings":{"exposureEv":0.25,"whiteBalance":{"mode":"as-shot"}}}"#.to_owned(),
            "input",
        ),
        (
            "nested-duplicate.json",
            r#"{"requestId":"save-001","expectedRecipeVersion":null,"expectedSourceRevision":"source-3","settings":{"exposureEv":0.25,"whiteBalance":{"mode":"temperature-tint","temperatureKelvin":6500,"temperatureKelvin":6501,"tintMilli":0}}}"#.to_owned(),
            "input",
        ),
        (
            "unknown-top-key.json",
            json!({
                "requestId": "save-001",
                "expectedRecipeVersion": null,
                "expectedSourceRevision": "source-3",
                "settings": {"exposureEv": 0.25, "whiteBalance": {"mode": "as-shot"}},
                "extra": 1,
            })
            .to_string(),
            "input",
        ),
        (
            "trailing.json",
            with("save-001", Value::Null, "source-3", &as_shot) + " trailing",
            "input",
        ),
        (
            "unknown-settings-key.json",
            json!({
                "requestId": "save-001",
                "expectedRecipeVersion": null,
                "expectedSourceRevision": "source-3",
                "settings": {"exposureEv": 0.25, "whiteBalance": {"mode": "as-shot"}, "extra": 1},
            })
            .to_string(),
            "input",
        ),
        (
            "omitted-guard.json",
            json!({
                "requestId": "save-001",
                "expectedSourceRevision": "source-3",
                "settings": {"exposureEv": 0.25, "whiteBalance": {"mode": "as-shot"}},
            })
            .to_string(),
            "input",
        ),
        ("array.json", "[]".to_owned(), "input"),
        (
            "illformed-request-id.json",
            with("save 001", Value::Null, "source-3", &as_shot),
            "requestId",
        ),
        (
            "empty-recipe-guard.json",
            with("save-001", json!(""), "source-3", &as_shot),
            "expectedRecipeVersion",
        ),
        (
            "empty-source-guard.json",
            with("save-001", Value::Null, "", &as_shot),
            "expectedSourceRevision",
        ),
        (
            "missing-tint.json",
            with(
                "save-001",
                Value::Null,
                "source-3",
                r#"{"mode":"temperature-tint","temperatureKelvin":6500}"#,
            ),
            "whiteBalance",
        ),
        (
            "as-shot-payload.json",
            with(
                "save-001",
                Value::Null,
                "source-3",
                r#"{"mode":"as-shot","tintMilli":0}"#,
            ),
            "whiteBalance",
        ),
    ];
    for (name, content, argument) in cases {
        let input = write_input(&base, name, &content);
        let (exit, envelope) = command(
            dead,
            &["photos", "recipe", "save", PHOTO_ID, "--input", &input],
        )
        .await;
        assert_eq!(exit, 2, "for {name}");
        assert_eq!(envelope["error"]["code"], "invalid_input", "for {name}");
        assert_eq!(envelope["error"]["effect"], "none", "for {name}");
        assert_eq!(
            envelope["error"]["details"]["argument"], argument,
            "for {name}"
        );
    }

    // The rebind document refuses duplicates the same way, and a locally
    // valid document of either write reaches the dead transport.
    let rebind = write_input(
        &base,
        "rebind-duplicate.json",
        r#"{"requestId":"rebind-1","requestId":"rebind-2","expectedRecipeVersion":"recipe-7","newSourceRevision":"source-9"}"#,
    );
    let (exit, envelope) = command(
        dead,
        &["photos", "recipe", "rebind", PHOTO_ID, "--input", &rebind],
    )
    .await;
    assert_eq!(exit, 2);
    assert_eq!(envelope["error"]["code"], "invalid_input");
    assert_eq!(envelope["error"]["details"]["argument"], "input");

    let valid_input = write_input(&base, "valid.json", &valid.to_string());
    let (exit, envelope) = command(
        dead,
        &[
            "photos",
            "recipe",
            "save",
            PHOTO_ID,
            "--input",
            &valid_input,
        ],
    )
    .await;
    assert_eq!(exit, 6);
    assert_eq!(envelope["error"]["code"], "transport_failed");
    assert_eq!(
        envelope["error"]["details"]["operation"],
        "photos-recipe-save"
    );

    let valid_rebind = write_input(
        &base,
        "valid-rebind.json",
        &json!({
            "requestId": "rebind-1",
            "expectedRecipeVersion": "recipe-7",
            "newSourceRevision": "source-9",
        })
        .to_string(),
    );
    let (exit, envelope) = command(
        dead,
        &[
            "photos",
            "recipe",
            "rebind",
            PHOTO_ID,
            "--input",
            &valid_rebind,
        ],
    )
    .await;
    assert_eq!(exit, 6);
    assert_eq!(envelope["error"]["code"], "transport_failed");
    assert_eq!(
        envelope["error"]["details"]["operation"],
        "photos-recipe-rebind"
    );

    fs::remove_dir_all(base).unwrap();
}

/// A save whose response is lost after admission is exit 7 with an unknown
/// effect, and the CLI never submits the write a second time.
#[tokio::test]
async fn a_lost_recipe_save_response_is_an_unknown_outcome_without_retry() {
    let base = temp_base("lost-save");
    let input = write_input(&base, "save.json", &save_document().to_string());
    let service = fake_service(vec![Step::Capabilities, Step::LoseAfterRead]);
    let (exit, envelope) = command(
        &service.url,
        &["photos", "recipe", "save", PHOTO_ID, "--input", &input],
    )
    .await;
    assert_eq!(exit, 7);
    assert_eq!(envelope["error"]["code"], "outcome_unknown");
    assert_eq!(envelope["error"]["effect"], "unknown");
    assert_eq!(
        envelope["error"]["details"]["operation"],
        "photos-recipe-save"
    );
    assert_eq!(envelope["error"]["details"]["photoIds"], json!([PHOTO_ID]));
    let (connections, requests) = service.finish();
    assert_eq!(connections, 2, "the handshake plus exactly one submission");
    assert_eq!(requests.len(), 2, "the write is never retried");
    fs::remove_dir_all(base).unwrap();
}

// ---------------------------------------------------------------- previews

/// An accepted render intent is a successful pending report: no file, no
/// staging leftover, and the one read of the route.
#[tokio::test]
async fn an_accepted_edit_preview_intent_leaves_the_destination_untouched() {
    let base = temp_base("accepted-preview");
    let destination = base.join("edit-preview.jpg");
    let service = fake_service(vec![Step::Capabilities, Step::AdmitPreview]);
    let (exit, envelope) = command(
        &service.url,
        &[
            "photos",
            "edit-preview",
            PHOTO_ID,
            "--file",
            destination.to_str().unwrap(),
            "--stage",
            "develop",
        ],
    )
    .await;
    assert_eq!(exit, 0);
    let data = &envelope["data"];
    assert_eq!(data["photoId"], PHOTO_ID);
    assert_eq!(data["stage"], "develop");
    assert_eq!(data["settings"], "current");
    assert_eq!(data["state"], "queued");
    assert_eq!(data["fileCommitted"], false);
    assert_eq!(
        data["webUrl"],
        format!("{}/?photoId={PHOTO_ID}", service.url)
    );
    assert!(!destination.exists());
    assert_eq!(fs::read_dir(&base).unwrap().count(), 0);
    let (connections, requests) = service.finish();
    assert_eq!(connections, 2);
    assert_eq!(
        requests[1].request_line,
        format!("GET /api/photos/{PHOTO_ID}/edit-preview/develop?settings=current HTTP/1.1")
    );
    fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn indeterminate_preview_admission_preserves_uncertainty_without_retry() {
    let base = temp_base("indeterminate-preview");
    let destination = base.join("edit-preview.jpg");
    let service = fake_service(vec![Step::Capabilities, Step::IndeterminatePreview]);
    let (exit, envelope) = command(
        &service.url,
        &[
            "photos",
            "edit-preview",
            PHOTO_ID,
            "--file",
            destination.to_str().unwrap(),
            "--stage",
            "develop",
        ],
    )
    .await;
    assert_eq!(exit, 7);
    assert_eq!(envelope["error"]["code"], "outcome_unknown");
    assert_eq!(envelope["error"]["effect"], "unknown");
    assert_eq!(
        envelope["error"]["details"]["operation"],
        "photos-edit-preview"
    );
    assert_eq!(envelope["error"]["details"]["photoIds"], json!([PHOTO_ID]));
    assert!(!destination.exists());
    let (connections, _) = service.finish();
    assert_eq!(connections, 2);
    fs::remove_dir_all(base).unwrap();
}

/// A ready rendition is published only after every header, the digest, and
/// the JPEG structure check out, and the directory holds exactly the one
/// published file.
#[tokio::test]
async fn a_ready_edit_preview_is_verified_and_published_once() {
    let rendition = Arc::new(jpeg_bytes());
    let base = temp_base("ready-preview");
    let destination = base.join("develop.jpg");
    let service = fake_service_with(
        vec![Step::Capabilities, Step::ServeRendition],
        Arc::clone(&rendition),
    );
    let (exit, envelope) = command(
        &service.url,
        &[
            "photos",
            "edit-preview",
            PHOTO_ID,
            "--file",
            destination.to_str().unwrap(),
            "--stage",
            "develop",
        ],
    )
    .await;
    assert_eq!(exit, 0);
    let data = &envelope["data"];
    assert_eq!(data["state"], "ready");
    assert_eq!(data["sourceRevision"], SOURCE_REVISION);
    assert_eq!(data["recipeVersion"], "recipe-7");
    assert_eq!(data["displayTransform"], "sRGB");
    assert_eq!(data["contentType"], "image/jpeg");
    assert_eq!(data["width"], 8);
    assert_eq!(data["height"], 4);
    assert_eq!(data["byteLength"].as_u64(), Some(rendition.len() as u64));
    assert_eq!(data["sha256"], sha256_hex(&rendition));
    assert_eq!(data["expiresAt"], EXPIRES_AT);
    assert_eq!(data["path"], destination.to_str().unwrap());
    assert_eq!(data["fileCommitted"], true);
    assert_eq!(fs::read(&destination).unwrap(), *rendition);
    assert!(image::load_from_memory(&rendition).is_ok());
    let entries = fs::read_dir(&base).unwrap().count();
    assert_eq!(entries, 1, "no staging leftover beside the publication");
    let (connections, requests) = service.finish();
    assert_eq!(connections, 2);
    assert_eq!(requests.len(), 2);
    fs::remove_dir_all(base).unwrap();
}
/// Film uses the same source/recipe identity and framed metadata validation as
/// Develop, while selecting the film route and its display transform.
#[tokio::test]
async fn a_ready_film_edit_preview_is_verified_and_published() {
    let rendition = Arc::new(jpeg_bytes());
    let base = temp_base("ready-film-preview");
    let destination = base.join("film.jpg");
    let service = fake_service_with(
        vec![Step::Capabilities, Step::ServeFilmRendition],
        Arc::clone(&rendition),
    );
    let (exit, envelope) = command(
        &service.url,
        &[
            "photos",
            "edit-preview",
            PHOTO_ID,
            "--file",
            destination.to_str().unwrap(),
            "--stage",
            "film",
        ],
    )
    .await;
    assert_eq!(exit, 0);
    let data = &envelope["data"];
    assert_eq!(data["stage"], "film");
    assert_eq!(data["sourceRevision"], SOURCE_REVISION);
    assert_eq!(data["recipeVersion"], "recipe-7");
    assert_eq!(data["displayTransform"], "display-transform-v1");
    assert_eq!(data["sha256"], sha256_hex(&rendition));
    assert_eq!(data["fileCommitted"], true);
    assert_eq!(fs::read(&destination).unwrap(), *rendition);
    let (connections, requests) = service.finish();
    assert_eq!(connections, 2);
    assert_eq!(
        requests[1].request_line,
        format!("GET /api/photos/{PHOTO_ID}/edit-preview/film?settings=current HTTP/1.1")
    );
    fs::remove_dir_all(base).unwrap();
}

/// An existing destination is refused before any network access, and the
/// file on disk is never replaced.
#[tokio::test]
async fn an_existing_edit_preview_destination_is_refused_before_any_network() {
    let base = temp_base("existing-destination");
    let destination = base.join("develop.jpg");
    fs::write(&destination, b"sentinel").unwrap();
    let service = fake_service(vec![Step::Capabilities, Step::AdmitPreview]);
    let (exit, envelope) = command(
        &service.url,
        &[
            "photos",
            "edit-preview",
            PHOTO_ID,
            "--file",
            destination.to_str().unwrap(),
            "--stage",
            "develop",
        ],
    )
    .await;
    assert_eq!(exit, 2);
    assert_eq!(envelope["error"]["code"], "invalid_input");
    assert_eq!(envelope["error"]["effect"], "none");
    assert_eq!(envelope["error"]["details"]["argument"], "file");
    assert_eq!(fs::read(&destination).unwrap(), b"sentinel".to_vec());
    let (connections, requests) = service.finish();
    assert_eq!(connections, 0, "the refusal happens before the handshake");
    assert!(requests.is_empty());
    fs::remove_dir_all(base).unwrap();
}

/// A rendition whose bytes do not match the receipt, and a transfer that
/// stops short of its declared length, are transport failures that leave
/// no file and no staging leftover.
#[tokio::test]
async fn unverifiable_edit_preview_transfers_publish_nothing() {
    let rendition = Arc::new(jpeg_bytes());
    for (name, step) in [
        ("wrong-digest", Step::ServeWrongDigest),
        ("short-transfer", Step::ServeShortTransfer),
    ] {
        let base = temp_base(name);
        let destination = base.join("develop.jpg");
        let service = fake_service_with(vec![Step::Capabilities, step], Arc::clone(&rendition));
        let (exit, envelope) = command(
            &service.url,
            &[
                "photos",
                "edit-preview",
                PHOTO_ID,
                "--file",
                destination.to_str().unwrap(),
                "--stage",
                "develop",
            ],
        )
        .await;
        assert_eq!(exit, 6, "for {name}");
        assert_eq!(envelope["error"]["code"], "transport_failed", "for {name}");
        assert_eq!(envelope["error"]["effect"], "none", "for {name}");
        assert_eq!(
            envelope["error"]["details"]["operation"], "photos-edit-preview",
            "for {name}"
        );
        assert!(!destination.exists(), "for {name}");
        assert_eq!(fs::read_dir(&base).unwrap().count(), 0, "for {name}");
        let (connections, requests) = service.finish();
        assert_eq!(connections, 2, "for {name}");
        assert_eq!(requests.len(), 2, "for {name}");
        fs::remove_dir_all(base).unwrap();
    }
}

// ---------------------------------------------------------------- real service

/// The two Originals of the shared fixture: JPEG sources only, with
/// processing unconfigured, so the deployment answers deterministically
/// without a launcher.
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

async fn wait_until_idle(server: &str) {
    let client = reqwest::Client::builder()
        .add_root_certificate(common::test_certificate())
        .build()
        .unwrap();
    for _ in 0..400 {
        let response = client
            .get(format!("{server}/api/status"))
            .header("Slipstream-CLI-Contract", "1")
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

/// Without a processing configuration the service reports the closed
/// `disabled` condition, and the CLI passes the report through with its
/// profiles, null identities, and unavailable stages.
#[tokio::test]
async fn the_real_service_capability_report_is_disabled_without_processing() {
    let (base, config) = real_service_fixture();
    let server = common::start_authenticated_server(config).await;
    wait_until_idle(&server.url).await;
    let (exit, report) = command(&server.url, &["processing", "capability"]).await;
    assert_eq!(exit, 0);
    let data = &report["data"];
    assert_eq!(data["state"], "disabled");
    assert_eq!(data["bundleId"], Value::Null);
    assert_eq!(data["incarnation"], Value::Null);
    assert_eq!(data["stages"]["develop"], "unavailable");
    assert_eq!(data["stages"]["film"], "unavailable");
    let profiles = data["profiles"].as_array().unwrap();
    assert!(!profiles.is_empty());
    for profile in profiles {
        assert!(!profile["profileId"].as_str().is_some_and(str::is_empty));
        let modes = profile["whiteBalanceModes"].as_array().unwrap();
        assert!(!modes.is_empty());
        assert!(
            modes
                .iter()
                .all(|mode| mode.as_str().is_some_and(|mode| !mode.is_empty()))
        );
        assert_eq!(profile["whiteBalanceRanges"], Value::Null);
    }
    let exposure = &data["exposure"];
    assert!(exposure["minimumEv"].as_f64().unwrap() < exposure["maximumEv"].as_f64().unwrap());
    assert!(exposure["stepEv"].as_f64().unwrap() > 0.0);
    server.close().await.unwrap();
    fs::remove_dir_all(base).unwrap();
}

/// A JPEG source class has no approved development profile: the real
/// service refuses the Edit Preview read and the CLI maps the confirmed
/// refusal onto exit 2 without creating the destination.
#[tokio::test]
async fn the_real_service_refuses_an_edit_preview_of_a_jpeg_source() {
    let (base, config) = real_service_fixture();
    let server = common::start_authenticated_server(config).await;
    wait_until_idle(&server.url).await;
    let (exit, page) = command(&server.url, &["photos", "list", "--limit", "60"]).await;
    assert_eq!(exit, 0);
    let photo_id = page["data"]["items"][0]["id"].as_str().unwrap().to_owned();
    let destination = base.join("refused.jpg");
    let (exit, refusal) = command(
        &server.url,
        &[
            "photos",
            "edit-preview",
            &photo_id,
            "--file",
            destination.to_str().unwrap(),
            "--stage",
            "develop",
        ],
    )
    .await;
    assert_eq!(exit, 2, "{refusal}");
    assert_eq!(refusal["error"]["code"], "unsupported_photo");
    assert_eq!(refusal["error"]["effect"], "none");
    assert_eq!(refusal["error"]["details"]["photoId"], photo_id);
    assert!(!destination.exists());
    server.close().await.unwrap();
    fs::remove_dir_all(base).unwrap();
}
