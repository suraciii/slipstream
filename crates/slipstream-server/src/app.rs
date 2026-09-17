use super::*;
use crate::config::{
    BROWSE_SNAPSHOT_IDLE, MAX_BROWSE_SNAPSHOTS, MAX_BROWSE_WINDOW, NEXT_BROWSE_NAMESPACE,
};
use crate::http::{is_hex_key, valid_id};
/// The published Library plus id indices, rebuilt atomically on each snapshot
/// replacement so bounded window requests never rescan the whole Library.
pub(crate) struct Published {
    pub(crate) snapshot: slipstream_core::ScanSnapshot,
    pub(crate) photos_by_id: std::collections::HashMap<String, usize>,
    pub(crate) originals_by_id: std::collections::HashMap<String, usize>,
    /// Opaque publication generation for File Location coherence.
    pub(crate) publication: u64,
    /// Folder index derived lazily from this immutable publication. Fact
    /// patches never change Original Locations, so the cache stays valid for
    /// the lifetime of this Published snapshot.
    folder_index: std::sync::OnceLock<crate::folders::FolderIndex>,
}

/// Allocates process-unique, monotonically increasing publication
/// generations. The process-unique base prevents a restarted server from
/// reissuing a publication value a browser still retains.
fn publication_generation() -> u64 {
    static BASE: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let base = *BASE.get_or_init(|| {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos() as u64)
            .unwrap_or(1);
        let mixed = nanos ^ (u64::from(std::process::id()) << 32);
        mixed | 1
    });
    base.wrapping_add(COUNTER.fetch_add(1, Ordering::Relaxed))
}

impl Published {
    fn new(snapshot: slipstream_core::ScanSnapshot) -> Self {
        let photos_by_id = snapshot
            .photos
            .iter()
            .enumerate()
            .map(|(position, photo)| (photo.id.clone(), position))
            .collect();
        let originals_by_id = snapshot
            .originals
            .iter()
            .enumerate()
            .map(|(position, original)| (original.id.clone(), position))
            .collect();
        Self {
            snapshot,
            photos_by_id,
            originals_by_id,
            publication: publication_generation(),
            folder_index: std::sync::OnceLock::new(),
        }
    }

    /// The derived Folder index for this publication, computed once.
    pub(crate) fn folder_index(&self) -> &crate::folders::FolderIndex {
        self.folder_index.get_or_init(|| {
            crate::folders::FolderIndex::derive(
                &self.snapshot.photos,
                &self.originals_by_id,
                &self.snapshot.originals,
            )
        })
    }

    pub(crate) fn publication_value(&self) -> String {
        format!("{:016x}", self.publication)
    }
}

/// The PhotoRecord for one Photo ID inside an immutable Published Library.
fn published_photo<'a>(
    published: &'a Published,
    id: &str,
) -> Option<&'a slipstream_core::PhotoRecord> {
    published
        .photos_by_id
        .get(id)
        .map(|position| &published.snapshot.photos[*position])
}

/// The authoritative Capture Time order key for one Photo: the RAW
/// Original's key when present, otherwise the paired JPEG's, matching the
/// persisted deterministic order's COALESCE.
fn published_capture_key<'a>(
    published: &'a Published,
    photo: &slipstream_core::PhotoRecord,
) -> Option<&'a str> {
    let original_key = |id: &Option<String>| -> Option<&'a str> {
        published
            .originals_by_id
            .get(id.as_deref()?)
            .and_then(|position| published.snapshot.originals.get(*position))
            .and_then(|original| original.capture.order_key.as_deref())
    };
    original_key(&photo.raw_original_id).or_else(|| original_key(&photo.jpeg_original_id))
}

/// The authoritative Capture Time order key for one Photo ID in the
/// Published Library. Unknown Photos have no key.
fn published_capture_key_for_id<'a>(published: &'a Published, id: &str) -> Option<&'a str> {
    published_photo(published, id).and_then(|photo| published_capture_key(published, photo))
}

/// Per-state Selection counts for one complete source ID list. Photos the
/// Published Library cannot resolve are not counted, so a count never claims
/// more than the facts the open source can show.
fn selection_counts_for_ids(published: &Published, ids: &[String]) -> SelectionCountsWire {
    SelectionCountsWire::from_selection_states(
        ids.iter()
            .filter_map(|id| published_photo(published, id))
            .map(|photo| photo.selection_state),
    )
}

/// Applies one Selection State filter to a complete ordered source ID list.
/// The filter selects from that order and never rewrites it.
fn filter_ids_by_selection(
    published: &Published,
    ids: Vec<String>,
    selection: BrowseSelectionFilter,
) -> Vec<String> {
    if selection == BrowseSelectionFilter::All {
        return ids;
    }
    ids.into_iter()
        .filter(|id| {
            published_photo(published, id)
                .is_some_and(|photo| selection.matches(photo.selection_state))
        })
        .collect()
}

/// Applies the requested view order to one complete source ID list.
/// Ascending is the Published Library's natural deterministic order.
/// Descending reverses only the Capture Time direction: missing-time Photos
/// stay in the trailing partition, and the ordering Location and Photo ID
/// tie-breakers keep their direction.
fn order_ids_by_capture_time(
    published: &Published,
    mut ids: Vec<String>,
    order: BrowseViewOrder,
) -> Vec<String> {
    if order != BrowseViewOrder::CaptureTimeDescending {
        return ids;
    }
    ids.sort_by(|a, b| {
        let a_key = published_capture_key_for_id(published, a);
        let b_key = published_capture_key_for_id(published, b);
        let a_photo = published_photo(published, a);
        let b_photo = published_photo(published, b);
        a_key
            .is_none()
            .cmp(&b_key.is_none())
            .then_with(|| match (a_key, b_key) {
                (Some(a), Some(b)) => b.cmp(a),
                _ => std::cmp::Ordering::Equal,
            })
            .then_with(|| match (a_photo, b_photo) {
                (Some(a), Some(b)) => a.sort_path.cmp(&b.sort_path),
                _ => std::cmp::Ordering::Equal,
            })
            .then_with(|| a.cmp(b))
    });
    ids
}

