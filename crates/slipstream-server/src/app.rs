use super::*;
use crate::config::{MAX_BROWSE_WINDOW, MAX_REMOVED_WINDOW, NEXT_BROWSE_NAMESPACE};
use crate::http::{is_hex_key, valid_id};
use crate::queries::{CursorSigner, QueryRegistry, RetainedKind};
use crate::wire::{CliDerivativeFacts, PhotoOperationRemainderWire};
fn permanent_deletion_state(state: slipstream_core::PermanentDeletionItemState) -> &'static str {
    match state {
        slipstream_core::PermanentDeletionItemState::Pending => "pending",
        slipstream_core::PermanentDeletionItemState::Deleting => "deleting",
        slipstream_core::PermanentDeletionItemState::Deleted => "deleted",
        slipstream_core::PermanentDeletionItemState::Missing => "missing",
        slipstream_core::PermanentDeletionItemState::Changed => "changed",
        slipstream_core::PermanentDeletionItemState::Failed => "failed",
        slipstream_core::PermanentDeletionItemState::Uncertain => "uncertain",
    }
}

fn permanent_deletion_response(
    result: slipstream_core::PermanentDeletionResult,
) -> PermanentDeletionResponse {
    PermanentDeletionResponse {
        operation_id: result.operation_id,
        reviewed: result.reviewed,
        logical_bytes_deleted: result.logical_bytes_deleted,
        items: result
            .items
            .into_iter()
            .map(|item| PermanentDeletionItemWire {
                photo_id: item.photo_id,
                original_location: item.relative_path.to_string(),
                original_kind: match item.kind {
                    slipstream_core::OriginalKind::Raw => "raw",
                    slipstream_core::OriginalKind::Jpeg => "jpeg",
                },
                state: permanent_deletion_state(item.state),
                size: item.size,
                message: item.message,
            })
            .collect(),
    }
}
fn explicit_photo_removal_response(
    result: slipstream_core::PhotoRemovalResult,
) -> ExplicitPhotoRemovalResponse {
    let slipstream_core::PhotoRemovalResult {
        operation_id,
        counts,
        ordered_photo_ids,
        removed,
        removed_markers,
        changed_elsewhere,
        missing,
        already_removed,
        ..
    } = result;
    let markers = removed_markers
        .into_iter()
        .map(|marker| (marker.photo_id, marker.removed_at_ms))
        .collect::<std::collections::HashMap<_, _>>();
    let removed = removed
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    let changed_elsewhere = changed_elsewhere
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    let missing = missing
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    let already_removed = already_removed
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    let results = ordered_photo_ids
        .into_iter()
        .map(|photo_id| {
            let (outcome, removed_at_ms) = if removed.contains(photo_id.as_str()) {
                ("removed", markers.get(&photo_id).copied())
            } else if changed_elsewhere.contains(photo_id.as_str()) {
                ("changed-elsewhere", None)
            } else if missing.contains(photo_id.as_str()) {
                ("unavailable", None)
            } else if already_removed.contains(photo_id.as_str()) {
                ("already-removed", None)
            } else {
                unreachable!("removal result omitted a requested Photo")
            };
            PhotoRemovalItemWire {
                photo_id,
                outcome,
                removed_at_ms,
            }
        })
        .collect();
    ExplicitPhotoRemovalResponse {
        operation_id,
        counts: PhotoRemovalCountsWire {
            removed: counts.removed,
            changed_elsewhere: counts.changed_elsewhere,
            missing: counts.missing,
            already_removed: counts.already_removed,
        },
        results,
    }
}

fn explicit_photo_restore_response(
    result: slipstream_core::ExplicitPhotoRestoreResult,
) -> ExplicitPhotoRestoreResponse {
    let slipstream_core::ExplicitPhotoRestoreResult {
        operation_id,
        ordered_photo_ids,
        counts,
        restored,
        already_active,
        changed_elsewhere,
        missing,
    } = result;
    let restored = restored
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    let already_active = already_active
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    let changed_elsewhere = changed_elsewhere
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    let missing = missing
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    let results = ordered_photo_ids
        .into_iter()
        .map(|photo_id| {
            let outcome = if restored.contains(photo_id.as_str()) {
                "restored"
            } else if already_active.contains(photo_id.as_str()) {
                "already-active"
            } else if changed_elsewhere.contains(photo_id.as_str()) {
                "changed-elsewhere"
            } else if missing.contains(photo_id.as_str()) {
                "unavailable"
            } else {
                unreachable!("restore result omitted a requested Photo")
            };
            PhotoRestoreItemWire { photo_id, outcome }
        })
        .collect();
    ExplicitPhotoRestoreResponse {
        operation_id,
        counts: ExplicitPhotoRestoreCountsWire {
            restored: counts.restored,
            already_active: counts.already_active,
            changed_elsewhere: counts.changed_elsewhere,
            missing: counts.missing,
        },
        results,
    }
}

/// The published Library plus id indices, rebuilt atomically on each snapshot
/// replacement so bounded window requests never rescan the whole Library.
pub(crate) struct Published {
    pub(crate) snapshot: slipstream_core::ScanSnapshot,
    pub(crate) photos_by_id: std::collections::HashMap<String, usize>,
    pub(crate) originals_by_id: std::collections::HashMap<String, usize>,
    /// Opaque publication generation for File Location coherence.
    pub(crate) publication: u64,
    /// The evaluation time shared by all Folder windows in this publication.
    pub(crate) evaluated_at: SystemTime,
    /// Immutable scan-owned query facts shared by every CLI query admitted
    /// against this publication.
    pub(crate) photo_query_projection: Arc<slipstream_core::PhotoQueryProjection>,
    /// Folder index derived lazily from this immutable publication. Fact
    /// patches never change Original Locations, so the cache stays valid until
    /// a removal fact changes which Photos a Folder projects.
    folder_index: std::sync::RwLock<Option<Arc<crate::folders::FolderIndex>>>,
}

#[derive(Clone)]
struct PublishedMetadataSource {
    relative_path: slipstream_core::RelativeOriginalPath,
    kind: slipstream_core::OriginalKind,
    source_revision: Option<String>,
}

pub(crate) struct PublishedPhotoDetail {
    pub(crate) photo: slipstream_core::PhotoRead,
    metadata_source: Option<PublishedMetadataSource>,
}

/// The current derivative one admitted CLI Preview request may download. Every
/// fact belongs to the bytes the caller is about to read.
pub(crate) struct CliPreviewReady {
    pub(crate) photo_id: String,
    pub(crate) source: slipstream_core::PreviewSource,
    pub(crate) source_revision: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) cache_key: String,
}

