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
    if slipstream_core::ProcessingArtifactId::new(&artifact_id).is_err()
        || !artifact_id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
        || artifact_id == "."
        || artifact_id == ".."
    {
        return error(
            StatusCode::NOT_FOUND,
            "unknown_artifact",
            "The Processing Artifact is unknown",
        );
    }
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
    let content_type = match output_format {
        "tiff" => "image/tiff",
        "jpeg" => "image/jpeg",
        _ => {
            release_lease.await;
            return error(
                StatusCode::CONFLICT,
                "output_unavailable",
                "The artifact output contract names an unsupported medium",
            );
        }
    };
    let extension = if output_format == "tiff" {
        "tiff"
    } else {
        "jpg"
    };
    let path = state
        .application
        .export_artifacts_directory
        .join(format!("{artifact_id}.{extension}"));
    let retention = match library.processing_artifact_retention(&artifact_id).await {
        Ok(Some(retention)) => retention,
        _ => {
            release_lease.await;
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "storage",
                "The artifact retention evidence could not be read",
            );
        }
    };
    let requested_range = {
        let mut values = request.headers().get_all(header::RANGE).iter();
        match (values.next(), values.next()) {
            (None, None) => None,
            (Some(value), None) => Some(value.to_str().ok()),
            _ => Some(None),
        }
    };
    let etag = format!("\"{}\"", artifact.sha256);
    let range_not_satisfiable = || {
        Response::builder()
            .status(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(
                header::CONTENT_RANGE,
                format!("bytes */{}", artifact.byte_length),
            )
            .body(Body::empty())
            .expect("valid range response")
    };
    let if_range = request
        .headers()
        .get(header::IF_RANGE)
        .map(|value| value.to_str().ok());
    let range = match (requested_range, if_range) {
        (Some(Some(value)), Some(Some(if_range))) if if_range == etag => {
            match parse_artifact_range(value, artifact.byte_length) {
                Some(range) => Some(range),
                None => {
                    release_lease.await;
                    return range_not_satisfiable();
                }
            }
        }
        (Some(Some(_)), Some(Some(_)) | Some(None)) => None,
        (Some(Some(value)), None) => match parse_artifact_range(value, artifact.byte_length) {
            Some(range) => Some(range),
            None => {
                release_lease.await;
                return range_not_satisfiable();
            }
        },
        (Some(None), _) => {
            release_lease.await;
            return range_not_satisfiable();
        }
        (None, _) => None,
    };
    let (stream_start, stream_length, status) = range
        .map(|(start, end)| (start, end - start + 1, StatusCode::PARTIAL_CONTENT))
        .unwrap_or((0, artifact.byte_length, StatusCode::OK));
    let mut file = match tokio::fs::File::open(&path).await {
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
    if stream_start != 0 {
        use tokio::io::AsyncSeekExt as _;
        if file
            .seek(std::io::SeekFrom::Start(stream_start))
            .await
            .is_err()
        {
            release_lease.await;
            return error(
                StatusCode::CONFLICT,
                "output_unavailable",
                "The artifact file could not be opened",
            );
        }
    }
    let (sender, receiver) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(4);
    // The response body reports its end — drained or dropped — through this
    // channel, which is what actually settles the download.
    let (body_done_tx, body_done_rx) = tokio::sync::oneshot::channel::<()>();
    // A live stream keeps its lease's liveness anchor fresh so the
    // staleness sweep can never reclaim it before the response settles.
    let renewer = {
        let library = Arc::clone(library);
        let lease_id = lease_id.clone();
        let interval = state
            .application
            .exports
            .as_ref()
            .map_or(std::time::Duration::from_secs(30), |exports| {
                exports.lease_renewal_interval()
            });
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
        let mut remaining = stream_length;
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt as _;
            let mut file = file;
            let mut buffer = vec![0_u8; 512 * 1024];
            while remaining > 0 {
                let read_size = remaining.min(buffer.len() as u64) as usize;
                match file.read(&mut buffer[..read_size]).await {
                    Ok(0) => break,
                    Ok(count) => {
                        remaining -= count as u64;
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
    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, stream_length.to_string())
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::ETAG, &etag);
    if let Some((start, end)) = range {
        builder = builder.header(
            header::CONTENT_RANGE,
            format!("bytes {}-{}/{}", start, end, artifact.byte_length),
        );
    }
    builder = builder
        .header(header::CACHE_CONTROL, "no-store")
        .header("x-content-type-options", "nosniff")
        .header("slipstream-artifact-id", artifact.artifact_id.as_str())
        .header("slipstream-artifact-photo-id", &artifact.photo_id)
        .header("slipstream-artifact-step-id", artifact.step_id.as_str())
        .header("slipstream-artifact-module", artifact.module.as_str())
        .header("slipstream-artifact-filename", artifact.filename())
        .header(
            "slipstream-artifact-published-at",
            artifact_timestamp(retention.published_at_unix_seconds),
        )
        .header(
            "slipstream-artifact-expires-at",
            artifact_timestamp(retention.expires_at_unix_seconds),
        )
        .header("slipstream-artifact-orientation", "top-left")
        .header("slipstream-artifact-icc-embedded", "true")
        .header(
            "slipstream-artifact-sample-format",
            &artifact.output_contract.precision,
        )
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
            format!("attachment; filename=\"{}\"", artifact.filename()),
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
        let poll = self.0.poll_recv(cx);
        if let std::task::Poll::Ready(None) = &poll {
            drop(self.1.take());
        }
        poll
    }
}

fn parse_artifact_range(value: &str, full_length: u64) -> Option<(u64, u64)> {
    let value = value.strip_prefix("bytes=")?;
    if value.is_empty() || value.contains(',') || value.chars().any(char::is_whitespace) {
        return None;
    }
    let (start, end) = value.split_once('-')?;
    if start.is_empty() {
        let suffix = end.parse::<u64>().ok()?;
        if suffix == 0 || full_length == 0 {
            return None;
        }
        return Some((full_length.saturating_sub(suffix), full_length - 1));
    }
    let start = start.parse::<u64>().ok()?;
    if start >= full_length {
        return None;
    }
    let end = if end.is_empty() {
        full_length - 1
    } else {
        end.parse::<u64>().ok()?.min(full_length - 1)
    };
    (start <= end).then_some((start, end))
}

#[cfg(test)]
mod tests {
    use super::parse_artifact_range;

    #[test]
    fn one_contiguous_range_is_the_only_admitted_form() {
        assert_eq!(parse_artifact_range("bytes=0-3", 10), Some((0, 3)));
        assert_eq!(parse_artifact_range("bytes=4-", 10), Some((4, 9)));
        assert_eq!(parse_artifact_range("bytes=-3", 10), Some((7, 9)));
        assert_eq!(parse_artifact_range("bytes=-99", 10), Some((0, 9)));
        assert_eq!(parse_artifact_range("bytes=7-99", 10), Some((7, 9)));
        // Multi-range, unattributable, and empty forms are refused, never
        // answered with a silently widened or narrowed byte span.
        for value in [
            "bytes=0-1,3-4",
            "bytes=0-1 2-3",
            "bytes=",
            "bytes=0",
            "items=0-1",
            "bytes=10-",
            "bytes=5-4",
            "bytes=-0",
            "bytes=99999999999999999999-",
        ] {
            assert_eq!(parse_artifact_range(value, 10), None, "for {value}");
        }
        assert_eq!(parse_artifact_range("bytes=0-", 0), None);
    }
}
