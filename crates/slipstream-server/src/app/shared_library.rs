// Shared Library publication and scan-cycle ownership.
use super::*;
pub(super) const MAX_APPLICATION_SCAN_WAITERS: usize = 64;
pub(crate) type ScanCycleOutcome = Result<ScanStatusWire, String>;

pub(super) struct ScanCycleState {
    pub(super) closed: bool,
    pub(super) in_flight: bool,
    pub(super) waiters: Vec<oneshot::Sender<ScanCycleOutcome>>,
}

pub(super) struct ScanCycle {
    pub(super) state: Mutex<ScanCycleState>,
    pub(super) idle: Notify,
}

impl ScanCycle {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(ScanCycleState {
                closed: false,
                in_flight: false,
                waiters: Vec::new(),
            }),
            idle: Notify::new(),
        }
    }

    pub(super) fn close(&self) {
        self.state.lock().expect("scan cycle poisoned").closed = true;
    }

    pub(super) async fn wait_for_idle(&self) {
        loop {
            let notified = self.idle.notified();
            if !self.state.lock().expect("scan cycle poisoned").in_flight {
                return;
            }
            notified.await;
        }
    }
}

/// The published Library plus shared scan-lifecycle flags. The snapshot is
/// refreshed from persisted state at every publication, so facts committed
/// after a scan's apply (Selection State, Rating, Review Preview seeds) can
/// never be reverted by the completed scan. `publication` serializes
/// publications against in-place fact patches: a patch either happens before
/// the publication's persisted read (the read includes its committed fact) or
/// after the swap (the patch applies to the new snapshot).
pub(crate) struct SharedLibrary {
    pub(crate) snapshot: RwLock<Option<Published>>,
    pub(crate) published: AtomicBool,
    pub(crate) failed: AtomicBool,
    pub(crate) awaiting_scan: AtomicUsize,
    pub(crate) runs_started: AtomicU64,
    pub(crate) runs_completed: AtomicU64,
    pub(crate) publication: tokio::sync::Mutex<()>,
}

impl SharedLibrary {
    /// Replaces the published snapshot with the current persisted state read
    /// while holding `publication`. The scan has already applied its result
    /// transactionally, so the fresh read is the authoritative merge of
    /// scan-owned changes (availability, order, source selection, Preview
    /// invalidation for changed revisions) plus every later committed fact.
    pub(super) async fn publish_fresh(
        &self,
        library: &Library,
    ) -> Result<Vec<ReviewWarmupRequest>, LibraryError> {
        let _publication = self.publication.lock().await;
        let persisted = library.snapshot().await?;
        let originals_by_id = persisted
            .originals
            .iter()
            .map(|original| (original.id.as_str(), original))
            .collect::<std::collections::HashMap<_, _>>();
        let warmup_requests = {
            let previous = self.snapshot.read().expect("published Library poisoned");
            persisted
                .photos
                .iter()
                .filter_map(|photo| {
                    let original = originals_by_id.get(photo.original_id.as_str()).copied()?;
                    if !photo.available || !original.available || original.error_category.is_some()
                    {
                        return None;
                    }
                    let facts = PreviewFacts::from_records(photo.clone(), vec![original.clone()]);
                    match previous.as_ref() {
                        None => Some(ReviewWarmupRequest {
                            photo_id: photo.id.clone(),
                            retry: false,
                        }),
                        Some(previous) => {
                            let previous_source = previous
                                .photos_by_id
                                .get(&photo.id)
                                .copied()
                                .and_then(|position| previous.snapshot.photos.get(position))
                                .and_then(|previous_photo| {
                                    previous
                                        .originals_by_id
                                        .get(&previous_photo.original_id)
                                        .copied()
                                        .and_then(|position| {
                                            previous
                                                .snapshot
                                                .originals
                                                .get(position)
                                                .map(|original| (previous_photo, original))
                                        })
                                });
                            match previous_source {
                                Some((previous_photo, previous_original)) => {
                                    let source_changed = !PreviewFacts::from_records(
                                        previous_photo.clone(),
                                        vec![previous_original.clone()],
                                    )
                                    .source_matches(&facts.photo, &facts.originals);
                                    // InspectionPending is also the normal state for a
                                    // never-requested Photo; only Failed is a durable
                                    // warmup failure worth retrying on the next scan.
                                    let retry = !source_changed
                                        && matches!(
                                            previous_photo.preview_state,
                                            slipstream_core::PreviewState::Failed
                                        );
                                    (source_changed || retry).then_some(ReviewWarmupRequest {
                                        photo_id: photo.id.clone(),
                                        retry,
                                    })
                                }
                                None => Some(ReviewWarmupRequest {
                                    photo_id: photo.id.clone(),
                                    retry: false,
                                }),
                            }
                        }
                    }
                })
                .collect()
        };
        *self.snapshot.write().expect("published Library poisoned") =
            Some(Published::new(persisted));
        self.failed.store(false, Ordering::Relaxed);
        self.published.store(true, Ordering::Relaxed);
        Ok(warmup_requests)
    }

