// Development Export route handlers.
// Photo Development Export surface. The routes share the closed error codes
// of the contract; a code is authoritative and no client parses messages.
use axum::{
    body::Body,
    extract::State,
    http::{Request, Response, StatusCode, header},
};
use serde::Deserialize;
use std::sync::Arc;

use super::{HttpState, cli_error, read_body_bytes};
use crate::Application;

fn export_error(status: StatusCode, code: &'static str, message: &'static str) -> Response<Body> {
    cli_error(status, code, message, serde_json::json!({}))
}

/// The `requestId` wire shape: 1 to 128 characters of ASCII letters, digits,
/// `.`, `_`, or `-`; unique per Photo and chosen by the caller.
fn valid_export_request_id(request_id: &str) -> bool {
    (1..=128).contains(&request_id.len())
        && request_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

/// The closed first-version Export targets.
const EXPORT_DEVELOPMENT_TIFF_TARGET: &str = "development-tiff";
const EXPORT_FILM_JPEG_TARGET: &str = "film-jpeg";

/// Reads one Export request body. Every body failure is a shape violation:
/// the wire contract refuses unknown fields, wrong types, and oversized or
/// malformed bodies with 422 `invalid_settings` before any state change.
async fn read_export_json_body<T: serde::de::DeserializeOwned>(
    request: Request<Body>,
) -> Result<T, Response<Body>> {
    let bytes = read_body_bytes(request).await.map_err(|_| {
        export_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_settings",
            "The request body is outside the closed wire shape",
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|_| {
        export_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_settings",
            "The request body is outside the closed wire shape",
        )
    })
}

/// The Export submission body. Unknown fields and values outside the closed
/// sets are refused before any state change.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExportSubmitBody {
    request_id: String,
    expected_recipe_version: String,
    expected_source_revision: String,
    target: String,
}

/// The explicit-retry body: one new caller-chosen request identity.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExportRetryBody {
    request_id: String,
}

fn unix_seconds_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn require_export_manager(
    application: &Arc<Application>,
) -> Result<Arc<crate::export_manager::ExportManager>, Box<Response<Body>>> {
    application.exports.as_ref().map(Arc::clone).ok_or_else(|| {
        Box::new(export_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "processing_unavailable",
            "Processing is not configured for this deployment",
        ))
    })
}

