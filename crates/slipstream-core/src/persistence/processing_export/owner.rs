use super::*;
use crate::persistence::owner::{Command as OwnerCommand, processing::Command};
/// The owner-facing request surface of the composable Export records: one
/// receiver per command so the central dispatch in `owner.rs` stays the
/// single serialized writer.
impl crate::persistence::owner::Persistence {
    pub(crate) fn replay_processing_export_retry_receiver(
        &self,
        photo_id: &str,
        previous_request_id: &str,
        request_id: &str,
    ) -> Result<
        oneshot::Receiver<Result<Option<ProcessingExportSubmitOutcome>, PersistenceError>>,
        PersistenceError,
    > {
        let (send, receive) = oneshot::channel();
        self.submit(OwnerCommand::Processing(
            Command::ReplayProcessingExportRetry {
                photo_id: photo_id.to_owned(),
                previous_request_id: previous_request_id.to_owned(),
                request_id: request_id.to_owned(),
                reply: send,
            },
        ))?;
        Ok(receive)
    }
    pub(crate) fn read_processing_artifact_retention_receiver(
        &self,
        artifact_id: &str,
    ) -> Result<
        oneshot::Receiver<
            Result<Option<crate::processing::ProcessingArtifactRetention>, PersistenceError>,
        >,
        PersistenceError,
    > {
        let (send, receive) = oneshot::channel();
        self.submit(OwnerCommand::Processing(
            Command::ReadProcessingArtifactRetention(artifact_id.to_owned(), send),
        ))?;
        Ok(receive)
    }
    pub(crate) fn list_processing_exports_receiver(
        &self,
        photo_id: &str,
        now: u64,
    ) -> Result<oneshot::Receiver<Result<ProcessingExportList, PersistenceError>>, PersistenceError>
    {
        let (send, receive) = oneshot::channel();
        self.submit(OwnerCommand::Processing(Command::ListProcessingExports(
            photo_id.to_owned(),
            now,
            send,
        )))?;
        Ok(receive)
    }

    pub(crate) fn replay_processing_export_receiver(
        &self,
        mutation: ReplayProcessingExport,
    ) -> Result<
        oneshot::Receiver<Result<Option<ProcessingExportSubmitOutcome>, PersistenceError>>,
        PersistenceError,
    > {
        let (send, receive) = oneshot::channel();
        self.submit(OwnerCommand::Processing(Command::ReplayProcessingExport(
            mutation, send,
        )))?;
        Ok(receive)
    }

    pub(crate) fn retry_processing_export_receiver(
        &self,
        mutation: RetryProcessingExport,
        now: u64,
    ) -> Result<
        oneshot::Receiver<Result<ProcessingExportSubmitOutcome, PersistenceError>>,
        PersistenceError,
    > {
        let (send, receive) = oneshot::channel();
        self.submit(OwnerCommand::Processing(Command::RetryProcessingExport(
            mutation, now, send,
        )))?;
        Ok(receive)
    }
    pub(crate) fn read_processing_artifact_receiver(
        &self,
        artifact_id: &str,
    ) -> Result<
        oneshot::Receiver<Result<Option<ProcessingArtifact>, PersistenceError>>,
        PersistenceError,
    > {
        let (send, receive) = oneshot::channel();
        self.submit(OwnerCommand::Processing(Command::ReadProcessingArtifact {
            artifact_id: artifact_id.to_owned(),
            reply: send,
        }))?;
        Ok(receive)
    }

    pub(crate) fn publish_processing_artifact_receiver(
        &self,
        artifact: ProcessingArtifact,
    ) -> Result<
        oneshot::Receiver<Result<ProcessingArtifactPublication, PersistenceError>>,
        PersistenceError,
    > {
        let (send, receive) = oneshot::channel();
        self.submit(OwnerCommand::Processing(
            Command::PublishProcessingArtifact(Box::new(artifact), send),
        ))?;
        Ok(receive)
    }

    pub(crate) fn submit_processing_export_receiver(
        &self,
        mutation: SubmitProcessingExport,
        now: u64,
    ) -> Result<
        oneshot::Receiver<Result<ProcessingExportSubmitOutcome, PersistenceError>>,
        PersistenceError,
    > {
        let (send, receive) = oneshot::channel();
        self.submit(OwnerCommand::Processing(Command::SubmitProcessingExport(
            mutation, now, send,
        )))?;
        Ok(receive)
    }

