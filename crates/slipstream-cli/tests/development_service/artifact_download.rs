use super::*;
// ------------------------------------------- resumable Artifact bytes

/// The published provenance record the bytes route must repeat field for
/// field. The input binding, parameter snapshot, and output contract are the
/// closed shape the CLI validates before it trusts any of it.
fn artifact_facts(bytes: &[u8], artifact_id: &str) -> Value {
    json!({
        "artifactId": artifact_id,
        "photoId": PHOTO_ID,
        "stepId": "develop-1",
        "module": "darktable",
        "adapterSchemaVersion": "darktable-adapter-1:darktable-params-1",
        "parameters": {"schemaVersion": "darktable-params-1", "tree": {}},
        "input": {
            "binding": {
                "kind": "original",
                "photoId": PHOTO_ID,
                "sourceRevision": SOURCE_REVISION,
            },
            "sha256": "a".repeat(64),
            "byteLength": 24,
        },
        "outputContract": {
            "format": "tiff",
            "precision": "float32",
            "colorSpace": "prophoto-rgb",
            "transfer": "linear",
            "encoding": "uncompressed",
            "geometry": {"width": 8, "height": 4},
        },
        "bundleId": "bundle-1",
        "sha256": sha256_hex(bytes),
        "byteLength": bytes.len(),
        "filename": format!("{artifact_id}.tiff"),
        "publishedAt": "2026-10-02T12:00:00Z",
        "expiresAt": "2030-01-01T00:00:00Z",
    })
}

/// A deterministic object larger than one transfer buffer, so a download
/// really crosses several chunks.
fn artifact_bytes() -> Vec<u8> {
    (0..192 * 1024).map(|index| (index % 251) as u8).collect()
}

/// The private files a transfer owns beside its destination.
fn private_state(destination: &std::path::Path) -> (PathBuf, PathBuf) {
    let part = PathBuf::from(format!("{}.slipstream-part", destination.display()));
    let sidecar = PathBuf::from(format!("{}.slipstream-part.json", destination.display()));
    (part, sidecar)
}

/// The exact identity sidecar a matching partial carries.
fn saved_identity(facts: &Value) -> String {
    json!({
        "artifactId": facts["artifactId"], "filename": facts["filename"],
        "byteLength": facts["byteLength"], "sha256": facts["sha256"],
        "contentType": "image/tiff", "outputFormat": "tiff",
    })
    .to_string()
}

/// A published artifact is streamed to its private partial, verified against
/// the provenance record and its own digest, and published exactly once.
#[tokio::test]
async fn artifact_download_publishes_the_named_object_once() {
    for (format, extension, payload) in [
        ("tiff", "tiff", artifact_bytes()),
        ("jpeg", "jpg", jpeg_bytes()),
    ] {
        let mut facts = artifact_facts(&payload, "artifact-1");
        facts["outputContract"]["format"] = json!(format);
        facts["filename"] = json!(format!("artifact-1.{extension}"));
        let base = temp_base(&format!("artifact-download-{format}"));
        let destination = base.join(format!("develop.{extension}"));
        let service = fake_service(vec![
            Step::Capabilities,
            Step::Json(200, facts.clone()),
            Step::ArtifactBytes(ArtifactReply::new(facts.clone(), payload.clone())),
        ]);
        let (exit, envelope) = command(
            &service.url,
            &[
                "processing",
                "artifact-download",
                "artifact-1",
                "--file",
                destination.to_str().unwrap(),
            ],
        )
        .await;
        assert_eq!(exit, 0, "for {format}: {envelope}");
        let data = &envelope["data"];
        assert_eq!(data["artifactId"], "artifact-1", "for {format}");
        assert_eq!(data["photoId"], PHOTO_ID, "for {format}");
        assert_eq!(data["stepId"], "develop-1", "for {format}");
        assert_eq!(data["module"], "darktable", "for {format}");
        assert_eq!(
            data["filename"],
            format!("artifact-1.{extension}"),
            "for {format}"
        );
        assert_eq!(data["sha256"], sha256_hex(&payload), "for {format}");
        assert_eq!(
            data["byteLength"].as_u64(),
            Some(payload.len() as u64),
            "for {format}"
        );
        assert_eq!(data["width"], 8, "for {format}");
        assert_eq!(data["height"], 4, "for {format}");
        assert_eq!(data["path"], destination.to_str().unwrap(), "for {format}");
        assert_eq!(data["fileCommitted"], true, "for {format}");
        assert_eq!(
            data["webUrl"],
            format!("{}/?photoId={PHOTO_ID}", service.url),
            "for {format}"
        );
        assert_eq!(fs::read(&destination).unwrap(), payload, "for {format}");
        assert_eq!(
            fs::read_dir(&base).unwrap().count(),
            1,
            "for {format}: no private state survives a published transfer"
        );
        let (connections, requests) = service.finish();
        assert_eq!(connections, 3, "for {format}");
        assert_eq!(
            requests[1].request_line, "GET /api/processing-artifacts/artifact-1 HTTP/1.1",
            "for {format}"
        );
        assert_eq!(
            requests[2].request_line, "GET /api/processing-artifacts/artifact-1/bytes HTTP/1.1",
            "for {format}"
        );
        assert_eq!(requests[2].range, None, "for {format}");
        assert_eq!(requests[2].if_range, None, "for {format}");
        fs::remove_dir_all(base).unwrap();
    }
}

