// Read-only historical Export records and retained artifact delivery.
use axum::{
    body::Body,
    extract::State,
    http::{Response, StatusCode, header},
};
use std::sync::Arc;

use super::{HttpState, cli_error};

fn export_error(status: StatusCode, code: &'static str, message: &'static str) -> Response<Body> {
    cli_error(status, code, message, serde_json::json!({}))
}

fn unix_seconds_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) async fn list_photo_exports(
    State(state): State<HttpState>,
    axum::extract::Path(photo_id): axum::extract::Path<String>,
) -> Response<Body> {
    match state.application.library.photo_exports(&photo_id).await {
        Ok(Some(records)) => crate::http::json_response(
            StatusCode::OK,
            &crate::wire::ExportListWire {
                exports: records.iter().map(crate::wire::export_summary).collect(),
            },
        ),
        Ok(None) => export_error(
            StatusCode::NOT_FOUND,
            "unknown_photo",
            "The Photo is not part of the persisted Library",
        ),
        Err(_) => export_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "processing_unavailable",
            "The retained Exports could not be read",
        ),
    }
}

pub(crate) async fn get_export(
    State(state): State<HttpState>,
    axum::extract::Path(export_id): axum::extract::Path<String>,
) -> Response<Body> {
    match state.application.library.export(&export_id).await {
        Ok(Some(record)) => {
            crate::http::json_response(StatusCode::OK, &crate::wire::export_inspect(&record))
        }
        Ok(None) => export_error(
            StatusCode::NOT_FOUND,
            "unknown_export",
            "The Export identity is unknown or expired",
        ),
        Err(_) => export_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "processing_unavailable",
            "The Export state could not be read",
        ),
    }
}