pub(crate) async fn submit_export(
    State(state): State<HttpState>,
    axum::extract::Path(photo_id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    let manager = match require_export_manager(&state.application) {
        Ok(manager) => manager,
        Err(response) => return *response,
    };
    let body: ExportSubmitBody = match read_export_json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    // Closed-set validation before any state change: the request identity
    // shape, the nonempty opaque revisions, and the closed target value. A
    // refused target creates no Export and no receipt.
    if !valid_export_request_id(&body.request_id)
        || body.expected_recipe_version.is_empty()
        || body.expected_source_revision.is_empty()
        || !matches!(
            body.target.as_str(),
            EXPORT_DEVELOPMENT_TIFF_TARGET | EXPORT_FILM_JPEG_TARGET
        )
    {
        return export_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_settings",
            "The submission carries a value outside the closed wire shape",
        );
    }
    // One serialized owner read: the Photo facts, the capture identity, and
    // the recipe source availability below all come from a single published
    // state, so a scan publication cannot change them mid-submission.
    let Some((photo, read)) = state
        .application
        .library
        .edit_recipe_surface(&photo_id)
        .await
        .ok()
        .flatten()
    else {
        return export_error(
            StatusCode::NOT_FOUND,
            "unknown_photo",
            "The Photo is not part of the published Library",
        );
    };
    // Receipt resolution precedes every source probe: a recorded identity
    // replays, expires, or conflicts without reading or classifying the
    // source, and a fresh identity flows on to classification and
    // admission. The digest covers only the caller's payload, so a
    // deployment bundle or policy change cannot break a replay.
    let payload_digest = slipstream_core::export_submission_payload_digest(
        &body.target,
        &body.expected_recipe_version,
        &body.expected_source_revision,
    );
    match state
        .application
        .library
        .resolve_export_receipt(&photo_id, &body.request_id, &payload_digest)
        .await
    {
        Ok(Some(slipstream_core::ExportSubmissionResolution::Existing(record))) => {
            return crate::http::json_response(
                StatusCode::OK,
                &crate::wire::export_submit(&record),
            );
        }
        Ok(Some(slipstream_core::ExportSubmissionResolution::Expired)) => {
            return export_error(
                StatusCode::GONE,
                "export_expired",
                "The request identity expired and cannot start new work",
            );
        }
        Ok(Some(slipstream_core::ExportSubmissionResolution::Conflict)) => {
            return export_error(
                StatusCode::CONFLICT,
                "export_conflict",
                "The request identity was already used with a different payload",
            );
        }
        Ok(None) => {}
        Err(_) => {
            return export_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "outcome_unknown",
                "The submission outcome is unconfirmed",
            );
        }
    }
    // The Film qualification gate follows receipt resolution: a recorded
    // Film identity still replays, expires, or conflicts from its receipt
    // alone, but no fresh Film identity is admitted. The photo-processing
    // launcher qualifies only the Development workload, so a full-resolution
    // Film Export has no qualified stage to run; the refusal is a closed
    // `processing_unavailable` and records neither an Export nor a receipt.
    if body.target == EXPORT_FILM_JPEG_TARGET {
        return export_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "processing_unavailable",
            "The Film stage is not qualified for a full-resolution Export",
        );
    }
    let proxy_profile = if !read.source_available {
        match state.application.proxies.as_ref() {
            Some(manager) => manager
                .current_artifact(&photo_id)
                .await
                .map(|(proxy, _)| proxy.profile_id),
            None => None,
        }
    } else {
        None
    };
    let support = crate::edit_recipe::derive_support(
        crate::edit_recipe::source_facts(&photo),
        read.source_available,
        photo.original_available,
        read.current_source_revision.as_deref(),
    );
    let source_profile_id = if let Some(profile_id) = proxy_profile {
        profile_id
    } else {
        match support.state {
            "unsupported" => {
                return export_error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "unsupported_photo",
                    "This Photo's source class has no approved profile.",
                );
            }
            "unavailable" => {
                return cli_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "resource_unavailable",
                    "Current source facts cannot be read, so no Export can be admitted.",
                    serde_json::json!({"photoId": photo_id, "supportReason": support.reason}),
                );
            }
            _ => {}
        }
        let slipstream_core::CameraIdentity::Observed { make, model } = &photo.capture.identity
        else {
            return export_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "resource_unavailable",
                "Current source facts cannot be read, so no Export can be admitted.",
            );
        };
        let (Some(make), Some(model)) = (make.as_deref(), model.as_deref()) else {
            return export_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "unsupported_photo",
                "The camera identity of the source could not be read",
            );
        };
        let Some(container) =
            slipstream_processing::photo_profile::container_of_filename(&photo.filename)
        else {
            return export_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "unsupported_photo",
                "The source has no RAW container",
            );
        };
        let Some(profile) = slipstream_processing::photo_profile::classify(make, model, &container)
        else {
            return export_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "unsupported_photo",
                "The source class has no approved profile",
            );
        };
        profile.profile_id.to_owned()
    };
    let submission = slipstream_core::ExportSubmission {
        request_id: body.request_id,
        photo_id,
        source_profile_id,
        workload: body.target,
        policy_id: state
            .processing
            .as_ref()
            .map(|processing| processing.policy_sha256.clone())
            .unwrap_or_default(),
        bundle_id: state
            .processing
            .as_ref()
            .map(|processing| processing.bundle_sha256.clone())
            .unwrap_or_default(),
        expected_recipe_revision: body.expected_recipe_version,
        expected_source_revision: body.expected_source_revision,
        exposure_range: slipstream_core::ExportExposureRange {
            minimum_milli_ev: slipstream_processing::photo_profile::APPROVED_EXPOSURE_MILLI_EV_MIN,
            maximum_milli_ev: slipstream_processing::photo_profile::APPROVED_EXPOSURE_MILLI_EV_MAX,
        },
        retained_output_bytes_max: manager.allowance(),
    };
    if manager.ensure_admissible().await.is_err() {
        return export_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "processing_unavailable",
            "The processing launcher is not admitting export work",
        );
    }
    match state.application.library.submit_export(submission).await {
        Ok(outcome) => match outcome {
            slipstream_core::ExportSubmitOutcome::Created(record) => {
                manager.start(record.clone());
                crate::http::json_response(
                    StatusCode::CREATED,
                    &crate::wire::export_submit(&record),
                )
            }
            slipstream_core::ExportSubmitOutcome::Existing(record) => {
                crate::http::json_response(StatusCode::OK, &crate::wire::export_submit(&record))
            }
            slipstream_core::ExportSubmitOutcome::RequestConflict => export_error(
                StatusCode::CONFLICT,
                "export_conflict",
                "The request identity was already used with a different payload",
            ),
            slipstream_core::ExportSubmitOutcome::RequiresRebind => export_error(
                StatusCode::CONFLICT,
                "requires_rebind",
                "The stored recipe is bound to a different source than the current revision",
            ),
            slipstream_core::ExportSubmitOutcome::Expired => export_error(
                StatusCode::GONE,
                "export_expired",
                "The request identity expired and cannot start new work",
            ),
            slipstream_core::ExportSubmitOutcome::UnknownPhoto => export_error(
                StatusCode::NOT_FOUND,
                "unknown_photo",
                "The Photo is not part of the persisted Library",
            ),
            slipstream_core::ExportSubmitOutcome::UnsupportedPhoto => export_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "unsupported_photo",
                "The source class has no approved profile",
            ),
            slipstream_core::ExportSubmitOutcome::MissingRecipe => export_error(
                StatusCode::NOT_FOUND,
                "missing_recipe",
                "Save the Edit Recipe before submitting an Export",
            ),
            slipstream_core::ExportSubmitOutcome::RecipeConflict(_) => export_error(
                StatusCode::CONFLICT,
                "recipe_conflict",
                "The expected recipe version is no longer current",
            ),
            slipstream_core::ExportSubmitOutcome::SourceChanged(_) => export_error(
                StatusCode::CONFLICT,
                "source_changed",
                "The published source revision changed before acceptance",
            ),
            slipstream_core::ExportSubmitOutcome::InvalidSettings => export_error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_settings",
                "The saved recipe is outside the approved execution range",
            ),
            slipstream_core::ExportSubmitOutcome::OriginalRequired => export_error(
                StatusCode::CONFLICT,
                "original_required",
                "The Original is required for a full-resolution Export",
            ),
            slipstream_core::ExportSubmitOutcome::RetainedOutputFull => export_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "retained_output_full",
                "The retained-output allowance cannot admit another artifact",
            ),
            slipstream_core::ExportSubmitOutcome::Unavailable => export_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "processing_unavailable",
                "Current source facts cannot be read",
            ),
        },
        Err(_) => export_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "outcome_unknown",
            "The submission outcome is unconfirmed",
        ),
    }
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