/// Why one admitted CLI Preview request has no current derivative.
pub(crate) enum CliPreviewRefusal {
    /// The Photo is unknown to the Published Library.
    Missing,
    /// No allowed source can produce a current Preview for the admitted
    /// revision.
    Unavailable,
    /// The Photo's own published Preview state when the request did not produce
    /// a current derivative. It is the state the Published Library publishes,
    /// so a CLI caller reads the same state the browser facts use.
    NotReady(&'static str),
    /// The shared Preview owner has no room for more demand.
    Busy,
}

impl CliPreviewRefusal {
    /// Reports the published Preview state for one Photo whose admitted request
    /// produced no current derivative. A Library that still claims a ready
    /// Preview the request did not produce is reported as `failed` rather than
    /// as a not-yet-completed inspection.
    fn published_state(published_facts: Option<&PreviewFacts>) -> Self {
        Self::NotReady(
            match published_facts.map(|facts| facts.photo.preview_state) {
                Some(state) if state != PreviewState::Ready => crate::wire::preview_state(state),
                _ => "failed",
            },
        )
    }
}

pub(crate) type CliPreviewOutcome = Result<CliPreviewReady, CliPreviewRefusal>;

#[cfg(test)]
type MetadataInspectionTestHook = dyn Fn(&slipstream_core::RelativeOriginalPath) + Send + Sync;

#[cfg(test)]
static METADATA_INSPECTION_TEST_HOOK: std::sync::OnceLock<
    std::sync::Mutex<Option<Arc<MetadataInspectionTestHook>>>,
> = std::sync::OnceLock::new();

#[cfg(test)]
static METADATA_INSPECTION_TEST_HOOK_LEASE: std::sync::OnceLock<std::sync::Mutex<()>> =
    std::sync::OnceLock::new();

#[cfg(test)]
pub(crate) struct MetadataInspectionTestHookGuard {
    _lease: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
pub(crate) fn install_metadata_inspection_test_hook(
    hook: impl Fn(&slipstream_core::RelativeOriginalPath) + Send + Sync + 'static,
) -> MetadataInspectionTestHookGuard {
    let lease = METADATA_INSPECTION_TEST_HOOK_LEASE
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap();
    *METADATA_INSPECTION_TEST_HOOK
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .unwrap() = Some(Arc::new(hook));
    MetadataInspectionTestHookGuard { _lease: lease }
}

#[cfg(test)]
impl Drop for MetadataInspectionTestHookGuard {
    fn drop(&mut self) {
        *METADATA_INSPECTION_TEST_HOOK
            .get_or_init(|| std::sync::Mutex::new(None))
            .lock()
            .unwrap() = None;
    }
}

#[cfg(test)]
fn metadata_inspection_test_hook(path: &slipstream_core::RelativeOriginalPath) {
    let hook = METADATA_INSPECTION_TEST_HOOK
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .unwrap()
        .clone();
    if let Some(hook) = hook {
        hook(path);
    }
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

/// The derived CLI projection of one snapshot: the Photos a machine client may
/// resolve, in capture-time-descending order. Removed Photos are excluded here
/// rather than at read time, so a committed removal and its projection are one
/// step and no reader can observe a removed Photo as present.
fn photo_query_projection(
    snapshot: &slipstream_core::ScanSnapshot,
    originals_by_id: &std::collections::HashMap<String, usize>,
) -> Arc<slipstream_core::PhotoQueryProjection> {
    let candidates = snapshot
        .photos
        .iter()
        .filter(|photo| !photo.removed)
        .map(|photo| {
            let original = &snapshot.originals[originals_by_id[&photo.original_id]];
            slipstream_core::PhotoQueryCandidate {
                photo_id: photo.id.clone(),
                relative_path: original.relative_path.as_str().to_owned(),
                sort_path: photo.sort_path.clone(),
                original_kind: original.kind,
                original_available: original.available,
                capture: original.capture.clone(),
                preview_state: photo.preview_state,
                preview_source_revision: photo.preview_source_revision.clone(),
                preview_width: photo.preview_width,
                preview_height: photo.preview_height,
            }
        })
        .collect::<Vec<_>>();
    let mut descending = (0..candidates.len()).collect::<Vec<_>>();
    descending.sort_by(|a, b| {
        let a = &candidates[*a];
        let b = &candidates[*b];
        a.capture_order_key()
            .is_none()
            .cmp(&b.capture_order_key().is_none())
            .then_with(|| match (a.capture_order_key(), b.capture_order_key()) {
                (Some(a), Some(b)) => b.cmp(a),
                _ => std::cmp::Ordering::Equal,
            })
            .then_with(|| a.sort_path.cmp(&b.sort_path))
            .then_with(|| a.photo_id.cmp(&b.photo_id))
    });
    Arc::new(
        slipstream_core::PhotoQueryProjection::new(candidates, descending)
            .expect("Published Photos have unique identities and complete order"),
    )
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
            .collect::<std::collections::HashMap<_, _>>();
        let photo_query_projection = photo_query_projection(&snapshot, &originals_by_id);
        Self {
            snapshot,
            photos_by_id,
            originals_by_id,
            publication: publication_generation(),
            evaluated_at: SystemTime::now(),
            photo_query_projection,
            folder_index: std::sync::RwLock::new(None),
        }
    }

    /// The derived Folder index for this publication, computed once per
    /// removal generation.
    pub(crate) fn folder_index(&self) -> Arc<crate::folders::FolderIndex> {
        if let Some(index) = self
            .folder_index
            .read()
            .expect("folder index poisoned")
            .as_ref()
        {
            return Arc::clone(index);
        }
        let derived = Arc::new(crate::folders::FolderIndex::derive(
            &self.snapshot.photos,
            &self.originals_by_id,
            &self.snapshot.originals,
        ));
        Arc::clone(
            self.folder_index
                .write()
                .expect("folder index poisoned")
                .get_or_insert_with(|| Arc::clone(&derived)),
        )
    }

    /// Rebuilds the derived CLI projection from the current Photo facts. Called
    /// from the same critical section that patches removal facts, so a removed
    /// Photo leaves the CLI query, and a restored Photo re-enters it, in the
    /// same step the Web sources change.
    fn rebuild_query_projection(&mut self) {
        self.photo_query_projection = photo_query_projection(&self.snapshot, &self.originals_by_id);
    }

    /// Drops the derived Folder index. Called from the same critical section
    /// that patches removal facts, so no reader can keep a Folder projection
    /// built from the previous removal generation.
    fn invalidate_folder_index(&self) {
        *self.folder_index.write().expect("folder index poisoned") = None;
    }

    pub(crate) fn publication_value(&self) -> String {
        format!("{:016x}", self.publication)
    }

    fn photo_read_projection(
        &self,
        photo_ids: &[String],
    ) -> Arc<slipstream_core::PhotoQueryProjection> {
        let candidates = photo_ids
            .iter()
            .filter_map(|photo_id| {
                let mut candidate = self.photo_query_projection.get(photo_id)?.clone();
                let photo = &self.snapshot.photos[self.photos_by_id[photo_id]];
                candidate.preview_state = photo.preview_state;
                candidate.preview_source_revision = photo.preview_source_revision.clone();
                candidate.preview_width = photo.preview_width;
                candidate.preview_height = photo.preview_height;
                Some(candidate)
            })
            .collect::<Vec<_>>();
        let candidate_count = candidates.len();
        Arc::new(
            slipstream_core::PhotoQueryProjection::new(candidates, (0..candidate_count).collect())
                .expect("a bounded Published Photo page has unique identities"),
        )
    }

    fn photo_metadata_source(&self, photo_id: &str) -> Option<PublishedMetadataSource> {
        let photo = self
            .photos_by_id
            .get(photo_id)
            .and_then(|position| self.snapshot.photos.get(*position))?;
        let original = self
            .originals_by_id
            .get(&photo.original_id)
            .and_then(|position| self.snapshot.originals.get(*position))?;
        Some(PublishedMetadataSource {
            relative_path: original.relative_path.clone(),
            kind: original.kind,
            source_revision: original.capture.source_revision.clone(),
        })
    }
}

/// One confirmed mapping from the review entry's apply request.
#[derive(Clone)]
pub(crate) struct RecoveryApplyItem {
    pub original_id: String,
    pub new_location: String,
    pub retire_destination: bool,
}

/// Structured failure for one manual recovery apply request.
pub(crate) enum RecoveryApplyError {
    Invalid,
    Rejected {
        message: &'static str,
        rejections: Vec<RecoveryRejectionWire>,
    },
    Server(ServerError),
}

impl From<ServerError> for RecoveryApplyError {
    fn from(error: ServerError) -> Self {
        Self::Server(error)
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
/// The Photo's single Original's Capture Time order key in the Published
/// Library, matching the persisted deterministic order.
fn published_capture_key<'a>(
    published: &'a Published,
    photo: &slipstream_core::PhotoRecord,
) -> Option<&'a str> {
    published
        .originals_by_id
        .get(&photo.original_id)
        .and_then(|position| published.snapshot.originals.get(*position))
        .and_then(|original| original.capture.order_key.as_deref())
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

/// The complete Library source order. Removed Photos are excluded: removal
/// hides a Photo from Library Views without deleting its Original File, so the
/// complete source a Browse Snapshot opens never contains one.
fn ordered_library_ids(published: &Published, order: BrowseViewOrder) -> Vec<String> {
    order_ids_by_capture_time(
        published,
        published
            .snapshot
            .photos
            .iter()
            .filter(|photo| !photo.removed)
            .map(|photo| photo.id.clone())
            .collect(),
        order,
    )
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

    /// Patches the removed fact of many Photos in one critical section and
    /// drops the derived Folder index and CLI projection with them, so a
    /// removal is either wholly visible to a reader or not visible at all.
    ///
    /// The caller holds `publication` from before the owning SQLite write
    /// until this patch returns, so every reader that serializes on that lock
    /// sees either the whole state before the commit or the whole state after
    /// its effect, never a commit whose effect is missing.
    fn patch_photo_removals(&self, photo_ids: &[String], removed: bool) {
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
    fn patch_permanently_deleted(&self, photo_ids: &[String]) {
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
    pub(crate) access: crate::access::Access,
    pub(crate) library: Arc<Library>,
    library_root: PathBuf,
    pub(crate) preview: PreviewService,
    pub(crate) shared: Arc<SharedLibrary>,
    /// The Export lifecycle owner when the deployment configures processing.
    pub(crate) exports: Option<Arc<crate::export_manager::ExportManager>>,
    scan_cycle: ScanCycle,
    pub(crate) retained_queries: Mutex<QueryRegistry>,
    pub(crate) browse_namespace: u128,
    pub(crate) browse_counter: AtomicU64,
    pub(crate) cursor_signer: CursorSigner,
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
        let access = match crate::access::Access::open(config) {
            Ok(access) => access,
            Err(error) => {
                let _ = library.shutdown();
                return Err(error);
            }
        };
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
        // The Export lifecycle runs only when the deployment configures the
        // processing capability and its finite retained-output allowance.
        let exports = match (&config.processing, config.export_retained_output_bytes) {
            (Some(processing), Some(allowance)) => {
                let processing = processing.clone();
                let library_for_exports = Arc::clone(&library);
                let root_for_exports = config.library_root.clone();
                let state_for_exports = config.state_directory.clone();
                let shared_for_exports = Arc::clone(&shared);
                let resolver = Arc::new(move |photo_id: &str| {
                    let guard = shared_for_exports
                        .snapshot
                        .read()
                        .expect("published Library poisoned");
                    let published = guard.as_ref()?;
                    published
                        .photo_metadata_source(photo_id)
                        .map(|source| source.relative_path)
                });
                let opened = tokio::task::spawn_blocking(move || {
                    crate::export_manager::ExportManager::open(
                        library_for_exports,
                        root_for_exports,
                        &state_for_exports,
                        resolver,
                        processing,
                        allowance,
                    )
                })
                .await
                .map_err(|error| ServerError::Join(error.to_string()))?;
                match opened {
                    Ok(manager) => Some(Arc::new(manager)),
                    Err(message) => {
                        let library_for_close = Arc::clone(&library);
                        let _ =
                            tokio::task::spawn_blocking(move || library_for_close.shutdown()).await;
                        return Err(ServerError::Export(message));
                    }
                }
            }
            _ => None,
        };
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
            access,
            library,
            library_root: config.library_root.clone(),
            preview,
            shared,
            exports,
            scan_cycle: ScanCycle::new(),
            retained_queries: Mutex::new(QueryRegistry::production()),
            browse_namespace,
            browse_counter: AtomicU64::new(0),
            cursor_signer: CursorSigner::new(),
            shutdown: Mutex::new(false),
        });
        // Admit the startup cycle synchronously before returning the
        // Application, so an immediate explicit rescan joins this leader.
        let startup = application
            .admit_scan_cycle(scan_gate, publish_gate)
            .expect("a new Application admits its startup scan");
        drop(startup);
        // Reconcile unfinished Exports once the published Library is served.
        // Queued work restarts through ordinary admission; interrupted running
        // work resolves from its launcher receipt without a replacement.
        if let Some(manager) = application.exports.as_ref() {
            manager.reconcile_after_restart();
            manager.schedule_expiry_sweep();
        }
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
        let source = {
            let guard = self
                .shared
                .snapshot
                .read()
                .expect("published Library poisoned");
            let published = guard.as_ref().ok_or(ServerError::NotPublished)?;
            if !published.photos_by_id.contains_key(photo_id) {
                return Err(ServerError::PhotoNotFound);
            }
            published.photo_metadata_source(photo_id)
        };
        self.inspect_metadata_source(source).await
    }

