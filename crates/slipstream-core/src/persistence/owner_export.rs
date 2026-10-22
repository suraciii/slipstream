//! Export-facing request surface of the persistence owner: submissions,
//! record reads, cancellation, settlement, attempt bookkeeping, retry and
//! request-identity resolution, publication claims, lease bookkeeping, and
//! expiry sweeps.

use super::export;
use super::owner::{Command, Persistence, PersistenceError, Reply};
use super::{DatabaseName, StateDirectory};
use crate::{
    ExportAttempt, ExportLeaseOutcome, ExportRecord, ExportRetryOutcome, ExportSettlement,
    ExportSubmission, ExportSubmissionResolution, ExportSubmitOutcome, ExportSweepResult,
};
use rusqlite::Connection;
use tokio::sync::oneshot;

pub(super) type PhotoExports = Result<Option<Vec<ExportRecord>>, PersistenceError>;
pub(super) type PhotoExportsReceiver = oneshot::Receiver<PhotoExports>;
pub(super) type ExportPublicationClaimReply = Result<Option<(String, u64)>, PersistenceError>;

/// Owner commands for the export surface: submission, record reads, lifecycle
/// transitions, publication claims, and download leases. Keeping them behind
/// the `Command::Export` boundary preserves the owner's queue ordering: every
/// variant executes in submission order like any other command.
pub(super) enum ExportCommand {
    SubmitExport(ExportSubmission, Reply<ExportSubmitOutcome>),
    ReadExport {
        export_id: String,
        reply: Reply<Option<ExportRecord>>,
    },
    ListPhotoExports {
        photo_id: String,
        reply: Reply<Option<Vec<ExportRecord>>>,
    },
    CancelExport {
        export_id: String,
        reply: Reply<Option<ExportRecord>>,
    },
    SettleExport {
        export_id: String,
        settlement: ExportSettlement,
        reply: Reply<Option<ExportRecord>>,
    },
    BeginExportAttempt {
        export_id: String,
        attempt: ExportAttempt,
        reply: Reply<Option<ExportRecord>>,
    },
    RecordExportSource {
        export_id: String,
        size: u64,
        sha256: String,
        reply: Reply<Option<ExportRecord>>,
    },
    RetryExport {
        export_id: String,
        request_id: String,
        expected_bundle_id: String,
        allowance: u64,
        reply: Reply<ExportRetryOutcome>,
    },
    ResolveExportSubmission {
        photo_id: String,
        request_id: String,
        payload_digest: String,
        reply: Reply<Option<ExportSubmissionResolution>>,
    },
    ClaimExportPublication {
        export_id: String,
        incarnation: String,
        sequence: u64,
        reply: Reply<bool>,
    },
    ExportPublicationClaim {
        export_id: String,
        reply: Reply<Option<(String, u64)>>,
    },
    RenewExportLease {
        lease_id: String,
        now: u64,
        reply: Reply<bool>,
    },
    SweepExportExpiry {
        now: u64,
        reply: Reply<ExportSweepResult>,
    },
    UnfinishedExports(Reply<Vec<ExportRecord>>),
    AcquireExportLease {
        export_id: String,
        now: u64,
        reply: Reply<ExportLeaseOutcome>,
    },
    ReleaseExportLease {
        lease_id: String,
        reply: Reply<bool>,
    },
}

