//! HTTP responses for durable composable Export admission outcomes.

use super::*;

pub(super) async fn outcome_response(
    state: &HttpState,
    outcome: ProcessingExportSubmitOutcome,
) -> Response {
    match outcome {
        ProcessingExportSubmitOutcome::Refused(refusal) => error_details(
            StatusCode::SERVICE_UNAVAILABLE,
            "module_parameters_unavailable",
            "The current Edit State has no qualified Export adapter in this deployment",
            json!({"refusal": refusal_json(&refusal), "replayed": false}),
        ),
        ProcessingExportSubmitOutcome::Replayed(refusal) => error_details(
            StatusCode::SERVICE_UNAVAILABLE,
            "module_parameters_unavailable",
            "The current Edit State has no qualified Export adapter in this deployment",
            json!({"refusal": refusal_json(&refusal), "replayed": true}),
        ),
        ProcessingExportSubmitOutcome::Admitted(_) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "outcome_unknown",
            "The admitted execution was not dispatched",
        ),
        ProcessingExportSubmitOutcome::Pending(admission) => {
            // A duplicate of one still-live accepted execution: the
            // committed receipt replays and no second execution starts.
            // The live state is read back from the same owner so the
            // receipt names exactly what the work record says.
            let receipt = match state
                .application
                .library
                .processing_export_work(&admission.request_id)
                .await
            {
                Ok(Some(work)) => work_json(&work),
                _ => {
                    let mut receipt = admission_json(&admission);
                    if let Some(object) = receipt.as_object_mut() {
                        object.insert("state".to_owned(), json!("accepted"));
                    }
                    receipt
                }
            };
            (
                StatusCode::ACCEPTED,
                Json(json!({"outcome": "replayed", "receipt": receipt})),
            )
                .into_response()
        }
        ProcessingExportSubmitOutcome::FailureReplayed(work) => error_details(
            StatusCode::CONFLICT,
            "export_terminal",
            "The Export request already reached a terminal decision",
            json!({"receipt": work_json(&work), "replayed": true}),
        ),
        ProcessingExportSubmitOutcome::CancelledReplayed(work) => error_details(
            StatusCode::CONFLICT,
            "export_terminal",
            "The Export request already reached a terminal decision",
            json!({"receipt": work_json(&work), "replayed": true}),
        ),
        ProcessingExportSubmitOutcome::Expired => error(
            StatusCode::GONE,
            "artifact_expired",
            "The settled artifact's retention window has passed",
        ),
        ProcessingExportSubmitOutcome::ReservationFull => error_details(
            StatusCode::SERVICE_UNAVAILABLE,
            "server_busy",
            "The Library holds its maximum live composable Exports",
            json!({"operation": "photos-processing-export", "retryAfterSeconds": null}),
        ),
        ProcessingExportSubmitOutcome::RetainedOutputFull => error_details(
            StatusCode::SERVICE_UNAVAILABLE,
            "retained_output_full",
            "The retained-output allowance cannot admit another composable artifact",
            json!({"operation": "photos-processing-export", "retryAfterSeconds": null}),
        ),
        ProcessingExportSubmitOutcome::ArtifactReplayed(artifact) => {
            artifact_created_response(state, &artifact, true).await
        }
        ProcessingExportSubmitOutcome::RecipeConflict(recipe) => error_details(
            StatusCode::CONFLICT,
            "recipe_conflict",
            "The expected Edit revision is no longer current",
            recipe
                .as_ref()
                .map(crate::processing_recipe::recipe_json)
                .unwrap_or_else(|| json!({})),
        ),
        ProcessingExportSubmitOutcome::SourceChanged(recipe) => error_details(
            StatusCode::CONFLICT,
            "source_changed",
            "The guarded source revision is no longer current",
            recipe
                .as_ref()
                .map(crate::processing_recipe::recipe_json)
                .unwrap_or_else(|| json!({})),
        ),
        ProcessingExportSubmitOutcome::MissingPhoto => error(
            StatusCode::NOT_FOUND,
            "unknown_photo",
            "The Photo is not part of the persisted Library",
        ),
        ProcessingExportSubmitOutcome::Unavailable => error(
            StatusCode::CONFLICT,
            "source_unavailable",
            "The guarded source revision is not currently available",
        ),
        ProcessingExportSubmitOutcome::MissingRecipe => error(
            StatusCode::NOT_FOUND,
            "missing_recipe",
            "Save an Edit State before submitting an Export",
        ),
        ProcessingExportSubmitOutcome::StepNotCurrent(_) => error(
            StatusCode::CONFLICT,
            "step_not_current",
            "Export is bounded to the current Edit State",
        ),
        ProcessingExportSubmitOutcome::IncompatibleInput(reason) => match reason {
            ProcessingInputHandoffError::OriginalPhotoMismatch
            | ProcessingInputHandoffError::OriginalSourceStale => error(
                StatusCode::CONFLICT,
                "source_changed",
                "The selected Original input is stale",
            ),
            ProcessingInputHandoffError::ArtifactMissing => error_details(
                StatusCode::UNPROCESSABLE_ENTITY,
                "incompatible_input",
                "The selected artifact input has no published Processing Artifact",
                json!({"reason": "artifact_missing"}),
            ),
            ProcessingInputHandoffError::ArtifactContractMismatch => error_details(
                StatusCode::UNPROCESSABLE_ENTITY,
                "incompatible_input",
                "The selected artifact input no longer matches its published contract",
                json!({"reason": "artifact_contract_mismatch"}),
            ),
        },
        ProcessingExportSubmitOutcome::Invalid(_) => error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_settings",
            "The submission carries a value outside the closed wire shape",
        ),
        ProcessingExportSubmitOutcome::RequestConflict => error(
            StatusCode::CONFLICT,
            "request_conflict",
            "The request identity was already used with a different payload",
        ),
    }
}