    async fn inspect_metadata_source(
        &self,
        source: Option<PublishedMetadataSource>,
    ) -> Result<slipstream_core::CaptureReviewMetadata, ServerError> {
        let Some(source) = source else {
            return Ok(slipstream_core::CaptureReviewMetadata::default());
        };
        let Some(expected_revision) = source.source_revision else {
            return Ok(slipstream_core::CaptureReviewMetadata::default());
        };
        // Admission is deliberately nonblocking and happens before a blocking
        // task exists. Saturation therefore cannot create a Tokio blocking-task
        // queue whose workers wait for Library-native capacity.
        let Some(permit) = self.library.try_admit_native_work() else {
            return Ok(slipstream_core::CaptureReviewMetadata::default());
        };
        let root = self.library_root.clone();
        tokio::task::spawn_blocking(move || {
            // Keep admission in the worker through candidate observation,
            // parsing, and descriptor revalidation. If the awaiting request is
            // cancelled, this worker still owns the permit until it completes.
            let _permit = permit;
            #[cfg(test)]
            metadata_inspection_test_hook(&source.relative_path);
            let Ok(root) = slipstream_core::LibraryRoot::open(root) else {
                return slipstream_core::CaptureReviewMetadata::default();
            };
            let Ok(capability) = root.original(source.relative_path.clone()) else {
                return slipstream_core::CaptureReviewMetadata::default();
            };
            let Ok(observed_facts) = capability.facts() else {
                return slipstream_core::CaptureReviewMetadata::default();
            };
            let Ok(observed_revision) = slipstream_core::capture_source_revision(
                source.relative_path.as_str(),
                observed_facts,
            ) else {
                return slipstream_core::CaptureReviewMetadata::default();
            };
            if observed_revision != expected_revision {
                return slipstream_core::CaptureReviewMetadata::default();
            }
            slipstream_core::inspect_review_metadata(&capability, source.kind, observed_facts)
                .unwrap_or_default()
        })
        .await
        .map_err(|error| ServerError::Join(error.to_string()))
    }

