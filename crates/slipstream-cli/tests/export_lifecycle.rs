//! Executable tests of the Export lifecycle surface: cancellation and
//! retained-snapshot retry against a compact scripted TLS service, plus the
//! deterministic refusal the real service answers for an Export identity it
//! never recorded (processing unconfigured, so no Export can exist). Every
//! scenario runs the actual CLI binary, so exit codes, envelopes, request
//! order, and the absence of hidden retries are the consumer-visible
//! contract.

use serde_json::{Value, json};
use slipstream_server::Config;
mod common;
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
const EXPORT_ID: &str = "00000000-0000-4000-8000-0000000000e1";
const REQUEST_ID: &str = "retry-002";

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------- helpers

fn temp_base(name: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "slipstream-cli-export-lifecycle-{name}-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&base).unwrap();
    base
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

// ---------------------------------------------------------------- fake service

/// One scripted reply of the compact TLS service. Every CLI invocation makes
/// one connection per request, so connection N receives reply N.
enum Step {
    /// Answer the capabilities handshake with the closed body.
    Capabilities,
    /// Answer the Export inspection read with the given closed state.
    ReadExport(&'static str),
    /// Answer the inspection read with the service's `unknown_export`.
    UnknownExport,
    /// Read one mutation request, then close without responding.
    LoseAfterRead,
    /// Answer the retry with 202 echoing the Export and the given state.
    RetryAccepted(&'static str),
    /// Answer the retry with 202 naming another Export.
    RetryWrongExport,
    /// Answer the retry with one confirmed refusal body.
    RetryRefused(u16, &'static str),
    /// Answer the cancellation with the given terminal settlement.
    CancelSettled(&'static str),
    /// Answer the cancellation 200 without a terminal settlement.
    CancelUnsettled,
    /// Answer the cancellation with the service's unconfirmed settlement.
    CancelUncertain,
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
                            answer(&mut stream, step);
                        }
                        next_step += 1;
                        drop(stream);
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(2))
                    }
                    Err(_) => break,
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

fn answer(stream: &mut impl Write, step: &Step) {
    match step {
        Step::Capabilities => write_json_response(stream, 200, "OK", &capabilities_body()),
        Step::ReadExport(state) => write_json_response(stream, 200, "OK", &inspect_body(state)),
        Step::UnknownExport => write_json_response(
            stream,
            404,
            "Not Found",
            &export_error_body(
                "unknown_export",
                "The Export identity is unknown or expired",
            ),
        ),
        // The request was read; dropping the stream loses the response.
        Step::LoseAfterRead => {}
        Step::RetryAccepted(state) => write_json_response(
            stream,
            202,
            "Accepted",
            &json!({"exportId": EXPORT_ID, "state": state}),
        ),
        Step::RetryWrongExport => write_json_response(
            stream,
            202,
            "Accepted",
            &json!({"exportId": "another-export", "state": "queued"}),
        ),
        Step::RetryRefused(status, code) => write_json_response(
            stream,
            *status,
            "Refused",
            &export_error_body(code, "The retry was refused"),
        ),
        Step::CancelSettled(state) => write_json_response(
            stream,
            200,
            "OK",
            &json!({
                "exportId": EXPORT_ID,
                "state": state,
                "terminalOutcome": state,
            }),
        ),
        Step::CancelUnsettled => write_json_response(
            stream,
            200,
            "OK",
            &json!({"exportId": EXPORT_ID, "state": "queued", "terminalOutcome": null}),
        ),
        Step::CancelUncertain => write_json_response(
            stream,
            500,
            "Internal Server Error",
            &export_error_body(
                "outcome_unknown",
                "The cancellation outcome is unconfirmed; reconcile through inspection",
            ),
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
            "retainedQueryIdleSeconds": 900
        }
    })
}

/// One Export inspection the CLI's validator accepts for the terminal and
/// active states used by these scenarios.
fn inspect_body(state: &str) -> Value {
    let terminal = match state {
        "succeeded" | "failed" | "cancelled" => Some(state),
        _ => None,
    };
    json!({
        "exportId": EXPORT_ID,
        "photoId": PHOTO_ID,
        "state": state,
        "target": "development-tiff",
        "recipeVersion": "recipe-2",
        "sourceRevision": "source-7",
        "bundleId": "bundle",
        "terminalOutcome": terminal,
        "failureReason": null,
        "receiptExpiresAt": null,
        "artifact": null,
    })
}

fn export_error_body(code: &str, message: &str) -> Value {
    json!({"error": {
        "code": code,
        "message": message,
        "effect": "none",
        "details": {}
    }})
}

// ---------------------------------------------------------------- retry

/// The retry reads the original Export first — to name its Photo in an
/// uncertain report, never to refresh the captured snapshot — then submits
/// exactly the caller's new request identity once.
#[tokio::test]
async fn retry_reads_the_original_and_submits_one_new_identity() {
    let service = fake_service(vec![
        Step::Capabilities,
        Step::ReadExport("failed"),
        Step::RetryAccepted("queued"),
    ]);
    let (exit, envelope) = command(
        &service.url,
        &[
            "photos",
            "export",
            "retry",
            EXPORT_ID,
            "--request-id",
            REQUEST_ID,
        ],
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(envelope["data"]["exportId"], EXPORT_ID);
    assert_eq!(envelope["data"]["state"], "queued");
    let (connections, requests) = service.finish();
    assert_eq!(connections, 3, "handshake, read, and one retry request");
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests[1].request_line,
        format!("GET /api/exports/{EXPORT_ID} HTTP/1.1")
    );
    assert_eq!(
        requests[2].request_line,
        format!("POST /api/exports/{EXPORT_ID}/retry HTTP/1.1")
    );
    // The exact submitted body: the request identity is the only field, and
    // no current recipe read precedes the retry.
    assert_eq!(
        serde_json::from_slice::<Value>(&requests[2].body).unwrap(),
        json!({"requestId": REQUEST_ID})
    );
}

/// A replayed identity answers 202 with the Export's current state and
/// resolves to one request; the CLI reports that state instead of assuming
/// a new attempt started.
#[tokio::test]
async fn a_replayed_retry_identity_reports_the_service_state() {
    let service = fake_service(vec![
        Step::Capabilities,
        Step::ReadExport("failed"),
        Step::RetryAccepted("running"),
    ]);
    let (exit, envelope) = command(
        &service.url,
        &[
            "photos",
            "export",
            "retry",
            EXPORT_ID,
            "--request-id",
            REQUEST_ID,
        ],
    )
    .await;
    assert_eq!(exit, 0);
    assert_eq!(envelope["data"]["state"], "running");
    let (connections, requests) = service.finish();
    assert_eq!(connections, 3);
    assert_eq!(requests.len(), 3);
}

/// Every confirmed retry refusal keeps the service's code and effect and
/// maps onto the reference exit codes: identity and state conflicts are 4,
/// expiry and resource conditions are 6.
#[tokio::test]
async fn confirmed_retry_refusals_keep_the_service_codes() {
    for (name, step, expected_exit, code) in [
        (
            "request-conflict",
            Step::RetryRefused(409, "request_conflict"),
            4,
            "request_conflict",
        ),
        (
            "export-conflict",
            Step::RetryRefused(409, "export_conflict"),
            4,
            "export_conflict",
        ),
        (
            "output-unavailable",
            Step::RetryRefused(409, "output_unavailable"),
            4,
            "output_unavailable",
        ),
        (
            "export-expired",
            Step::RetryRefused(410, "export_expired"),
            6,
            "export_expired",
        ),
        (
            "resource-unavailable",
            Step::RetryRefused(503, "resource_unavailable"),
            6,
            "resource_unavailable",
        ),
        (
            "retained-output-full",
            Step::RetryRefused(503, "retained_output_full"),
            6,
            "retained_output_full",
        ),
        (
            "processing-unavailable",
            Step::RetryRefused(503, "processing_unavailable"),
            6,
            "processing_unavailable",
        ),
    ] {
        let service = fake_service(vec![Step::Capabilities, Step::ReadExport("failed"), step]);
        let (exit, envelope) = command(
            &service.url,
            &[
                "photos",
                "export",
                "retry",
                EXPORT_ID,
                "--request-id",
                REQUEST_ID,
            ],
        )
        .await;
        assert_eq!(exit, expected_exit, "for {name}");
        assert_eq!(envelope["error"]["code"], code, "for {name}");
        assert_eq!(envelope["error"]["effect"], "none", "for {name}");
        service.finish();
    }
}

/// A retry identity outside the closed shape is refused before any network
/// access: nothing listens, so a network-touching path would fail as
/// transport instead.
#[tokio::test]
async fn invalid_retry_identities_are_refused_before_any_network() {
    let dead = "https://127.0.0.1:9";
    for request_id in [
        "has space".to_owned(),
        "slash/none".to_owned(),
        "a".repeat(129),
    ] {
        let (exit, envelope) = command(
            dead,
            &[
                "photos",
                "export",
                "retry",
                EXPORT_ID,
                "--request-id",
                &request_id,
            ],
        )
        .await;
        assert_eq!(exit, 2, "for {request_id}");
        assert_eq!(envelope["error"]["code"], "invalid_input");
        assert_eq!(envelope["error"]["effect"], "none");
        assert_eq!(envelope["error"]["details"]["argument"], "request-id");
    }
}

/// A confirmed unknown Export is the write's own refusal: the read stops
/// the command before any retry request is sent.
#[tokio::test]
async fn an_unknown_export_stops_the_retry_before_any_request() {
    let service = fake_service(vec![Step::Capabilities, Step::UnknownExport]);
    let (exit, envelope) = command(
        &service.url,
        &[
            "photos",
            "export",
            "retry",
            EXPORT_ID,
            "--request-id",
            REQUEST_ID,
        ],
    )
    .await;
    assert_eq!(exit, 3);
    assert_eq!(envelope["error"]["code"], "unknown_export");
    assert_eq!(envelope["error"]["effect"], "none");
    let (connections, requests) = service.finish();
    assert_eq!(connections, 2, "no retry request may follow the refusal");
    assert_eq!(requests.len(), 2);
}

/// A lost retry response cannot prove refusal or admission: the outcome is
/// unknown, names the affected Photo, and the CLI neither retries the write
/// nor invents a new identity.
#[tokio::test]
async fn a_lost_retry_response_is_an_unknown_outcome_naming_the_photo() {
    let service = fake_service(vec![
        Step::Capabilities,
        Step::ReadExport("failed"),
        Step::LoseAfterRead,
    ]);
    let (exit, envelope) = command(
        &service.url,
        &[
            "photos",
            "export",
            "retry",
            EXPORT_ID,
            "--request-id",
            REQUEST_ID,
        ],
    )
    .await;
    assert_eq!(exit, 7);
    assert_eq!(envelope["error"]["code"], "outcome_unknown");
    assert_eq!(envelope["error"]["effect"], "unknown");
    assert_eq!(
        envelope["error"]["details"]["operation"],
        "photos-export-retry"
    );
    assert_eq!(
        envelope["error"]["details"]["photoIds"],
        json!([PHOTO_ID]),
        "the original Export read identifies the Photo at risk"
    );
    let (connections, requests) = service.finish();
    assert_eq!(
        connections, 3,
        "no hidden retry may follow the lost response"
    );
    assert_eq!(requests.len(), 3);
}

/// An admitted response that names another Export is not a receipt for this
/// command's retry: it stays an unknown outcome.
#[tokio::test]
async fn an_admitted_retry_naming_another_export_is_unknown() {
    let service = fake_service(vec![
        Step::Capabilities,
        Step::ReadExport("failed"),
        Step::RetryWrongExport,
    ]);
    let (exit, envelope) = command(
        &service.url,
        &[
            "photos",
            "export",
            "retry",
            EXPORT_ID,
            "--request-id",
            REQUEST_ID,
        ],
    )
    .await;
    assert_eq!(exit, 7);
    assert_eq!(envelope["error"]["code"], "outcome_unknown");
    assert_eq!(envelope["error"]["effect"], "unknown");
    service.finish();
}
// ---------------------------------------------------------------- cancel

/// The cancellation reports the actual terminal settlement. A completion
/// that raced the request is a success report, and the route carries no
/// request fields.
#[tokio::test]
async fn cancel_reports_the_actual_terminal_settlement() {
    for (name, state) in [
        ("raced-completion", "succeeded"),
        ("cancelled", "cancelled"),
    ] {
        let service = fake_service(vec![
            Step::Capabilities,
            Step::ReadExport("running"),
            Step::CancelSettled(state),
        ]);
        let (exit, envelope) =
            command(&service.url, &["photos", "export", "cancel", EXPORT_ID]).await;
        assert_eq!(exit, 0, "for {name}");
        assert_eq!(envelope["data"]["exportId"], EXPORT_ID, "for {name}");
        assert_eq!(envelope["data"]["state"], state, "for {name}");
        assert_eq!(envelope["data"]["terminalOutcome"], state, "for {name}");
        let (connections, requests) = service.finish();
        assert_eq!(connections, 3, "for {name}");
        assert_eq!(
            requests[2].request_line,
            format!("POST /api/exports/{EXPORT_ID}/cancel HTTP/1.1"),
            "for {name}"
        );
        // The cancellation carries no request fields.
        assert!(requests[2].body.is_empty(), "for {name}");
    }
}

/// A confirmed unknown Export stops the cancellation before any request is
/// sent, exactly as for the retry.
#[tokio::test]
async fn an_unknown_export_stops_the_cancellation_before_any_request() {
    let service = fake_service(vec![Step::Capabilities, Step::UnknownExport]);
    let (exit, envelope) = command(&service.url, &["photos", "export", "cancel", EXPORT_ID]).await;
    assert_eq!(exit, 3);
    assert_eq!(envelope["error"]["code"], "unknown_export");
    assert_eq!(envelope["error"]["effect"], "none");
    let (connections, requests) = service.finish();
    assert_eq!(connections, 2, "no cancellation may follow the refusal");
    assert_eq!(requests.len(), 2);
}

/// An unconfirmed settlement is an unknown outcome that names the Photo;
/// the CLI does not guess the settlement and sends nothing further.
#[tokio::test]
async fn an_unconfirmed_cancellation_is_an_unknown_outcome_naming_the_photo() {
    let service = fake_service(vec![
        Step::Capabilities,
        Step::ReadExport("running"),
        Step::CancelUncertain,
    ]);
    let (exit, envelope) = command(&service.url, &["photos", "export", "cancel", EXPORT_ID]).await;
    assert_eq!(exit, 7);
    assert_eq!(envelope["error"]["code"], "outcome_unknown");
    assert_eq!(envelope["error"]["effect"], "unknown");
    assert_eq!(
        envelope["error"]["details"]["operation"],
        "photos-export-cancel"
    );
    assert_eq!(
        envelope["error"]["details"]["photoIds"],
        json!([PHOTO_ID]),
        "the original Export read identifies the Photo at risk"
    );
    let (connections, requests) = service.finish();
    assert_eq!(connections, 3, "no hidden second cancellation may follow");
    assert_eq!(requests.len(), 3);
}

/// A 200 that carries no terminal settlement is not a cancellation receipt:
/// the admitted response is unusable and the outcome stays unknown.
#[tokio::test]
async fn a_cancellation_without_a_terminal_settlement_is_unknown() {
    let service = fake_service(vec![
        Step::Capabilities,
        Step::ReadExport("running"),
        Step::CancelUnsettled,
    ]);
    let (exit, envelope) = command(&service.url, &["photos", "export", "cancel", EXPORT_ID]).await;
    assert_eq!(exit, 7);
    assert_eq!(envelope["error"]["code"], "outcome_unknown");
    assert_eq!(envelope["error"]["effect"], "unknown");
    service.finish();
}

// ---------------------------------------------------------------- real service

/// The two Originals of the shared fixture: JPEG sources only, with
/// processing unconfigured, so no Export can ever be recorded and the
/// Export-scoped commands answer the deterministic unknown-Export refusal.
fn real_service_fixture() -> (PathBuf, Config) {
    let base = temp_base("real-service");
    let originals = base.join("originals");
    let web = base.join("web");
    fs::create_dir(&originals).unwrap();
    fs::create_dir(originals.join("trip")).unwrap();
    fs::create_dir(&web).unwrap();
    fs::write(web.join("index.html"), b"<main>fixture</main>").unwrap();
    for name in ["one.JPG", "two.JPG"] {
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

/// Without processing, no Export exists: the real service answers the
/// Export read with `unknown_export`, and both the cancellation and the
/// retry stop on that confirmed refusal without sending their write.
#[tokio::test]
async fn the_real_service_refuses_cancel_and_retry_of_an_unknown_export() {
    for (name, arguments) in [
        ("cancel", vec!["photos", "export", "cancel", EXPORT_ID]),
        (
            "retry",
            vec![
                "photos",
                "export",
                "retry",
                EXPORT_ID,
                "--request-id",
                REQUEST_ID,
            ],
        ),
    ] {
        let (base, config) = real_service_fixture();
        let server = common::start_authenticated_server(config).await;
        wait_until_idle(&server.url).await;
        let (exit, refusal) = command(&server.url, &arguments).await;
        assert_eq!(exit, 3, "for {name}");
        assert_eq!(refusal["error"]["code"], "unknown_export", "for {name}");
        assert_eq!(refusal["error"]["effect"], "none", "for {name}");
        server.close().await.unwrap();
        fs::remove_dir_all(base).unwrap();
    }
}
