//! Consumer-visible composable Processing Module, recipe, Preview, and Export
//! contracts exercised through the actual CLI and an authenticated TLS service.

use image::ImageEncoder;
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

/// A deterministic 8x4 PNG verified by digest and complete image structure.
fn rendition_bytes() -> Vec<u8> {
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&[50; 8 * 4 * 3], 8, 4, image::ExtendedColorType::Rgb8)
        .unwrap();
    png
}

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
    /// Answer the composable save with the service's `recipe_conflict`
    /// facts.
    ComposableRecipeConflict,
    /// Answer the Edit Preview read with the pending admission.
    AdmitPreview,
    IndeterminatePreview,
    /// Serve a complete, correctly described ready Develop rendition.
    ServeRendition,
    /// Serve the rendition with a digest that names other bytes.
    ServeWrongDigest,
    /// Announce more body bytes than the transfer sends, then close.
    ServeShortTransfer,
    /// Serve a caller-selected JSON contract response.
    Json(u16, Value),
    HistoricalBytes(Value, Vec<u8>),
    /// Serve the Artifact bytes route with a caller-shaped reply.
    ArtifactBytes(ArtifactReply),
}

/// One scripted reply of the Artifact bytes route. The published facts are
/// the provenance record the response headers must repeat; `bytes` is the
/// object, of which `sent` may be short for an interrupted transfer.
struct ArtifactReply {
    facts: Value,
    bytes: Vec<u8>,
    sent: Option<usize>,
    declared: Option<usize>,
    honor_range: bool,
    header_override: Option<(String, String)>,
    pace: Option<(usize, Duration)>,
    stall: Option<Duration>,
}

impl ArtifactReply {
    /// The complete object with the published facts.
    fn new(facts: Value, bytes: Vec<u8>) -> Self {
        Self {
            facts,
            bytes,
            sent: None,
            declared: None,
            honor_range: false,
            header_override: None,
            pace: None,
            stall: None,
        }
    }

    /// Announce the full published length but write only `sent` bytes, as a
    /// cut connection does.
    fn interrupted(mut self, sent: usize) -> Self {
        self.sent = Some(sent);
        self
    }

    /// Advertise a length other than the bytes actually written.
    fn declared(mut self, declared: usize) -> Self {
        self.declared = Some(declared);
        self
    }

    /// Answer the request's single range when its validator matches.
    fn honor_range(mut self) -> Self {
        self.honor_range = true;
        self
    }

    /// Rewrite one repeated provenance header after framing.
    fn header(mut self, name: &str, value: &str) -> Self {
        self.header_override = Some((name.to_owned(), value.to_owned()));
        self
    }

    /// Write the body in fixed chunks with a delay after each one.
    fn pace(mut self, chunk: usize, delay: Duration) -> Self {
        self.pace = Some((chunk, delay));
        self
    }

    /// Keep the connection open this long after the written bytes, as a
    /// stalled peer does.
    fn stall(mut self, hold: Duration) -> Self {
        self.stall = Some(hold);
        self
    }
}