/// Every transfer the record cannot confirm publishes nothing: substituted
/// bytes, a described length that disagrees with the record, a cut stream,
/// and a response whose repeated provenance header disagrees with the record.
#[tokio::test]
async fn artifact_download_never_publishes_unverifiable_transfers() {
    let payload = artifact_bytes();
    let substituted: Vec<u8> = payload.iter().map(|byte| byte.wrapping_add(1)).collect();
    let truncated = payload.len() / 2;
    let modes: [(&str, ArtifactReply, bool); 4] = [
        (
            "substituted",
            ArtifactReply::new(artifact_facts(&payload, "artifact-1"), substituted),
            false,
        ),
        (
            "foreign-length",
            ArtifactReply::new(artifact_facts(&payload, "artifact-1"), payload.clone())
                .declared(payload.len() + 1),
            false,
        ),
        (
            "truncated",
            ArtifactReply::new(artifact_facts(&payload, "artifact-1"), payload.clone())
                .interrupted(truncated),
            true,
        ),
        (
            "foreign-header",
            ArtifactReply::new(artifact_facts(&payload, "artifact-1"), payload.clone())
                .header("slipstream-artifact-sha256", &"b".repeat(64)),
            false,
        ),
    ];
    for (name, reply, keeps_partial) in modes {
        let base = temp_base(name);
        let destination = base.join("develop.tiff");
        let (part, _) = private_state(&destination);
        let service = fake_service(vec![
            Step::Capabilities,
            Step::Json(200, artifact_facts(&payload, "artifact-1")),
            Step::ArtifactBytes(reply),
        ]);
        let (exit, envelope) = command(
            &service.url,
            &[
                "processing",
                "artifact-download",
                "artifact-1",
                "--file",
                destination.to_str().unwrap(),
            ],
        )
        .await;
        assert_eq!(exit, 6, "for {name}: {envelope}");
        assert_eq!(envelope["error"]["code"], "transport_failed", "for {name}");
        assert_eq!(
            envelope["error"]["details"]["operation"], "processing-artifact-download",
            "for {name}"
        );
        assert!(!destination.exists(), "for {name}");
        if keeps_partial {
            assert_eq!(
                fs::metadata(&part).unwrap().len(),
                truncated as u64,
                "for {name}: a cut stream stays resumable"
            );
            assert_eq!(fs::read_dir(&base).unwrap().count(), 2, "for {name}");
        } else {
            assert_eq!(
                fs::read_dir(&base).unwrap().count(),
                0,
                "for {name}: unusable state is discarded"
            );
        }
        let (_, requests) = service.finish();
        assert_eq!(requests.len(), 3, "for {name}");
        fs::remove_dir_all(base).unwrap();
    }
}

