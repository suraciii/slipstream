//! Processing Artifact download streams and lease lifetime.

use super::*;

/// `GET /api/processing-artifacts/{id}/bytes`: one published immutable
/// artifact's bytes under a finite download lease. The lease holds the
/// artifact against expiry cleanup while the live stream renews it, and
/// the response headers carry the artifact record field for field so a
/// client validates the download against the provenance route.
pub(crate) async fn get_processing_artifact_bytes(
    State(state): State<HttpState>,
    Path(artifact_id): Path<String>,
    request: Request<Body>,
) -> Response {
    if request.headers().contains_key(CLI_CONTRACT_HEADER)
        && let Err(response) = require_cli_contract(&request)
    {
        return *response;
    }
    if let Err(response) = require_published(&state.application) {
        return *response;
    }
    if slipstream_core::ProcessingArtifactId::new(&artifact_id).is_err() {
        return error(
            StatusCode::NOT_FOUND,
            "unknown_artifact",
            "The Processing Artifact is unknown",
        );
    }
    let exports = match state.application.exports.as_ref() {
        Some(exports) => exports,
        None => {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "processing_unavailable",
                "Processing is not configured for this deployment",
            );
        }
    };
    let library = &state.application.library;
    // The closed per-retention refusals come before any lease is taken.
    let lease = match library
        .acquire_processing_artifact_lease(&artifact_id, unix_seconds_now())
        .await
    {
        Ok(slipstream_core::ProcessingArtifactLeaseOutcome::Acquired { lease_id, artifact }) => {
            (lease_id, *artifact)
        }
        Ok(slipstream_core::ProcessingArtifactLeaseOutcome::Unknown) => {
            return error(
                StatusCode::NOT_FOUND,
                "unknown_artifact",
                "The Processing Artifact is unknown",
            );
        }
        Ok(slipstream_core::ProcessingArtifactLeaseOutcome::Expired) => {
            return error(
                StatusCode::GONE,
                "artifact_expired",
                "The artifact retention window has passed",
            );
        }
        Err(_) => {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "processing_unavailable",
                "The download lease could not be persisted",
            );
        }
    };
    let (lease_id, artifact) = lease;
    let release_lease = {
        let library = Arc::clone(library);
        let lease_id = lease_id.clone();
        async move {
            let _ = library.release_processing_artifact_lease(&lease_id).await;
        }
    };
    let output_format = artifact
        .output_contract
        .format
        .strip_prefix("image/")
        .unwrap_or(&artifact.output_contract.format);
    let (content_type, extension) = match output_format {
        "tiff" => ("image/tiff", "tiff"),
        "jpeg" => ("image/jpeg", "jpg"),
        _ => {
            release_lease.await;
            return error(
                StatusCode::CONFLICT,
                "output_unavailable",
                "The artifact output contract names an unsupported medium",
            );
        }
    };
    let Some(path) = exports.artifact_path_for_module(&artifact_id, artifact.module.as_str())
    else {
        release_lease.await;
        return error(
            StatusCode::NOT_FOUND,
            "unknown_artifact",
            "The Processing Artifact identity is invalid",
        );
    };
    let file = match tokio::fs::File::open(&path).await {
        Ok(file) => file,
        Err(_) => {
            release_lease.await;
            return error(
                StatusCode::CONFLICT,
                "output_unavailable",
                "The artifact file could not be opened",
            );
        }
    };
    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(4);
    // The response body reports its end — drained or dropped — through this
    // channel, which is what actually settles the download.
    let (body_done_tx, body_done_rx) = tokio::sync::oneshot::channel::<()>();
    // A live stream keeps its lease's liveness anchor fresh so the
    // staleness sweep can never reclaim it before the response settles.
    let renewer = {
        let library = Arc::clone(library);
        let lease_id = lease_id.clone();
        let interval = exports.lease_renewal_interval();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if library
                    .renew_processing_artifact_lease(&lease_id, unix_seconds_now())
                    .await
                    .unwrap_or(false)
                {
                    continue;
                }
                break;
            }
        })
    };
    let pump = {
        let library = Arc::clone(library);
        let lease_id = lease_id.clone();
        let renewer = renewer;
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt as _;
            let mut file = file;
            let mut buffer = vec![0_u8; 512 * 1024];
            loop {
                match file.read(&mut buffer).await {
                    Ok(0) => break,
                    Ok(count) => {
                        if sender.send(Ok(buffer[..count].to_vec())).await.is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(Err(error)).await;
                        break;
                    }
                }
            }
            // The producer reaching end of file is not the stream
            // settling: the lease holds until the response body drains or
            // is dropped, so a slow client keeps its protection.
            drop(sender);
            let _ = body_done_rx.await;
            let _ = library.release_processing_artifact_lease(&lease_id).await;
            renewer.abort();
        })
    };
    drop(pump);
    let builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, artifact.byte_length.to_string())
        .header(header::CACHE_CONTROL, "no-store")
        .header("x-content-type-options", "nosniff")
        .header("slipstream-artifact-id", artifact.artifact_id.as_str())
        .header("slipstream-artifact-photo-id", &artifact.photo_id)
        .header("slipstream-artifact-step-id", artifact.step_id.as_str())
        .header("slipstream-artifact-module", artifact.module.as_str())
        .header(
            "slipstream-artifact-adapter-schema-version",
            &artifact.adapter_schema_version,
        )
        .header("slipstream-artifact-bundle-id", &artifact.bundle_id)
        .header(
            "slipstream-artifact-width",
            artifact.output_contract.geometry.width.to_string(),
        )
        .header(
            "slipstream-artifact-height",
            artifact.output_contract.geometry.height.to_string(),
        )
        .header(
            "slipstream-artifact-byte-length",
            artifact.byte_length.to_string(),
        )
        .header("slipstream-artifact-sha256", &artifact.sha256)
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{artifact_id}.{extension}\""),
        );
    builder
        .body(Body::from_stream(ArtifactFileStream(
            receiver,
            Some(body_done_tx),
        )))
        .expect("valid artifact response")
}

/// The streamed artifact body: the pump task's channel plus the signal
/// that fires when the response body drains or is dropped, releasing the
/// download lease.
struct ArtifactFileStream(
    tokio::sync::mpsc::Receiver<Result<Vec<u8>, std::io::Error>>,
    Option<tokio::sync::oneshot::Sender<()>>,
);

impl Drop for ArtifactFileStream {
    fn drop(&mut self) {
        // Dropping the completion sender is itself the signal: the pump
        // task's receive side resolves as cancelled and releases the lease.
        drop(self.1.take());
    }
}

impl futures_core::Stream for ArtifactFileStream {
    type Item = Result<Vec<u8>, std::io::Error>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.0.poll_recv(cx)
    }
}