/// Executes one export command against the owner connection, sending the
/// identical reply the owner would have sent for the equivalent command.
pub(super) fn dispatch(
    command: ExportCommand,
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
) {
    match command {
        ExportCommand::SubmitExport(submission, reply) => {
            let result = export::submit_export(state, database_name, connection, submission);
            let _ = reply.send(result);
        }
        ExportCommand::ReadExport { export_id, reply } => {
            let _ = reply.send(export::export_record(connection, &export_id));
        }
        ExportCommand::ListPhotoExports { photo_id, reply } => {
            let _ = reply.send(export::list_photo_exports(connection, &photo_id));
        }
        ExportCommand::CancelExport { export_id, reply } => {
            let result = export::cancel_export(state, database_name, connection, &export_id);
            let _ = reply.send(result);
        }
        ExportCommand::SettleExport {
            export_id,
            settlement,
            reply,
        } => {
            let result =
                export::settle_export(state, database_name, connection, &export_id, settlement);
            let _ = reply.send(result);
        }
        ExportCommand::BeginExportAttempt {
            export_id,
            attempt,
            reply,
        } => {
            let result =
                export::begin_export_attempt(state, database_name, connection, &export_id, attempt);
            let _ = reply.send(result);
        }
        ExportCommand::RecordExportSource {
            export_id,
            size,
            sha256,
            reply,
        } => {
            let result = export::record_export_source(
                state,
                database_name,
                connection,
                &export_id,
                size,
                &sha256,
            );
            let _ = reply.send(result);
        }
        ExportCommand::RetryExport {
            export_id,
            request_id,
            expected_bundle_id,
            allowance,
            reply,
        } => {
            let result = export::retry_export(
                state,
                database_name,
                connection,
                &export_id,
                &request_id,
                &expected_bundle_id,
                allowance,
            );
            let _ = reply.send(result);
        }
        ExportCommand::ResolveExportSubmission {
            photo_id,
            request_id,
            payload_digest,
            reply,
        } => {
            let _ = reply.send(Ok(export::resolve_export_submission(
                connection,
                &photo_id,
                &request_id,
                &payload_digest,
            )));
        }
        ExportCommand::ClaimExportPublication {
            export_id,
            incarnation,
            sequence,
            reply,
        } => {
            let _ = reply.send(export::claim_export_publication(
                connection,
                &export_id,
                &incarnation,
                sequence,
            ));
        }
        ExportCommand::ExportPublicationClaim { export_id, reply } => {
            let _ = reply.send(Ok(export::export_publication_claim(connection, &export_id)));
        }
        ExportCommand::RenewExportLease {
            lease_id,
            now,
            reply,
        } => {
            let _ = reply.send(Ok(export::renew_export_lease(connection, &lease_id, now)));
        }
        ExportCommand::SweepExportExpiry { now, reply } => {
            let result = export::sweep_export_expiry(state, database_name, connection, now);
            let _ = reply.send(result);
        }
        ExportCommand::UnfinishedExports(reply) => {
            let _ = reply.send(export::unfinished_exports(connection));
        }
        ExportCommand::AcquireExportLease {
            export_id,
            now,
            reply,
        } => {
            let result =
                export::acquire_export_lease(state, database_name, connection, &export_id, now);
            let _ = reply.send(result);
        }
        ExportCommand::ReleaseExportLease { lease_id, reply } => {
            let result = export::release_export_lease(state, database_name, connection, &lease_id);
            let _ = reply.send(result);
        }
    }
}

impl Persistence {
    pub(crate) fn submit_export_receiver(
        &self,
        submission: ExportSubmission,
    ) -> Result<oneshot::Receiver<Result<ExportSubmitOutcome, PersistenceError>>, PersistenceError>
    {
        self.submit_persistence_receiver(|reply| {
            Command::Export(ExportCommand::SubmitExport(submission, reply))
        })
    }

    pub(crate) fn export_receiver(
        &self,
        export_id: &str,
    ) -> Result<oneshot::Receiver<Result<Option<ExportRecord>, PersistenceError>>, PersistenceError>
    {
        self.submit_persistence_receiver(|reply| {
            Command::Export(ExportCommand::ReadExport {
                export_id: export_id.to_owned(),
                reply,
            })
        })
    }

    pub(crate) fn photo_exports_receiver(
        &self,
        photo_id: &str,
    ) -> Result<PhotoExportsReceiver, PersistenceError> {
        self.submit_persistence_receiver(|reply| {
            Command::Export(ExportCommand::ListPhotoExports {
                photo_id: photo_id.to_owned(),
                reply,
            })
        })
    }

    pub(crate) fn cancel_export_receiver(
        &self,
        export_id: &str,
    ) -> Result<oneshot::Receiver<Result<Option<ExportRecord>, PersistenceError>>, PersistenceError>
    {
        self.submit_persistence_receiver(|reply| {
            Command::Export(ExportCommand::CancelExport {
                export_id: export_id.to_owned(),
                reply,
            })
        })
    }

    pub(crate) fn settle_export_receiver(
        &self,
        export_id: &str,
        settlement: ExportSettlement,
    ) -> Result<oneshot::Receiver<Result<Option<ExportRecord>, PersistenceError>>, PersistenceError>
    {
        self.submit_persistence_receiver(|reply| {
            Command::Export(ExportCommand::SettleExport {
                export_id: export_id.to_owned(),
                settlement,
                reply,
            })
        })
    }

    pub(crate) fn begin_export_attempt_receiver(
        &self,
        export_id: &str,
        attempt: ExportAttempt,
    ) -> Result<oneshot::Receiver<Result<Option<ExportRecord>, PersistenceError>>, PersistenceError>
    {
        self.submit_persistence_receiver(|reply| {
            Command::Export(ExportCommand::BeginExportAttempt {
                export_id: export_id.to_owned(),
                attempt,
                reply,
            })
        })
    }

