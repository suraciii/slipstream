//! Service-owned execution and durable settlement of admitted exports.

use super::*;
use crate::processing_policy::ProcessingModulePolicy;

/// Executes one admitted qualified export through the deployment's
/// confined development workload. The durable attempt begins before the
/// heavy run, so a crash, failure, or cancellation always finds a
/// committed record, and every execution or validation failure is
/// recorded as the work's first terminal decision. The immutable artifact
/// is settled with the request's acceptance receipt in one serialized
/// owner operation; an unconfirmed settlement preserves the retained
/// bytes — only a definitive conflict or identity mismatch, where
/// persistence proves this execution's freshly minted bytes unclaimed,
/// removes them.
pub(super) async fn execute_admitted_export(
    state: &HttpState,
    processing: &crate::config::ProcessingConfig,
    admission: slipstream_core::ProcessingExportAdmission,
) -> Response {
    let exports = state
        .application
        .exports
        .as_ref()
        .expect("processing configured");
    let request_id = admission.request_id.clone();
    // The durable accepted receipt exists before the engine: the attempt
    // begins — `Executing` under a fresh sequence — in one owner
    // operation, and a first terminal decision that already won answers
    // for the committed record instead of this dispatch.
    match state
        .application
        .library
        .begin_processing_export_attempt(&request_id, unix_seconds_now())
        .await
    {
        Ok(slipstream_core::ProcessingExportAttemptOutcome::Began(_)) => {}
        Ok(slipstream_core::ProcessingExportAttemptOutcome::Terminal(work)) => {
            return terminal_work_response(state, work).await;
        }
        Ok(slipstream_core::ProcessingExportAttemptOutcome::Missing) | Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "outcome_unknown",
                "The admitted execution could not begin",
            );
        }
    }
    let parameters = slipstream_processing::modules::Parameters {
        module: admission.module.as_str().to_owned(),
        version: admission.parameters.schema_version.clone(),
        tree: admission.parameters.tree.clone(),
    };
    let executed = match (admission.module.as_str(), &admission.input) {
        (
            DARKTABLE_MODULE,
            slipstream_core::ProcessingInput::Original {
                photo_id: input_photo,
                source_revision,
            },
        ) => {
            exports
                .run_processing_export(&request_id, input_photo, source_revision, parameters)
                .await
        }
        (
            SPEKTRAFILM_MODULE,
            slipstream_core::ProcessingInput::Artifact {
                artifact_id,
                contract,
            },
        ) => {
            exports
                .run_film_export(&request_id, artifact_id.as_str(), contract, parameters)
                .await
        }
        _ => Err(NO_QUALIFIED_ADAPTER.to_owned()),
    };
    let executed = match executed {
        Ok(executed) => executed,
        Err(_) => {
            return fail_admitted_export(state, &request_id, EXECUTION_FAILED).await;
        }
    };
    let bundle_id = ProcessingModulePolicy::new(processing).bundle_id(admission.module.as_str());
    let artifact = match processing_artifact_of(&admission, &executed, &bundle_id) {
        Ok(artifact) => artifact,
        Err(_) => {
            // This execution's freshly minted bytes are definitively
            // unclaimed — no committed record can ever reference the
            // identity it just minted — so they are removed before the
            // terminal failure is recorded.
            delete_retained_artifact(exports, &executed.artifact_id, admission.module.as_str())
                .await;
            executed.publication.release().await;
            return fail_admitted_export(state, &request_id, RECORD_FORMATION_FAILED).await;
        }
    };
    match state
        .application
        .library
        .settle_processing_export(
            artifact.clone(),
            &request_id,
            &admission.payload_digest,
            unix_seconds_now(),
        )
        .await
    {
        Ok(slipstream_core::ProcessingExportSettlement::Settled(_)) => {
            executed.publication.release().await;
            artifact_created_response(state, &artifact, false).await
        }
        Ok(slipstream_core::ProcessingExportSettlement::Replayed(_)) => {
            executed.publication.release().await;
            // The committed publication owns this identity and identical
            // bytes already sit under it; the retained file stays.
            artifact_created_response(state, &artifact, true).await
        }
        Ok(slipstream_core::ProcessingExportSettlement::Terminal(work)) => {
            // The first terminal decision won while this attempt executed
            // — for example a cancellation — so this settlement was
            // refused unchanged and this execution's freshly minted bytes
            // are definitively unclaimed.
            delete_retained_artifact(exports, &executed.artifact_id, admission.module.as_str())
                .await;
            executed.publication.release().await;
            terminal_work_response(state, work).await
        }
        Ok(slipstream_core::ProcessingExportSettlement::Conflict) => {
            // This execution's identity definitively conflicts with the
            // committed record, so its private retained bytes are
            // unclaimed. The caller's outcome stays unconfirmed: the
            // write transaction may have committed while its
            // acknowledgement was lost, and the retained bytes of a
            // committed publication are never touched here.
            delete_retained_artifact(exports, &executed.artifact_id, admission.module.as_str())
                .await;
            executed.publication.release().await;
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "outcome_unknown",
                "The executed Export settlement is unconfirmed",
            )
        }
        Ok(slipstream_core::ProcessingExportSettlement::Missing) | Err(_) => {
            // An unconfirmed settlement preserves the bytes untouched;
            // only persistence can prove them claimed or unclaimed.
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "outcome_unknown",
                "The executed Export settlement is unconfirmed",
            )
        }
    }
}