/// Killing a transfer and rerunning the same command resumes the verified
/// prefix with one range against a service that restarted in between.
#[tokio::test]
async fn artifact_download_resumes_a_matching_partial_after_a_service_restart() {
    let payload = artifact_bytes();
    let facts = artifact_facts(&payload, "artifact-1");
    let settled = payload.len() / 2;
    let base = temp_base("artifact-resume");
    let destination = base.join("develop.tiff");
    let (part, sidecar) = private_state(&destination);
    let arguments = [
        "processing",
        "artifact-download",
        "artifact-1",
        "--file",
        destination.to_str().unwrap(),
    ];

    let first = fake_service(vec![
        Step::Capabilities,
        Step::Json(200, facts.clone()),
        Step::ArtifactBytes(
            ArtifactReply::new(facts.clone(), payload.clone()).interrupted(settled),
        ),
    ]);
    let (exit, envelope) = command(&first.url, &arguments).await;
    assert_eq!(exit, 6, "{envelope}");
    assert!(!destination.exists());
    assert_eq!(fs::metadata(&part).unwrap().len(), settled as u64);
    assert_eq!(
        fs::read_to_string(&sidecar).unwrap(),
        saved_identity(&facts)
    );
    first.finish();

    // The service restarts on a new origin; the Artifact stays retained.
    let second = fake_service(vec![
        Step::Capabilities,
        Step::Json(200, facts.clone()),
        Step::ArtifactBytes(ArtifactReply::new(facts.clone(), payload.clone()).honor_range()),
    ]);
    let (exit, envelope) = command(&second.url, &arguments).await;
    assert_eq!(exit, 0, "{envelope}");
    assert_eq!(envelope["data"]["fileCommitted"], true);
    assert_eq!(fs::read(&destination).unwrap(), payload);
    assert_eq!(fs::read_dir(&base).unwrap().count(), 1);
    let (_, requests) = second.finish();
    assert_eq!(
        requests[2].range.as_deref(),
        Some(format!("bytes={settled}-").as_str()),
        "the resume asks for exactly the verified prefix"
    );
    assert_eq!(
        requests[2].if_range.as_deref(),
        Some(format!("\"{}\"", sha256_hex(&payload)).as_str())
    );
    fs::remove_dir_all(base).unwrap();
}

/// A peer that ignores the range answers the complete representation; the
/// partial is discarded instead of being appended twice.
#[tokio::test]
async fn artifact_download_restarts_from_zero_when_the_service_ignores_the_range() {
    let payload = artifact_bytes();
    let facts = artifact_facts(&payload, "artifact-1");
    let settled = 32 * 1024;
    let base = temp_base("artifact-restart");
    let destination = base.join("develop.tiff");
    let (part, _) = private_state(&destination);
    let arguments = [
        "processing",
        "artifact-download",
        "artifact-1",
        "--file",
        destination.to_str().unwrap(),
    ];
    let service = fake_service(vec![
        Step::Capabilities,
        Step::Json(200, facts.clone()),
        Step::ArtifactBytes(
            ArtifactReply::new(facts.clone(), payload.clone()).interrupted(settled),
        ),
        Step::Capabilities,
        Step::Json(200, facts.clone()),
        Step::ArtifactBytes(ArtifactReply::new(facts.clone(), payload.clone())),
    ]);
    let (exit, envelope) = command(&service.url, &arguments).await;
    assert_eq!(exit, 6, "{envelope}");
    assert_eq!(fs::metadata(&part).unwrap().len(), settled as u64);
    let (exit, envelope) = command(&service.url, &arguments).await;
    assert_eq!(exit, 0, "{envelope}");
    assert_eq!(fs::read(&destination).unwrap(), payload);
    assert_eq!(fs::read_dir(&base).unwrap().count(), 1);
    let (_, requests) = service.finish();
    assert!(
        requests[5]
            .range
            .as_deref()
            .is_some_and(|range| range.starts_with("bytes=")),
        "the retry offered its prefix before the full answer"
    );
    fs::remove_dir_all(base).unwrap();
}

