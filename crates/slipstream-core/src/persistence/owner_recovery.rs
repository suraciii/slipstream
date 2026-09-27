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
        let (send, receive) = oneshot::channel();
        self.submit(Command::ApplyScan {
            discovered,
            errors,
            recovery,
            failure_after_first: false,
            reply: send,
        })?;
        Ok(receive)
    }

    #[cfg(test)]
    #[allow(dead_code)]
    pub(crate) async fn apply_scan_failure(
        &self,
        discovered: Vec<DiscoveredOriginal>,
        errors: Vec<OriginalScanError>,
    ) -> Result<ScanApplication, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::ApplyScan {
            discovered,
            errors,
            recovery: ScanRecoveryPlan::default(),
            failure_after_first: true,
            reply: send,
        })?;
        receive.await.unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn apply_scan_recovered_blocking(
        &self,
        discovered: Vec<DiscoveredOriginal>,
        errors: Vec<OriginalScanError>,
        recovery: ScanRecoveryPlan,
    ) -> Result<ScanApplication, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::ApplyScan {
            discovered,
            errors,
            recovery,
            failure_after_first: false,
            reply: send,
        })?;
        receive
            .blocking_recv()
            .unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn recovery_facts_blocking(
        &self,
        original_ids: Vec<String>,
    ) -> Result<Vec<OriginalFingerprint>, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::RecoveryFacts {
            original_ids,
            reply: send,
        })?;
        receive
            .blocking_recv()
            .unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn next_fingerprint_target_blocking(
        &self,
    ) -> Result<Option<FingerprintTarget>, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::NextFingerprintTarget(send))?;
        receive
            .blocking_recv()
            .unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn store_fingerprint_blocking(
        &self,
        fingerprint: OriginalFingerprint,
    ) -> Result<(), PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::StoreFingerprint(fingerprint, send))?;
        receive
            .blocking_recv()
            .unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn fingerprint_counts_blocking(
        &self,
    ) -> Result<FingerprintCounts, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::FingerprintCounts(send))?;
        receive
            .blocking_recv()
            .unwrap_or(Err(PersistenceError::OwnerStopped))
    }

    pub(crate) fn recovery_survey_receiver(
        &self,
    ) -> Result<oneshot::Receiver<Result<RecoverySurvey, PersistenceError>>, PersistenceError> {
        let (send, receive) = oneshot::channel();
        self.submit(Command::RecoverySurvey(send))?;
        Ok(receive)
    }

    /// Resolves current facts for one retained review membership, preserving
    /// the requested order.
    pub(crate) fn recovery_records_receiver(
        &self,
        original_ids: Vec<String>,
    ) -> Result<oneshot::Receiver<Result<RecoveryRecords, PersistenceError>>, PersistenceError>
    {
        let (send, receive) = oneshot::channel();
        self.submit(Command::RecoveryRecords {
            original_ids,
            reply: send,
        })?;
        Ok(receive)
    }

    pub(crate) fn apply_relocations_receiver(
        &self,
        root: LibraryRoot,
        relocations: Vec<RequestedRelocation>,
    ) -> Result<oneshot::Receiver<Result<AppliedRelocations, PersistenceError>>, PersistenceError>
    {
        let (send, receive) = oneshot::channel();
        self.submit(Command::ApplyRelocations {
            root,
            relocations,
            reply: send,
        })?;
        Ok(receive)
    }
}