pub(crate) async fn get_export_artifact(
    State(state): State<HttpState>,
    axum::extract::Path(export_id): axum::extract::Path<String>,
) -> Response<Body> {
    let library = &state.application.library;
    let Some(record) = library.export(&export_id).await.ok().flatten() else {
        return export_error(
            StatusCode::NOT_FOUND,
            "unknown_export",
            "The Export identity is unknown or expired",
        );
    };
    // The closed per-state refusals come before any lease is taken.
    let Some(artifact) = crate::wire::export_artifact_object(&record) else {
        return match record.state {
            slipstream_core::ExportState::Queued | slipstream_core::ExportState::Running => {
                export_error(
                    StatusCode::CONFLICT,
                    "export_conflict",
                    "The Export has not settled yet",
                )
            }
            _ => export_error(
                StatusCode::CONFLICT,
                "output_unavailable",
                "The Export has no validated retained artifact",
            ),
        };
    };
    let lease = library
        .acquire_export_lease(&export_id, unix_seconds_now())
        .await;
    let lease_id = match lease {
        Ok(slipstream_core::ExportLeaseOutcome::Acquired { lease_id, .. }) => lease_id,
        Ok(slipstream_core::ExportLeaseOutcome::Expired) => {
            return export_error(
                StatusCode::GONE,
                "artifact_expired",
                "The artifact retention window has passed",
            );
        }
        Ok(slipstream_core::ExportLeaseOutcome::Unknown) => {
            return export_error(
                StatusCode::CONFLICT,
                "output_unavailable",
                "The Export has no validated retained artifact",
            );
        }
        Err(_) => {
            return export_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "processing_unavailable",
                "The download lease could not be persisted",
            );
        }
    };
    let release_lease = {
        let library = Arc::clone(&state.application.library);
        let lease_id = lease_id.clone();
        async move {
            let _ = library.release_export_lease(&lease_id).await;
        }
    };
    let valid_id = (1..=128).contains(&export_id.len())
        && export_id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        });
    let extension = match record.snapshot.workload.as_str() {
        "development-tiff" => "tiff",
        "film-jpeg" => "jpg",
        _ => {
            release_lease.await;
            return export_error(
                StatusCode::CONFLICT,
                "output_unavailable",
                "The retained output contract is unknown",
            );
        }
    };
    if !valid_id || export_id == "." || export_id == ".." {
        release_lease.await;
        return export_error(
            StatusCode::NOT_FOUND,
            "unknown_export",
            "The Export identity is invalid",
        );
    }
    let path = state
        .application
        .export_artifacts_directory
        .join(format!("{export_id}.{extension}"));
    let file = match tokio::fs::File::open(&path).await {
        Ok(file) => file,
        Err(_) => {
            release_lease.await;
            return export_error(
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
    // A live stream keeps its lease's liveness anchor fresh so the staleness
    // sweep can never reclaim it before the response settles.
    let renewer = {
        let library = Arc::clone(&state.application.library);
        let lease_id = lease_id.clone();
        let interval = state
            .application
            .exports
            .as_ref()
            .map_or(std::time::Duration::from_secs(30), |manager| {
                manager.lease_renewal_interval()
            });
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            ticker.tick().await;
            loop {
                ticker.tick().await;
                if library
                    .renew_export_lease(&lease_id, unix_seconds_now())
                    .await
                    .unwrap_or(false)
                {
                    continue;
                }
                break;
            }
        })
    };
    let release_on_settle = {
        let library = Arc::clone(&state.application.library);
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
            // The producer reaching end of file is not the stream settling:
            // the lease holds until the response body drains or is dropped,
            // so a slow client keeps its protection.
            drop(sender);
            let _ = body_done_rx.await;
            let _ = library.release_export_lease(&lease_id).await;
            renewer.abort();
        })
    };
    drop(release_on_settle);
    // The typed metadata framing for this route is response headers; they are
    // the artifact object field for field, so a client validates the download
    // by comparing every header with `GET /api/exports/{id}`.
    let builder = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, artifact.content_type)
        .header(header::CONTENT_LENGTH, artifact.byte_length.to_string())
        .header(header::CACHE_CONTROL, "no-store")
        .header("x-content-type-options", "nosniff")
        .header("slipstream-artifact-export-id", &artifact.export_id)
        .header("slipstream-artifact-target", artifact.target)
        .header("slipstream-artifact-stage", artifact.stage)
        .header("slipstream-artifact-content-type", artifact.content_type)
        .header("slipstream-artifact-filename", &artifact.filename)
        .header("slipstream-artifact-orientation", artifact.orientation)
        .header("slipstream-artifact-sample-format", artifact.sample_format)
        .header("slipstream-artifact-color-space", artifact.color_space)
        .header(
            "slipstream-artifact-icc-embedded",
            artifact.icc_embedded.to_string(),
        )
        .header("slipstream-artifact-width", artifact.width.to_string())
        .header("slipstream-artifact-height", artifact.height.to_string())
        .header(
            "slipstream-artifact-profile-identity",
            &artifact.profile_identity,
        )
        .header(
            "slipstream-artifact-byte-length",
            artifact.byte_length.to_string(),
        )
        .header("slipstream-artifact-sha256", &artifact.sha256)
        .header("slipstream-artifact-expires-at", &artifact.expires_at)
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", artifact.filename),
        );
    builder
        .body(Body::from_stream(ExportFileStream(
            receiver,
            Some(body_done_tx),
        )))
        .expect("valid artifact response")
}

/// `tokio`'s mpsc receiver has no `Stream` impl without tokio-stream, so the
/// artifact body adapts it through the receiver's own `poll_recv`.
struct ExportFileStream(
    tokio::sync::mpsc::Receiver<Result<Vec<u8>, std::io::Error>>,
    // Dropping the response body — after it drains or is abandoned — fires
    // this signal, which releases the download lease in the pump task.
    Option<tokio::sync::oneshot::Sender<()>>,
);

impl Drop for ExportFileStream {
    fn drop(&mut self) {
        // Dropping the completion sender is itself the signal: the pump
        // task's receive side resolves as cancelled and releases the lease.
        drop(self.1.take());
    }
}

impl futures_core::Stream for ExportFileStream {
    type Item = Result<Vec<u8>, std::io::Error>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.0.poll_recv(cx)
    }
}