#[derive(Clone, Debug)]
struct RecordedRequest {
    request_line: String,
    body: Vec<u8>,
    range: Option<String>,
    if_range: Option<String>,
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
    fake_service_with(steps, Arc::new(rendition_bytes()))
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
                        let header = |name: &str| {
                            head.lines().find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.trim()
                                    .eq_ignore_ascii_case(name)
                                    .then(|| value.trim().to_owned())
                            })
                        };
                        sender
                            .send(RecordedRequest {
                                request_line: head.lines().next().unwrap_or_default().to_owned(),
                                body,
                                range: header("range"),
                                if_range: header("if-range"),
                            })
                            .expect("the test holds the recording receiver");
                        if let Some(step) = steps.get(next_step) {
                            answer(&mut stream, step, &rendition, &head);
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

fn answer(stream: &mut impl Write, step: &Step, rendition: &[u8], request_head: &str) {
    match step {
        Step::Capabilities => write_json_response(stream, 200, "OK", &capabilities_body()),
        Step::CapabilitiesWithoutRemovalLimit => {
            write_json_response(stream, 200, "OK", &handshake_without_removal_limit())
        }
        // The request was read; dropping the stream loses the response.
        Step::LoseAfterRead => {}
        Step::ComposableRecipeConflict => {
            write_json_response(stream, 409, "Conflict", &composable_conflict_body())
        }
        Step::AdmitPreview => write_json_response(
            stream,
            202,
            "Accepted",
            &json!({"state": "queued", "stepId": "develop-1"}),
        ),
        Step::IndeterminatePreview => write_json_response(
            stream,
            500,
            "Internal Server Error",
            &json!({"error":{"code":"outcome_unknown", "message":"The render admission outcome is unknown.", "effect":"none", "details":{"stepId":"develop-1"}}}),
        ),
        Step::ServeRendition => write_rendition(
            stream,
            "develop",
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
        Step::Json(status, report) => write_json_response(stream, *status, "Response", report),
        Step::HistoricalBytes(artifact, bytes) => {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
                artifact["contentType"].as_str().unwrap(),
                bytes.len()
            );
            for (header, key) in [
                ("export-id", "exportId"),
                ("target", "target"),
                ("stage", "stage"),
                ("content-type", "contentType"),
                ("filename", "filename"),
                ("orientation", "orientation"),
                ("sample-format", "sampleFormat"),
                ("color-space", "colorSpace"),
                ("profile-identity", "profileIdentity"),
                ("sha256", "sha256"),
                ("expires-at", "expiresAt"),
                ("width", "width"),
                ("height", "height"),
                ("byte-length", "byteLength"),
                ("icc-embedded", "iccEmbedded"),
            ] {
                let value = artifact[key]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| artifact[key].to_string());
                let _ = write!(stream, "slipstream-artifact-{header}: {value}\r\n");
            }
            let _ = stream.write_all(b"\r\n");
            let _ = stream.write_all(bytes);
        }
        Step::ArtifactBytes(reply) => write_artifact_bytes(stream, reply, request_head),
    }
}

