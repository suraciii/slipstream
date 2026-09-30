//! Recovery-facing request surface of the persistence owner: applying scans
//! with a recovery plan, reading recovery facts, surveys, and retained review
//! records, Original fingerprint bookkeeping, and applying relocations.

use super::owner::{Command, Persistence, PersistenceError, RecoveryRecords};
use super::scan::{FingerprintCounts, FingerprintTarget, ScanApplication, ScanRecoveryPlan};
use crate::{
    AppliedRelocations, DiscoveredOriginal, LibraryRoot, OriginalFingerprint, OriginalScanError,
    RecoverySurvey, RequestedRelocation,
};
use tokio::sync::oneshot;

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
        self.submit_persistence_receiver(|reply| Command::RecoveryFacts {
            original_ids,
            reply,
        })?
        .blocking_recv()
        .unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn next_fingerprint_target_blocking(
        &self,
    ) -> Result<Option<FingerprintTarget>, PersistenceError> {
        self.submit_persistence_receiver(Command::NextFingerprintTarget)?
            .blocking_recv()
            .unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn store_fingerprint_blocking(
        &self,
        fingerprint: OriginalFingerprint,
    ) -> Result<(), PersistenceError> {
        self.submit_persistence_receiver(|reply| Command::StoreFingerprint(fingerprint, reply))?
            .blocking_recv()
            .unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn fingerprint_counts_blocking(
        &self,
    ) -> Result<FingerprintCounts, PersistenceError> {
        self.submit_persistence_receiver(Command::FingerprintCounts)?
            .blocking_recv()
            .unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn recovery_survey_receiver(
        &self,
    ) -> Result<oneshot::Receiver<Result<RecoverySurvey, PersistenceError>>, PersistenceError> {
        self.submit_persistence_receiver(Command::RecoverySurvey)
    }

    /// Resolves current facts for one retained review membership, preserving
    /// the requested order.
    pub(crate) fn recovery_records_receiver(
        &self,
        original_ids: Vec<String>,
    ) -> Result<oneshot::Receiver<Result<RecoveryRecords, PersistenceError>>, PersistenceError>
    {
        self.submit_persistence_receiver(|reply| Command::RecoveryRecords {
            original_ids,
            reply,
        })
    }

    pub(crate) fn apply_relocations_receiver(
        &self,
        root: LibraryRoot,
        relocations: Vec<RequestedRelocation>,
    ) -> Result<oneshot::Receiver<Result<AppliedRelocations, PersistenceError>>, PersistenceError>
    {
        self.submit_persistence_receiver(|reply| Command::ApplyRelocations {
            root,
            relocations,
            reply,
        })
    }
}