/// Private state that does not describe this Artifact — and a partial longer
/// than the Artifact — never contributes bytes to the publication.
#[tokio::test]
async fn artifact_download_discards_private_state_that_cannot_be_reused() {
    let payload = artifact_bytes();
    let facts = artifact_facts(&payload, "artifact-1");
    for mode in ["foreign", "overlong"] {
        let base = temp_base(&format!("artifact-state-{mode}"));
        let destination = base.join("develop.tiff");
        let (part, sidecar) = private_state(&destination);
        let seeded = match mode {
            "foreign" => {
                let mut foreign = facts.clone();
                foreign["artifactId"] = json!("artifact-2");
                foreign["sha256"] = json!("c".repeat(64));
                fs::write(&sidecar, saved_identity(&foreign)).unwrap();
                4096
            }
            _ => {
                fs::write(&sidecar, saved_identity(&facts)).unwrap();
                payload.len() + 1
            }
        };
        fs::write(&part, vec![7_u8; seeded]).unwrap();
        let service = fake_service(vec![
            Step::Capabilities,
            Step::Json(200, facts.clone()),
            Step::ArtifactBytes(ArtifactReply::new(facts.clone(), payload.clone())),
        ]);
        let (exit, envelope) = command(
            &service.url,
            &[
                "processing",
                "artifact-download",
                "artifact-1",
                "--file",
                destination.to_str().unwrap(),
            ],
        )
        .await;
        assert_eq!(exit, 0, "for {mode}: {envelope}");
        assert_eq!(fs::read(&destination).unwrap(), payload, "for {mode}");
        assert_eq!(fs::read_dir(&base).unwrap().count(), 1, "for {mode}");
        let (_, requests) = service.finish();
        assert_eq!(
            requests[2].range, None,
            "for {mode}: nothing is appended to unusable state"
        );
        fs::remove_dir_all(base).unwrap();
    }
}

/// An expired and an unknown Artifact are named refusals that leave no
/// private state and create no work.
#[tokio::test]
async fn artifact_download_clears_private_state_for_unretained_artifacts() {
    for (name, status, code, expected_exit) in [
        ("expired", 410, "artifact_expired", 6),
        ("unknown", 404, "unknown_artifact", 3),
    ] {
        let base = temp_base(&format!("artifact-terminal-{name}"));
        let destination = base.join("develop.tiff");
        let (part, sidecar) = private_state(&destination);
        fs::write(&part, vec![7_u8; 4096]).unwrap();
        fs::write(&sidecar, "{}").unwrap();
        let service = fake_service(vec![
            Step::Capabilities,
            Step::Json(
                status,
                json!({"error": {
                    "code": code,
                    "message": "The Processing Artifact is unknown",
                    "effect": "none",
                    "details": {},
                }}),
            ),
        ]);
        let (exit, envelope) = command(
            &service.url,
            &[
                "processing",
                "artifact-download",
                "artifact-1",
                "--file",
                destination.to_str().unwrap(),
            ],
        )
        .await;
        assert_eq!(exit, expected_exit, "for {name}: {envelope}");
        assert_eq!(envelope["error"]["code"], code, "for {name}");
        assert!(!destination.exists(), "for {name}");
        assert_eq!(
            fs::read_dir(&base).unwrap().count(),
            0,
            "for {name}: terminal state is never resumed"
        );
        let (connections, _) = service.finish();
        assert_eq!(connections, 2, "for {name}");
        fs::remove_dir_all(base).unwrap();
    }
}