/// The Artifact bytes response in the service's own header naming: the
/// published facts repeated field for field, one optional contiguous range,
/// and a body that a caller may cut short or pace.
fn write_artifact_bytes(stream: &mut impl Write, reply: &ArtifactReply, request_head: &str) {
    let facts = &reply.facts;
    let total = facts["byteLength"].as_u64().expect("published byte length");
    let sha256 = facts["sha256"].as_str().expect("published digest");
    let etag = format!("\"{sha256}\"");
    let header = |name: &str| {
        request_head.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.trim()
                .eq_ignore_ascii_case(name)
                .then(|| value.trim().to_owned())
        })
    };
    let content_type = match facts["outputContract"]["format"]
        .as_str()
        .unwrap_or_default()
        .strip_prefix("image/")
        .unwrap_or_else(|| {
            facts["outputContract"]["format"]
                .as_str()
                .unwrap_or_default()
        }) {
        "tiff" => "image/tiff",
        _ => "image/jpeg",
    };
    let mut status = 200_u16;
    let mut content_range = None;
    let mut start = 0_usize;
    let mut end = reply.sent.unwrap_or(reply.bytes.len());
    let mut advertised = reply.declared.unwrap_or(total as usize);
    if reply.honor_range
        && let Some(value) = header("range")
    {
        let matches = header("if-range").as_deref() == Some(etag.as_str());
        let requested = value
            .strip_prefix("bytes=")
            .filter(|value| !value.contains(','))
            .and_then(|value| value.split_once('-'))
            .and_then(|(start, _)| start.parse::<u64>().ok());
        if matches {
            match requested {
                Some(offset) if offset < total => {
                    start = offset as usize;
                    end = reply.bytes.len();
                    advertised = end - start;
                    status = 206;
                    content_range = Some(format!("bytes {offset}-{}/{total}", total - 1));
                }
                _ => {
                    let _ = write!(
                        stream,
                        "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: 0\r\nContent-Range: bytes */{total}\r\nConnection: close\r\n\r\n"
                    );
                    return;
                }
            }
        }
    }
    let mut head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: {content_type}\r\nContent-Length: {advertised}\r\nETag: {etag}\r\nAccept-Ranges: bytes\r\n",
        if status == 206 {
            "Partial Content"
        } else {
            "OK"
        }
    );
    if let Some(content_range) = &content_range {
        head.push_str(&format!("Content-Range: {content_range}\r\n"));
    }
    for (name, value) in [
        ("id", facts["artifactId"].clone()),
        ("photo-id", facts["photoId"].clone()),
        ("filename", facts["filename"].clone()),
        ("step-id", facts["stepId"].clone()),
        ("module", facts["module"].clone()),
        (
            "adapter-schema-version",
            facts["adapterSchemaVersion"].clone(),
        ),
        ("bundle-id", facts["bundleId"].clone()),
        (
            "width",
            facts["outputContract"]["geometry"]["width"].clone(),
        ),
        (
            "height",
            facts["outputContract"]["geometry"]["height"].clone(),
        ),
        ("byte-length", facts["byteLength"].clone()),
        ("sha256", facts["sha256"].clone()),
    ] {
        let value = value
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| value.to_string());
        let line_name = format!("slipstream-artifact-{name}");
        let value = match &reply.header_override {
            Some((override_name, override_value)) if *override_name == line_name => {
                override_value.clone()
            }
            _ => value,
        };
        head.push_str(&format!("{line_name}: {value}\r\n"));
    }
    head.push_str("Connection: close\r\n\r\n");
    let _ = stream.write_all(head.as_bytes());
    let body = &reply.bytes[start..end];
    match reply.pace {
        None => {
            let _ = stream.write_all(body);
        }
        Some((chunk, delay)) => {
            for chunk in body.chunks(chunk.max(1)) {
                if stream.write_all(chunk).is_err() {
                    return;
                }
                let _ = stream.flush();
                std::thread::sleep(delay);
            }
        }
    }
    if let Some(hold) = reply.stall {
        std::thread::sleep(hold);
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
    _module: &str,
    digest: &str,
    declared: usize,
    sent: &[u8],
) {
    let identity = "a".repeat(64);
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: image/png\r\nContent-Length: {declared}\r\nslipstream-processing-preview-photo-id: {PHOTO_ID}\r\nslipstream-processing-preview-step-id: develop-1\r\nslipstream-processing-preview-comparison: current\r\nslipstream-processing-preview-input-sha256: {identity}\r\nslipstream-processing-preview-input-byte-length: 24\r\nslipstream-processing-preview-width: 8\r\nslipstream-processing-preview-height: 4\r\nslipstream-processing-preview-sha256: {digest}\r\nslipstream-processing-preview-source-revision: {}\r\nslipstream-processing-preview-recipe-revision: recipe-7\r\nslipstream-processing-preview-module: darktable\r\nslipstream-processing-preview-adapter-schema-version: darktable-adapter-1:darktable-params-1\r\nslipstream-processing-preview-parameter-digest: {identity}\r\nslipstream-processing-preview-output-contract: {identity}\r\nslipstream-processing-preview-display-conversion: display-transform-v1\r\nslipstream-processing-preview-identity: {identity}\r\nConnection: close\r\n\r\n",
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

/// The composable save document: one darktable step bound to the guarded
/// Original, with a module-owned parameter tree the CLI must neither
/// flatten nor rewrite.
fn composable_save_document() -> Value {
    json!({
        "requestId": "composable-001",
        "expectedRecipeRevision": Value::Null,
        "expectedSourceRevision": SOURCE_REVISION,
        "currentStepId": "develop-1",
        "steps": [{
            "stepId": "develop-1",
            "module": "darktable",
            "input": {
                "kind": "original",
                "photoId": PHOTO_ID,
                "sourceRevision": SOURCE_REVISION,
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
}

/// The service's composable `recipe_conflict` refusal carrying the retained
/// recipe the caller needs to recover without a second read.
fn composable_conflict_body() -> Value {
    json!({"error": {
        "code": "recipe_conflict",
        "message": "The expected composable recipe revision is no longer current.",
        "effect": "none",
        "details": {
            "photoId": PHOTO_ID,
            "revision": CURRENT_RECIPE_VERSION,
            "sourceRevision": SOURCE_REVISION,
            "currentStepId": "develop-1",
            "steps": [{
                "stepId": "develop-1",
                "module": "darktable",
                "input": {"kind": "original", "photoId": PHOTO_ID, "sourceRevision": SOURCE_REVISION},
                "parameters": {"schemaVersion": "darktable-params-1", "tree": {"stack": []}},
            }],
        },
    }})
}

// ---------------------------------------------------------------- writes

/// A capability report that omits one closed limit is an incomplete report:
/// the CLI keeps the versions the service did publish, names the missing
/// field, and never reaches the operational write.
#[tokio::test]
async fn recipe_save_writes_nothing_when_the_capability_report_is_incomplete() {
    let base = temp_base("capability-shape");
    let input = write_input(&base, "save.json", &composable_save_document().to_string());
    let service = fake_service(vec![Step::CapabilitiesWithoutRemovalLimit]);
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

#[path = "development_service/artifact_download.rs"]
mod artifact_download;
#[path = "development_service/composable.rs"]
mod composable;
// ---------------------------------------------------------------- previews

/// An accepted render intent is a successful pending report: no file, no
/// staging leftover, and the one read of the route.
#[tokio::test]
async fn an_accepted_processing_preview_intent_leaves_the_destination_untouched() {
    let base = temp_base("accepted-preview");
    let destination = base.join("edit-preview.jpg");
    let service = fake_service(vec![Step::Capabilities, Step::AdmitPreview]);
    let (exit, envelope) = command(
        &service.url,
        &[
            "photos",
            "processing-preview",
            PHOTO_ID,
            "--file",
            destination.to_str().unwrap(),
            "--step",
            "develop-1",
        ],
    )
    .await;
    assert_eq!(exit, 0);
    let data = &envelope["data"];
    assert_eq!(data["photoId"], PHOTO_ID);
    assert_eq!(data["stepId"], "develop-1");
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
        format!("GET /api/photos/{PHOTO_ID}/processing-preview/develop-1 HTTP/1.1")
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
            "processing-preview",
            PHOTO_ID,
            "--file",
            destination.to_str().unwrap(),
            "--step",
            "develop-1",
        ],
    )
    .await;
    assert_eq!(exit, 7);
    assert_eq!(envelope["error"]["code"], "outcome_unknown");
    assert_eq!(envelope["error"]["effect"], "unknown");
    assert_eq!(
        envelope["error"]["details"]["operation"],
        "photos-processing-preview"
    );
    assert_eq!(envelope["error"]["details"]["photoIds"], json!([PHOTO_ID]));
    assert!(!destination.exists());
    let (connections, _) = service.finish();
    assert_eq!(connections, 2);
    fs::remove_dir_all(base).unwrap();
}

/// A ready rendition is published only after every header, the digest, and
/// the PNG structure check out, and the directory holds exactly the one
/// published file.
#[tokio::test]
async fn a_ready_processing_preview_is_verified_and_published_once() {
    let rendition = Arc::new(rendition_bytes());
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
            "processing-preview",
            PHOTO_ID,
            "--file",
            destination.to_str().unwrap(),
            "--step",
            "develop-1",
        ],
    )
    .await;
    assert_eq!(exit, 0);
    let data = &envelope["data"];
    assert_eq!(data["state"], "ready");
    assert_eq!(data["sourceRevision"], SOURCE_REVISION);
    assert_eq!(data["recipeRevision"], "recipe-7");
    assert_eq!(data["comparison"], "current");
    assert_eq!(data["inputSha256"], "a".repeat(64));
    assert_eq!(data["inputByteLength"], 24);
    assert_eq!(data["contentType"], "image/png");
    assert_eq!(data["width"], 8);
    assert_eq!(data["height"], 4);
    assert_eq!(data["byteLength"].as_u64(), Some(rendition.len() as u64));
    assert_eq!(data["sha256"], sha256_hex(&rendition));
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

/// An existing destination is refused before any network access, and the
/// file on disk is never replaced.
#[tokio::test]
async fn an_existing_processing_preview_destination_is_refused_before_any_network() {
    let base = temp_base("existing-destination");
    let destination = base.join("develop.jpg");
    fs::write(&destination, b"sentinel").unwrap();
    let service = fake_service(vec![Step::Capabilities, Step::AdmitPreview]);
    let (exit, envelope) = command(
        &service.url,
        &[
            "photos",
            "processing-preview",
            PHOTO_ID,
            "--file",
            destination.to_str().unwrap(),
            "--step",
            "develop-1",
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
async fn unverifiable_processing_preview_transfers_publish_nothing() {
    let rendition = Arc::new(rendition_bytes());
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
                "processing-preview",
                PHOTO_ID,
                "--file",
                destination.to_str().unwrap(),
                "--step",
                "develop-1",
            ],
        )
        .await;
        assert_eq!(exit, 6, "for {name}");
        assert_eq!(envelope["error"]["code"], "transport_failed", "for {name}");
        assert_eq!(envelope["error"]["effect"], "none", "for {name}");
        assert_eq!(
            envelope["error"]["details"]["operation"], "photos-processing-preview",
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

#[path = "development_service/real_service.rs"]
mod real_service;