    /// Patches one mutable Photo fact in place. Called only after the
    /// owning SQLite write committed, under `publication`, so the patch can
    /// never be applied to a snapshot a concurrent publication is replacing.
    pub(super) async fn patch_photo(
        &self,
        photo_id: &str,
        apply: impl FnOnce(&mut slipstream_core::PhotoRecord),
    ) {
        let _publication = self.publication.lock().await;
        let mut guard = self.snapshot.write().expect("published Library poisoned");
        let Some(published) = guard.as_mut() else {
            return;
        };
        let Some(position) = published.photos_by_id.get(photo_id).copied() else {
            return;
        };
        let Some(photo) = published.snapshot.photos.get_mut(position) else {
            return;
        };
        apply(photo);
    }

    /// Patches the removed fact of many Photos in one critical section and
    /// drops the derived Folder index and CLI projection with them, so a
    /// removal is either wholly visible to a reader or not visible at all.
    ///
    /// The caller holds `publication` from before the owning SQLite write
    /// until this patch returns, so every reader that serializes on that lock
    /// sees either the whole state before the commit or the whole state after
    /// its effect, never a commit whose effect is missing.
    pub(super) fn patch_photo_removals(&self, photo_ids: &[String], removed: bool) {
        let mut guard = self.snapshot.write().expect("published Library poisoned");
        let Some(published) = guard.as_mut() else {
            return;
        };
        for photo_id in photo_ids {
            let Some(position) = published.photos_by_id.get(photo_id).copied() else {
                continue;
            };
            let Some(photo) = published.snapshot.photos.get_mut(position) else {
                continue;
            };
            photo.removed = removed;
        }
        published.invalidate_folder_index();
        published.rebuild_query_projection();
    }
    /// Permanently deleted Originals leave the published Photo identity
    /// unavailable and invalidate every derived source without rebuilding the
    /// whole snapshot.
    pub(super) fn patch_permanently_deleted(&self, photo_ids: &[String]) {
        let mut guard = self.snapshot.write().expect("published Library poisoned");
        let Some(published) = guard.as_mut() else {
            return;
        };
        for photo_id in photo_ids {
            let Some(position) = published.photos_by_id.get(photo_id).copied() else {
                continue;
            };
            let Some(photo) = published.snapshot.photos.get_mut(position) else {
                continue;
            };
            photo.available = false;
            photo.preview_state = slipstream_core::PreviewState::Unavailable;
            photo.preview_source_revision = None;
            photo.preview_width = None;
            photo.preview_height = None;
            photo.cache_revision = None;
            if let Some(original_position) = published.originals_by_id.get(&photo.original_id)
                && let Some(original) = published.snapshot.originals.get_mut(*original_position)
            {
                original.available = false;
                original.error_category = Some(slipstream_core::OriginalErrorCategory::Unreadable);
                original.error_message = Some("Original permanently deleted".to_owned());
            }
        }
        published.invalidate_folder_index();
        published.rebuild_query_projection();
    }

    /// Patches Preview facts only while the source bundle that produced them is
    /// still the currently published bundle. Mutable Selection/Rating fields
    /// are intentionally excluded from this guard.
    pub(super) async fn patch_photo_if_source_matches(
        &self,
        facts: &PreviewFacts,
        apply: impl FnOnce(&mut slipstream_core::PhotoRecord),
    ) -> bool {
        let _publication = self.publication.lock().await;
        let mut guard = self.snapshot.write().expect("published Library poisoned");
        let Some(published) = guard.as_mut() else {
            return false;
        };
        let Some(position) = published.photos_by_id.get(&facts.photo.id).copied() else {
            return false;
        };
        let originals = {
            let Some(photo) = published.snapshot.photos.get(position) else {
                return false;
            };
            [Some(&photo.original_id)]
                .into_iter()
                .flatten()
                .filter_map(|id| published.originals_by_id.get(id))
                .filter_map(|position| published.snapshot.originals.get(*position))
                .cloned()
                .collect::<Vec<_>>()
        };
        let Some(photo) = published.snapshot.photos.get_mut(position) else {
            return false;
        };
        if !facts.source_matches(photo, &originals) {
            return false;
        }
        apply(photo);
        true
    }

    /// One application-owned scan leader calls this for each admitted cycle.
    /// The optional publish gate (test-only) parks this cycle after the scan's
    /// apply and before publication so tests can commit facts in between.
    pub(super) async fn run_scan(
        &self,
        library: &Library,
        publish_gate: Option<oneshot::Receiver<()>>,
    ) -> Result<Vec<ReviewWarmupRequest>, LibraryError> {
        self.runs_started.fetch_add(1, Ordering::Relaxed);
        self.awaiting_scan.fetch_add(1, Ordering::Relaxed);
        let outcome = match library.scan().await {
            Ok(_) => {
                if let Some(gate) = publish_gate {
                    let _ = gate.await;
                }
                match self.publish_fresh(library).await {
                    Ok(warmup_ids) => Ok(warmup_ids),
                    Err(error) => {
                        self.failed.store(true, Ordering::Relaxed);
                        Err(error)
                    }
                }
            }
            // Shutdown drained this admitted scan; it is not a Library failure.
            Err(error) => {
                // Application shutdown drains an admitted cycle before
                // closing the Library, so Closed/ScannerStopped here is an
                // unexpected scan failure and must remain visible as failed.
                self.failed.store(true, Ordering::Relaxed);
                Err(error)
            }
        };
        self.awaiting_scan.fetch_sub(1, Ordering::Relaxed);
        self.runs_completed.fetch_add(1, Ordering::Relaxed);
        outcome
    }
}