/// Orders one Album's members for a time view while keeping each member's
/// availability with its identity, so saved-position resolution applies to
/// the same order the Photographer sees. Membership positions themselves
/// are never changed.
fn order_album_members(
    published: &Published,
    mut members: Vec<slipstream_core::AlbumBrowseMember>,
    order: BrowseViewOrder,
) -> Vec<slipstream_core::AlbumBrowseMember> {
    if order == BrowseViewOrder::AlbumOrder {
        return members;
    }
    let ascending = order == BrowseViewOrder::CaptureTimeAscending;
    members.sort_by(|a, b| {
        let a_key = published_capture_key_for_id(published, &a.photo_id);
        let b_key = published_capture_key_for_id(published, &b.photo_id);
        let a_photo = published_photo(published, &a.photo_id);
        let b_photo = published_photo(published, &b.photo_id);
        a_key
            .is_none()
            .cmp(&b_key.is_none())
            .then_with(|| match (a_key, b_key) {
                (Some(a), Some(b)) => {
                    if ascending {
                        a.cmp(b)
                    } else {
                        b.cmp(a)
                    }
                }
                _ => std::cmp::Ordering::Equal,
            })
            .then_with(|| match (a_photo, b_photo) {
                (Some(a), Some(b)) => a.sort_path.cmp(&b.sort_path),
                _ => std::cmp::Ordering::Equal,
            })
            .then_with(|| a.photo_id.cmp(&b.photo_id))
    });
    members
}

/// The member that opening an Album resumes at, expressed as a Photo
/// identity. The durable saved position and its unavailable-member fallback
/// resolve by membership position, which is the Album's own durable order;
/// the caller maps the identity into the requested view order.
fn album_resume_member(
    members: &[slipstream_core::AlbumBrowseMember],
    saved_photo_id: Option<&str>,
) -> Option<String> {
    let saved =
        saved_photo_id.and_then(|saved| members.iter().position(|member| member.photo_id == saved));
    let index = saved
        .and_then(|saved| {
            members[saved].available.then_some(saved).or_else(|| {
                (1..=members.len())
                    .map(|offset| (saved + offset) % members.len())
                    .find(|index| members[*index].available)
            })
        })
        .or_else(|| members.iter().position(|member| member.available))
        .or(saved);
    index.map(|index| members[index].photo_id.clone())
}

fn ordered_library_ids(published: &Published, order: BrowseViewOrder) -> Vec<String> {
    order_ids_by_capture_time(
        published,
        published
            .snapshot
            .photos
            .iter()
            .map(|photo| photo.id.clone())
            .collect(),
        order,
    )
}

pub(crate) struct BrowseSnapshot {
    pub(crate) photo_ids: Vec<String>,
    pub(crate) last_used: Instant,
}

const MAX_APPLICATION_SCAN_WAITERS: usize = 64;
pub(crate) type ScanCycleOutcome = Result<ScanStatusWire, String>;

struct ScanCycleState {
    closed: bool,
    in_flight: bool,
    waiters: Vec<oneshot::Sender<ScanCycleOutcome>>,
}

struct ScanCycle {
    state: Mutex<ScanCycleState>,
    idle: Notify,
}

impl ScanCycle {
    fn new() -> Self {
        Self {
            state: Mutex::new(ScanCycleState {
                closed: false,
                in_flight: false,
                waiters: Vec::new(),
            }),
            idle: Notify::new(),
        }
    }

    fn close(&self) {
        self.state.lock().expect("scan cycle poisoned").closed = true;
    }