    pub(crate) fn record_export_source_receiver(
        &self,
        export_id: &str,
        size: u64,
        sha256: &str,
    ) -> Result<oneshot::Receiver<Result<Option<ExportRecord>, PersistenceError>>, PersistenceError>
    {
        self.submit_persistence_receiver(|reply| {
            Command::Export(ExportCommand::RecordExportSource {
                export_id: export_id.to_owned(),
                size,
                sha256: sha256.to_owned(),
                reply,
            })
        })
    }

    pub(crate) fn retry_export_receiver(
        &self,
        export_id: &str,
        request_id: &str,
        expected_bundle_id: &str,
        allowance: u64,
    ) -> Result<oneshot::Receiver<Result<ExportRetryOutcome, PersistenceError>>, PersistenceError>
    {
        self.submit_persistence_receiver(|reply| {
            Command::Export(ExportCommand::RetryExport {
                export_id: export_id.to_owned(),
                request_id: request_id.to_owned(),
                expected_bundle_id: expected_bundle_id.to_owned(),
                allowance,
                reply,
            })
        })
    }

    /// Resolves a request identity without any admission or state change: a
    /// recorded identity replays, expires, or conflicts before the submit
    /// transaction ever runs.
    pub(crate) fn resolve_export_submission_receiver(
        &self,
        photo_id: &str,
        request_id: &str,
        payload_digest: &str,
    ) -> Result<
        oneshot::Receiver<Result<Option<ExportSubmissionResolution>, PersistenceError>>,
        PersistenceError,
    > {
        self.submit_persistence_receiver(|reply| {
            Command::Export(ExportCommand::ResolveExportSubmission {
                photo_id: photo_id.to_owned(),
                request_id: request_id.to_owned(),
                payload_digest: payload_digest.to_owned(),
                reply,
            })
        })
    }

    /// Claims publication before the staged artifact is renamed into place.
    pub(crate) fn claim_export_publication_receiver(
        &self,
        export_id: &str,
        incarnation: &str,
        sequence: u64,
    ) -> Result<oneshot::Receiver<Result<bool, PersistenceError>>, PersistenceError> {
        self.submit_persistence_receiver(|reply| {
            Command::Export(ExportCommand::ClaimExportPublication {
                export_id: export_id.to_owned(),
                incarnation: incarnation.to_owned(),
                sequence,
                reply,
            })
        })
    }

    /// Reads the durable publication claim of an Export, if any.
    pub(crate) fn export_publication_claim_receiver(
        &self,
        export_id: &str,
    ) -> Result<oneshot::Receiver<ExportPublicationClaimReply>, PersistenceError> {
        self.submit_persistence_receiver(|reply| {
            Command::Export(ExportCommand::ExportPublicationClaim {
                export_id: export_id.to_owned(),
                reply,
            })
        })
    }

    /// Refreshes a download lease's liveness anchor while its stream runs.
    pub(crate) fn renew_export_lease_receiver(
        &self,
        lease_id: &str,
        now: u64,
    ) -> Result<oneshot::Receiver<Result<bool, PersistenceError>>, PersistenceError> {
        self.submit_persistence_receiver(|reply| {
            Command::Export(ExportCommand::RenewExportLease {
                lease_id: lease_id.to_owned(),
                now,
                reply,
            })
        })
    }

    pub(crate) fn sweep_export_expiry_receiver(
        &self,
        now: u64,
    ) -> Result<oneshot::Receiver<Result<ExportSweepResult, PersistenceError>>, PersistenceError>
    {
        self.submit_persistence_receiver(|reply| {
            Command::Export(ExportCommand::SweepExportExpiry { now, reply })
        })
    }

    pub(crate) fn unfinished_exports_receiver(
        &self,
    ) -> Result<oneshot::Receiver<Result<Vec<ExportRecord>, PersistenceError>>, PersistenceError>
    {
        self.submit_persistence_receiver(|reply| {
            Command::Export(ExportCommand::UnfinishedExports(reply))
        })
    }

    pub(crate) fn acquire_export_lease_receiver(
        &self,
        export_id: &str,
        now: u64,
    ) -> Result<oneshot::Receiver<Result<ExportLeaseOutcome, PersistenceError>>, PersistenceError>
    {
        self.submit_persistence_receiver(|reply| {
            Command::Export(ExportCommand::AcquireExportLease {
                export_id: export_id.to_owned(),
                now,
                reply,
            })
        })
    }

    pub(crate) fn release_export_lease_receiver(
        &self,
        lease_id: &str,
    ) -> Result<oneshot::Receiver<Result<bool, PersistenceError>>, PersistenceError> {
        self.submit_persistence_receiver(|reply| {
            Command::Export(ExportCommand::ReleaseExportLease {
                lease_id: lease_id.to_owned(),
                reply,
            })
        })
    }
}
