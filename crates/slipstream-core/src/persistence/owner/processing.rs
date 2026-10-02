use super::Reply;
use crate::persistence::{DatabaseName, StateDirectory, composable_recipe, processing_export};
use crate::{ComposableEditRecipe, ComposableEditRecipeWriteOutcome, SaveComposableEditRecipe};
use rusqlite::Connection;

pub(in crate::persistence) enum Command {
    ReadComposableEditRecipe {
        photo_id: String,
        reply: Reply<Option<ComposableEditRecipe>>,
    },
    ReadComposableEditRecipeFacts {
        photo_id: String,
        reply: Reply<Option<crate::processing::ComposableEditRecipeRead>>,
    },
    RebindComposableEditRecipe(
        crate::processing::RebindComposableEditRecipe,
        Reply<ComposableEditRecipeWriteOutcome>,
    ),
    ReplayComposableEditRecipe(
        SaveComposableEditRecipe,
        Reply<Option<ComposableEditRecipeWriteOutcome>>,
    ),
    SaveComposableEditRecipe(
        SaveComposableEditRecipe,
        Reply<ComposableEditRecipeWriteOutcome>,
    ),
    ReadProcessingArtifact {
        artifact_id: String,
        reply: Reply<Option<crate::processing::ProcessingArtifact>>,
    },
    PublishProcessingArtifact(
        Box<crate::processing::ProcessingArtifact>,
        Reply<crate::processing::ProcessingArtifactPublication>,
    ),
    SubmitProcessingExport(
        crate::processing::SubmitProcessingExport,
        u64,
        Reply<crate::processing::ProcessingExportSubmitOutcome>,
    ),
    ListProcessingExports(String, u64, Reply<crate::processing::ProcessingExportList>),
    ReplayProcessingExport(
        crate::processing::ReplayProcessingExport,
        Reply<Option<crate::processing::ProcessingExportSubmitOutcome>>,
    ),
    RetryProcessingExport(
        crate::processing::RetryProcessingExport,
        u64,
        Reply<crate::processing::ProcessingExportSubmitOutcome>,
    ),
    ReadProcessingArtifactRetention(
        String,
        Reply<Option<crate::processing::ProcessingArtifactRetention>>,
    ),
    ReplayProcessingExportRetry {
        photo_id: String,
        previous_request_id: String,
        request_id: String,
        reply: Reply<Option<crate::processing::ProcessingExportSubmitOutcome>>,
    },
    SettleProcessingExport {
        artifact: Box<crate::processing::ProcessingArtifact>,
        request_id: String,
        payload_digest: String,
        now: u64,
        reply: Reply<crate::processing::ProcessingExportSettlement>,
    },
    ReadProcessingExportWork {
        request_id: String,
        reply: Reply<Option<crate::processing::ProcessingExportWork>>,
    },
    BeginProcessingExportAttempt {
        request_id: String,
        now: u64,
        reply: Reply<crate::processing::ProcessingExportAttemptOutcome>,
    },
    FailProcessingExport {
        request_id: String,
        reason_code: String,
        now: u64,
        reply: Reply<crate::processing::ProcessingExportFailureOutcome>,
    },
    CancelProcessingExport {
        request_id: String,
        now: u64,
        reply: Reply<crate::processing::ProcessingExportCancelOutcome>,
    },
    UnfinishedProcessingExports(Reply<Vec<crate::processing::ProcessingExportWork>>),
    SweepProcessingExportExpiry {
        now: u64,
        reply: Reply<Vec<String>>,
    },
    AcquireProcessingArtifactLease {
        artifact_id: String,
        now: u64,
        reply: Reply<crate::processing::ProcessingArtifactLeaseOutcome>,
    },
    RenewProcessingArtifactLease {
        lease_id: String,
        now: u64,
        reply: Reply<bool>,
    },
    ReleaseProcessingArtifactLease {
        lease_id: String,
        reply: Reply<bool>,
    },
}