    async fn wait_for_idle(&self) {
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
    async fn publish_fresh(&self, library: &Library) -> Result<(), LibraryError> {
        let _publication = self.publication.lock().await;
        let persisted = library.snapshot().await?;
        *self.snapshot.write().expect("published Library poisoned") =
            Some(Published::new(persisted));
        self.failed.store(false, Ordering::Relaxed);
        self.published.store(true, Ordering::Relaxed);
        Ok(())
    }

    /// Patches one mutable Photo fact in place. Called only after the
    /// owning SQLite write committed, under `publication`, so the patch can
    /// never be applied to a snapshot a concurrent publication is replacing.
    async fn patch_photo(
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

    /// Patches Preview facts only while the source bundle that produced them is
    /// still the currently published bundle. Mutable Selection/Rating fields
    /// are intentionally excluded from this guard.
    async fn patch_photo_if_source_matches(
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
            [
                photo.jpeg_original_id.as_ref(),
                photo.raw_original_id.as_ref(),
            ]
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
    async fn run_scan(
        &self,
        library: &Library,
        publish_gate: Option<oneshot::Receiver<()>>,
    ) -> Result<(), LibraryError> {
        self.runs_started.fetch_add(1, Ordering::Relaxed);
        self.awaiting_scan.fetch_add(1, Ordering::Relaxed);
        let outcome = match library.scan().await {
            Ok(_) => {
                if let Some(gate) = publish_gate {
                    let _ = gate.await;
                }
                match self.publish_fresh(library).await {
                    Ok(()) => Ok(()),
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

pub struct Application {
    pub(crate) library: Arc<Library>,
    library_root: PathBuf,
    pub(crate) preview: PreviewService,
    pub(crate) shared: Arc<SharedLibrary>,
    scan_cycle: ScanCycle,
    pub(crate) browse_snapshots: Mutex<std::collections::HashMap<String, BrowseSnapshot>>,
    pub(crate) browse_namespace: u128,
    pub(crate) browse_counter: AtomicU64,
    pub(crate) shutdown: Mutex<bool>,
}

impl Application {
    pub(crate) fn admit_scan_cycle(
        self: &Arc<Self>,
        scan_gate: Option<oneshot::Receiver<()>>,
        publish_gate: Option<oneshot::Receiver<()>>,
    ) -> Result<oneshot::Receiver<ScanCycleOutcome>, ServerError> {
        let (reply, receive) = oneshot::channel();
        let starts_leader = {
            let mut cycle = self.scan_cycle.state.lock().expect("scan cycle poisoned");
            if cycle.closed {
                return Err(ServerError::Library(LibraryError::Closed));
            }
            // A dropped HTTP future closes its receiver immediately. Prune
            // those presentation waiters before enforcing the bounded live
            // waiter limit; the application-owned leader remains in flight.
            cycle.waiters.retain(|waiter| !waiter.is_closed());
            if cycle.waiters.len() >= MAX_APPLICATION_SCAN_WAITERS {
                return Err(ServerError::Library(LibraryError::ScanBusy));
            }
            cycle.waiters.push(reply);
            if cycle.in_flight {
                false
            } else {
                cycle.in_flight = true;
                true
            }
        };
        if starts_leader {
            let application = Arc::clone(self);
            tokio::spawn(async move {
                if let Some(gate) = scan_gate {
                    let _ = gate.await;
                }
                let outcome = application
                    .shared
                    .run_scan(&application.library, publish_gate)
                    .await
                    .map(|()| application.scan_status())
                    .map_err(|error| error.to_string());
                {
                    // Fan-out is non-blocking. Keep admission serialized until
                    // every terminal outcome is delivered, then expose idle to
                    // shutdown and the next cycle together.
                    let mut cycle = application
                        .scan_cycle
                        .state
                        .lock()
                        .expect("scan cycle poisoned");
                    for waiter in std::mem::take(&mut cycle.waiters) {
                        let _ = waiter.send(outcome.clone());
                    }
                    cycle.in_flight = false;
                }
                application.scan_cycle.idle.notify_waiters();
            });
        }
        Ok(receive)
    }

    async fn await_scan_cycle(
        receive: oneshot::Receiver<ScanCycleOutcome>,
    ) -> Result<ScanStatusWire, ServerError> {
        receive
            .await
            .map_err(|_| ServerError::Join("scan cycle stopped before settlement".to_owned()))?
            .map_err(ServerError::Join)
    }

    pub async fn open(config: &Config) -> Result<Arc<Self>, ServerError> {
        Self::open_with_gate(config, ScanLimits::default(), None, None).await
    }

    pub(crate) async fn open_with_gate(
        config: &Config,
        scan_limits: ScanLimits,
        scan_gate: Option<oneshot::Receiver<()>>,
        publish_gate: Option<oneshot::Receiver<()>>,
    ) -> Result<Arc<Self>, ServerError> {
        validate_storage_layout(config)?;
        let cache = CacheDirectory::open(&config.cache_directory, &config.library_root)?;
        let library_config = LibraryConfig {
            library_root: config.library_root.clone(),
            state_directory: config.state_directory.clone(),
            database_basename: config.database_basename.clone(),
            limits: scan_limits,
            ..LibraryConfig::default()
        };
        let library = tokio::task::spawn_blocking(move || Library::open(library_config))
            .await
            .map_err(|error| ServerError::Join(error.to_string()))??;
        let library = Arc::new(library);
        // Admission is complete: serve the last committed Library immediately
        // while the ordinary startup rescan runs in the background. A store
        // without a published Library stays initializing until its first scan
        // publishes one.
        let persisted = library.snapshot().await?;
        // Non-empty v2-v5 stores predate the durable marker and remain
        // published-compatible. Every completed scan now writes the marker,
        // which also preserves an intentionally empty Published Library.
        let published_initial =
            persisted.published || !persisted.photos.is_empty() || !persisted.originals.is_empty();
        let shared = Arc::new(SharedLibrary {
            snapshot: RwLock::new(published_initial.then(|| Published::new(persisted))),
            published: AtomicBool::new(published_initial),
            failed: AtomicBool::new(false),
            awaiting_scan: AtomicUsize::new(0),
            runs_started: AtomicU64::new(0),
            runs_completed: AtomicU64::new(0),
            publication: tokio::sync::Mutex::new(()),
        });
        let library_for_preview = Arc::clone(&library);
        let preview = match tokio::task::spawn_blocking(move || {
            PreviewService::from_cache(library_for_preview, cache)
        })
        .await
        .map_err(|error| ServerError::Join(error.to_string()))?
        {
            Ok(preview) => preview,
            Err(error) => {
                let library_for_close = Arc::clone(&library);
                let _ = tokio::task::spawn_blocking(move || library_for_close.shutdown()).await;
                return Err(ServerError::Preview(error.to_string()));
            }
        };
        let browse_namespace = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            ^ (u128::from(std::process::id()) << 64)
            ^ u128::from(NEXT_BROWSE_NAMESPACE.fetch_add(1, Ordering::Relaxed));
        let application = Arc::new(Self {
            library,
            library_root: config.library_root.clone(),
            preview,
            shared,
            scan_cycle: ScanCycle::new(),
            browse_snapshots: Mutex::new(std::collections::HashMap::new()),
            browse_namespace,
            browse_counter: AtomicU64::new(0),
            shutdown: Mutex::new(false),
        });
        // Admit the startup cycle synchronously before returning the
        // Application, so an immediate explicit rescan joins this leader.
        let startup = application
            .admit_scan_cycle(scan_gate, publish_gate)
            .expect("a new Application admits its startup scan");
        drop(startup);
        Ok(application)
    }

    /// Reads the small review metadata view from the Original that owns the
    /// authoritative Capture Time. This is intentionally on demand so the
    /// existing persisted state and scan contract do not grow a second EXIF
    /// schema just to support Photo View details.
    pub async fn photo_metadata(
        &self,
        photo_id: &str,
    ) -> Result<slipstream_core::CaptureReviewMetadata, ServerError> {
        let candidates = {
            let guard = self
                .shared
                .snapshot
                .read()
                .expect("published Library poisoned");
            let published = guard.as_ref().ok_or(ServerError::NotPublished)?;
            let position = published
                .photos_by_id
                .get(photo_id)
                .copied()
                .ok_or(ServerError::PhotoNotFound)?;
            let photo = published
                .snapshot
                .photos
                .get(position)
                .ok_or(ServerError::PhotoNotFound)?;
            let mut candidates = Vec::new();
            for id in [
                photo.raw_original_id.as_ref(),
                photo.jpeg_original_id.as_ref(),
            ]
            .into_iter()
            .flatten()
            {
                let Some(position) = published.originals_by_id.get(id).copied() else {
                    continue;
                };
                let Some(original) = published.snapshot.originals.get(position) else {
                    continue;
                };
                candidates.push((
                    original.relative_path.clone(),
                    original.kind,
                    original.facts,
                    original.capture.state,
                ));
            }
            candidates
        };
        let root = self.library_root.clone();
        tokio::task::spawn_blocking(move || {
            let Ok(root) = slipstream_core::LibraryRoot::open(root) else {
                return Ok(slipstream_core::CaptureReviewMetadata::default());
            };
            let selected = candidates
                .iter()
                .find(|(_, _, _, state)| *state == slipstream_core::CaptureMetadataState::Known)
                .or_else(|| candidates.first());
            let Some((path, kind, _, _)) = selected else {
                return Ok(slipstream_core::CaptureReviewMetadata::default());
            };
            let Ok(capability) = root.original(path.clone()) else {
                return Ok(slipstream_core::CaptureReviewMetadata::default());
            };
            let Ok(facts) = capability.facts() else {
                return Ok(slipstream_core::CaptureReviewMetadata::default());
            };
            slipstream_core::inspect_review_metadata(&capability, *kind, facts)
                .or_else(|_| Ok(slipstream_core::CaptureReviewMetadata::default()))
        })
        .await
        .map_err(|error| ServerError::Join(error.to_string()))?
    }

    /// Truthful Library status: the scanner owns measurable phases and
    /// counters, and the shared flags decide idle, failed, or initializing.
    pub(crate) fn scan_status(&self) -> ScanStatusWire {
        let progress = self.library.scan_progress();
        let publication = self.current_publication();
        match progress.phase {
            ScanPhase::Discovering => ScanStatusWire {
                state: "discovering",
                publication: publication.clone(),
                completed: Some(usize::try_from(progress.discovered).unwrap_or(usize::MAX)),
                total: None,
            },
            ScanPhase::Inspecting => ScanStatusWire {
                state: "inspecting",
                publication: publication.clone(),
                completed: Some(usize::try_from(progress.inspected).unwrap_or(usize::MAX)),
                total: progress
                    .inspect_total
                    .map(|total| usize::try_from(total).unwrap_or(usize::MAX)),
            },
            ScanPhase::Applying => ScanStatusWire {
                state: "applying",
                publication: publication.clone(),
                completed: None,
                total: None,
            },
            ScanPhase::Idle => {
                if self.shared.awaiting_scan.load(Ordering::Relaxed) > 0 {
                    // The scan finished; its result is being published.
                    ScanStatusWire {
                        state: "applying",
                        publication: publication.clone(),
                        completed: None,
                        total: None,
                    }
                } else if self.shared.failed.load(Ordering::Relaxed) {
                    ScanStatusWire {
                        state: "failed",
                        publication: publication.clone(),
                        completed: None,
                        total: None,
                    }
                } else if self.shared.published.load(Ordering::Relaxed) {
                    let photo_count = self.published_photo_count();
                    ScanStatusWire {
                        state: "idle",
                        publication: publication.clone(),
                        completed: Some(photo_count),
                        total: Some(photo_count),
                    }
                } else {
                    ScanStatusWire {
                        state: "initializing",
                        publication,
                        completed: None,
                        total: None,
                    }
                }
            }
        }
    }

    pub(crate) fn current_publication(&self) -> Option<String> {
        self.shared
            .snapshot
            .read()
            .expect("published Library poisoned")
            .as_ref()
            .map(Published::publication_value)
    }

    pub(crate) fn published_photo_count(&self) -> usize {
        self.shared
            .snapshot
            .read()
            .expect("published Library poisoned")
            .as_ref()
            .map_or(0, |published| published.snapshot.photos.len())
    }

    /// One bounded direct-child Folder window from the current publication.
    ///
    /// The first request may omit `publication` and binds to the current
    /// Published Library. Later requests carrying a superseded value fail as
    /// expired so the browser reloads one coherent publication instead of
    /// combining windows from different generations.
    pub async fn file_locations(
        &self,
        publication: Option<&str>,
        parent: &str,
        start: usize,
        limit: usize,
    ) -> Result<FileLocationsResponse, ServerError> {
        if !crate::folders::valid_folder_location(parent) {
            return Err(ServerError::FolderInvalid);
        }
        if limit == 0 || limit > crate::folders::MAXIMUM_FILE_LOCATION_WINDOW {
            return Err(ServerError::FileLocationWindow);
        }
        let guard = self
            .shared
            .snapshot
            .read()
            .expect("published Library poisoned");
        let Some(published) = guard.as_ref() else {
            return Err(ServerError::NotPublished);
        };
        let current = published.publication_value();
        if publication.is_some_and(|requested| requested != current) {
            return Err(ServerError::FileLocationsExpired);
        }
        let index = published.folder_index();
        if !index.is_known(parent) {
            return Err(ServerError::FolderNotFound);
        }
        let (children, total) = index.window(parent, start, limit);
        drop(guard);
        Ok(FileLocationsResponse {
            publication: current,
            parent: parent.to_owned(),
            start,
            limit,
            total,
            children: children
                .into_iter()
                .map(|child| FolderChildWire {
                    location: child.location,
                    name: child.name,
                    photo_count: child.photo_count,
                    has_descendant_folders: child.has_descendant_folders,
                })
                .collect(),
        })
    }

    pub async fn overview(&self) -> Result<LibraryOverviewResponse, ServerError> {
        let albums = self
            .library
            .list_album_summaries()
            .await?
            .into_iter()
            .map(album_summary)
            .collect();
        let (publication, photo_count) = {
            let guard = self
                .shared
                .snapshot
                .read()
                .expect("published Library poisoned");
            guard.as_ref().map_or((None, 0), |published| {
                (
                    Some(published.publication_value()),
                    published.snapshot.photos.len(),
                )
            })
        };
        Ok(LibraryOverviewResponse {
            published: publication.is_some(),
            publication,
            photo_count,
            scan: self.scan_status(),
            albums,
        })
    }

    pub async fn browse_open(
        &self,
        source: BrowseSourceRequest,
        order: BrowseViewOrder,
        selection: BrowseSelectionFilter,
        preferred_photo_id: Option<&str>,
    ) -> Result<BrowseOpenResponse, ServerError> {
        // Only an Album source owns persisted membership position, so
        // `album-order` is rejected for every other source before any
        // Snapshot is created instead of silently behaving as a time view.
        if order == BrowseViewOrder::AlbumOrder && !matches!(source, BrowseSourceRequest::Album(_))
        {
            return Err(ServerError::BrowseOrder);
        }
        // Each arm resolves the complete source order, its per-state counts,
        // one Selection State filter, and the identity the open should anchor
        // on. Counts always describe the unfiltered source.
        let (photo_ids, selection_counts, anchor_ids): (
            Vec<String>,
            SelectionCountsWire,
            Vec<String>,
        ) = match source {
            BrowseSourceRequest::Library => {
                let guard = self
                    .shared
                    .snapshot
                    .read()
                    .expect("published Library poisoned");
                let Some(published) = guard.as_ref() else {
                    return Err(ServerError::NotPublished);
                };
                let photo_ids = ordered_library_ids(published, order);
                let selection_counts = selection_counts_for_ids(published, &photo_ids);
                let photo_ids = filter_ids_by_selection(published, photo_ids, selection);
                (
                    photo_ids,
                    selection_counts,
                    preferred_photo_id.map(str::to_owned).into_iter().collect(),
                )
            }
            BrowseSourceRequest::Folder {
                location,
                publication,
            } => {
                if !crate::folders::valid_folder_location(&location) {
                    return Err(ServerError::FolderInvalid);
                }
                let guard = self
                    .shared
                    .snapshot
                    .read()
                    .expect("published Library poisoned");
                let Some(published) = guard.as_ref() else {
                    return Err(ServerError::NotPublished);
                };
                if published.publication_value() != publication {
                    return Err(ServerError::FileLocationsExpired);
                }
                let index = published.folder_index();
                if !index.is_known(&location) {
                    return Err(ServerError::FolderNotFound);
                }
                let photo_ids = index.filter_photo_ids(
                    &published.snapshot.photos,
                    &published.originals_by_id,
                    &published.snapshot.originals,
                    &location,
                );
                let photo_ids = order_ids_by_capture_time(published, photo_ids, order);
                let selection_counts = selection_counts_for_ids(published, &photo_ids);
                let photo_ids = filter_ids_by_selection(published, photo_ids, selection);
                (
                    photo_ids,
                    selection_counts,
                    preferred_photo_id.map(str::to_owned).into_iter().collect(),
                )
            }
            BrowseSourceRequest::Album(id) => {
                let target = self
                    .library
                    .album_browse_target(&id)
                    .await?
                    .ok_or(ServerError::BrowseNotFound)?;
                let resume_member_id =
                    album_resume_member(&target.members, target.saved_photo_id.as_deref());
                let guard = self
                    .shared
                    .snapshot
                    .read()
                    .expect("published Library poisoned");
                let Some(published) = guard.as_ref() else {
                    return Err(ServerError::NotPublished);
                };
                let members = if matches!(order, BrowseViewOrder::AlbumOrder) {
                    target.members
                } else {
                    order_album_members(published, target.members, order)
                };
                let photo_ids: Vec<String> =
                    members.into_iter().map(|member| member.photo_id).collect();
                let selection_counts = selection_counts_for_ids(published, &photo_ids);
                let photo_ids = filter_ids_by_selection(published, photo_ids, selection);
                let mut anchor_ids: Vec<String> =
                    preferred_photo_id.map(str::to_owned).into_iter().collect();
                if let Some(resume) = resume_member_id {
                    anchor_ids.push(resume);
                }
                (photo_ids, selection_counts, anchor_ids)
            }
        };
        // Positions resolve inside the view the browser actually opens: a
        // filtered Snapshot has its own sequence, so one position has one
        // meaning for windows, identity lookup, and navigation.
        let position = anchor_ids
            .into_iter()
            .find_map(|anchor| photo_ids.iter().position(|id| id == &anchor))
            .unwrap_or(0);
        let token = format!(
            "b{:032x}{:016x}",
            self.browse_namespace,
            self.browse_counter.fetch_add(1, Ordering::Relaxed)
        );
        let mut snapshots = self
            .browse_snapshots
            .lock()
            .expect("browse snapshots poisoned");
        let now = Instant::now();
        snapshots
            .retain(|_, snapshot| now.duration_since(snapshot.last_used) < BROWSE_SNAPSHOT_IDLE);
        while snapshots.len() >= MAX_BROWSE_SNAPSHOTS {
            let Some(oldest) = snapshots
                .iter()
                .min_by_key(|(_, snapshot)| snapshot.last_used)
                .map(|(token, _)| token.clone())
            else {
                break;
            };
            snapshots.remove(&oldest);
        }
        let total = photo_ids.len();
        snapshots.insert(
            token.clone(),
            BrowseSnapshot {
                photo_ids,
                last_used: now,
            },
        );
        Ok(BrowseOpenResponse {
            token,
            total,
            position,
            selection_counts,
        })
    }

    pub async fn browse_window(
        &self,
        token: &str,
        start: usize,
        limit: usize,
    ) -> Result<BrowseWindowResponse, ServerError> {
        if limit == 0 || limit > MAX_BROWSE_WINDOW {
            return Err(ServerError::BrowseLimit);
        }
        let (ids, total) = {
            let mut snapshots = self
                .browse_snapshots
                .lock()
                .expect("browse snapshots poisoned");
            let now = Instant::now();
            if snapshots.get(token).is_some_and(|snapshot| {
                now.duration_since(snapshot.last_used) >= BROWSE_SNAPSHOT_IDLE
            }) {
                snapshots.remove(token);
                return Err(ServerError::BrowseNotFound);
            }
            let snapshot = snapshots
                .get_mut(token)
                .ok_or(ServerError::BrowseNotFound)?;
            snapshot.last_used = now;
            let total = snapshot.photo_ids.len();
            let ids = snapshot
                .photo_ids
                .iter()
                .skip(start)
                .take(limit)
                .cloned()
                .collect::<Vec<_>>();
            (ids, total)
        };
        let facts = {
            let source_guard = self
                .shared
                .snapshot
                .read()
                .expect("published Library poisoned");
            let Some(source) = source_guard.as_ref() else {
                return Err(ServerError::NotPublished);
            };
            ids.iter()
                .filter_map(|id| source.photos_by_id.get(id).copied())
                .filter_map(|position| source.snapshot.photos.get(position))
                .map(|photo| {
                    let originals = [
                        photo.jpeg_original_id.as_ref(),
                        photo.raw_original_id.as_ref(),
                    ]
                    .into_iter()
                    .flatten()
                    .filter_map(|id| source.originals_by_id.get(id))
                    .filter_map(|position| source.snapshot.originals.get(*position))
                    .cloned()
                    .collect();
                    PreviewFacts::from_records(photo.clone(), originals)
                })
                .collect::<Vec<_>>()
        };
        let mut photos = Vec::with_capacity(facts.len());
        for facts in facts {
            let preview_url = if facts.photo.preview_state != PreviewState::Unavailable {
                self.preview
                    .lookup_current_key(&facts, DerivativeTarget::Review2560)
                    .await
                    .ok()
                    .flatten()
                    .map(|cache_key| {
                        format!(
                            "/api/derivatives/{}/review/{}.jpg",
                            facts.photo.id, cache_key
                        )
                    })
            } else {
                None
            };
            let thumbnail_url = if facts.photo.preview_state != PreviewState::Unavailable {
                self.preview
                    .lookup_current_key(&facts, DerivativeTarget::Thumbnail512)
                    .await
                    .ok()
                    .flatten()
                    .map(|cache_key| {
                        format!(
                            "/api/derivatives/{}/thumbnail/{}.jpg",
                            facts.photo.id, cache_key
                        )
                    })
            } else {
                None
            };
            let originals_by_id = facts
                .originals
                .iter()
                .enumerate()
                .map(|(position, original)| (original.id.clone(), position))
                .collect::<std::collections::HashMap<_, _>>();
            photos.push(photo_summary_indexed_with_url(
                &facts.photo,
                &facts.originals,
                &originals_by_id,
                preview_url,
                thumbnail_url,
            ));
        }
        Ok(BrowseWindowResponse {
            start,
            total,
            photos,
        })
    }

    /// Resolves one stable Photo identity against an immutable Browse Snapshot
    /// without transferring Photo facts or materializing the source.
    pub fn browse_position(
        &self,
        token: &str,
        photo_id: &str,
    ) -> Result<BrowsePositionResponse, ServerError> {
        let mut snapshots = self
            .browse_snapshots
            .lock()
            .expect("browse snapshots poisoned");
        let now = Instant::now();
        if snapshots
            .get(token)
            .is_some_and(|snapshot| now.duration_since(snapshot.last_used) >= BROWSE_SNAPSHOT_IDLE)
        {
            snapshots.remove(token);
            return Err(ServerError::BrowseNotFound);
        }
        let snapshot = snapshots
            .get_mut(token)
            .ok_or(ServerError::BrowseNotFound)?;
        snapshot.last_used = now;
        Ok(BrowsePositionResponse {
            position: snapshot.photo_ids.iter().position(|id| id == photo_id),
        })
    }

    pub fn browse_close(&self, token: &str) {
        self.browse_snapshots
            .lock()
            .expect("browse snapshots poisoned")
            .remove(token);
    }

    pub async fn albums(&self) -> Result<AlbumSummaryListResponse, ServerError> {
        Ok(AlbumSummaryListResponse {
            albums: self
                .library
                .list_album_summaries()
                .await?
                .into_iter()
                .map(album_summary)
                .collect(),
        })
    }

    /// Bounded per-Photo Album membership for the Photo View membership
    /// query. Resolved from the membership tables; never materializes any
    /// Album's member list.
    pub async fn photo_albums(&self, photo_id: &str) -> Result<PhotoAlbumsResponse, ServerError> {
        let albums = self
            .library
            .photo_albums(photo_id)
            .await?
            .ok_or(ServerError::PhotoNotFound)?;
        Ok(PhotoAlbumsResponse {
            albums: albums
                .into_iter()
                .map(|membership| PhotoAlbumMembershipWire {
                    id: membership.album_id,
                    name: membership.album_name,
                })
                .collect(),
        })
    }

    pub async fn rescan(self: &Arc<Self>) -> Result<ScanStatusWire, ServerError> {
        let receive = self.admit_scan_cycle(None, None)?;
        Self::await_scan_cycle(receive).await
    }

    pub async fn mutate_album(
        &self,
        mutation: slipstream_core::AlbumMutation,
    ) -> Result<AlbumSummaryListResponse, ServerError> {
        self.library.mutate_album(mutation).await?;
        self.albums().await
    }

    /// Adds all Photos projected into one Folder from the exact Published
    /// Library generation supplied by the browser. The Folder index is
    /// already ordered like the corresponding recursive Folder source, so the
    /// resulting Album append is deterministic and does not require sending
    /// the Photo IDs through the browser.
    pub async fn add_folder_to_album(
        &self,
        album_id: &str,
        folder_path: &str,
        publication: &str,
    ) -> Result<FolderAlbumMutationResponse, ServerError> {
        if !valid_id(album_id) || !crate::folders::valid_folder_location(folder_path) {
            return Err(ServerError::FolderInvalid);
        }
        let photo_ids = {
            let guard = self
                .shared
                .snapshot
                .read()
                .expect("published Library poisoned");
            let Some(published) = guard.as_ref() else {
                return Err(ServerError::NotPublished);
            };
            if published.publication_value() != publication {
                return Err(ServerError::FileLocationsExpired);
            }
            let index = published.folder_index();
            if !index.is_known(folder_path) {
                return Err(ServerError::FolderNotFound);
            }
            let photo_ids = index.filter_photo_ids(
                &published.snapshot.photos,
                &published.originals_by_id,
                &published.snapshot.originals,
                folder_path,
            );
            if photo_ids.len() > slipstream_core::MAXIMUM_FOLDER_ALBUM_PHOTOS {
                return Err(ServerError::FolderAlbumLimit);
            }
            photo_ids
        };
        let result = self
            .library
            .mutate_album(slipstream_core::AlbumMutation::AddFolderMembers {
                album_id: album_id.to_owned(),
                photo_ids,
            })
            .await?;
        let albums = self
            .library
            .list_album_summaries()
            .await?
            .into_iter()
            .map(album_summary)
            .collect();
        Ok(FolderAlbumMutationResponse {
            album_id: result.album_id,
            folder_path: folder_path.to_owned(),
            matched_count: result.added_count + result.already_member_count,
            added_count: result.added_count,
            already_member_count: result.already_member_count,
            albums,
        })
    }

    pub async fn mutate_photo_state(
        &self,
        mutation: slipstream_core::PhotoStateMutation,
    ) -> Result<slipstream_core::PhotoStateMutationResult, ServerError> {
        let photo_id = mutation.photo_id.clone();
        let field = mutation.field;
        let value = mutation.value;
        let result = self.library.mutate_photo_state(mutation).await?;
        self.shared
            .patch_photo(&photo_id, |photo| match (field, value) {
                (PhotoStateField::SelectionState, PhotoStateValue::Selection(selection)) => {
                    photo.selection_state = selection;
                }
                (PhotoStateField::Rating, PhotoStateValue::Rating(rating)) => {
                    photo.rating = rating;
                }
                _ => {}
            })
            .await;
        Ok(result)
    }

    fn published_preview_facts(&self, photo_id: &str) -> Option<PreviewFacts> {
        let guard = self
            .shared
            .snapshot
            .read()
            .expect("published Library poisoned");
        let published = guard.as_ref()?;
        let position = published.photos_by_id.get(photo_id).copied()?;
        let photo = published.snapshot.photos.get(position)?;
        let originals = [
            photo.jpeg_original_id.as_ref(),
            photo.raw_original_id.as_ref(),
        ]
        .into_iter()
        .flatten()
        .filter_map(|id| published.originals_by_id.get(id))
        .filter_map(|position| published.snapshot.originals.get(*position))
        .cloned()
        .collect();
        Some(PreviewFacts::from_records(photo.clone(), originals))
    }

    pub async fn preview(&self, photo_id: &str) -> Result<PreviewResponse, ServerError> {
        self.preview_with_priority(photo_id, slipstream_core::DerivativePriority::Current)
            .await
    }

    pub async fn preview_with_priority(
        &self,
        photo_id: &str,
        priority: slipstream_core::DerivativePriority,
    ) -> Result<PreviewResponse, ServerError> {
        self.preview_target(photo_id, DerivativeTarget::Review2560, priority)
            .await
    }

    pub async fn thumbnail(&self, photo_id: &str) -> Result<PreviewResponse, ServerError> {
        self.preview_target(
            photo_id,
            DerivativeTarget::Thumbnail512,
            slipstream_core::DerivativePriority::VisibleGrid,
        )
        .await
    }

    async fn preview_target(
        &self,
        photo_id: &str,
        target: DerivativeTarget,
        priority: slipstream_core::DerivativePriority,
    ) -> Result<PreviewResponse, ServerError> {
        if !valid_id(photo_id) {
            return Ok(PreviewResponse::unavailable("Unknown Photo"));
        }
        let published_facts = self.published_preview_facts(photo_id);
        let result = if let Some(facts) = published_facts.clone() {
            self.preview
                .request_with_facts(facts, target, priority)
                .await
        } else {
            self.preview
                .request(photo_id.to_owned(), target, priority)
                .await
        };
        let response = match result {
            Ok(slipstream_core::PreviewRequestResult::Current(ready)) => {
                if target == DerivativeTarget::Review2560 {
                    let source_revision = published_facts
                        .as_ref()
                        .and_then(|facts| preview_source_revision(facts, ready.source));
                    if let Some(facts) = published_facts.as_ref() {
                        self.shared
                            .patch_photo_if_source_matches(facts, |photo| {
                                photo.preview_state = PreviewState::Ready;
                                photo.preview_source = Some(ready.source);
                                photo.preview_source_revision = source_revision.clone();
                                photo.preview_width = Some(ready.width);
                                photo.preview_height = Some(ready.height);
                                photo.cache_revision = Some(ready.cache_key.clone());
                            })
                            .await;
                    } else {
                        self.shared
                            .patch_photo(photo_id, |photo| {
                                photo.preview_state = PreviewState::Ready;
                                photo.preview_source = Some(ready.source);
                                photo.preview_source_revision = source_revision.clone();
                                photo.preview_width = Some(ready.width);
                                photo.preview_height = Some(ready.height);
                                photo.cache_revision = Some(ready.cache_key.clone());
                            })
                            .await;
                    }
                }
                PreviewResponse::ready(photo_id, &ready, false)
            }
            Ok(slipstream_core::PreviewRequestResult::Stale(ready)) => {
                PreviewResponse::ready(photo_id, &ready, true)
            }
            Ok(slipstream_core::PreviewRequestResult::Unavailable(unavailable)) => {
                if target == DerivativeTarget::Review2560
                    && unavailable.reason
                        == slipstream_core::PreviewUnavailableReason::NoUsableSource
                {
                    if let Some(facts) = published_facts.as_ref() {
                        self.sync_unavailable_preview_from_persisted(facts).await;
                    } else {
                        self.patch_preview_state(photo_id, PreviewState::Unavailable)
                            .await;
                    }
                }
                let message = match unavailable.reason {
                    slipstream_core::PreviewUnavailableReason::PhotoNotFound => "Unknown Photo",
                    slipstream_core::PreviewUnavailableReason::OriginalUnavailable => {
                        "Original File is unavailable"
                    }
                    slipstream_core::PreviewUnavailableReason::NoUsableSource => {
                        "No usable camera-produced Preview"
                    }
                };
                PreviewResponse::unavailable(message)
            }
            Ok(slipstream_core::PreviewRequestResult::Failed(_)) => {
                if target == DerivativeTarget::Review2560 {
                    if let Some(facts) = published_facts.as_ref() {
                        self.patch_preview_state_if_source_matches(facts, PreviewState::Failed)
                            .await;
                    } else {
                        self.patch_preview_state(photo_id, PreviewState::Failed)
                            .await;
                    }
                }
                PreviewResponse::failed("Preview generation failed")
            }
            Ok(slipstream_core::PreviewRequestResult::StaleIgnored) => {
                PreviewResponse::unavailable("Original File changed; rescan required")
            }
            Err(slipstream_core::PreviewServiceError::Changed) => {
                PreviewResponse::unavailable("Original File changed; rescan required")
            }
            Err(slipstream_core::PreviewServiceError::Saturated)
            | Err(slipstream_core::PreviewServiceError::Closed) => {
                return Err(ServerError::PreviewUnavailable);
            }
            Err(_) => PreviewResponse::failed("Request failed"),
        };
        Ok(response)
    }

    async fn patch_preview_state(&self, photo_id: &str, state: PreviewState) {
        self.shared
            .patch_photo(photo_id, |photo| {
                photo.preview_state = state;
                photo.preview_source = None;
                photo.preview_width = None;
                photo.preview_height = None;
                photo.cache_revision = None;
            })
            .await;
    }

    async fn patch_preview_state_if_source_matches(
        &self,
        facts: &PreviewFacts,
        state: PreviewState,
    ) {
        self.shared
            .patch_photo_if_source_matches(facts, |photo| {
                photo.preview_state = state;
                photo.preview_source = None;
                photo.preview_width = None;
                photo.preview_height = None;
                photo.cache_revision = None;
            })
            .await;
    }

    /// Mirrors the exact Preview fields committed by a durable NoUsableSource
    /// seed. The persisted snapshot is authoritative; the source guards prevent
    /// a concurrent rescan from copying newer facts onto an older publication.
    async fn sync_unavailable_preview_from_persisted(&self, facts: &PreviewFacts) {
        let Ok(snapshot) = self.library.snapshot().await else {
            return;
        };
        let Some(persisted) = PreviewFacts::from_snapshot(&snapshot, &facts.photo.id) else {
            return;
        };
        if !facts.source_matches(&persisted.photo, &persisted.originals) {
            return;
        }
        self.shared
            .patch_photo_if_source_matches(facts, |photo| {
                photo.preview_state = persisted.photo.preview_state;
                photo.preview_candidate = persisted.photo.preview_candidate;
                photo.preview_source = persisted.photo.preview_source;
                photo.preview_source_revision = persisted.photo.preview_source_revision.clone();
                photo.preview_width = persisted.photo.preview_width;
                photo.preview_height = persisted.photo.preview_height;
                photo.cache_revision = persisted.photo.cache_revision.clone();
            })
            .await;
    }

    pub async fn derivative(
        &self,
        photo_id: &str,
        cache_key: &str,
        target: DerivativeTarget,
    ) -> Result<Option<DerivativeDelivery>, ServerError> {
        if !valid_id(photo_id) || !is_hex_key(cache_key) {
            return Ok(None);
        }
        let result = if let Some(facts) = self.published_preview_facts(photo_id) {
            self.preview
                .request_with_facts(facts, target, slipstream_core::DerivativePriority::Current)
                .await
        } else {
            self.preview
                .request(
                    photo_id.to_owned(),
                    target,
                    slipstream_core::DerivativePriority::Current,
                )
                .await
        };
        let ready = match result {
            Ok(slipstream_core::PreviewRequestResult::Current(ready))
            | Ok(slipstream_core::PreviewRequestResult::Stale(ready))
                if ready.cache_key == cache_key =>
            {
                ready
            }
            _ => return Ok(None),
        };
        let cache = self.preview.scheduler().cache().clone();
        let cache_key = cache_key.to_owned();
        let bytes = tokio::task::spawn_blocking(move || cache.read_derivative(&cache_key, target))
            .await
            .map_err(|error| ServerError::Join(error.to_string()))?
            .ok();
        Ok(bytes.map(|bytes| DerivativeDelivery {
            cache_key: ready.cache_key,
            bytes,
        }))
    }

    fn shutdown_blocking(&self) -> Result<(), ServerError> {
        let mut closed = self.shutdown.lock().expect("application shutdown poisoned");
        if *closed {
            return Ok(());
        }
        *closed = true;
        drop(closed);
        let preview_result = self
            .preview
            .shutdown()
            .map_err(|error| ServerError::Preview(error.to_string()));
        let library_result = self.library.shutdown().map_err(ServerError::Library);
        preview_result.and(library_result)
    }

    pub async fn shutdown(self: &Arc<Self>) -> Result<(), ServerError> {
        // Stop new admissions and drain the application-owned leader before
        // closing the Library, so publication and status accounting complete.
        self.scan_cycle.close();
        self.scan_cycle.wait_for_idle().await;
        let application = Arc::clone(self);
        tokio::task::spawn_blocking(move || application.shutdown_blocking())
            .await
            .map_err(|error| ServerError::Join(error.to_string()))?
    }
}

fn preview_source_revision(facts: &PreviewFacts, source: PreviewCandidate) -> Option<String> {
    let original_id = match source {
        PreviewCandidate::MatchingJpeg => facts.photo.jpeg_original_id.as_ref(),
        PreviewCandidate::EmbeddedRawJpeg => facts.photo.raw_original_id.as_ref(),
    }?;
    let original = facts.originals.iter().find(|original| {
        &original.id == original_id && original.available && original.error_category.is_none()
    })?;
    source_revision(
        original.relative_path.as_str(),
        original.facts.size,
        original.facts.mtime_ms,
    )
    .ok()
}