pub(crate) async fn cancel_export(
    State(state): State<HttpState>,
    axum::extract::Path(export_id): axum::extract::Path<String>,
) -> Response<Body> {
    // The route carries no request fields; any body is outside the closed
    // shape.
    let manager = match require_export_manager(&state.application) {
        Ok(manager) => manager,
        Err(response) => return *response,
    };
    match manager.cancel(&export_id).await {
        crate::export_manager::ExportCancelOutcome::Settled(record) => {
            crate::http::json_response(StatusCode::OK, &crate::wire::export_cancel(&record))
        }
        crate::export_manager::ExportCancelOutcome::Unknown => export_error(
            StatusCode::NOT_FOUND,
            "unknown_export",
            "The Export identity is unknown or expired",
        ),
        crate::export_manager::ExportCancelOutcome::Uncertain => export_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "outcome_unknown",
            "The cancellation outcome is unconfirmed; reconcile through inspection",
        ),
    }
}

pub(crate) async fn retry_export(
    State(state): State<HttpState>,
    axum::extract::Path(export_id): axum::extract::Path<String>,
    request: Request<Body>,
) -> Response<Body> {
    let manager = match require_export_manager(&state.application) {
        Ok(manager) => manager,
        Err(response) => return *response,
    };
    let body: ExportRetryBody = match read_export_json_body(request).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    if !valid_export_request_id(&body.request_id) {
        return export_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_settings",
            "The retry carries a request identity outside the closed wire shape",
        );
    }
    let expected_bundle_id = state
        .processing
        .as_ref()
        .map(|processing| processing.bundle_sha256.clone())
        .unwrap_or_default();
    match state
        .application
        .library
        .retry_export(
            &export_id,
            &body.request_id,
            &expected_bundle_id,
            manager.allowance(),
        )
        .await
    {
        Ok(outcome) => match outcome {
            slipstream_core::ExportRetryOutcome::Retried(record) => {
                let record = *record;
                manager.start(record.clone());
                crate::http::json_response(
                    StatusCode::ACCEPTED,
                    &crate::wire::export_retry(&record),
                )
            }
            slipstream_core::ExportRetryOutcome::Replayed(record) => {
                // An accepted retry identity resolves to its Export; no new
                // attempt starts.
                crate::http::json_response(
                    StatusCode::ACCEPTED,
                    &crate::wire::export_retry(&record),
                )
            }
            slipstream_core::ExportRetryOutcome::Unknown => export_error(
                StatusCode::NOT_FOUND,
                "unknown_export",
                "The Export identity is unknown or expired",
            ),
            slipstream_core::ExportRetryOutcome::RequestConflict => export_error(
                StatusCode::CONFLICT,
                "request_conflict",
                "The retry request identity was already used with a different payload",
            ),
            slipstream_core::ExportRetryOutcome::NotRetriable => export_error(
                StatusCode::CONFLICT,
                "export_conflict",
                "Only a failed or cancelled Export within retention can be retried",
            ),
            slipstream_core::ExportRetryOutcome::Expired => export_error(
                StatusCode::GONE,
                "export_expired",
                "The retained snapshot expired and cannot be retried",
            ),
            slipstream_core::ExportRetryOutcome::OutputUnavailable => export_error(
                StatusCode::CONFLICT,
                "output_unavailable",
                "The captured source or approved bundle is no longer available",
            ),
            slipstream_core::ExportRetryOutcome::ResourceUnavailable => export_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "resource_unavailable",
                "Current source facts cannot be read",
            ),
            slipstream_core::ExportRetryOutcome::RetainedOutputFull => export_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "retained_output_full",
                "The retained-output allowance cannot admit another artifact",
            ),
        },
        Err(_) => export_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "outcome_unknown",
            "The retry outcome is unconfirmed",
        ),
    }
}

pub(crate) async fn get_export_artifact(
    State(state): State<HttpState>,
    axum::extract::Path(export_id): axum::extract::Path<String>,
) -> Response<Body> {
    let manager = match require_export_manager(&state.application) {
        Ok(manager) => manager,
        Err(response) => return *response,
    };
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
    let Some(path) = manager.artifact_path_for_workload(&export_id, &record.snapshot.workload)
    else {
        release_lease.await;
        return export_error(
            StatusCode::NOT_FOUND,
            "unknown_export",
            "The Export identity is invalid",
        );
    };
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
        let interval = manager.lease_renewal_interval();
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
    let extension = if record.snapshot.workload == "film-jpeg" {
        "jpg"
    } else {
        "tiff"
    };
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
            format!("attachment; filename=\"{export_id}.{extension}\""),
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
