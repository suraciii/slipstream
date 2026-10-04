//! Stateful Agent editing CLI facade for GitHub Issue #509.

use super::{
    AdmissionState, CommandFailure, MutationIdentity, Operation, PublicationState, ServiceClient,
    json, processing_preview_download, valid_request_identity,
};
use clap::Subcommand;
use reqwest::{Method, StatusCode};
use serde_json::Value;

use std::path::PathBuf;
#[derive(Debug, Subcommand)]
pub enum EditCommand {
    /// Read the current stateful Edit State without exposing the complete recipe tree.
    Get {
        #[arg(value_parser = crate::nonempty)]
        photo_id: String,
    },
    /// Set one qualified Engine Module control.
    Set {
        #[arg(value_parser = crate::nonempty)]
        photo_id: String,
        #[arg(value_name = "ENGINE.MODULE", value_parser = crate::nonempty)]
        target: String,
        #[arg(value_name = "CONTROL", value_parser = crate::nonempty)]
        control: String,
        #[arg(value_name = "JSON_VALUE")]
        value: String,
        #[arg(long, value_name = "original|artifact:ARTIFACT_ID")]
        from: Option<String>,
        #[arg(long, value_name = "REVISION", value_parser = crate::nonempty)]
        revision: Option<String>,
        #[arg(long, value_name = "REQUEST_ID", value_parser = crate::nonempty)]
        request: String,
    },
    /// Reset one qualified Engine Module control to its discovery reset value.
    Reset {
        #[arg(value_parser = crate::nonempty)]
        photo_id: String,
        #[arg(value_name = "ENGINE.MODULE", value_parser = crate::nonempty)]
        target: String,
        #[arg(value_name = "CONTROL", value_parser = crate::nonempty)]
        control: String,
        #[arg(long, value_name = "REVISION", value_parser = crate::nonempty, required = true)]
        revision: String,
        #[arg(long, value_name = "REQUEST_ID", value_parser = crate::nonempty)]
        request: String,
    },
    /// Download a Preview of the current stateful edit.
    Preview {
        #[arg(value_parser = crate::nonempty)]
        photo_id: String,
        #[arg(long, value_name = "PATH", required = true)]
        file: PathBuf,
    },
    /// Submit the current stateful edit for an explicit Export.
    Export {
        #[arg(value_parser = crate::nonempty)]
        photo_id: String,
        #[arg(long, value_name = "REVISION", value_parser = crate::nonempty, required = true)]
        revision: String,
        #[arg(long, value_name = "REQUEST_ID", value_parser = crate::nonempty)]
        request: String,
    },
    /// Read the durable result of one stateful Export request.
    ExportStatus {
        #[arg(value_parser = crate::nonempty)]
        photo_id: String,
        #[arg(value_parser = crate::nonempty)]
        request_id: String,
    },
}

fn validate_request(request: &str) -> Result<(), CommandFailure> {
    if valid_request_identity(request) {
        Ok(())
    } else {
        Err(CommandFailure::invalid(
            "requestId",
            "The request ID must be 1 to 128 characters of ASCII letters, digits, `.`, `_`, or `-`.",
        ))
    }
}

fn state_revision(state: &Value, key: &str) -> Result<String, CommandFailure> {
    state[key]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            CommandFailure::invalid(key, "The current Edit State has no usable revision.")
        })
}

fn state_step(state: &Value) -> Result<String, CommandFailure> {
    state["currentStepId"]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| {
            CommandFailure::invalid("currentStepId", "The Photo has no current stateful edit.")
        })
}

async fn get_state(client: &ServiceClient, photo_id: &str) -> Result<Value, CommandFailure> {
    let mut state: Value = client
        .json(
            Operation::PhotosProcessingRecipeGet,
            Method::GET,
            client.endpoint(&["api", "photos", photo_id, "edit"]),
            None,
        )
        .await?;
    if state["photoId"] != photo_id
        || !state["sourceAvailable"].is_boolean()
        || !state["engineModules"].is_array()
    {
        return Err(CommandFailure::transport(
            Operation::PhotosProcessingRecipeGet,
        ));
    }
    state["webUrl"] = Value::String(
        super::web_url(&client.origin, &format!("/?photoId={photo_id}"))
            .map_err(|()| CommandFailure::transport(Operation::PhotosProcessingRecipeGet))?,
    );
    Ok(state)
}

