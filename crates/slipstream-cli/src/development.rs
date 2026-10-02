//! Composable recipe intent, selected-step execution, and durable Export reconciliation.
use super::{
    AdmissionState, CommandFailure, MutationIdentity, Operation, ServiceClient, read_input_bytes,
    valid_request_identity, valid_sha256, web_url,
};
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use std::collections::HashSet;
use url::Url;

const MODULES_OPERATION: Operation = Operation::ProcessingModules;
const PROCESSING_GET_OPERATION: Operation = Operation::PhotosProcessingRecipeGet;
const PROCESSING_SAVE_OPERATION: Operation = Operation::PhotosProcessingRecipeSave;
const PROCESSING_REBIND_OPERATION: Operation = Operation::PhotosProcessingRecipeRebind;
const PROCESSING_EXPORT_OPERATION: Operation = Operation::PhotosProcessingExport;
const ARTIFACT_OPERATION: Operation = Operation::ProcessingArtifact;
const PROCESSING_EXPORT_STATUS_OPERATION: Operation = Operation::PhotosProcessingExportStatus;
const PROCESSING_EXPORT_CANCEL_OPERATION: Operation = Operation::PhotosProcessingExportCancel;
const PROCESSING_EXPORT_LIST_OPERATION: Operation = Operation::PhotosProcessingExportList;
const PROCESSING_EXPORT_RETRY_OPERATION: Operation = Operation::PhotosProcessingExportRetry;

fn required_nullable_string<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}
fn required_nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(deserializer)
}
fn validate_request_identity(request_id: &str) -> Result<(), CommandFailure> {
    if valid_request_identity(request_id) {
        Ok(())
    } else {
        Err(CommandFailure::invalid(
            "requestId",
            "The requestId must be 1 to 128 characters of ASCII letters, digits, `.`, `_`, or `-`.",
        ))
    }
}
fn validate_nonempty_revision(
    argument: &str,
    revision: Option<&str>,
) -> Result<(), CommandFailure> {
    let maximum = if argument == "currentStepId" {
        MAXIMUM_STEP_ID_BYTES
    } else {
        MAXIMUM_REVISION_BYTES
    };
    if revision.is_none_or(|value| !value.is_empty() && value.len() <= maximum) {
        Ok(())
    } else {
        Err(CommandFailure::invalid(
            argument,
            "The revision or step guard exceeds its disclosed bound or is empty.",
        ))
    }
}
fn validate_source_revision(argument: &str, revision: &str) -> Result<(), CommandFailure> {
    if bounded_source_revision(revision) {
        Ok(())
    } else {
        Err(CommandFailure::invalid(
            argument,
            "The source revision must be 1 through 16384 bytes.",
        ))
    }
}
pub(super) fn unusable_input() -> CommandFailure {
    CommandFailure::invalid("input", "The input document could not be rendered.")
}
mod composable;
pub use composable::ProcessingRecipeCommand;
pub(super) use composable::*;
mod processing_export;
pub use processing_export::ProcessingExportArgs;
pub(crate) use processing_export::processing_work_valid;
pub(super) use processing_export::*;

pub(super) async fn modules(client: &ServiceClient) -> Result<Value, CommandFailure> {
    let report: Value = client
        .json(
            MODULES_OPERATION,
            Method::GET,
            client.endpoint(&["api", "processing", "modules"]),
            None,
        )
        .await?;
    if !report.get("modules").is_some_and(Value::is_array)
        || !report.get("contractVersion").is_some_and(Value::is_string)
    {
        return Err(CommandFailure::transport(MODULES_OPERATION));
    }
    Ok(report)
}
#[cfg(test)]
mod tests;
