// Published Library snapshots and projections.
use super::*;
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
pub(super) struct PublishedMetadataSource {
    pub(super) relative_path: slipstream_core::RelativeOriginalPath,
    pub(super) kind: slipstream_core::OriginalKind,
    pub(super) source_revision: Option<String>,
}

pub(crate) struct PublishedPhotoDetail {
    pub(crate) photo: slipstream_core::PhotoRead,
    pub(super) metadata_source: Option<PublishedMetadataSource>,
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
    pub(super) fn published_state(published_facts: Option<&PreviewFacts>) -> Self {
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
pub(super) type MetadataInspectionTestHook =
    dyn Fn(&slipstream_core::RelativeOriginalPath) + Send + Sync;

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
pub(super) fn metadata_inspection_test_hook(path: &slipstream_core::RelativeOriginalPath) {
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
pub(super) fn publication_generation() -> u64 {
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
pub(super) fn photo_query_projection(
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
    pub(super) fn new(snapshot: slipstream_core::ScanSnapshot) -> Self {
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
    pub(super) fn rebuild_query_projection(&mut self) {
        self.photo_query_projection = photo_query_projection(&self.snapshot, &self.originals_by_id);
    }

    /// Drops the derived Folder index. Called from the same critical section
    /// that patches removal facts, so no reader can keep a Folder projection
    /// built from the previous removal generation.
    pub(super) fn invalidate_folder_index(&self) {
        *self.folder_index.write().expect("folder index poisoned") = None;
    }

    pub(crate) fn publication_value(&self) -> String {
        format!("{:016x}", self.publication)
    }

    pub(super) fn photo_read_projection(
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

    pub(super) fn photo_metadata_source(&self, photo_id: &str) -> Option<PublishedMetadataSource> {
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
pub(super) fn published_photo<'a>(
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
pub(super) fn published_capture_key<'a>(
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
pub(super) fn published_capture_key_for_id<'a>(
    published: &'a Published,
    id: &str,
) -> Option<&'a str> {
    published_photo(published, id).and_then(|photo| published_capture_key(published, photo))
}

/// Per-state Selection counts for one complete source ID list. Photos the
/// Published Library cannot resolve are not counted, so a count never claims
/// more than the facts the open source can show.
pub(super) fn selection_counts_for_ids(
    published: &Published,
    ids: &[String],
) -> SelectionCountsWire {
    SelectionCountsWire::from_selection_states(
        ids.iter()
            .filter_map(|id| published_photo(published, id))
            .map(|photo| photo.selection_state),
    )
}

/// Applies one Selection State filter to a complete ordered source ID list.
/// The filter selects from that order and never rewrites it.
pub(super) fn filter_ids_by_selection(
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
pub(super) fn order_ids_by_capture_time(
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
pub(super) fn order_album_members(
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
pub(super) fn album_resume_member(
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
pub(super) fn ordered_library_ids(published: &Published, order: BrowseViewOrder) -> Vec<String> {
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