/// A transfer that keeps making progress outlives the per-request timeout,
/// because only forward progress and idle gaps are bounded.
#[tokio::test]
async fn artifact_download_streams_past_the_control_timeout_while_progressing() {
    let payload = artifact_bytes();
    let facts = artifact_facts(&payload, "artifact-1");
    let base = temp_base("artifact-slow");
    let destination = base.join("develop.tiff");
    let service = fake_service(vec![
        Step::Capabilities,
        Step::Json(200, facts.clone()),
        Step::ArtifactBytes(
            ArtifactReply::new(facts.clone(), payload.clone())
                .pace(32 * 1024, Duration::from_millis(350)),
        ),
    ]);
    let (exit, envelope) = command(
        &service.url,
        &[
            "--timeout",
            "1",
            "processing",
            "artifact-download",
            "artifact-1",
            "--file",
            destination.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(exit, 0, "{envelope}");
    assert_eq!(fs::read(&destination).unwrap(), payload);
    assert_eq!(fs::read_dir(&base).unwrap().count(), 1);
    service.finish();
    fs::remove_dir_all(base).unwrap();
}

/// A stream that stops moving is a bounded idle failure: no destination, and
/// the verified prefix stays for the next attempt.
#[tokio::test]
async fn artifact_download_stops_a_stalled_stream_and_keeps_its_prefix() {
    let payload = artifact_bytes();
    let facts = artifact_facts(&payload, "artifact-1");
    let received = 8 * 1024;
    let base = temp_base("artifact-stalled");
    let destination = base.join("develop.tiff");
    let (part, sidecar) = private_state(&destination);
    let service = fake_service(vec![
        Step::Capabilities,
        Step::Json(200, facts.clone()),
        Step::ArtifactBytes(
            ArtifactReply::new(facts.clone(), payload.clone())
                .interrupted(received)
                .stall(Duration::from_millis(2200)),
        ),
    ]);
    let (exit, envelope) = command(
        &service.url,
        &[
            "--timeout",
            "1",
            "processing",
            "artifact-download",
            "artifact-1",
            "--file",
            destination.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(exit, 6, "{envelope}");
    assert_eq!(envelope["error"]["code"], "transport_failed");
    assert!(!destination.exists());
    assert_eq!(fs::metadata(&part).unwrap().len(), received as u64);
    assert!(sidecar.exists());
    let (_, requests) = service.finish();
    assert_eq!(requests.len(), 3);
    fs::remove_dir_all(base).unwrap();
}

/// An existing destination is refused before any network access, and a file
/// or symbolic link is never followed or replaced.
#[tokio::test]
async fn artifact_download_refuses_an_existing_destination_before_any_network() {
    for mode in ["file", "symlink"] {
        let base = temp_base(&format!("artifact-existing-{mode}"));
        let destination = base.join("develop.tiff");
        let sentinel = base.join("sentinel.bin");
        fs::write(&sentinel, b"sentinel").unwrap();
        match mode {
            "file" => fs::write(&destination, b"sentinel").unwrap(),
            _ => std::os::unix::fs::symlink(&sentinel, &destination).unwrap(),
        }
        let service = fake_service(vec![Step::Capabilities]);
        let (exit, envelope) = command(
            &service.url,
            &[
                "processing",
                "artifact-download",
                "artifact-1",
                "--file",
                destination.to_str().unwrap(),
            ],
        )
        .await;
        assert_eq!(exit, 2, "for {mode}: {envelope}");
        assert_eq!(envelope["error"]["code"], "invalid_input", "for {mode}");
        assert_eq!(envelope["error"]["effect"], "none", "for {mode}");
        assert_eq!(
            envelope["error"]["details"]["argument"], "file",
            "for {mode}"
        );
        assert_eq!(fs::read(&sentinel).unwrap(), b"sentinel", "for {mode}");
        if mode == "symlink" {
            assert!(
                fs::symlink_metadata(&destination)
                    .unwrap()
                    .file_type()
                    .is_symlink(),
                "the symbolic link is never replaced"
            );
            assert_eq!(fs::read(&destination).unwrap(), b"sentinel");
        } else {
            assert_eq!(fs::read(&destination).unwrap(), b"sentinel");
        }
        let (connections, requests) = service.finish();
        assert_eq!(
            connections, 0,
            "for {mode}: the refusal happens before the handshake"
        );
        assert!(requests.is_empty(), "for {mode}");
        fs::remove_dir_all(base).unwrap();
    }
}