    pub(crate) fn settle_processing_export_receiver(
        &self,
        artifact: ProcessingArtifact,
        request_id: &str,
        payload_digest: &str,
        now: u64,
    ) -> Result<
        oneshot::Receiver<Result<ProcessingExportSettlement, PersistenceError>>,
        PersistenceError,
    > {
        let (send, receive) = oneshot::channel();
        self.submit(OwnerCommand::Processing(Command::SettleProcessingExport {
            artifact: Box::new(artifact),
            request_id: request_id.to_owned(),
            payload_digest: payload_digest.to_owned(),
            now,
            reply: send,
        }))?;
        Ok(receive)
    }

    pub(crate) fn read_processing_export_work_receiver(
        &self,
        request_id: &str,
    ) -> Result<
        oneshot::Receiver<Result<Option<ProcessingExportWork>, PersistenceError>>,
        PersistenceError,
    > {
        let (send, receive) = oneshot::channel();
        self.submit(OwnerCommand::Processing(
            Command::ReadProcessingExportWork {
                request_id: request_id.to_owned(),
                reply: send,
            },
        ))?;
        Ok(receive)
    }

    pub(crate) fn begin_processing_export_attempt_receiver(
        &self,
        request_id: &str,
        now: u64,
    ) -> Result<
        oneshot::Receiver<Result<ProcessingExportAttemptOutcome, PersistenceError>>,
        PersistenceError,
    > {
        let (send, receive) = oneshot::channel();
        self.submit(OwnerCommand::Processing(
            Command::BeginProcessingExportAttempt {
                request_id: request_id.to_owned(),
                now,
                reply: send,
            },
        ))?;
        Ok(receive)
    }

    pub(crate) fn fail_processing_export_receiver(
        &self,
        request_id: &str,
        reason_code: &str,
        now: u64,
    ) -> Result<
        oneshot::Receiver<Result<ProcessingExportFailureOutcome, PersistenceError>>,
        PersistenceError,
    > {
        let (send, receive) = oneshot::channel();
        self.submit(OwnerCommand::Processing(Command::FailProcessingExport {
            request_id: request_id.to_owned(),
            reason_code: reason_code.to_owned(),
            now,
            reply: send,
        }))?;
        Ok(receive)
    }

    pub(crate) fn cancel_processing_export_receiver(
        &self,
        request_id: &str,
        now: u64,
    ) -> Result<
        oneshot::Receiver<Result<ProcessingExportCancelOutcome, PersistenceError>>,
        PersistenceError,
    > {
        let (send, receive) = oneshot::channel();
        self.submit(OwnerCommand::Processing(Command::CancelProcessingExport {
            request_id: request_id.to_owned(),
            now,
            reply: send,
        }))?;
        Ok(receive)
    }

    pub(crate) fn unfinished_processing_exports_receiver(
        &self,
    ) -> Result<
        oneshot::Receiver<Result<Vec<ProcessingExportWork>, PersistenceError>>,
        PersistenceError,
    > {
        let (send, receive) = oneshot::channel();
        self.submit(OwnerCommand::Processing(
            Command::UnfinishedProcessingExports(send),
        ))?;
        Ok(receive)
    }

    pub(crate) fn sweep_processing_export_expiry_receiver(
        &self,
        now: u64,
    ) -> Result<oneshot::Receiver<Result<Vec<String>, PersistenceError>>, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(OwnerCommand::Processing(
            Command::SweepProcessingExportExpiry { now, reply: send },
        ))?;
        Ok(receive)
    }

    pub(crate) fn acquire_processing_artifact_lease_receiver(
        &self,
        artifact_id: &str,
        now: u64,
    ) -> Result<
        oneshot::Receiver<Result<ProcessingArtifactLeaseOutcome, PersistenceError>>,
        PersistenceError,
    > {
        let (send, receive) = oneshot::channel();
        self.submit(OwnerCommand::Processing(
            Command::AcquireProcessingArtifactLease {
                artifact_id: artifact_id.to_owned(),
                now,
                reply: send,
            },
        ))?;
        Ok(receive)
    }

    pub(crate) fn renew_processing_artifact_lease_receiver(
        &self,
        lease_id: &str,
        now: u64,
    ) -> Result<oneshot::Receiver<Result<bool, PersistenceError>>, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(OwnerCommand::Processing(
            Command::RenewProcessingArtifactLease {
                lease_id: lease_id.to_owned(),
                now,
                reply: send,
            },
        ))?;
        Ok(receive)
    }

    pub(crate) fn release_processing_artifact_lease_receiver(
        &self,
        lease_id: &str,
    ) -> Result<oneshot::Receiver<Result<bool, PersistenceError>>, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(OwnerCommand::Processing(
            Command::ReleaseProcessingArtifactLease {
                lease_id: lease_id.to_owned(),
                reply: send,
            },
        ))?;
        Ok(receive)
    }
}