/// Records one terminal execution failure durably — the bounded reason
/// code is the closed wire name — and answers with the committed record.
pub(super) async fn fail_admitted_export(
    state: &HttpState,
    request_id: &str,
    reason_code: &str,
) -> Response {
    match state
        .application
        .library
        .fail_processing_export(request_id, reason_code, unix_seconds_now())
        .await
    {
        Ok(slipstream_core::ProcessingExportFailureOutcome::Failed(work))
        | Ok(slipstream_core::ProcessingExportFailureOutcome::Terminal(work)) => {
            terminal_work_response(state, work).await
        }
        Ok(slipstream_core::ProcessingExportFailureOutcome::Missing)
        | Ok(slipstream_core::ProcessingExportFailureOutcome::Invalid(_))
        | Err(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "outcome_unknown",
            "The execution failure could not be recorded",
        ),
    }
}

/// Answers one request whose committed work record is terminal: the first
/// terminal decision wins, so a settled artifact replays its publication
/// and a failure or cancellation answers with the committed record.
pub(super) async fn terminal_work_response(
    state: &HttpState,
    work: slipstream_core::ProcessingExportWork,
) -> Response {
    match work.state {
        slipstream_core::ProcessingExportWorkState::Succeeded => {
            let Some(artifact_id) = work.artifact_id.as_ref() else {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "outcome_unknown",
                    "The settled Export artifact is unconfirmed",
                );
            };
            match state
                .application
                .library
                .processing_artifact(artifact_id.as_str())
                .await
            {
                Ok(Some(artifact)) => artifact_created_response(state, &artifact, true).await,
                _ => error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "outcome_unknown",
                    "The settled Export artifact is unconfirmed",
                ),
            }
        }
        slipstream_core::ProcessingExportWorkState::Failed
        | slipstream_core::ProcessingExportWorkState::Cancelled => error_details(
            StatusCode::CONFLICT,
            "export_terminal",
            "The Export request already reached a terminal decision",
            json!({"receipt": work_json(&work)}),
        ),
        slipstream_core::ProcessingExportWorkState::Accepted
        | slipstream_core::ProcessingExportWorkState::Executing => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "outcome_unknown",
            "The Export outcome is unconfirmed",
        ),
    }
}

/// Builds the immutable artifact record of one validated execution.
fn processing_artifact_of(
    admission: &slipstream_core::ProcessingExportAdmission,
    executed: &crate::export_manager::ProcessingExportExecution,
    bundle_id: &str,
) -> Result<slipstream_core::ProcessingArtifact, String> {
    let artifact_id = slipstream_core::ProcessingArtifactId::new(&executed.artifact_id)
        .map_err(|error| error.to_string())?;
    let geometry = slipstream_core::ProcessingGeometry::new(executed.width, executed.height)
        .map_err(|error| error.to_string())?;
    let output_contract = slipstream_core::ProcessingImageContract {
        format: executed.output.format.to_owned(),
        precision: executed.output.precision.to_owned(),
        color_space: executed.output.color_space.to_owned(),
        transfer: executed.output.transfer.to_owned(),
        geometry,
        encoding: executed.output.encoding.to_owned(),
    };
    output_contract
        .validate()
        .map_err(|error| error.to_string())?;
    let input = slipstream_core::ProcessingInputEvidence::new(
        admission.input.clone(),
        &executed.input_sha256,
        executed.input_size,
    )
    .map_err(|error| error.to_string())?;
    let artifact = slipstream_core::ProcessingArtifact {
        artifact_id,
        photo_id: admission.photo_id.clone(),
        step_id: admission.step_id.clone(),
        module: admission.module.clone(),
        adapter_schema_version: admission.adapter_schema_version.clone(),
        parameters: admission.parameters.clone(),
        input,
        output_contract,
        bundle_id: bundle_id.to_owned(),
        sha256: executed.sha256.clone(),
        byte_length: executed.size,
    };
    artifact.validate().map_err(|error| error.to_string())?;
    Ok(artifact)
}

async fn delete_retained_artifact(
    exports: &std::sync::Arc<crate::export_manager::ExportManager>,
    artifact_id: &str,
    module: &str,
) {
    if let Some(path) = exports.artifact_path_for_module(artifact_id, module) {
        let _ = tokio::fs::remove_file(path).await;
    }
}
