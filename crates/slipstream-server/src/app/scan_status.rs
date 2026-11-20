//! Loading Status projection for the Library Browser.

use super::*;

impl Application {
    pub(crate) fn scan_status(&self) -> ScanStatusWire {
        let progress = self.library.scan_progress();
        let publication = self.current_publication();
        let last_recovery = self.library.scan_outcome().map(|outcome| ScanRecoveryWire {
            relocated_photos: outcome.relocated_originals,
            fingerprinted_originals: outcome.fingerprinted_originals,
            unavailable_photos: outcome.unavailable_photos,
        });
        let counts = self.library.fingerprint_counts();
        let fingerprints = Some(FingerprintProgressWire {
            enrolled: counts.enrolled,
            pending: counts.pending,
        });
        let scope = self.library.scan_scope().or_else(|| {
            self.scan_cycle
                .state
                .lock()
                .expect("scan cycle poisoned")
                .options
                .as_ref()
                .and_then(|options| options.verification_scope.clone())
        });
        match progress.phase {
            ScanPhase::Discovering => ScanStatusWire {
                state: "discovering",
                updated_at: (progress.updated_ms != 0).then_some(progress.updated_ms),
                publication: publication.clone(),
                completed: Some(usize::try_from(progress.discovered).unwrap_or(usize::MAX)),
                total: None,
                updated_ms: progress.updated_ms,
                last_recovery,
                fingerprints,
                scope: scope.clone(),
            },
            ScanPhase::Inspecting => ScanStatusWire {
                state: "inspecting",
                updated_at: (progress.updated_ms != 0).then_some(progress.updated_ms),
                publication: publication.clone(),
                completed: Some(usize::try_from(progress.inspected).unwrap_or(usize::MAX)),
                total: progress
                    .inspect_total
                    .map(|total| usize::try_from(total).unwrap_or(usize::MAX)),
                updated_ms: progress.updated_ms,
                last_recovery,
                fingerprints,
                scope: scope.clone(),
            },
            ScanPhase::Recovering => ScanStatusWire {
                state: "recovering",
                updated_at: (progress.updated_ms != 0).then_some(progress.updated_ms),
                publication: publication.clone(),
                completed: Some(usize::try_from(progress.hashed).unwrap_or(usize::MAX)),
                total: progress
                    .hash_total
                    .map(|total| usize::try_from(total).unwrap_or(usize::MAX)),
                updated_ms: progress.updated_ms,
                last_recovery,
                fingerprints,
                scope: scope.clone(),
            },
            ScanPhase::Applying => ScanStatusWire {
                state: "applying",
                updated_at: (progress.updated_ms != 0).then_some(progress.updated_ms),
                publication: publication.clone(),
                completed: None,
                total: None,
                updated_ms: progress.updated_ms,
                last_recovery,
                fingerprints,
                scope: scope.clone(),
            },
            ScanPhase::Idle => {
                if self.shared.awaiting_scan.load(Ordering::Relaxed) > 0 {
                    // The scan finished; its result is being published.
                    ScanStatusWire {
                        state: "applying",
                        updated_at: (progress.updated_ms != 0).then_some(progress.updated_ms),
                        publication: publication.clone(),
                        completed: None,
                        total: None,
                        updated_ms: progress.updated_ms,
                        last_recovery,
                        fingerprints,
                        scope: scope.clone(),
                    }
                } else if self.shared.failed.load(Ordering::Relaxed) {
                    ScanStatusWire {
                        state: "failed",
                        updated_at: (progress.updated_ms != 0).then_some(progress.updated_ms),
                        publication: publication.clone(),
                        completed: None,
                        updated_ms: progress.updated_ms,
                        total: None,
                        last_recovery,
                        fingerprints,
                        scope: scope.clone(),
                    }
                } else if self.shared.published.load(Ordering::Relaxed) {
                    let photo_count = self.published_photo_count();
                    ScanStatusWire {
                        state: "idle",
                        updated_at: (progress.updated_ms != 0).then_some(progress.updated_ms),
                        publication: publication.clone(),
                        completed: Some(photo_count),
                        updated_ms: progress.updated_ms,
                        total: Some(photo_count),
                        last_recovery,
                        fingerprints,
                        scope: scope.clone(),
                    }
                } else {
                    ScanStatusWire {
                        state: "initializing",
                        updated_at: (progress.updated_ms != 0).then_some(progress.updated_ms),
                        publication,
                        completed: None,
                        total: None,
                        updated_ms: progress.updated_ms,
                        last_recovery,
                        fingerprints,
                        scope: scope.clone(),
                    }
                }
            }
        }
    }
}