    /// One consistent read of unavailable Photos plus Album memberships for
    /// the bounded recovery review entry.
    pub(crate) async fn recovery_survey(&self) -> Result<RecoverySurveyWire, ServerError> {
        Ok(self.library.recovery_survey().await?.into())
    }

    /// Proposes one folder-prefix batch of relocations for unavailable
    /// Originals, reading candidates through confined descriptors.
    pub(crate) async fn recovery_propose_batch(
        &self,
        old_prefix: &str,
        new_prefix: &str,
    ) -> Result<Vec<RecoveryProposalWire>, ServerError> {
        let survey = self.library.recovery_survey().await?;
        let snapshot = self.library.snapshot().await?;
        let old = old_prefix.to_owned();
        let new = new_prefix.to_owned();
        let root = self.library_root.clone();
        let proposals = tokio::task::spawn_blocking(move || {
            let root =
                slipstream_core::LibraryRoot::open(root).map_err(|_| ServerError::StorageLayout)?;
            let native_work = slipstream_core::NativeWorkBudget::new();
            slipstream_core::plan_manual_relocations(
                &root,
                &native_work,
                &survey,
                &snapshot,
                &old,
                &new,
            )
            .map_err(|_| ServerError::FolderInvalid)
        })
        .await
        .map_err(|error| ServerError::Join(error.to_string()))??;
        Ok(proposals.into_iter().map(Into::into).collect())
    }

    /// Proposes one mapping for a single unavailable Original, for renamed
    /// or split files a folder-prefix batch cannot express.
    pub(crate) async fn recovery_propose_single(
        &self,
        original_id: &str,
        new_location: &str,
    ) -> Result<RecoveryProposalWire, ServerError> {
        let survey = self.library.recovery_survey().await?;
        let snapshot = self.library.snapshot().await?;
        if !survey
            .unavailable
            .iter()
            .any(|record| record.original_id == original_id)
        {
            return Err(ServerError::PhotoNotFound);
        }
        let original_id = original_id.to_owned();
        let location = new_location.to_owned();
        let root = self.library_root.clone();
        let proposal = tokio::task::spawn_blocking(move || {
            let root =
                slipstream_core::LibraryRoot::open(root).map_err(|_| ServerError::StorageLayout)?;
            let native_work = slipstream_core::NativeWorkBudget::new();
            slipstream_core::plan_single_relocation(
                &root,
                &native_work,
                &survey,
                &snapshot,
                &original_id,
                &location,
            )
            .map_err(|_| ServerError::FolderInvalid)
        })
        .await
        .map_err(|error| ServerError::Join(error.to_string()))??;
        Ok(proposal.into())
    }

    /// Commits one confirmed manual relocation batch. Filesystem evidence is
    /// gathered through confined read-only descriptors, digest-verified
    /// whenever a persisted fingerprint exists, and the whole batch commits
    /// atomically or is refused without partial association.
    pub(crate) async fn recovery_apply(
        self: &Arc<Self>,
        items: Vec<RecoveryApplyItem>,
    ) -> Result<RecoveryApplyResponseWire, RecoveryApplyError> {
        if items.is_empty() {
            return Err(RecoveryApplyError::Invalid);
        }
        let survey = self
            .library
            .recovery_survey()
            .await
            .map_err(|error| RecoveryApplyError::Server(error.into()))?;
        let mut unavailable_ids = std::collections::HashSet::new();
        let mut original_ids = std::collections::HashSet::new();
        let mut fingerprints = std::collections::HashMap::new();
        for record in &survey.unavailable {
            unavailable_ids.insert(record.original_id.clone());
            if let Some(digest) = &record.fingerprint {
                fingerprints.insert(record.original_id.clone(), digest.clone());
            }
        }
        let root = self.library_root.clone();
        let worker = tokio::task::spawn_blocking(move || {
            let root =
                slipstream_core::LibraryRoot::open(root).map_err(|_| ServerError::StorageLayout)?;
            let mut rejections = Vec::new();
            let mut relocations = Vec::new();
            for item in items {
                let reject = |rejections: &mut Vec<RecoveryRejectionWire>, reason: &'static str| {
                    rejections.push(RecoveryRejectionWire {
                        original_id: item.original_id.clone(),
                        reason,
                    });
                };
                if !original_ids.insert(item.original_id.clone()) {
                    reject(&mut rejections, "colliding");
                    continue;
                }
                if !unavailable_ids.contains(&item.original_id) {
                    reject(&mut rejections, "stale");
                    continue;
                }
                let Ok(path) =
                    slipstream_core::RelativeOriginalPath::parse(item.new_location.clone())
                else {
                    reject(&mut rejections, "invalid-location");
                    continue;
                };
                let Ok(capability) = root.original(path) else {
                    reject(&mut rejections, "missing");
                    continue;
                };
                let facts = match fingerprints.get(&item.original_id) {
                    Some(expected) => {
                        let digest = capability.digest_file();
                        match digest {
                            Ok(checked) if checked.digest == *expected => checked.facts,
                            Ok(_) => {
                                reject(&mut rejections, "content-mismatch");
                                continue;
                            }
                            Err(_) => {
                                reject(&mut rejections, "unreadable");
                                continue;
                            }
                        }
                    }
                    None => match capability.facts() {
                        Ok(facts) => facts,
                        Err(_) => {
                            reject(&mut rejections, "unreadable");
                            continue;
                        }
                    },
                };
                relocations.push(slipstream_core::RequestedRelocation {
                    original_id: item.original_id,
                    to_location: item.new_location,
                    facts,
                    retire_destination: item.retire_destination,
                });
            }
            Ok::<_, ServerError>((rejections, relocations))
        })
        .await
        .map_err(|error| RecoveryApplyError::Server(ServerError::Join(error.to_string())))?
        .map_err(RecoveryApplyError::Server)?;
        let (rejections, relocations) = worker;
        if !rejections.is_empty() {
            return Err(RecoveryApplyError::Rejected {
                message: "Recovery batch rejected without changes",
                rejections,
            });
        }
        let applied = self
            .library
            .apply_relocations(relocations)
            .await
            .map_err(|error| match error {
                LibraryError::Persistence(
                    slipstream_core::persistence::PersistenceError::InvalidRecoveryMapping {
                        original_id,
                        reason,
                    },
                ) => RecoveryApplyError::Rejected {
                    message: "Recovery batch conflicts with current Library state; rescan and review again",
                    rejections: vec![RecoveryRejectionWire {
                        original_id,
                        reason,
                    }],
                },
                LibraryError::Persistence(
                    slipstream_core::persistence::PersistenceError::InvalidRecovery,
                ) => RecoveryApplyError::Rejected {
                    message: "Recovery batch conflicts with current Library state; rescan and review again",
                    rejections: Vec::new(),
                },
                other => RecoveryApplyError::Server(other.into()),
            })?;
        self.shared
            .publish_fresh(&self.library)
            .await
            .map_err(|error| RecoveryApplyError::Server(error.into()))?;
        // The committed counts become the recovery counts the scan status
        // reports, so the review notice stays truthful between scans.
        self.library
            .note_manual_recovery(applied.relocated_photos, applied.unavailable_photos);
        Ok(RecoveryApplyResponseWire {
            relocated_photos: applied.relocated_photos,
            unavailable_photos: applied.unavailable_photos,
        })
    }