async fn mutate(
    client: &ServiceClient,
    admission: &AdmissionState,
    photo_id: &str,
    request: &str,
    endpoint: &str,
    body: Value,
) -> Result<Value, CommandFailure> {
    validate_request(request)?;
    let identity = MutationIdentity {
        operation: Operation::PhotosProcessingRecipeSave,
        photo_ids: vec![photo_id.to_owned()],
        album_id: None,
        album_name: None,
        mappings: Vec::new(),
    };
    let result: Value = client
        .mutation_admitting(
            &identity,
            admission,
            client.endpoint(&["api", "photos", photo_id, "edit", endpoint]),
            body,
            &[StatusCode::OK, StatusCode::CREATED],
        )
        .await?;
    if !result["outcome"].is_string()
        || !result["edit"]["photoId"]
            .as_str()
            .is_some_and(|id| id == photo_id)
    {
        return Err(CommandFailure::unknown(&identity));
    }
    Ok(result)
}

pub(crate) async fn execute(
    client: &ServiceClient,
    admission: &AdmissionState,
    command: &EditCommand,
    publication: &PublicationState,
) -> Result<Value, CommandFailure> {
    let destination = preflight(command)?;
    match command {
        EditCommand::Get { photo_id } => get_state(client, photo_id).await,
        EditCommand::Set {
            photo_id,
            target,
            control,
            value,
            from,
            revision,
            request,
        } => {
            let parsed: Value = serde_json::from_str(value)
                .map_err(|_| CommandFailure::invalid("value", "VALUE must be one JSON value."))?;
            mutate(
                client,
                admission,
                photo_id,
                request,
                "set",
                json!({
                    "requestId": request,
                    "expectedEditRevision": revision,
                    "target": target,
                    "control": control,
                    "value": parsed,
                    "from": from,
                }),
            )
            .await
        }
        EditCommand::Reset {
            photo_id,
            target,
            control,
            revision,
            request,
        } => {
            mutate(
                client,
                admission,
                photo_id,
                request,
                "reset",
                json!({
                    "requestId": request,
                    "expectedEditRevision": revision,
                    "target": target,
                    "control": control,
                }),
            )
            .await
        }
        EditCommand::Preview { photo_id, .. } => {
            let state = get_state(client, photo_id).await?;
            let step_id = state_step(&state)?;
            processing_preview_download::download(
                client,
                photo_id,
                &step_id,
                destination.expect("stateful edit Preview destination was checked"),
                publication,
            )
            .await
        }
        EditCommand::Export {
            photo_id,
            revision,
            request,
        } => {
            validate_request(request)?;
            let state = get_state(client, photo_id).await?;
            let step_id = state_step(&state)?;
            let source_revision = state_revision(&state, "sourceRevision")?;
            let args = super::development::ProcessingExportArgs {
                photo_id: photo_id.clone(),
                input: String::new(),
            };
            super::development::execute_processing_export(
                client,
                admission,
                &args,
                json!({
                    "requestId": request,
                    "stepId": step_id,
                    "expectedRecipeRevision": revision,
                    "expectedSourceRevision": source_revision,
                }),
            )
            .await
        }
        EditCommand::ExportStatus {
            photo_id,
            request_id,
        } => super::development::processing_export_status(client, photo_id, request_id).await,
    }
}

fn preflight(
    command: &EditCommand,
) -> Result<Option<super::preview_download::Destination>, CommandFailure> {
    match command {
        EditCommand::Preview { file, .. } => {
            Ok(Some(super::preview_download::Destination::preflight(
                super::preview_download::DestinationKind::ProcessingPreview,
                file,
            )?))
        }
        _ => Ok(None),
    }
}
