//! Recovery-facing request surface of the persistence owner: applying scans
//! with a recovery plan, reading recovery facts, surveys, and retained review
//! records, Original fingerprint bookkeeping, and applying relocations.

use super::owner::{Command, Persistence, PersistenceError, RecoveryRecords, Reply};
use super::scan::ScanApplication;
use super::scan_recovery::{self, FingerprintCounts, FingerprintTarget, ScanRecoveryPlan};
use super::{DatabaseName, StateDirectory};
use crate::{
    AppliedRelocations, DiscoveredOriginal, LibraryRoot, OriginalFingerprint, OriginalScanError,
    RecoverySurvey, RequestedRelocation,
};
use rusqlite::Connection;
use tokio::sync::oneshot;

/// Owner commands for the recovery surface: fingerprint reads and bookkeeping,
/// recovery surveys and records, and manual relocation batches. Keeping them
/// behind the `Command::Recovery` boundary preserves the owner's queue
/// ordering: every variant executes in submission order like any other
/// command.
pub(super) enum RecoveryCommand {
    RecoveryFacts {
        original_ids: Vec<String>,
        reply: Reply<Vec<OriginalFingerprint>>,
    },
    NextFingerprintTarget(Reply<Option<FingerprintTarget>>),
    StoreFingerprint(OriginalFingerprint, Reply<()>),
    FingerprintCounts(Reply<FingerprintCounts>),
    RecoverySurvey(Reply<RecoverySurvey>),
    RecoveryRecords {
        original_ids: Vec<String>,
        reply: Reply<RecoveryRecords>,
    },
    ApplyRelocations {
        root: LibraryRoot,
        relocations: Vec<RequestedRelocation>,
        reply: Reply<AppliedRelocations>,
    },
}

/// Executes one recovery command against the owner connection, sending the
/// identical reply the owner would have sent for the equivalent command.
pub(super) fn dispatch(
    command: RecoveryCommand,
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
) {
    match command {
        RecoveryCommand::RecoveryFacts {
            original_ids,
            reply,
        } => {
            let result = scan_recovery::recovery_facts(connection, &original_ids);
            let _ = reply.send(result);
        }
        RecoveryCommand::NextFingerprintTarget(reply) => {
            let result = scan_recovery::next_fingerprint_target(connection);
            let _ = reply.send(result);
        }
        RecoveryCommand::StoreFingerprint(fingerprint, reply) => {
            let result =
                scan_recovery::store_fingerprint(state, database_name, connection, fingerprint);
            let _ = reply.send(result);
        }
        RecoveryCommand::FingerprintCounts(reply) => {
            let result = scan_recovery::fingerprint_counts(connection);
            let _ = reply.send(result);
        }
        RecoveryCommand::RecoverySurvey(reply) => {
            let result = scan_recovery::recovery_survey(connection);
            let _ = reply.send(result);
        }
        RecoveryCommand::RecoveryRecords {
            original_ids,
            reply,
        } => {
            let result = scan_recovery::recovery_records(connection, &original_ids);
            let _ = reply.send(result);
        }
        RecoveryCommand::ApplyRelocations {
            root,
            relocations,
            reply,
        } => {
            let result = scan_recovery::apply_manual_relocations(
                state,
                database_name,
                connection,
                &root,
                &relocations,
            );
            let _ = reply.send(result);
        }
    }
}

impl Persistence {
    pub async fn apply_scan_recovered(
        &self,
        discovered: Vec<DiscoveredOriginal>,
        errors: Vec<OriginalScanError>,
        recovery: ScanRecoveryPlan,
    ) -> Result<ScanApplication, PersistenceError> {
        let receive = self.apply_scan_recovered_receiver(discovered, errors, recovery)?;
        receive.await.unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    fn apply_scan_recovered_receiver(
        &self,
        discovered: Vec<DiscoveredOriginal>,
        errors: Vec<OriginalScanError>,
        recovery: ScanRecoveryPlan,
    ) -> Result<oneshot::Receiver<Result<ScanApplication, PersistenceError>>, PersistenceError>
    {
        self.submit_persistence_receiver(|reply| Command::ApplyScan {
            discovered,
            errors,
            recovery,
            failure_after_first: false,
            reply,
        })
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) async fn apply_scan_failure(
        &self,
        discovered: Vec<DiscoveredOriginal>,
        errors: Vec<OriginalScanError>,
    ) -> Result<ScanApplication, PersistenceError> {
        let receive = self.submit_persistence_receiver(|reply| Command::ApplyScan {
            discovered,
            errors,
            recovery: ScanRecoveryPlan::default(),
            failure_after_first: true,
            reply,
        })?;
        receive.await.unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn apply_scan_recovered_blocking(
        &self,
        discovered: Vec<DiscoveredOriginal>,
        errors: Vec<OriginalScanError>,
        recovery: ScanRecoveryPlan,
    ) -> Result<ScanApplication, PersistenceError> {
        self.submit_persistence_receiver(|reply| Command::ApplyScan {
            discovered,
            errors,
            recovery,
            failure_after_first: false,
            reply,
        })?
        .blocking_recv()
        .unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn recovery_facts_blocking(
        &self,
        original_ids: Vec<String>,
    ) -> Result<Vec<OriginalFingerprint>, PersistenceError> {
        self.submit_persistence_receiver(|reply| {
            Command::Recovery(RecoveryCommand::RecoveryFacts {
                original_ids,
                reply,
            })
        })?
        .blocking_recv()
        .unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn next_fingerprint_target_blocking(
        &self,
    ) -> Result<Option<FingerprintTarget>, PersistenceError> {
        self.submit_persistence_receiver(|reply| {
            Command::Recovery(RecoveryCommand::NextFingerprintTarget(reply))
        })?
        .blocking_recv()
        .unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn store_fingerprint_blocking(
        &self,
        fingerprint: OriginalFingerprint,
    ) -> Result<(), PersistenceError> {
        self.submit_persistence_receiver(|reply| {
            Command::Recovery(RecoveryCommand::StoreFingerprint(fingerprint, reply))
        })?
        .blocking_recv()
        .unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn fingerprint_counts_blocking(
        &self,
    ) -> Result<FingerprintCounts, PersistenceError> {
        self.submit_persistence_receiver(|reply| {
            Command::Recovery(RecoveryCommand::FingerprintCounts(reply))
        })?
        .blocking_recv()
        .unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn recovery_survey_receiver(
        &self,
    ) -> Result<oneshot::Receiver<Result<RecoverySurvey, PersistenceError>>, PersistenceError> {
        self.submit_persistence_receiver(|reply| {
            Command::Recovery(RecoveryCommand::RecoverySurvey(reply))
        })
    }

    /// Resolves current facts for one retained review membership, preserving
    /// the requested order.
    pub(crate) fn recovery_records_receiver(
        &self,
        original_ids: Vec<String>,
    ) -> Result<oneshot::Receiver<Result<RecoveryRecords, PersistenceError>>, PersistenceError>
    {
        self.submit_persistence_receiver(|reply| {
            Command::Recovery(RecoveryCommand::RecoveryRecords {
                original_ids,
                reply,
            })
        })
    }

    pub(crate) fn apply_relocations_receiver(
        &self,
        root: LibraryRoot,
        relocations: Vec<RequestedRelocation>,
    ) -> Result<oneshot::Receiver<Result<AppliedRelocations, PersistenceError>>, PersistenceError>
    {
        self.submit_persistence_receiver(|reply| {
            Command::Recovery(RecoveryCommand::ApplyRelocations {
                root,
                relocations,
                reply,
            })
        })
    }
}