    /// Truthful Library status: the scanner owns measurable phases and
    /// counters, and the shared flags decide idle, failed, or initializing.
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
        match progress.phase {
            ScanPhase::Discovering => ScanStatusWire {
                state: "discovering",
                publication: publication.clone(),
                completed: Some(usize::try_from(progress.discovered).unwrap_or(usize::MAX)),
                total: None,
                last_recovery,
                fingerprints,
            },
            ScanPhase::Inspecting => ScanStatusWire {
                state: "inspecting",
                publication: publication.clone(),
                completed: Some(usize::try_from(progress.inspected).unwrap_or(usize::MAX)),
                total: progress
                    .inspect_total
                    .map(|total| usize::try_from(total).unwrap_or(usize::MAX)),
                last_recovery,
                fingerprints,
            },
            ScanPhase::Recovering => ScanStatusWire {
                state: "recovering",
                publication: publication.clone(),
                completed: Some(usize::try_from(progress.hashed).unwrap_or(usize::MAX)),
                total: progress
                    .hash_total
                    .map(|total| usize::try_from(total).unwrap_or(usize::MAX)),
                last_recovery,
                fingerprints,
            },
            ScanPhase::Applying => ScanStatusWire {
                state: "applying",
                publication: publication.clone(),
                completed: None,
                total: None,
                last_recovery,
                fingerprints,
            },
            ScanPhase::Idle => {
                if self.shared.awaiting_scan.load(Ordering::Relaxed) > 0 {
                    // The scan finished; its result is being published.
                    ScanStatusWire {
                        state: "applying",
                        publication: publication.clone(),
                        completed: None,
                        total: None,
                        last_recovery,
                        fingerprints,
                    }
                } else if self.shared.failed.load(Ordering::Relaxed) {
                    ScanStatusWire {
                        state: "failed",
                        publication: publication.clone(),
                        completed: None,
                        total: None,
                        last_recovery,
                        fingerprints,
                    }
                } else if self.shared.published.load(Ordering::Relaxed) {
                    let photo_count = self.published_photo_count();
                    ScanStatusWire {
                        state: "idle",
                        publication: publication.clone(),
                        completed: Some(photo_count),
                        total: Some(photo_count),
                        last_recovery,
                        fingerprints,
                    }
                } else {
                    ScanStatusWire {
                        state: "initializing",
                        publication,
                        completed: None,
                        total: None,
                        last_recovery,
                        fingerprints,
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

    pub(crate) async fn create_photo_query(
        &self,
        query: slipstream_core::PhotoQuery,
        maximum_results: usize,
    ) -> Result<Vec<String>, LibraryError> {
        let _publication = self.shared.publication.lock().await;
        let projection = self
            .shared
            .snapshot
            .read()
            .expect("published Library poisoned")
            .as_ref()
            .map(|published| Arc::clone(&published.photo_query_projection))
            .expect("published query admission requires a Published Library");
        self.library
            .create_photo_query(query, projection, maximum_results)
            .await
    }

    pub(crate) async fn published_photos_by_id(
        &self,
        photo_ids: Vec<String>,
    ) -> Result<Vec<Option<slipstream_core::PhotoRead>>, LibraryError> {
        let _publication = self.shared.publication.lock().await;
        let projection = self
            .shared
            .snapshot
            .read()
            .expect("published Library poisoned")
            .as_ref()
            .map(|published| published.photo_read_projection(&photo_ids))
            .expect("published Photo reads require a Published Library");
        self.library.photos_by_id(photo_ids, projection).await
    }

    pub(crate) async fn published_photo_detail(
        &self,
        photo_id: &str,
    ) -> Result<Option<PublishedPhotoDetail>, LibraryError> {
        let photo_id = photo_id.to_owned();
        let (photo, metadata_source) = {
            let _publication = self.shared.publication.lock().await;
            let (projection, metadata_source) = {
                let guard = self
                    .shared
                    .snapshot
                    .read()
                    .expect("published Library poisoned");
                let published = guard
                    .as_ref()
                    .expect("published Photo reads require a Published Library");
                (
                    published.photo_read_projection(std::slice::from_ref(&photo_id)),
                    published.photo_metadata_source(&photo_id),
                )
            };
            let photo = self
                .library
                .photos_by_id(vec![photo_id], projection)
                .await?
                .pop()
                .flatten();
            (photo, metadata_source)
        };
        Ok(photo.map(|photo| PublishedPhotoDetail {
            photo,
            metadata_source,
        }))
    }

    pub(crate) async fn inspect_published_photo_detail(
        &self,
        detail: PublishedPhotoDetail,
    ) -> (
        slipstream_core::PhotoRead,
        slipstream_core::CaptureReviewMetadata,
    ) {
        let metadata = if detail.photo.capture.state == slipstream_core::CaptureMetadataState::Known
        {
            self.inspect_metadata_source(detail.metadata_source)
                .await
                .unwrap_or_default()
        } else {
            slipstream_core::CaptureReviewMetadata::default()
        };
        (detail.photo, metadata)
    }

    pub(crate) fn publication_evaluated_at(&self, publication: &str) -> Option<SystemTime> {
        self.shared
            .snapshot
            .read()
            .expect("published Library poisoned")
            .as_ref()
            .filter(|published| published.publication_value() == publication)
            .map(|published| published.evaluated_at)
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
                    published
                        .snapshot
                        .photos
                        .iter()
                        .filter(|photo| !photo.removed)
                        .count(),
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
                // The persisted member list and the published facts are read
                // as one publication: a removal committed in between must not
                // leave a removed Photo in the Album source that open returns.
                let _publication = self.shared.publication.lock().await;
                let target = self
                    .library
                    .album_browse_target(&id)
                    .await?
                    .ok_or(ServerError::BrowseNotFound)?;
                // A view change supplies the browser's current Photo as the
                // anchor; only an open without one resumes at the durable
                // saved position, so a filter or order change never lands on
                // a different Photo than the one current in the browser.
                let resume_member_id = preferred_photo_id
                    .is_none()
                    .then(|| album_resume_member(&target.members, target.saved_photo_id.as_deref()))
                    .flatten();
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
        let total = photo_ids.len();
        self.retained_queries
            .lock()
            .expect("retained queries poisoned")
            .insert(
                token.clone(),
                RetainedKind::Browse,
                Some(selection),
                photo_ids,
                Instant::now(),
                SystemTime::now(),
            )
            .map_err(|()| ServerError::QueryCapacity)?;
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
        let page = self
            .retained_queries
            .lock()
            .expect("retained queries poisoned")
            .page(
                token,
                RetainedKind::Browse,
                start,
                limit,
                Instant::now(),
                SystemTime::now(),
            )
            .ok_or(ServerError::BrowseNotFound)?;
        let (ids, total) = (page.ids, page.total);
        let facts = {
            let source_guard = self
                .shared
                .snapshot
                .read()
                .expect("published Library poisoned");
            let Some(source) = source_guard.as_ref() else {
                return Err(ServerError::NotPublished);
            };
            // A page is complete for its position: a window that could not
            // present every Photo it names is not answered with a shorter
            // page. A Photo the publication no longer holds, or one whose
            // removal marker is set, expires the Snapshot instead — the
            // browser reopens the source and reads the state the Library now
            // holds.
            let mut facts = Vec::with_capacity(ids.len());
            for id in &ids {
                let Some(position) = source.photos_by_id.get(id).copied() else {
                    return Err(ServerError::BrowseNotFound);
                };
                let Some(photo) = source.snapshot.photos.get(position) else {
                    return Err(ServerError::BrowseNotFound);
                };
                if photo.removed {
                    return Err(ServerError::BrowseNotFound);
                }
                let originals = [Some(&photo.original_id)]
                    .into_iter()
                    .flatten()
                    .filter_map(|id| source.originals_by_id.get(id))
                    .filter_map(|position| source.snapshot.originals.get(*position))
                    .cloned()
                    .collect();
                facts.push(PreviewFacts::from_records(photo.clone(), originals));
            }
            facts
        };
        let mut photos = Vec::with_capacity(facts.len());
        for facts in facts {
            let (preview_url, thumbnail_url) = self.derivative_urls(&facts).await;
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

    /// The current derivative URLs one Photo's summary may reference. A Photo
    /// without a current Review Preview has neither URL.
    async fn derivative_urls(&self, facts: &PreviewFacts) -> (Option<String>, Option<String>) {
        if facts.photo.preview_state == PreviewState::Unavailable {
            return (None, None);
        }
        let preview_url = self
            .preview
            .lookup_current_key(facts, DerivativeTarget::Review2560)
            .await
            .ok()
            .flatten()
            .map(|cache_key| {
                format!(
                    "/api/private/derivatives/{}/review/{}.jpg",
                    facts.photo.id, cache_key
                )
            });
        let thumbnail_url = self
            .preview
            .lookup_current_key(facts, DerivativeTarget::Thumbnail512)
            .await
            .ok()
            .flatten()
            .map(|cache_key| {
                format!(
                    "/api/private/derivatives/{}/thumbnail/{}.jpg",
                    facts.photo.id, cache_key
                )
            });
        (preview_url, thumbnail_url)
    }

    /// Confirms the removal of one reviewed rejected result.
    ///
    /// The Browse Snapshot supplies the complete reviewed result and the
    /// Selection State filter it was reviewed under, so removal can only ever
    /// hide Photos the browser actually showed as Rejected. The browser
    /// supplies the operation id, so a retried request repeats one operation
    /// instead of creating a second one, and Undo names the group by that id
    /// rather than by a transferred Photo list.
    pub async fn remove_photos(
        &self,
        token: &str,
        operation_id: &str,
    ) -> Result<PhotoRemovalResponse, ServerError> {
        let (photo_ids, filter) = self
            .retained_queries
            .lock()
            .expect("retained queries poisoned")
            .browse_snapshot(token, Instant::now())
            .ok_or(ServerError::BrowseNotFound)?;
        if filter != Some(BrowseSelectionFilter::Rejected) {
            return Err(ServerError::RemovalFilter);
        }
        // The write and the publication patch are one critical section: a
        // reader that serializes on `publication` cannot acquire it between
        // the commit and the patch and observe the Photos as still present.
        let _publication = self.shared.publication.lock().await;
        let result = self
            .library
            .remove_photos(slipstream_core::PhotoRemovalMutation {
                photo_ids,
                operation_id: operation_id.to_owned(),
            })
            .await?;
        self.shared
            .patch_photo_removals(&result.newly_removed, true);
        Ok(PhotoRemovalResponse {
            operation_id: result.operation_id,
            counts: PhotoRemovalCountsWire {
                removed: result.counts.removed,
                changed_elsewhere: result.counts.changed_elsewhere,
                missing: result.counts.missing,
                already_removed: result.counts.already_removed,
            },
            changed_elsewhere: result.changed_elsewhere,
            missing: result.missing,
            already_removed: result.already_removed,
        })
    }
    /// Applies one explicit removal set after rechecking every submitted
    /// decision and removal-state evidence.
    pub async fn remove_photos_explicit(
        &self,
        mutation: slipstream_core::ExplicitPhotoRemovalMutation,
    ) -> Result<ExplicitPhotoRemovalResponse, ServerError> {
        let _publication = self.shared.publication.lock().await;
        let result = self.library.remove_photos_explicit(mutation).await?;
        self.shared
            .patch_photo_removals(&result.newly_removed, true);
        Ok(explicit_photo_removal_response(result))
    }

    /// Reads an accepted explicit removal result without changing state.
    pub async fn photo_removal_operation(
        &self,
        operation_id: String,
    ) -> Result<Option<ExplicitPhotoRemovalResponse>, ServerError> {
        Ok(self
            .library
            .photo_removal_operation(operation_id)
            .await?
            .map(explicit_photo_removal_response))
    }

    /// Applies one explicit Restore attempt and retains its result in the
    /// persistence owner for replay and inspection.
    pub async fn restore_photos_explicit(
        &self,
        mutation: slipstream_core::ExplicitPhotoRestoreMutation,
    ) -> Result<ExplicitPhotoRestoreResponse, ServerError> {
        let _publication = self.shared.publication.lock().await;
        let result = self.library.restore_photos_explicit(mutation).await?;
        self.shared.patch_photo_removals(&result.restored, false);
        Ok(explicit_photo_restore_response(result))
    }

    /// Reads an accepted explicit Restore result without changing state.
    pub async fn photo_restore_operation(
        &self,
        operation_id: String,
    ) -> Result<Option<ExplicitPhotoRestoreResponse>, ServerError> {
        Ok(self
            .library
            .photo_restore_operation(operation_id)
            .await?
            .map(explicit_photo_restore_response))
    }

    /// Restores every Photo one operation still owns, or one explicit bounded
    /// set, and patches the removed fact back into the published Library.
    pub async fn restore_photos(
        &self,
        restoration: slipstream_core::PhotoRestoration,
    ) -> Result<PhotoRestorationResponse, ServerError> {
        // As for a removal, the write and its published effect are one
        // critical section: a restored Photo cannot be missing from a source
        // opened after the restore committed.
        let _publication = self.shared.publication.lock().await;
        let result = self.library.restore_photos(restoration).await?;
        self.shared.patch_photo_removals(&result.restored, false);
        Ok(PhotoRestorationResponse {
            counts: PhotoRestorationCountsWire {
                restored: result.counts.restored,
                changed_elsewhere: result.counts.changed_elsewhere,
                missing: result.counts.missing,
            },
            changed_elsewhere: result.changed_elsewhere,
            missing: result.missing,
            operations: result
                .operations
                .into_iter()
                .map(|remainder| PhotoOperationRemainderWire {
                    operation_id: remainder.operation_id,
                    removed: remainder.removed,
                })
                .collect(),
        })
    }

    /// One bounded page of removed Photos, newest removal first. The persisted
    /// removal order is authoritative; the published Library supplies the same
    /// Grid facts and current derivative URLs Grid View renders, so a removed
    /// Photo is recognizable without leaving the removal view.
    pub async fn removed_photos(
        &self,
        start: usize,
        limit: usize,
    ) -> Result<RemovedPhotosResponse, ServerError> {
        if limit == 0 || limit > MAX_REMOVED_WINDOW {
            return Err(ServerError::RemovedWindow);
        }
        // The persisted page and the published facts are read as one
        // publication, so a removal or restore committed between them cannot
        // present a row whose removal the Library no longer holds.
        let _publication = self.shared.publication.lock().await;
        let (records, total, operation) = self.library.removed_photos(start, limit).await?;
        let facts = {
            let guard = self
                .shared
                .snapshot
                .read()
                .expect("published Library poisoned");
            let Some(published) = guard.as_ref() else {
                return Err(ServerError::NotPublished);
            };
            records
                .into_iter()
                .filter_map(|record| {
                    let position = published.photos_by_id.get(&record.photo_id).copied()?;
                    let photo = published.snapshot.photos.get(position)?;
                    let originals = [Some(&photo.original_id)]
                        .into_iter()
                        .flatten()
                        .filter_map(|id| published.originals_by_id.get(id))
                        .filter_map(|position| published.snapshot.originals.get(*position))
                        .cloned()
                        .collect();
                    Some((
                        record.removed_at_ms,
                        record.pending_verification,
                        PreviewFacts::from_records(photo.clone(), originals),
                    ))
                })
                .collect::<Vec<_>>()
        };
        let mut photos = Vec::with_capacity(facts.len());
        for (removed_at_ms, pending_verification_operation_id, facts) in facts {
            let Some(original) = facts
                .originals
                .iter()
                .find(|original| original.id == facts.photo.original_id)
            else {
                continue;
            };
            let original_kind = match original.kind {
                slipstream_core::OriginalKind::Raw => "raw",
                slipstream_core::OriginalKind::Jpeg => "jpeg",
            };
            let (preview_url, thumbnail_url) = self.derivative_urls(&facts).await;
            let originals_by_id = facts
                .originals
                .iter()
                .enumerate()
                .map(|(position, original)| (original.id.clone(), position))
                .collect::<std::collections::HashMap<_, _>>();
            photos.push(RemovedPhotoWire {
                removed_at_ms,
                original_location: original.relative_path.to_string(),
                original_kind,
                original_size: original.available.then_some(original.facts.size),
                pending_verification_operation_id,
                photo: photo_summary_indexed_with_url(
                    &facts.photo,
                    &facts.originals,
                    &originals_by_id,
                    preview_url,
                    thumbnail_url,
                ),
            });
        }
        Ok(RemovedPhotosResponse {
            start,
            limit,
            total,
            review_maximum: slipstream_core::PERMANENT_DELETION_MAX,
            operation: operation.map(|operation| PhotoOperationRemainderWire {
                operation_id: operation.operation_id,
                removed: operation.removed,
            }),
            photos,
        })
    }
    pub async fn prepare_permanent_deletion(
        &self,
        operation_id: String,
        selection: slipstream_core::PermanentDeletionSelection,
    ) -> Result<PermanentDeletionReviewResponse, ServerError> {
        let _publication = self.shared.publication.lock().await;
        let review = self
            .library
            .prepare_permanent_deletion(operation_id, selection)
            .await?;
        Ok(PermanentDeletionReviewResponse {
            operation_id: review.operation_id,
            items: review
                .items
                .into_iter()
                .map(|item| PermanentDeletionReviewItemWire {
                    photo_id: item.photo_id,
                    removed_at_ms: item.removed_at_ms,
                    original_id: item.original_id,
                    original_location: item.relative_path.to_string(),
                    original_kind: match item.kind {
                        slipstream_core::OriginalKind::Raw => "raw",
                        slipstream_core::OriginalKind::Jpeg => "jpeg",
                    }
                    .to_owned(),
                    size: item.size,
                    albums: item
                        .albums
                        .into_iter()
                        .map(|album| PhotoAlbumMembershipWire {
                            id: album.album_id,
                            name: album.album_name,
                        })
                        .collect(),
                })
                .collect(),
            rejected: review
                .rejected
                .into_iter()
                .map(|(photo_id, reason)| PermanentDeletionRejectionWire {
                    photo_id,
                    reason: match reason {
                        slipstream_core::PermanentDeletionRejection::Missing => "missing",
                        slipstream_core::PermanentDeletionRejection::ChangedElsewhere => {
                            "changed-elsewhere"
                        }
                        slipstream_core::PermanentDeletionRejection::PendingVerification => {
                            "pending-verification"
                        }
                    },
                })
                .collect(),
        })
    }

    pub async fn permanently_delete(
        &self,
        operation_id: String,
    ) -> Result<PermanentDeletionResponse, ServerError> {
        let _publication = self.shared.publication.lock().await;
        let result = self.library.permanently_delete(operation_id).await?;
        let deleted_photo_ids = result
            .items
            .iter()
            .filter(|item| item.state == slipstream_core::PermanentDeletionItemState::Deleted)
            .map(|item| item.photo_id.clone())
            .collect::<Vec<_>>();
        self.shared.patch_permanently_deleted(&deleted_photo_ids);
        Ok(permanent_deletion_response(result))
    }

    pub async fn read_permanent_deletion(
        &self,
        operation_id: String,
    ) -> Result<PermanentDeletionResponse, ServerError> {
        let _publication = self.shared.publication.lock().await;
        Ok(permanent_deletion_response(
            self.library.read_permanent_deletion(operation_id).await?,
        ))
    }

    /// Resolves one stable Photo identity against an immutable Browse Snapshot
    /// without transferring Photo facts or materializing the source.
    pub fn browse_position(
        &self,
        token: &str,
        photo_id: &str,
    ) -> Result<BrowsePositionResponse, ServerError> {
        let position = self
            .retained_queries
            .lock()
            .expect("retained queries poisoned")
            .position(token, photo_id, Instant::now())
            .ok_or(ServerError::BrowseNotFound)?;
        Ok(BrowsePositionResponse { position })
    }

    pub fn browse_close(&self, token: &str) {
        self.retained_queries
            .lock()
            .expect("retained queries poisoned")
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

    pub async fn create_album_checked(
        &self,
        name: String,
    ) -> Result<slipstream_core::AlbumCreationResult, LibraryError> {
        self.library.create_album_checked(name).await
    }

    pub async fn mutate_album_checked(
        &self,
        mutation: slipstream_core::CheckedAlbumMutation,
    ) -> Result<slipstream_core::CheckedAlbumMutationResult, LibraryError> {
        self.library.mutate_album_checked(mutation).await
    }

    pub async fn add_album_members(
        &self,
        album_id: &str,
        photo_ids: Vec<String>,
    ) -> Result<AlbumMembershipAddResponse, ServerError> {
        let result = self
            .library
            .mutate_album_membership(slipstream_core::AlbumMembershipMutation::Add {
                album_id: album_id.to_owned(),
                photo_ids,
            })
            .await?;
        Ok(AlbumMembershipAddResponse {
            album_id: result.album_id,
            added_photo_ids: result.added_photo_ids,
            already_member_photo_ids: result.already_member_photo_ids,
            albums: self.albums().await?.albums,
        })
    }

    pub async fn remove_added_album_members(
        &self,
        album_id: &str,
        photo_ids: Vec<String>,
    ) -> Result<AlbumMembershipRemoveResponse, ServerError> {
        let result = self
            .library
            .mutate_album_membership(slipstream_core::AlbumMembershipMutation::RemoveAdded {
                album_id: album_id.to_owned(),
                photo_ids,
            })
            .await?;
        Ok(AlbumMembershipRemoveResponse {
            album_id: result.album_id,
            removed_photo_ids: result.removed_photo_ids,
            already_absent_photo_ids: result.already_absent_photo_ids,
            albums: self.albums().await?.albums,
        })
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

    /// Applies one version-checked Photo decision batch and mirrors every
    /// effective change into the published snapshot, so Web browsing shows
    /// the same facts the confirmed result reports before the next scan.
    pub async fn mutate_photo_decision_checked(
        &self,
        mutation: slipstream_core::CheckedPhotoDecisionMutation,
    ) -> Result<slipstream_core::CheckedPhotoDecisionResult, LibraryError> {
        let (field, value) = (mutation.field, mutation.value);
        let result = self.library.mutate_photo_decision_checked(mutation).await?;
        for item in &result.results {
            if matches!(
                item.outcome,
                slipstream_core::CheckedPhotoDecisionOutcome::Changed { .. }
            ) {
                self.shared
                    .patch_photo(&item.photo_id, |photo| match (field, value) {
                        (
                            PhotoStateField::SelectionState,
                            PhotoStateValue::Selection(selection),
                        ) => photo.selection_state = selection,
                        (PhotoStateField::Rating, PhotoStateValue::Rating(rating)) => {
                            photo.rating = rating;
                        }
                        _ => {}
                    })
                    .await;
            }
        }
        Ok(result)
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

    pub async fn mutate_photo_state_batch(
        &self,
        mutation: slipstream_core::PhotoStateBatchMutation,
    ) -> Result<slipstream_core::PhotoStateBatchResult, ServerError> {
        let value = mutation.value;
        let result = self.library.mutate_photo_state_batch(mutation).await?;
        for applied in &result.applied {
            self.shared
                .patch_photo(&applied.photo_id, |photo| {
                    photo.selection_state = value;
                })
                .await;
        }
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
        let originals = [Some(&photo.original_id)]
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
        self.admitted_derivative(
            photo_id,
            cache_key,
            target,
            slipstream_core::DerivativePriority::Current,
            false,
        )
        .await
    }

    /// Answers one CLI Preview request with the current supported derivative for
    /// the requested target, or the reason no current derivative exists. CLI
    /// demand is admitted at the shared Background lane, below every Web lane.
    pub(crate) async fn cli_preview(
        &self,
        photo_id: &str,
        target: DerivativeTarget,
    ) -> CliPreviewOutcome {
        if !valid_id(photo_id) {
            return Err(CliPreviewRefusal::Missing);
        }
        let published_facts = self.published_preview_facts(photo_id);
        let result = if let Some(facts) = published_facts.clone() {
            self.preview
                .request_with_facts(
                    facts,
                    target,
                    slipstream_core::DerivativePriority::Background,
                )
                .await
        } else {
            return Err(CliPreviewRefusal::Missing);
        };
        let ready = match result {
            Ok(slipstream_core::PreviewRequestResult::Current(ready)) => ready,
            // Current generation failed for the admitted source, or the source
            // changed while it ran. Older bytes are never a current Preview.
            Ok(slipstream_core::PreviewRequestResult::Stale(_))
            | Ok(slipstream_core::PreviewRequestResult::StaleIgnored)
            | Ok(slipstream_core::PreviewRequestResult::Failed(_)) => {
                return Err(CliPreviewRefusal::published_state(published_facts.as_ref()));
            }
            Ok(slipstream_core::PreviewRequestResult::Unavailable(unavailable)) => {
                return Err(match unavailable.reason {
                    slipstream_core::PreviewUnavailableReason::PhotoNotFound => {
                        CliPreviewRefusal::Missing
                    }
                    slipstream_core::PreviewUnavailableReason::OriginalUnavailable
                    | slipstream_core::PreviewUnavailableReason::NoUsableSource => {
                        CliPreviewRefusal::Unavailable
                    }
                });
            }
            // The shared Preview owner is saturated or closed; the caller may
            // ask again once the shared bounds have room.
            Err(slipstream_core::PreviewServiceError::Saturated)
            | Err(slipstream_core::PreviewServiceError::Closed) => {
                return Err(CliPreviewRefusal::Busy);
            }
            Err(_) => {
                return Err(CliPreviewRefusal::published_state(published_facts.as_ref()));
            }
        };
        // A current derivative whose admitted source revision cannot be
        // established must not be offered as one.
        let source_revision = published_facts
            .as_ref()
            .and_then(|facts| preview_source_revision(facts, ready.source))
            .ok_or_else(|| CliPreviewRefusal::published_state(published_facts.as_ref()))?;
        Ok(CliPreviewReady {
            photo_id: photo_id.to_owned(),
            source: ready.source,
            source_revision,
            width: ready.width,
            height: ready.height,
            cache_key: ready.cache_key,
        })
    }

    /// Reads the exact derivative one admitted CLI request was given, refuses
    /// bytes that are no longer current, and repeats the admitted facts.
    pub(crate) async fn cli_derivative(
        &self,
        photo_id: &str,
        cache_key: &str,
        target: DerivativeTarget,
    ) -> Result<Option<DerivativeDelivery>, ServerError> {
        self.admitted_derivative(
            photo_id,
            cache_key,
            target,
            slipstream_core::DerivativePriority::Background,
            true,
        )
        .await
    }

    /// Admits one derivative delivery under one Preview request. `current_only`
    /// refuses a stale result for a caller that must not receive older bytes as
    /// a current Preview, and repeats that caller's typed facts.
    async fn admitted_derivative(
        &self,
        photo_id: &str,
        cache_key: &str,
        target: DerivativeTarget,
        priority: slipstream_core::DerivativePriority,
        current_only: bool,
    ) -> Result<Option<DerivativeDelivery>, ServerError> {
        if !valid_id(photo_id) || !is_hex_key(cache_key) {
            return Ok(None);
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
        let ready = match result {
            Ok(slipstream_core::PreviewRequestResult::Current(ready)) => ready,
            Ok(slipstream_core::PreviewRequestResult::Stale(ready)) if !current_only => ready,
            _ => return Ok(None),
        };
        if ready.cache_key != cache_key {
            return Ok(None);
        }
        let source_revision = published_facts
            .as_ref()
            .and_then(|facts| preview_source_revision(facts, ready.source));
        if current_only && source_revision.is_none() {
            return Ok(None);
        }
        let cache = self.preview.scheduler().cache().clone();
        let cache_key = cache_key.to_owned();
        let bytes = tokio::task::spawn_blocking(move || cache.read_derivative(&cache_key, target))
            .await
            .map_err(|error| ServerError::Join(error.to_string()))?
            .ok();
        Ok(bytes.map(|bytes| {
            // A CLI caller compares these facts with the metadata it was
            // admitted with, so an unlabelled current derivative is refused
            // rather than served without them.
            let cli_facts = match (current_only, source_revision) {
                (true, Some(source_revision)) => Some(CliDerivativeFacts {
                    photo_id: photo_id.to_owned(),
                    source: ready.source.wire_name(),
                    source_revision,
                    width: ready.width,
                    height: ready.height,
                }),
                _ => None,
            };
            DerivativeDelivery {
                cache_key: ready.cache_key,
                bytes,
                cli_facts,
            }
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

fn preview_source_revision(facts: &PreviewFacts, _source: PreviewSource) -> Option<String> {
    let original = facts.originals.iter().find(|original| {
        original.id == facts.photo.original_id
            && original.available
            && original.error_category.is_none()
    })?;
    source_revision(
        original.relative_path.as_str(),
        original.facts.size,
        original.facts.mtime_ms,
    )
    .ok()
}