pub(super) fn dispatch(
    command: Command,
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
) {
    match command {
        Command::ReadComposableEditRecipe { photo_id, reply } => {
            let _ = reply.send(composable_recipe::read_composable_edit_recipe(
                connection, &photo_id,
            ));
        }
        Command::ReadComposableEditRecipeFacts { photo_id, reply } => {
            let _ = reply.send(composable_recipe::read_composable_edit_recipe_facts(
                connection, &photo_id,
            ));
        }
        Command::RebindComposableEditRecipe(mutation, reply) => {
            let _ = reply.send(composable_recipe::rebind_composable_edit_recipe(
                state,
                database_name,
                connection,
                mutation,
            ));
        }
        Command::ReplayComposableEditRecipe(mutation, reply) => {
            let _ = reply.send(composable_recipe::replay_composable_edit_recipe(
                state,
                database_name,
                connection,
                mutation,
            ));
        }
        Command::SaveComposableEditRecipe(mutation, reply) => {
            let result = composable_recipe::save_composable_edit_recipe(
                state,
                database_name,
                connection,
                mutation,
            );
            let _ = reply.send(result);
        }
        Command::ReadProcessingArtifact { artifact_id, reply } => {
            let _ = reply.send(processing_export::read_processing_artifact(
                connection,
                &artifact_id,
            ));
        }
        Command::PublishProcessingArtifact(artifact, reply) => {
            let result = processing_export::publish_processing_artifact(
                state,
                database_name,
                connection,
                *artifact,
            );
            let _ = reply.send(result);
        }
        Command::SubmitProcessingExport(mutation, now, reply) => {
            let result = processing_export::submit_processing_export(
                state,
                database_name,
                connection,
                mutation,
                now,
            );
            let _ = reply.send(result);
        }
        Command::ListProcessingExports(photo_id, now, reply) => {
            let _ = reply.send(processing_export::list_processing_exports(
                connection, &photo_id, now,
            ));
        }
        Command::ReplayProcessingExport(mutation, reply) => {
            let _ = reply.send(processing_export::replay_processing_export(
                connection, mutation,
            ));
        }
        Command::RetryProcessingExport(mutation, now, reply) => {
            let _ = reply.send(processing_export::retry_processing_export(
                state,
                database_name,
                connection,
                mutation,
                now,
            ));
        }
        Command::ReadProcessingArtifactRetention(artifact_id, reply) => {
            let _ = reply.send(processing_export::read_processing_artifact_retention(
                connection,
                &artifact_id,
            ));
        }
        Command::ReplayProcessingExportRetry {
            photo_id,
            previous_request_id,
            request_id,
            reply,
        } => {
            let _ = reply.send(processing_export::replay_processing_export_retry(
                connection,
                &photo_id,
                &previous_request_id,
                &request_id,
            ));
        }
        Command::SettleProcessingExport {
            artifact,
            request_id,
            payload_digest,
            now,
            reply,
        } => {
            let result = processing_export::settle_processing_export(
                state,
                database_name,
                connection,
                *artifact,
                &request_id,
                &payload_digest,
                now,
            );
            let _ = reply.send(result);
        }
        Command::ReadProcessingExportWork { request_id, reply } => {
            let _ = reply.send(processing_export::read_processing_export_work(
                connection,
                &request_id,
            ));
        }
        Command::BeginProcessingExportAttempt {
            request_id,
            now,
            reply,
        } => {
            let result = processing_export::begin_processing_export_attempt(
                state,
                database_name,
                connection,
                &request_id,
                now,
            );
            let _ = reply.send(result);
        }
        Command::FailProcessingExport {
            request_id,
            reason_code,
            now,
            reply,
        } => {
            let result = processing_export::fail_processing_export(
                state,
                database_name,
                connection,
                &request_id,
                &reason_code,
                now,
            );
            let _ = reply.send(result);
        }
        Command::CancelProcessingExport {
            request_id,
            now,
            reply,
        } => {
            let result = processing_export::cancel_processing_export(
                state,
                database_name,
                connection,
                &request_id,
                now,
            );
            let _ = reply.send(result);
        }
        Command::UnfinishedProcessingExports(reply) => {
            let _ = reply.send(processing_export::unfinished_processing_exports(connection));
        }
        Command::SweepProcessingExportExpiry { now, reply } => {
            let result = processing_export::sweep_processing_export_expiry(
                state,
                database_name,
                connection,
                now,
            );
            let _ = reply.send(result);
        }
        Command::AcquireProcessingArtifactLease {
            artifact_id,
            now,
            reply,
        } => {
            let result = processing_export::acquire_processing_artifact_lease(
                state,
                database_name,
                connection,
                &artifact_id,
                now,
            );
            let _ = reply.send(result);
        }
        Command::RenewProcessingArtifactLease {
            lease_id,
            now,
            reply,
        } => {
            let result = processing_export::renew_processing_artifact_lease(
                state,
                database_name,
                connection,
                &lease_id,
                now,
            );
            let _ = reply.send(result);
        }
        Command::ReleaseProcessingArtifactLease { lease_id, reply } => {
            let result = processing_export::release_processing_artifact_lease(
                state,
                database_name,
                connection,
                &lease_id,
            );
            let _ = reply.send(result);
        }
    }
}
