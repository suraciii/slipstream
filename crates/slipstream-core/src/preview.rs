//! Demand-driven Preview inspection and derivative generation.
//!
//! Requests contain only a Photo identity, target, and priority.  Source paths are
//! resolved from an immutable Library snapshot and immediately converted to confined
//! Original capabilities before any native work begins.

use crate::{
    CacheDirectory, CacheError, CachedDerivative, DerivativeFailureKind, DerivativeIdentity,
    DerivativePriority, DerivativeResult, DerivativeScheduler, DerivativeTarget, Library,
    LibraryError, OriginalCapability, OriginalRecord, PhotoRecord, PreviewSeed, PreviewSeedResult,
    PreviewSource, PreviewState, ScanSnapshot, extract_embedded_jpeg, inspect_matching_jpeg,
    source_revision,
};
use std::{
    collections::HashMap,
    fmt,
    path::Path,
    sync::{Arc, Condvar, Mutex, Weak},
    thread::{self, JoinHandle},
};

const CACHE_LOOKUP_CAPACITY: usize = 2;
const CACHE_HYDRATION_CAPACITY: usize = 1;

pub const DEFAULT_PREVIEW_WORKERS: usize = 2;
pub const DEFAULT_PREVIEW_QUEUE_CAPACITY: usize = 64;
pub const DEFAULT_PREVIEW_WAITER_CAPACITY: usize = 64;

/// The bounded, immutable facts a server request received from its published
/// Library. It contains one Photo and its single Original record; it never
/// contains an Original capability or an opened file.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewFacts {
    pub photo: PhotoRecord,
    pub originals: Vec<OriginalRecord>,
}

impl PreviewFacts {
    pub fn from_records(photo: PhotoRecord, originals: Vec<OriginalRecord>) -> Self {
        Self { photo, originals }
    }

    pub fn from_snapshot(snapshot: &ScanSnapshot, photo_id: &str) -> Option<Self> {
        let photo = snapshot.photos.iter().find(|photo| photo.id == photo_id)?;
        let originals = snapshot
            .originals
            .iter()
            .find(|original| original.id == photo.original_id)
            .cloned()
            .into_iter()
            .collect();
        Some(Self::from_records(photo.clone(), originals))
    }

    fn selected_source(&self) -> Option<(PreviewSource, &OriginalRecord)> {
        self.originals.iter().find_map(|original| {
            (original.id == self.photo.original_id
                && original.available
                && original.error_category.is_none())
            .then_some((original.kind.preview_source(), original))
        })
    }

    pub fn source_matches(&self, photo: &PhotoRecord, originals: &[OriginalRecord]) -> bool {
        source_bundle_key(&self.photo, &self.originals) == source_bundle_key(photo, originals)
    }

    fn request_key(&self) -> String {
        source_bundle_key(&self.photo, &self.originals)
    }

    fn durable_unavailable_current(&self) -> bool {
        if self.photo.preview_state != PreviewState::Unavailable {
            return false;
        }
        let Some((_source, original)) = self.selected_source() else {
            return true;
        };
        let Some(stored_revision) = self.photo.preview_source_revision.as_deref() else {
            return false;
        };
        source_revision(
            original.relative_path.as_str(),
            original.facts.size,
            original.facts.mtime_ms,
        )
        .is_ok_and(|revision| revision == stored_revision)
    }
}

fn source_bundle_key(photo: &PhotoRecord, originals: &[OriginalRecord]) -> String {
    let original_key = |id: &str| {
        let Some(original) = originals.iter().find(|original| original.id == id) else {
            return format!("missing:{id}");
        };
        let kind = match original.kind {
            crate::OriginalKind::Raw => "raw",
            crate::OriginalKind::Jpeg => "jpeg",
        };
        let error = match original.error_category {
            None => "none",
            Some(crate::OriginalErrorCategory::Unreadable) => "unreadable",
            Some(crate::OriginalErrorCategory::Changed) => "changed",
        };
        format!(
            "{id}\0{kind}\0{}\0{}\0{}\0{}\0{error}",
            original.relative_path,
            original.facts.size,
            original.facts.mtime_ms.to_bits(),
            original.available,
        )
    };
    format!(
        "{}\0{}\0{}\0{}",
        photo.id,
        photo.original_id,
        photo.available,
        original_key(&photo.original_id),
    )
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreviewReady {
    pub cache_key: String,
    pub target: DerivativeTarget,
    pub source: PreviewSource,
    pub source_size: u64,
    pub source_mtime_ms: f64,
    pub embedded_candidate_identity: Option<String>,
    pub width: u32,
    pub height: u32,
    pub generated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewUnavailableReason {
    PhotoNotFound,
    OriginalUnavailable,
    NoUsableSource,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviewUnavailable {
    pub reason: PreviewUnavailableReason,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewFailureKind {
    Unsupported,
    Malformed,
    ResourceLimit,
    OutputLimit,
    Io,
    Internal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviewFailure {
    pub kind: PreviewFailureKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PreviewRequestResult {
    Current(PreviewReady),
    Stale(PreviewReady),
    StaleIgnored,
    Unavailable(PreviewUnavailable),
    Failed(PreviewFailure),
}

#[derive(Clone, Debug)]
pub enum PreviewServiceError {
    Saturated,
    Closed,
    Changed,
    Library(LibraryError),
    Cache(CacheError),
}

impl fmt::Display for PreviewServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Saturated => formatter.write_str("Preview service is saturated"),
            Self::Closed => formatter.write_str("Preview service is closed"),
            Self::Changed => formatter.write_str("Original File changed; rescan required"),
            Self::Library(error) => error.fmt(formatter),
            Self::Cache(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for PreviewServiceError {}

impl From<LibraryError> for PreviewServiceError {
    fn from(error: LibraryError) -> Self {
        Self::Library(error)
    }
}

impl From<CacheError> for PreviewServiceError {
    fn from(error: CacheError) -> Self {
        match error {
            CacheError::Saturated => Self::Saturated,
            CacheError::Closed => Self::Closed,
            other => Self::Cache(other),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct RequestKey {
    photo_id: String,
    target: DerivativeTarget,
    retry: bool,
    published_source: Option<String>,
}
fn request_key(
    photo_id: &str,
    target: DerivativeTarget,
    retry: bool,
    facts: Option<&PreviewFacts>,
) -> RequestKey {
    RequestKey {
        photo_id: photo_id.to_owned(),
        target,
        retry,
        published_source: facts.map(PreviewFacts::request_key),
    }
}

type PreviewWaitResult = Result<PreviewRequestResult, PreviewServiceError>;
type PreviewWaitState = (Mutex<Option<PreviewWaitResult>>, Condvar);

#[derive(Clone)]
struct WaitCell {
    result: Arc<PreviewWaitState>,
}

impl WaitCell {
    fn new() -> Self {
        Self {
            result: Arc::new((Mutex::new(None), Condvar::new())),
        }
    }

    fn complete(&self, result: Result<PreviewRequestResult, PreviewServiceError>) {
        let (lock, signal) = &*self.result;
        *lock.lock().expect("Preview waiter lock poisoned") = Some(result);
        signal.notify_all();
    }

    fn wait(&self) -> Result<PreviewRequestResult, PreviewServiceError> {
        let (lock, signal) = &*self.result;
        let mut result = lock.lock().expect("Preview waiter lock poisoned");
        while result.is_none() {
            result = signal.wait(result).expect("Preview waiter lock poisoned");
        }
        result
            .as_ref()
            .expect("Preview waiter result disappeared")
            .clone()
    }
}

struct PreviewJob {
    photo_id: String,
    target: DerivativeTarget,
    priority: Mutex<DerivativePriority>,
    order: u64,
    key: RequestKey,
    retry: bool,
    facts: Option<PreviewFacts>,
    cell: WaitCell,
}

struct ServiceState {
    closed: bool,
    terminate_workers: bool,
    active: usize,
    waiters: usize,
    next_order: u64,
    queue: Vec<Arc<PreviewJob>>,
    in_flight: HashMap<RequestKey, Arc<PreviewJob>>,
}

struct WaiterAdmission {
    inner: Arc<ServiceInner>,
}

impl Drop for WaiterAdmission {
    fn drop(&mut self) {
        let mut state = self.inner.state.lock().expect("Preview service poisoned");
        state.waiters = state.waiters.saturating_sub(1);
        self.inner.signal.notify_all();
    }
}

struct ServiceInner {
    library: Arc<Library>,
    scheduler: DerivativeScheduler,
    state: Mutex<ServiceState>,
    signal: Condvar,
    workers: Mutex<Vec<JoinHandle<()>>>,
    worker_count: usize,
    queue_capacity: usize,
    waiter_capacity: usize,
    cache_lookup: Arc<tokio::sync::Semaphore>,
    cache_hydration: Arc<tokio::sync::Semaphore>,
    shutdown_lock: Mutex<()>,
    public_handles: std::sync::atomic::AtomicUsize,
}

#[derive(Clone, Debug)]
pub struct PreviewServiceOptions {
    pub workers: usize,
    pub queue_capacity: usize,
    pub waiter_capacity: usize,
}

impl Default for PreviewServiceOptions {
    fn default() -> Self {
        Self {
            workers: DEFAULT_PREVIEW_WORKERS,
            queue_capacity: DEFAULT_PREVIEW_QUEUE_CAPACITY,
            waiter_capacity: DEFAULT_PREVIEW_WAITER_CAPACITY,
        }
    }
}

pub struct PreviewService {
    inner: Arc<ServiceInner>,
}

impl Clone for PreviewService {
    fn clone(&self) -> Self {
        self.inner
            .public_handles
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl PreviewService {
    pub fn new(
        library: Arc<Library>,
        cache_directory: impl AsRef<Path>,
    ) -> Result<Self, PreviewServiceError> {
        Self::with_options(library, cache_directory, PreviewServiceOptions::default())
    }

    pub fn from_cache(
        library: Arc<Library>,
        cache: CacheDirectory,
    ) -> Result<Self, PreviewServiceError> {
        Self::with_open_cache(library, cache, PreviewServiceOptions::default())
    }

    pub fn with_options(
        library: Arc<Library>,
        cache_directory: impl AsRef<Path>,
        options: PreviewServiceOptions,
    ) -> Result<Self, PreviewServiceError> {
        let cache = CacheDirectory::open(cache_directory, library.canonical_root())?;
        Self::with_open_cache(library, cache, options)
    }

    pub fn with_open_cache(
        library: Arc<Library>,
        cache: CacheDirectory,
        options: PreviewServiceOptions,
    ) -> Result<Self, PreviewServiceError> {
        if cache.original_root() != library.canonical_root() {
            return Err(PreviewServiceError::Cache(
                CacheError::InvalidCacheDirectory,
            ));
        }
        if options.workers == 0 || options.queue_capacity == 0 || options.waiter_capacity == 0 {
            return Err(PreviewServiceError::Saturated);
        }
        // Library Capture inspection, Preview source inspection, and derivative
        // processing all share this Library-owned capacity-two native budget.
        let scheduler = DerivativeScheduler::with_native_work_budget(
            cache,
            crate::DerivativeSchedulerOptions {
                workers: 1,
                ..Default::default()
            },
            library.native_work_budget(),
        )?;
        let inner = Arc::new(ServiceInner {
            library,
            scheduler,
            state: Mutex::new(ServiceState {
                closed: false,
                terminate_workers: false,
                active: 0,
                waiters: 0,
                next_order: 0,
                queue: Vec::new(),
                in_flight: HashMap::new(),
            }),
            signal: Condvar::new(),
            workers: Mutex::new(Vec::new()),
            worker_count: options.workers,
            queue_capacity: options.queue_capacity,
            waiter_capacity: options.waiter_capacity,
            cache_lookup: Arc::new(tokio::sync::Semaphore::new(CACHE_LOOKUP_CAPACITY)),
            cache_hydration: Arc::new(tokio::sync::Semaphore::new(CACHE_HYDRATION_CAPACITY)),
            shutdown_lock: Mutex::new(()),
            public_handles: std::sync::atomic::AtomicUsize::new(1),
        });
        let mut workers = inner.workers.lock().expect("Preview worker list poisoned");
        for index in 0..inner.worker_count {
            let worker = Arc::downgrade(&inner);
            workers.push(
                thread::Builder::new()
                    .name(format!("slipstream-preview-{index}"))
                    .spawn(move || worker_loop(worker))
                    .map_err(|_| PreviewServiceError::Cache(CacheError::Io))?,
            );
        }
        drop(workers);
        Ok(Self { inner })
    }

    pub fn scheduler(&self) -> &DerivativeScheduler {
        &self.inner.scheduler
    }

    /// Enqueue a demand-driven, path-free Preview request.
    pub async fn request(
        &self,
        photo_id: impl Into<String>,
        target: DerivativeTarget,
        priority: DerivativePriority,
    ) -> Result<PreviewRequestResult, PreviewServiceError> {
        self.request_with_mode(photo_id, target, priority, false, None)
            .await
    }

    /// Resolves a request against one published Photo fact bundle before
    /// admitting the normal source-inspection job. A valid current cache hit
    /// never creates an Original capability or enters native work. Cache misses
    /// retain the existing bounded request path.
    pub async fn request_with_facts(
        &self,
        facts: PreviewFacts,
        target: DerivativeTarget,
        priority: DerivativePriority,
    ) -> Result<PreviewRequestResult, PreviewServiceError> {
        self.ensure_open()?;
        if facts.durable_unavailable_current() {
            return Ok(PreviewRequestResult::Unavailable(PreviewUnavailable {
                reason: if facts.photo.available {
                    PreviewUnavailableReason::NoUsableSource
                } else {
                    PreviewUnavailableReason::OriginalUnavailable
                },
            }));
        }
        if let Some(ready) = self.lookup_current(&facts, target).await? {
            return Ok(PreviewRequestResult::Current(ready_result(&ready, target)));
        }
        self.request_with_mode(facts.photo.id.clone(), target, priority, false, Some(facts))
            .await
    }

    /// Looks up one current derivative under bounded cache-read admission.
    /// This is also used while hydrating bounded Browse Windows.
    pub async fn lookup_current(
        &self,
        facts: &PreviewFacts,
        target: DerivativeTarget,
    ) -> Result<Option<CachedDerivative>, PreviewServiceError> {
        self.ensure_open()?;
        let permit = self
            .inner
            .cache_lookup
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| PreviewServiceError::Closed)?;
        self.ensure_open()?;
        let cache = self.inner.scheduler.cache().clone();
        let lookup_facts = facts.clone();
        let result = tokio::task::spawn_blocking(move || {
            cache.lookup_current(&lookup_facts.photo, &lookup_facts.originals, target)
        })
        .await
        .map_err(|_| PreviewServiceError::Closed)??;
        drop(permit);
        self.ensure_open()?;
        Ok(result)
    }

    /// Returns a bounded, metadata-only current cache key for Browse Window
    /// URL hydration. The eventual Preview/derivative request validates bytes.
    /// Hydration has one lower-priority lane so current Preview lookups retain
    /// one of the shared capacity-two cache admissions.
    pub async fn lookup_current_key(
        &self,
        facts: &PreviewFacts,
        target: DerivativeTarget,
    ) -> Result<Option<String>, PreviewServiceError> {
        let _hydration_permit = self
            .inner
            .cache_hydration
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| PreviewServiceError::Closed)?;
        self.lookup_current_key_admitted(facts, target).await
    }

    async fn lookup_current_key_admitted(
        &self,
        facts: &PreviewFacts,
        target: DerivativeTarget,
    ) -> Result<Option<String>, PreviewServiceError> {
        self.ensure_open()?;
        let permit = self
            .inner
            .cache_lookup
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| PreviewServiceError::Closed)?;
        self.ensure_open()?;
        let cache = self.inner.scheduler.cache().clone();
        let lookup_facts = facts.clone();
        let result = tokio::task::spawn_blocking(move || {
            cache.lookup_current_key(&lookup_facts.photo, &lookup_facts.originals, target)
        })
        .await
        .map_err(|_| PreviewServiceError::Closed)??;
        drop(permit);
        self.ensure_open()?;
        Ok(result)
    }

    /// Re-inspects a source even when the durable state says it is currently
    /// unavailable. This is the explicit operator retry path for a source that
    /// may have been repaired without a Library rescan.
    pub async fn retry(
        &self,
        photo_id: impl Into<String>,
        target: DerivativeTarget,
        priority: DerivativePriority,
    ) -> Result<PreviewRequestResult, PreviewServiceError> {
        self.request_with_mode(photo_id, target, priority, true, None)
            .await
    }

    fn ensure_open(&self) -> Result<(), PreviewServiceError> {
        let state = self.inner.state.lock().expect("Preview service poisoned");
        if state.closed {
            Err(PreviewServiceError::Closed)
        } else {
            Ok(())
        }
    }

    async fn request_with_mode(
        &self,
        photo_id: impl Into<String>,
        target: DerivativeTarget,
        priority: DerivativePriority,
        retry: bool,
        facts: Option<PreviewFacts>,
    ) -> Result<PreviewRequestResult, PreviewServiceError> {
        let photo_id = photo_id.into();
        let key = request_key(&photo_id, target, retry, facts.as_ref());
        let cell = {
            let mut state = self.inner.state.lock().expect("Preview service poisoned");
            if state.closed {
                return Err(PreviewServiceError::Closed);
            }
            if state.waiters >= self.inner.waiter_capacity {
                return Err(PreviewServiceError::Saturated);
            }
            let cell = if let Some(existing) = state.in_flight.get(&key) {
                let mut current_priority = existing.priority.lock().expect("Preview job poisoned");
                if priority < *current_priority {
                    *current_priority = priority;
                }
                existing.cell.clone()
            } else {
                if state.queue.len() >= self.inner.queue_capacity {
                    return Err(PreviewServiceError::Saturated);
                }
                state.next_order = state
                    .next_order
                    .checked_add(1)
                    .ok_or(PreviewServiceError::Saturated)?;
                let cell = WaitCell::new();
                let job = Arc::new(PreviewJob {
                    photo_id,
                    target,
                    priority: Mutex::new(priority),
                    order: state.next_order,
                    key: key.clone(),
                    retry,
                    facts,
                    cell: cell.clone(),
                });
                state.in_flight.insert(key, job.clone());
                state.queue.push(job);
                self.inner.signal.notify_one();
                cell
            };
            state.waiters += 1;
            cell
        };
        let _admission = WaiterAdmission {
            inner: Arc::clone(&self.inner),
        };
        tokio::task::spawn_blocking(move || cell.wait())
            .await
            .map_err(|_| PreviewServiceError::Closed)?
    }

    pub async fn thumbnail(
        &self,
        photo_id: impl Into<String>,
        priority: DerivativePriority,
    ) -> Result<PreviewRequestResult, PreviewServiceError> {
        self.request(photo_id, DerivativeTarget::Thumbnail512, priority)
            .await
    }

    pub async fn review(
        &self,
        photo_id: impl Into<String>,
        priority: DerivativePriority,
    ) -> Result<PreviewRequestResult, PreviewServiceError> {
        self.request(photo_id, DerivativeTarget::Review2560, priority)
            .await
    }

    pub async fn retry_thumbnail(
        &self,
        photo_id: impl Into<String>,
        priority: DerivativePriority,
    ) -> Result<PreviewRequestResult, PreviewServiceError> {
        self.retry(photo_id, DerivativeTarget::Thumbnail512, priority)
            .await
    }

    pub async fn retry_review(
        &self,
        photo_id: impl Into<String>,
        priority: DerivativePriority,
    ) -> Result<PreviewRequestResult, PreviewServiceError> {
        self.retry(photo_id, DerivativeTarget::Review2560, priority)
            .await
    }

    pub fn shutdown(&self) -> Result<(), PreviewServiceError> {
        let _shutdown = self.inner.shutdown_lock.lock().expect("shutdown poisoned");
        let workers = {
            let mut state = self.inner.state.lock().expect("Preview service poisoned");
            if !state.closed {
                state.closed = true;
                let queued = std::mem::take(&mut state.queue);
                for job in queued {
                    state.in_flight.remove(&job.key);
                    job.cell.complete(Err(PreviewServiceError::Closed));
                }
                self.inner.signal.notify_all();
            }
            while state.active != 0 {
                state = self
                    .inner
                    .signal
                    .wait(state)
                    .expect("Preview service poisoned");
            }
            if state.terminate_workers {
                Vec::new()
            } else {
                state.terminate_workers = true;
                self.inner.signal.notify_all();
                self.inner
                    .workers
                    .lock()
                    .expect("Preview worker list poisoned")
                    .drain(..)
                    .collect::<Vec<_>>()
            }
        };
        for worker in workers {
            worker
                .join()
                .map_err(|_| PreviewServiceError::Cache(CacheError::Io))?;
        }
        self.inner.scheduler.shutdown()?;
        Ok(())
    }
}

impl Drop for PreviewService {
    fn drop(&mut self) {
        if self
            .inner
            .public_handles
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel)
            == 1
        {
            // Worker threads hold only a weak handle. The explicit public-handle
            // count avoids treating a worker's temporary Arc as a live clone.
            let _ = self.shutdown();
        }
    }
}

fn worker_loop(inner: Weak<ServiceInner>) {
    loop {
        let Some(inner) = inner.upgrade() else { return };
        let job = {
            let mut state = inner.state.lock().expect("Preview service poisoned");
            loop {
                if state.terminate_workers {
                    return;
                }
                if !state.queue.is_empty() {
                    let selected = state
                        .queue
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, job)| {
                            (
                                job.priority.lock().expect("Preview job poisoned").rank(),
                                job.order,
                            )
                        })
                        .map(|(index, _)| index)
                        .expect("non-empty Preview queue");
                    state.active += 1;
                    break state.queue.swap_remove(selected);
                }
                if state.closed {
                    state.terminate_workers = true;
                    inner.signal.notify_all();
                    return;
                }
                state = inner.signal.wait(state).expect("Preview service poisoned");
            }
        };
        let result = process_job(&inner, &job);
        let mut state = inner.state.lock().expect("Preview service poisoned");
        state.active = state.active.saturating_sub(1);
        state.in_flight.remove(&job.key);
        job.cell.complete(result);
        if state.closed && state.active == 0 && state.queue.is_empty() {
            inner.signal.notify_all();
        } else {
            inner.signal.notify_one();
        }
    }
}

fn process_invalid_source(
    inner: &ServiceInner,
    job: &PreviewJob,
    context: &PreviewContext,
    invalid: InvalidSource,
) -> Result<PreviewRequestResult, PreviewServiceError> {
    let identity = DerivativeIdentity {
        photo_identity: context.photo.id.clone(),
        source: match invalid.source {
            PreviewSource::JpegOriginal => crate::cache::DerivativeSource::MatchingJpeg,
            PreviewSource::RawEmbeddedJpeg => crate::cache::DerivativeSource::EmbeddedRawJpeg,
        },
        source_relative_path: invalid.actual.relative_path.as_str().to_owned(),
        source_size: invalid.actual.facts.size,
        source_mtime_ms: invalid.actual.facts.mtime_ms,
        embedded_candidate_identity: None,
        target: job.target,
    };
    let generated = inner
        .scheduler
        .generate(identity, invalid.jpeg, job.priority())?;
    match generated {
        DerivativeResult::Ready(ready) if ready.stale => Ok(PreviewRequestResult::Stale(
            ready_result(&ready, job.target),
        )),
        DerivativeResult::Ready(_) => Ok(PreviewRequestResult::Failed(PreviewFailure {
            kind: PreviewFailureKind::Malformed,
        })),
        DerivativeResult::Failed(_failure) => {
            // Thumbnail targets stay derivative cache/manifest state only; they
            // never publish persisted Review Preview facts.
            if job.target != DerivativeTarget::Review2560 {
                return Ok(PreviewRequestResult::Unavailable(PreviewUnavailable {
                    reason: PreviewUnavailableReason::NoUsableSource,
                }));
            }
            let publication = inner.state.lock().expect("Preview service poisoned");
            if publication.closed {
                return Err(PreviewServiceError::Closed);
            }
            drop(publication);
            let seed = inner.library.seed_preview_blocking(PreviewSeed {
                photo_id: context.photo.id.clone(),
                state: PreviewState::Unavailable,
                source: invalid.source,
                expected_source_revision: invalid.revision,
                width: None,
                height: None,
                cache_revision: None,
            })?;
            Ok(match seed {
                PreviewSeedResult::Applied => {
                    PreviewRequestResult::Unavailable(PreviewUnavailable {
                        reason: PreviewUnavailableReason::NoUsableSource,
                    })
                }
                PreviewSeedResult::StaleIgnored => PreviewRequestResult::StaleIgnored,
            })
        }
    }
}

fn process_job(
    inner: &ServiceInner,
    job: &PreviewJob,
) -> Result<PreviewRequestResult, PreviewServiceError> {
    let context = if let Some(facts) = job.facts.clone() {
        PreviewContext::compose_facts(&inner.library, facts)?
    } else {
        let snapshot = inner.library.snapshot_blocking()?;
        PreviewContext::compose(&inner.library, snapshot, &job.photo_id)?
    };
    let Some(context) = context else {
        return Ok(PreviewRequestResult::Unavailable(PreviewUnavailable {
            reason: PreviewUnavailableReason::PhotoNotFound,
        }));
    };
    if !job.retry && context.durable_unavailable_current()? {
        return Ok(PreviewRequestResult::Unavailable(PreviewUnavailable {
            reason: if context.photo.available {
                PreviewUnavailableReason::NoUsableSource
            } else {
                PreviewUnavailableReason::OriginalUnavailable
            },
        }));
    }
    let inspection = {
        let _permit = inner.scheduler.acquire_native_work();
        context.inspect()?
    };
    let inspected = match inspection {
        Some(Inspection::Source(inspected)) => inspected,
        Some(Inspection::InvalidSource(invalid)) => {
            return process_invalid_source(inner, job, &context, invalid);
        }
        Some(Inspection::Unavailable(unavailable)) => {
            // Thumbnail targets stay derivative cache/manifest state only; they
            // never publish persisted Review Preview facts.
            if job.target != DerivativeTarget::Review2560 {
                return Ok(PreviewRequestResult::Unavailable(PreviewUnavailable {
                    reason: PreviewUnavailableReason::NoUsableSource,
                }));
            }
            let publication = inner.state.lock().expect("Preview service poisoned");
            if publication.closed {
                return Err(PreviewServiceError::Closed);
            }
            let seed = inner.library.seed_preview_blocking(PreviewSeed {
                photo_id: context.photo.id,
                state: PreviewState::Unavailable,
                source: unavailable.source,
                expected_source_revision: unavailable.revision,
                width: None,
                height: None,
                cache_revision: None,
            })?;
            return Ok(match seed {
                PreviewSeedResult::Applied => {
                    PreviewRequestResult::Unavailable(PreviewUnavailable {
                        reason: PreviewUnavailableReason::NoUsableSource,
                    })
                }
                PreviewSeedResult::StaleIgnored => PreviewRequestResult::StaleIgnored,
            });
        }
        None => {
            return Ok(PreviewRequestResult::Unavailable(PreviewUnavailable {
                reason: if context.photo.available {
                    PreviewUnavailableReason::NoUsableSource
                } else {
                    PreviewUnavailableReason::OriginalUnavailable
                },
            }));
        }
    };
    let identity = DerivativeIdentity {
        photo_identity: context.photo.id.clone(),
        source: match inspected.source {
            PreviewSource::JpegOriginal => crate::cache::DerivativeSource::MatchingJpeg,
            PreviewSource::RawEmbeddedJpeg => crate::cache::DerivativeSource::EmbeddedRawJpeg,
        },
        source_relative_path: inspected.actual.relative_path.as_str().to_owned(),
        source_size: inspected.actual.facts.size,
        source_mtime_ms: inspected.actual.facts.mtime_ms,
        embedded_candidate_identity: inspected
            .preview
            .candidate_index
            .map(|index| index.to_string()),
        target: job.target,
    };
    let generated = if job.retry {
        inner.scheduler.retry_with_orientation(
            identity,
            inspected.preview.jpeg,
            inspected.preview.container_orientation,
            job.priority(),
        )?
    } else {
        inner.scheduler.generate_with_orientation(
            identity,
            inspected.preview.jpeg,
            inspected.preview.container_orientation,
            job.priority(),
        )?
    };
    match generated {
        DerivativeResult::Ready(ready) if ready.stale => Ok(PreviewRequestResult::Stale(
            ready_result(&ready, job.target),
        )),
        DerivativeResult::Ready(ready) => {
            context.verify_source(inspected.source)?;
            let ready = ready_result(&ready, job.target);
            // Thumbnail targets stay derivative cache/manifest state only; they
            // never publish persisted Review Preview facts.
            if job.target != DerivativeTarget::Review2560 {
                return Ok(PreviewRequestResult::Current(ready));
            }
            // This lock is the publication linearization point.  Shutdown marks the
            // service closed before waiting for active jobs, so a closed service never
            // seeds a successful completion after this check.
            let publication = inner.state.lock().expect("Preview service poisoned");
            if publication.closed {
                return Err(PreviewServiceError::Closed);
            }
            match inner.library.seed_preview_blocking(PreviewSeed {
                photo_id: context.photo.id,
                state: PreviewState::Ready,
                source: inspected.source,
                expected_source_revision: inspected.revision,
                width: Some(ready.width),
                height: Some(ready.height),
                cache_revision: Some(ready.cache_key.clone()),
            })? {
                PreviewSeedResult::Applied => Ok(PreviewRequestResult::Current(ready)),
                PreviewSeedResult::StaleIgnored => Ok(PreviewRequestResult::StaleIgnored),
            }
        }
        DerivativeResult::Failed(failure) => {
            let result = PreviewRequestResult::Failed(PreviewFailure {
                kind: map_failure_kind(failure.kind),
            });
            // Thumbnail targets stay derivative cache/manifest state only; they
            // never publish persisted Review Preview facts.
            if job.target == DerivativeTarget::Review2560 {
                let publication = inner.state.lock().expect("Preview service poisoned");
                if !publication.closed {
                    let _ = inner.library.seed_preview_blocking(PreviewSeed {
                        photo_id: context.photo.id,
                        state: PreviewState::Failed,
                        source: inspected.source,
                        expected_source_revision: inspected.revision,
                        width: None,
                        height: None,
                        cache_revision: None,
                    });
                }
            }
            Ok(result)
        }
    }
}

impl PreviewJob {
    fn priority(&self) -> DerivativePriority {
        *self.priority.lock().expect("Preview job poisoned")
    }
}

fn ready_result(ready: &CachedDerivative, target: DerivativeTarget) -> PreviewReady {
    PreviewReady {
        cache_key: ready.cache_key.clone(),
        target,
        source: match ready.source {
            crate::cache::DerivativeSource::MatchingJpeg => PreviewSource::JpegOriginal,
            crate::cache::DerivativeSource::EmbeddedRawJpeg => PreviewSource::RawEmbeddedJpeg,
        },
        source_size: ready.source_size,
        source_mtime_ms: ready.source_mtime_ms,
        embedded_candidate_identity: ready.embedded_candidate_identity.clone(),
        width: ready.width,
        height: ready.height,
        generated: ready.generated,
    }
}

fn map_failure_kind(kind: DerivativeFailureKind) -> PreviewFailureKind {
    match kind {
        DerivativeFailureKind::Unsupported => PreviewFailureKind::Unsupported,
        DerivativeFailureKind::Malformed => PreviewFailureKind::Malformed,
        DerivativeFailureKind::ResourceLimit => PreviewFailureKind::ResourceLimit,
        DerivativeFailureKind::OutputLimit => PreviewFailureKind::OutputLimit,
        DerivativeFailureKind::Io => PreviewFailureKind::Io,
        DerivativeFailureKind::Internal => PreviewFailureKind::Internal,
    }
}

fn map_confinement_error(error: crate::confinement::ConfinementError) -> PreviewServiceError {
    match error {
        crate::confinement::ConfinementError::Changed => PreviewServiceError::Changed,
        other => PreviewServiceError::Library(LibraryError::Confinement(other)),
    }
}

fn map_preview_error(error: crate::PreviewError) -> PreviewServiceError {
    match error {
        crate::PreviewError::Confinement(error) => map_confinement_error(error),
        crate::PreviewError::Native(crate::NativePreviewError::Io) => {
            PreviewServiceError::Cache(CacheError::Io)
        }
        crate::PreviewError::Native(crate::NativePreviewError::ResourceLimit) => {
            PreviewServiceError::Cache(CacheError::InvalidCachedDerivative)
        }
        crate::PreviewError::Native(crate::NativePreviewError::Internal) => {
            PreviewServiceError::Cache(CacheError::Io)
        }
        crate::PreviewError::Native(
            crate::NativePreviewError::Malformed
            | crate::NativePreviewError::Unsupported
            | crate::NativePreviewError::NoUsablePreview,
        ) => PreviewServiceError::Cache(CacheError::InvalidCachedDerivative),
    }
}

struct PreviewContext {
    photo: PhotoRecord,
    original: Option<(OriginalRecord, OriginalCapability)>,
}

struct InspectedSource {
    source: PreviewSource,
    revision: String,
    actual: OriginalRecord,
    preview: crate::NativePreview,
}

struct UnavailableSource {
    source: PreviewSource,
    revision: String,
}

struct InvalidSource {
    source: PreviewSource,
    revision: String,
    actual: OriginalRecord,
    jpeg: Vec<u8>,
}

enum Inspection {
    Source(InspectedSource),
    InvalidSource(InvalidSource),
    Unavailable(UnavailableSource),
}

impl PreviewContext {
    fn compose(
        library: &Library,
        snapshot: ScanSnapshot,
        photo_id: &str,
    ) -> Result<Option<Self>, PreviewServiceError> {
        let Some(photo) = snapshot
            .photos
            .iter()
            .find(|photo| photo.id == photo_id)
            .cloned()
        else {
            return Ok(None);
        };
        Self::compose_records(library, photo, snapshot.originals)
    }

    fn compose_facts(
        library: &Library,
        facts: PreviewFacts,
    ) -> Result<Option<Self>, PreviewServiceError> {
        Self::compose_records(library, facts.photo, facts.originals)
    }

    fn compose_records(
        library: &Library,
        photo: PhotoRecord,
        originals: Vec<OriginalRecord>,
    ) -> Result<Option<Self>, PreviewServiceError> {
        let original = originals
            .iter()
            .find(|item| item.id == photo.original_id)
            .cloned();
        let original = match original {
            Some(original) => {
                if !original.available || original.error_category.is_some() {
                    None
                } else {
                    Some((
                        original.clone(),
                        library.original(original.relative_path.clone())?,
                    ))
                }
            }
            None => None,
        };
        Ok(Some(Self { original, photo }))
    }

    fn inspect(&self) -> Result<Option<Inspection>, PreviewServiceError> {
        let Some((original, capability)) = &self.original else {
            return Ok(None);
        };
        match original.kind {
            crate::OriginalKind::Jpeg => match inspect_matching_jpeg(capability) {
                Ok(preview) => {
                    self.verify_current(original, capability)?;
                    let revision = revision_of(original)?;
                    Ok(Some(Inspection::Source(InspectedSource {
                        source: PreviewSource::JpegOriginal,
                        revision,
                        actual: original.clone(),
                        preview,
                    })))
                }
                Err(crate::PreviewError::Native(
                    crate::NativePreviewError::Malformed
                    | crate::NativePreviewError::Unsupported
                    | crate::NativePreviewError::NoUsablePreview,
                )) => {
                    let revision = revision_of(original)?;
                    let checked = capability
                        .read_whole(128 * 1024 * 1024)
                        .map_err(map_confinement_error)?;
                    if checked.facts.size != original.facts.size
                        || checked.facts.mtime_ms != original.facts.mtime_ms
                    {
                        return Err(PreviewServiceError::Changed);
                    }
                    Ok(Some(Inspection::InvalidSource(InvalidSource {
                        source: PreviewSource::JpegOriginal,
                        revision,
                        actual: original.clone(),
                        jpeg: checked.bytes,
                    })))
                }
                Err(error) => Err(map_preview_error(error)),
            },
            crate::OriginalKind::Raw => {
                let preview = match extract_embedded_jpeg(capability) {
                    Ok(preview) => preview,
                    Err(crate::PreviewError::Native(
                        crate::NativePreviewError::Malformed
                        | crate::NativePreviewError::Unsupported
                        | crate::NativePreviewError::NoUsablePreview,
                    )) => {
                        self.verify_current(original, capability)?;
                        return Ok(Some(Inspection::Unavailable(UnavailableSource {
                            source: PreviewSource::RawEmbeddedJpeg,
                            revision: revision_of(original)?,
                        })));
                    }
                    Err(error) => return Err(map_preview_error(error)),
                };
                self.verify_current(original, capability)?;
                let revision = revision_of(original)?;
                Ok(Some(Inspection::Source(InspectedSource {
                    source: PreviewSource::RawEmbeddedJpeg,
                    revision,
                    actual: original.clone(),
                    preview,
                })))
            }
        }
    }

    fn durable_unavailable_current(&self) -> Result<bool, PreviewServiceError> {
        if self.photo.preview_state != PreviewState::Unavailable {
            return Ok(false);
        }
        let Some((_source, original, capability)) = self.current_source() else {
            return Ok(true);
        };
        let Some(stored_revision) = self.photo.preview_source_revision.as_deref() else {
            return Ok(false);
        };
        let current = capability.facts().map_err(map_confinement_error)?;
        if current.size != original.facts.size || current.mtime_ms != original.facts.mtime_ms {
            return Ok(false);
        }
        Ok(source_revision(
            original.relative_path.as_str(),
            current.size,
            current.mtime_ms,
        )
        .map_err(|_| PreviewServiceError::Changed)?
            == stored_revision)
    }

    fn current_source(&self) -> Option<(PreviewSource, &OriginalRecord, &OriginalCapability)> {
        self.original
            .as_ref()
            .map(|(original, capability)| (original.kind.preview_source(), original, capability))
    }

    fn verify_current(
        &self,
        original: &OriginalRecord,
        capability: &OriginalCapability,
    ) -> Result<(), PreviewServiceError> {
        let facts = capability.facts().map_err(map_confinement_error)?;
        if facts.size != original.facts.size || facts.mtime_ms != original.facts.mtime_ms {
            return Err(PreviewServiceError::Changed);
        }
        Ok(())
    }

    fn verify_source(&self, source: PreviewSource) -> Result<(), PreviewServiceError> {
        match self.original.as_ref() {
            Some((original, capability)) if original.kind.preview_source() == source => {
                self.verify_current(original, capability)
            }
            _ => Err(PreviewServiceError::Changed),
        }
    }
}

fn revision_of(original: &OriginalRecord) -> Result<String, PreviewServiceError> {
    source_revision(
        original.relative_path.as_str(),
        original.facts.size,
        original.facts.mtime_ms,
    )
    .map_err(|_| PreviewServiceError::Changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        LibraryConfig, ScanLimits,
        test_support::{generated_dng, original_snapshot, raw_sample},
    };
    use image::{ExtendedColorType, codecs::jpeg::JpegEncoder};
    use std::{
        fs,
        num::NonZeroUsize,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        base: std::path::PathBuf,
        library: Arc<Library>,
        service: PreviewService,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = self.service.shutdown();
            let _ = self.library.shutdown();
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        JpegEncoder::new_with_quality(&mut bytes, 90)
            .encode(
                &vec![127_u8; width as usize * height as usize * 3],
                width,
                height,
                ExtendedColorType::Rgb8,
            )
            .unwrap();
        bytes
    }

    fn marker_complete_corrupt_jpeg(width: u16, height: u16) -> Vec<u8> {
        let mut bytes = vec![0xff, 0xd8, 0xff, 0xc0, 0x00, 0x11, 0x08];
        bytes.extend_from_slice(&height.to_be_bytes());
        bytes.extend_from_slice(&width.to_be_bytes());
        bytes.extend_from_slice(&[0; 11]);
        bytes.extend_from_slice(&[0xff, 0xda, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00]);
        bytes.extend_from_slice(&[0xff, 0xd9]);
        bytes
    }

    fn fixture(jpeg_original: Option<&[u8]>) -> Fixture {
        fixture_with_original(jpeg_original.map(|bytes| ("one.JPG", bytes)))
    }

    fn fixture_with_original(original: Option<(&str, &[u8])>) -> Fixture {
        let originals: Vec<(&str, &[u8])> = original.into_iter().collect();
        fixture_with_originals(&originals, DEFAULT_PREVIEW_WORKERS)
    }

    fn fixture_with_originals(originals: &[(&str, &[u8])], workers: usize) -> Fixture {
        fixture_with_options(
            originals,
            PreviewServiceOptions {
                workers,
                ..PreviewServiceOptions::default()
            },
        )
    }

    fn fixture_with_options(
        originals: &[(&str, &[u8])],
        options: PreviewServiceOptions,
    ) -> Fixture {
        let base = std::env::temp_dir().join(format!(
            "slipstream-preview-service-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(base.join("originals")).unwrap();
        for (name, bytes) in originals {
            fs::write(base.join("originals").join(name), bytes).unwrap();
        }
        let config = LibraryConfig {
            library_root: base.join("originals"),
            state_directory: base.join("state"),
            database_basename: "library.sqlite".to_owned(),
            limits: ScanLimits::default(),
            command_capacity: NonZeroUsize::new(64).unwrap(),
        };
        let library = Arc::new(Library::open(config).unwrap());
        let service =
            PreviewService::with_options(library.clone(), base.join("cache"), options).unwrap();
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(library.scan())
            .unwrap();
        Fixture {
            base,
            library,
            service,
        }
    }

    /// The Photo identities for the given Original Locations, in the supplied
    /// order, so a test names its demand deterministically.
    fn photo_ids_by_location(library: &Library, locations: &[&str]) -> Vec<String> {
        let snapshot = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(library.snapshot())
            .unwrap();
        locations
            .iter()
            .map(|location| {
                let original = snapshot
                    .originals
                    .iter()
                    .find(|original| original.relative_path.as_str() == *location)
                    .expect("Original is discovered");
                snapshot
                    .photos
                    .iter()
                    .find(|photo| photo.original_id == original.id)
                    .expect("Original has one Photo")
                    .id
                    .clone()
            })
            .collect()
    }

    fn photo_id(library: &Library) -> String {
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(library.snapshot())
            .unwrap()
            .photos[0]
            .id
            .clone()
    }

    /// CLI Preview demand is admitted at the Background lane, so it never runs
    /// ahead of Web Photo View demand even when the CLI request arrives first.
    /// The active Web Photo View job occupies the single worker and both shared
    /// native-work slots, so the CLI request is admitted first and still runs
    /// last.
    #[test]
    fn cli_preview_demand_ranks_below_active_web_photo_view_demand() {
        let fixture = fixture_with_originals(
            &[
                ("active-a.JPG", &jpeg(80, 40)),
                ("cli-b.JPG", &jpeg(80, 40)),
                ("current-c.JPG", &jpeg(80, 40)),
            ],
            // One worker keeps this test about admission order only: that worker
            // is inside generation, so queue selection is the only variable.
            1,
        );
        let ids = photo_ids_by_location(
            &fixture.library,
            &["active-a.JPG", "cli-b.JPG", "current-c.JPG"],
        );
        let (active, cli, current) = (ids[0].clone(), ids[1].clone(), ids[2].clone());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        // Both shared native-work slots are held, so the worker blocks inside
        // generation instead of completing the active Web Photo View job.
        let permit = fixture
            .library
            .try_admit_native_work()
            .expect("first shared native-work slot");
        let second_permit = (0..200)
            .find_map(|_| {
                let permit = fixture.library.try_admit_native_work();
                if permit.is_none() {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                permit
            })
            .expect("second shared native-work slot after initial inspection");
        let completed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let request = |label: &'static str, id: String, priority: DerivativePriority| {
            let service = fixture.service.clone();
            let recorded = Arc::clone(&completed);
            runtime.spawn(async move {
                let _ = service.review(id, priority).await;
                recorded
                    .lock()
                    .expect("completion order poisoned")
                    .push(label);
            })
        };
        let active_request = request("active-a.JPG", active, DerivativePriority::Current);
        std::thread::sleep(std::time::Duration::from_millis(250));
        assert!(
            completed.lock().unwrap().is_empty(),
            "the active Web Photo View job must still wait for the shared budget"
        );
        // Both compared requests are admitted while the worker is busy, so the
        // Background lane can only be selected after the Current lane.
        let cli_request = request("cli-b.JPG", cli, DerivativePriority::Background);
        let current_request = request("current-c.JPG", current, DerivativePriority::Current);
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(
            completed.lock().unwrap().is_empty(),
            "no request may complete while the shared budget is held"
        );
        drop((permit, second_permit));
        runtime.block_on(async {
            for handle in [active_request, cli_request, current_request] {
                handle.await.expect("Preview request task joins");
            }
        });
        let order = completed.lock().expect("completion order poisoned");
        assert_eq!(
            order.as_slice(),
            ["active-a.JPG", "current-c.JPG", "cli-b.JPG"]
        );
    }

    #[test]
    fn shared_preview_waiter_pressure_refuses_background_demand() {
        let fixture = fixture_with_options(
            &[("web.JPG", &jpeg(80, 40)), ("cli.JPG", &jpeg(80, 40))],
            PreviewServiceOptions {
                workers: 1,
                queue_capacity: 1,
                waiter_capacity: 1,
            },
        );
        let ids = photo_ids_by_location(&fixture.library, &["web.JPG", "cli.JPG"]);
        let permit = fixture.library.try_admit_native_work().unwrap();
        let second_permit = (0..200)
            .find_map(|_| {
                let permit = fixture.library.try_admit_native_work();
                if permit.is_none() {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                permit
            })
            .expect("second shared native-work slot after initial inspection");
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let service = fixture.service.clone();
        let web = ids[0].clone();
        let active =
            runtime.spawn(async move { service.review(web, DerivativePriority::Current).await });
        runtime.block_on(async {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            loop {
                if fixture.service.inner.state.lock().unwrap().waiters == 1 {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "Web Preview occupies the shared waiter budget"
                );
                tokio::task::yield_now().await;
            }
            assert!(matches!(
                fixture
                    .service
                    .review(ids[1].clone(), DerivativePriority::Background)
                    .await,
                Err(PreviewServiceError::Saturated)
            ));
        });
        drop((permit, second_permit));
        assert!(matches!(
            runtime.block_on(active).unwrap(),
            Ok(PreviewRequestResult::Current(_))
        ));
    }

    #[test]
    fn demand_driven_service_generates_both_targets_without_precomputing() {
        let fixture = fixture(Some(&jpeg(80, 40)));
        let id = photo_id(&fixture.library);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let thumbnail = runtime
            .block_on(
                fixture
                    .service
                    .thumbnail(id.clone(), DerivativePriority::Current),
            )
            .unwrap();
        assert!(matches!(
            thumbnail,
            PreviewRequestResult::Current(PreviewReady {
                target: DerivativeTarget::Thumbnail512,
                ..
            })
        ));
        let review = runtime
            .block_on(fixture.service.review(id, DerivativePriority::Current))
            .unwrap();
        assert!(matches!(
            review,
            PreviewRequestResult::Current(PreviewReady {
                target: DerivativeTarget::Review2560,
                ..
            })
        ));
    }

    #[test]
    fn raw_container_orientation_reaches_both_cached_derivative_targets() {
        let raw = generated_dng(&jpeg(120, 80), 6);
        let fixture = fixture_with_original(Some(("portrait.DNG", &raw)));
        let id = photo_id(&fixture.library);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        for target in [DerivativeTarget::Thumbnail512, DerivativeTarget::Review2560] {
            let result = runtime
                .block_on(
                    fixture
                        .service
                        .request(id.clone(), target, DerivativePriority::Current),
                )
                .unwrap();
            let PreviewRequestResult::Current(ready) = result else {
                panic!("expected generated RAW derivative")
            };
            assert_eq!(ready.source, PreviewSource::RawEmbeddedJpeg);
            assert_eq!(ready.embedded_candidate_identity.as_deref(), Some("0"));
            assert_eq!((ready.width, ready.height), (80, 120));
            assert!(ready.generated);
        }
        let snapshot = runtime.block_on(fixture.library.snapshot()).unwrap();
        let photo = snapshot.photos.iter().find(|photo| photo.id == id).unwrap();
        assert_eq!(
            (photo.preview_width, photo.preview_height),
            (Some(80), Some(120))
        );
    }

    #[test]
    fn published_cache_hits_reuse_derivatives_without_an_original() {
        let fixture = fixture(Some(&jpeg(80, 40)));
        let id = photo_id(&fixture.library);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime
            .block_on(
                fixture
                    .service
                    .review(id.clone(), DerivativePriority::Current),
            )
            .unwrap();
        runtime
            .block_on(
                fixture
                    .service
                    .thumbnail(id.clone(), DerivativePriority::Current),
            )
            .unwrap();
        let snapshot = runtime.block_on(fixture.library.snapshot()).unwrap();
        let facts = PreviewFacts::from_snapshot(&snapshot, &id).unwrap();
        let original = fixture.base.join("originals/one.JPG");
        fs::remove_file(original).unwrap();
        let mut pending_facts = facts.clone();
        pending_facts.photo.preview_state = PreviewState::InspectionPending;
        pending_facts.photo.cache_revision = None;

        let review = runtime
            .block_on(fixture.service.request_with_facts(
                pending_facts.clone(),
                DerivativeTarget::Review2560,
                DerivativePriority::Current,
            ))
            .unwrap();
        let PreviewRequestResult::Current(review) = review else {
            panic!("expected a cached current Review Preview")
        };
        assert!(!review.generated);
        assert_eq!(review.target, DerivativeTarget::Review2560);

        let thumbnail = runtime
            .block_on(fixture.service.request_with_facts(
                pending_facts,
                DerivativeTarget::Thumbnail512,
                DerivativePriority::Current,
            ))
            .unwrap();
        let PreviewRequestResult::Current(thumbnail) = thumbnail else {
            panic!("expected a cached current thumbnail")
        };
        assert!(!thumbnail.generated);
        assert_eq!(thumbnail.target, DerivativeTarget::Thumbnail512);

        fixture.service.shutdown().unwrap();
        fixture.library.shutdown().unwrap();
        let config = LibraryConfig {
            library_root: fixture.base.join("originals"),
            state_directory: fixture.base.join("state"),
            database_basename: "library.sqlite".to_owned(),
            limits: ScanLimits::default(),
            command_capacity: NonZeroUsize::new(64).unwrap(),
        };
        let reopened_library = Arc::new(Library::open(config).unwrap());
        let reopened_service =
            PreviewService::new(reopened_library.clone(), fixture.base.join("cache")).unwrap();
        let reopened_snapshot = runtime.block_on(reopened_library.snapshot()).unwrap();
        let reopened_facts = PreviewFacts::from_snapshot(&reopened_snapshot, &id).unwrap();
        let reopened = runtime
            .block_on(reopened_service.request_with_facts(
                reopened_facts,
                DerivativeTarget::Review2560,
                DerivativePriority::Current,
            ))
            .unwrap();
        let PreviewRequestResult::Current(reopened) = reopened else {
            panic!("expected the reopened cached Review Preview")
        };
        assert!(!reopened.generated);
        reopened_service.shutdown().unwrap();
        reopened_library.shutdown().unwrap();
    }

    #[test]
    fn current_cache_misses_regenerate_missing_or_corrupt_derivatives() {
        let fixture = fixture(Some(&jpeg(80, 40)));
        let id = photo_id(&fixture.library);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let first = runtime
            .block_on(
                fixture
                    .service
                    .review(id.clone(), DerivativePriority::Current),
            )
            .unwrap();
        let PreviewRequestResult::Current(first) = first else {
            panic!("expected an initial Review Preview")
        };
        let snapshot = runtime.block_on(fixture.library.snapshot()).unwrap();
        let facts = PreviewFacts::from_snapshot(&snapshot, &id).unwrap();
        let path = fixture
            .service
            .scheduler()
            .cache()
            .root()
            .join("rust-vips-v2")
            .join(format!("{}.jpg", first.cache_key));

        fs::remove_file(&path).unwrap();
        let regenerated = runtime
            .block_on(fixture.service.request_with_facts(
                facts.clone(),
                DerivativeTarget::Review2560,
                DerivativePriority::Current,
            ))
            .unwrap();
        let PreviewRequestResult::Current(regenerated) = regenerated else {
            panic!("expected missing cache regeneration")
        };
        assert!(regenerated.generated);

        fs::write(&path, marker_complete_corrupt_jpeg(80, 40)).unwrap();
        let repaired = runtime
            .block_on(fixture.service.request_with_facts(
                facts,
                DerivativeTarget::Review2560,
                DerivativePriority::Current,
            ))
            .unwrap();
        let PreviewRequestResult::Current(repaired) = repaired else {
            panic!("expected corrupt cache regeneration")
        };
        assert!(repaired.generated);
    }

    #[test]
    fn thumbnail_requests_never_overwrite_review_preview_facts() {
        let review_fixture = fixture(Some(&jpeg(80, 40)));
        let id = photo_id(&review_fixture.library);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let review = runtime
            .block_on(
                review_fixture
                    .service
                    .review(id.clone(), DerivativePriority::Current),
            )
            .unwrap();
        let PreviewRequestResult::Current(review_ready) = review else {
            panic!("expected a current Review Preview")
        };
        assert_eq!(review_ready.target, DerivativeTarget::Review2560);
        let facts = |library: &Library, id: &str| {
            let photo = runtime
                .block_on(library.snapshot())
                .unwrap()
                .photos
                .into_iter()
                .find(|photo| photo.id == id)
                .unwrap();
            (
                photo.preview_state,
                photo.preview_width,
                photo.preview_height,
                photo.cache_revision,
            )
        };
        let established = facts(&review_fixture.library, &id);
        assert_eq!(established.0, PreviewState::Ready);

        let thumbnail = runtime
            .block_on(
                review_fixture
                    .service
                    .thumbnail(id.clone(), DerivativePriority::Current),
            )
            .unwrap();
        let PreviewRequestResult::Current(thumbnail_ready) = thumbnail else {
            panic!("expected a current thumbnail")
        };
        assert_eq!(thumbnail_ready.target, DerivativeTarget::Thumbnail512);
        assert_ne!(thumbnail_ready.cache_key, review_ready.cache_key);
        assert_eq!(facts(&review_fixture.library, &id), established);

        review_fixture.service.shutdown().unwrap();
        review_fixture.library.shutdown().unwrap();
        let config = LibraryConfig {
            library_root: review_fixture.base.join("originals"),
            state_directory: review_fixture.base.join("state"),
            database_basename: "library.sqlite".to_owned(),
            limits: ScanLimits::default(),
            command_capacity: NonZeroUsize::new(64).unwrap(),
        };
        let library = Arc::new(Library::open(config).unwrap());
        let service =
            PreviewService::new(library.clone(), review_fixture.base.join("cache")).unwrap();
        assert_eq!(facts(&library, &id), established);
        let cached = runtime
            .block_on(service.thumbnail(id.clone(), DerivativePriority::Current))
            .unwrap();
        let PreviewRequestResult::Current(cached_ready) = cached else {
            panic!("expected the reopened thumbnail cache hit")
        };
        assert!(!cached_ready.generated);
        assert_eq!(cached_ready.cache_key, thumbnail_ready.cache_key);
        assert_eq!(facts(&library, &id), established);
        service.shutdown().unwrap();
        library.shutdown().unwrap();

        // A thumbnail failure must not persist unavailable facts, while the
        // Review pipeline still owns that publication for the same photo.
        let malformed_fixture = fixture(Some(b"not jpeg"));
        let malformed_id = photo_id(&malformed_fixture.library);
        let failed = runtime
            .block_on(
                malformed_fixture
                    .service
                    .thumbnail(malformed_id.clone(), DerivativePriority::Current),
            )
            .unwrap();
        assert!(matches!(
            failed,
            PreviewRequestResult::Unavailable(PreviewUnavailable {
                reason: PreviewUnavailableReason::NoUsableSource
            })
        ));
        let photo = runtime
            .block_on(malformed_fixture.library.snapshot())
            .unwrap()
            .photos
            .into_iter()
            .find(|photo| photo.id == malformed_id)
            .unwrap();
        assert_eq!(photo.preview_state, PreviewState::InspectionPending);
        assert!(photo.cache_revision.is_none());
        assert!(matches!(
            runtime.block_on(
                malformed_fixture
                    .service
                    .review(malformed_id, DerivativePriority::Current)
            ),
            Ok(PreviewRequestResult::Unavailable(PreviewUnavailable {
                reason: PreviewUnavailableReason::NoUsableSource
            }))
        ));
        let photo = runtime
            .block_on(malformed_fixture.library.snapshot())
            .unwrap()
            .photos
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(photo.preview_state, PreviewState::Unavailable);
    }

    #[test]
    fn unavailable_photo_does_not_enqueue_work() {
        let fixture = fixture(None);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let result = runtime
            .block_on(
                fixture
                    .service
                    .review("missing", DerivativePriority::Current),
            )
            .unwrap();
        assert!(matches!(
            result,
            PreviewRequestResult::Unavailable(PreviewUnavailable {
                reason: PreviewUnavailableReason::PhotoNotFound
            })
        ));
    }

    #[test]
    fn malformed_matching_jpeg_is_reported_unavailable_without_raw_fallback() {
        let fixture = fixture(Some(b"not jpeg"));
        let id = photo_id(&fixture.library);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let result = runtime
            .block_on(fixture.service.review(id, DerivativePriority::Current))
            .unwrap();
        assert!(matches!(
            result,
            PreviewRequestResult::Unavailable(PreviewUnavailable {
                reason: PreviewUnavailableReason::NoUsableSource
            })
        ));
    }

    #[test]
    fn unavailable_preview_is_seeded_and_short_circuited_after_reopen() {
        let fixture = fixture(Some(b"not jpeg"));
        let id = photo_id(&fixture.library);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let first = runtime
            .block_on(
                fixture
                    .service
                    .review(id.clone(), DerivativePriority::Current),
            )
            .unwrap();
        assert!(matches!(
            first,
            PreviewRequestResult::Unavailable(PreviewUnavailable {
                reason: PreviewUnavailableReason::NoUsableSource
            })
        ));
        let snapshot = runtime.block_on(fixture.library.snapshot()).unwrap();
        let photo = snapshot.photos.iter().find(|photo| photo.id == id).unwrap();
        assert_eq!(photo.preview_state, PreviewState::Unavailable);
        assert!(photo.preview_source_revision.is_some());

        fixture.service.shutdown().unwrap();
        fixture.library.shutdown().unwrap();
        let config = LibraryConfig {
            library_root: fixture.base.join("originals"),
            state_directory: fixture.base.join("state"),
            database_basename: "library.sqlite".to_owned(),
            limits: ScanLimits::default(),
            command_capacity: NonZeroUsize::new(64).unwrap(),
        };
        let library = Arc::new(Library::open(config).unwrap());
        let service = PreviewService::new(library.clone(), fixture.base.join("cache")).unwrap();
        let reopened = runtime
            .block_on(service.review(id, DerivativePriority::Current))
            .unwrap();
        assert!(matches!(
            reopened,
            PreviewRequestResult::Unavailable(PreviewUnavailable {
                reason: PreviewUnavailableReason::NoUsableSource
            })
        ));
        service.shutdown().unwrap();
        library.shutdown().unwrap();
    }

    #[test]
    fn explicit_retry_bypasses_durable_unavailable_without_rescan() {
        let fixture = fixture(Some(&jpeg(64, 32)));
        let id = photo_id(&fixture.library);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let snapshot = runtime.block_on(fixture.library.snapshot()).unwrap();
        let original = snapshot
            .originals
            .iter()
            .find(|original| original.kind == crate::OriginalKind::Jpeg)
            .unwrap();
        let revision = source_revision(
            original.relative_path.as_str(),
            original.facts.size,
            original.facts.mtime_ms,
        )
        .unwrap();
        assert!(matches!(
            runtime.block_on(fixture.library.seed_preview(PreviewSeed {
                photo_id: id.clone(),
                state: PreviewState::Unavailable,
                source: PreviewSource::JpegOriginal,
                expected_source_revision: revision,
                width: None,
                height: None,
                cache_revision: None,
            })),
            Ok(PreviewSeedResult::Applied)
        ));
        let retried = runtime
            .block_on(
                fixture
                    .service
                    .retry_review(id.clone(), DerivativePriority::Current),
            )
            .unwrap();
        assert!(matches!(retried, PreviewRequestResult::Current(_)));
        let snapshot = runtime.block_on(fixture.library.snapshot()).unwrap();
        assert_eq!(snapshot.photos[0].preview_state, PreviewState::Ready);
    }

    #[test]
    fn close_is_idempotent_and_rejects_new_requests() {
        let fixture = fixture(Some(&jpeg(16, 8)));
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let id = photo_id(&fixture.library);
        let snapshot = runtime.block_on(fixture.library.snapshot()).unwrap();
        let facts = PreviewFacts::from_snapshot(&snapshot, &id).unwrap();
        fixture.service.shutdown().unwrap();
        fixture.service.shutdown().unwrap();
        assert!(matches!(
            runtime.block_on(fixture.service.request_with_facts(
                facts,
                DerivativeTarget::Review2560,
                DerivativePriority::Current,
            )),
            Err(PreviewServiceError::Closed)
        ));
        assert!(matches!(
            runtime.block_on(fixture.service.review("photo", DerivativePriority::Current)),
            Err(PreviewServiceError::Closed)
        ));
    }

    #[test]
    #[ignore = "requires SLIPSTREAM_RAW_SAMPLE"]
    fn raw_photo_previews_from_embedded_jpeg_independent_of_sibling_jpeg() {
        let (sample, source_before) = raw_sample();
        let raw_name = format!(
            "one.{}",
            sample
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or("ARW")
        );
        let base = std::env::temp_dir().join(format!(
            "slipstream-preview-independent-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(base.join("originals")).unwrap();
        let copied_raw = base.join("originals").join(&raw_name);
        fs::copy(&sample, &copied_raw).unwrap();
        let copied_before = original_snapshot(&copied_raw);
        fs::write(base.join("originals/one.JPG"), b"not jpeg").unwrap();
        let config = || LibraryConfig {
            library_root: base.join("originals"),
            state_directory: base.join("state"),
            database_basename: "library.sqlite".to_owned(),
            limits: ScanLimits::default(),
            command_capacity: NonZeroUsize::new(64).unwrap(),
        };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let library = Arc::new(Library::open(config()).unwrap());
        runtime.block_on(library.scan()).unwrap();
        let service = PreviewService::new(library.clone(), base.join("cache")).unwrap();
        let snapshot = runtime.block_on(library.snapshot()).unwrap();
        // The RAW and its corrupt same-basename JPEG are independent Photos.
        let raw_photo = snapshot
            .photos
            .iter()
            .find(|photo| {
                snapshot.originals.iter().any(|original| {
                    original.id == photo.original_id && original.kind == crate::OriginalKind::Raw
                })
            })
            .unwrap();
        let jpeg_photo = snapshot
            .photos
            .iter()
            .find(|photo| {
                snapshot.originals.iter().any(|original| {
                    original.id == photo.original_id && original.kind == crate::OriginalKind::Jpeg
                })
            })
            .unwrap();
        assert_ne!(raw_photo.id, jpeg_photo.id);

        // The RAW Photo previews from its own embedded JPEG.
        let raw_ready = runtime
            .block_on(service.review(raw_photo.id.clone(), DerivativePriority::Current))
            .unwrap();
        let PreviewRequestResult::Current(raw_ready) = raw_ready else {
            panic!("expected a RAW embedded Preview")
        };
        assert_eq!(raw_ready.source, PreviewSource::RawEmbeddedJpeg);
        assert_eq!((raw_ready.width, raw_ready.height), (2560, 1707));

        // The corrupt JPEG Photo is independently unavailable.
        let jpeg_result = runtime
            .block_on(service.review(jpeg_photo.id.clone(), DerivativePriority::Current))
            .unwrap();
        assert!(matches!(
            jpeg_result,
            PreviewRequestResult::Unavailable(PreviewUnavailable {
                reason: PreviewUnavailableReason::NoUsableSource
            })
        ));

        // Replacing the JPEG bytes recovers only the JPEG Photo; the RAW
        // Photo keeps its embedded source and its Preview.
        fs::write(base.join("originals/one.JPG"), jpeg(64, 32)).unwrap();
        runtime.block_on(library.scan()).unwrap();
        let repaired = runtime
            .block_on(service.review(jpeg_photo.id.clone(), DerivativePriority::Current))
            .unwrap();
        let PreviewRequestResult::Current(repaired) = repaired else {
            panic!("expected the replaced JPEG Photo to preview")
        };
        assert_eq!(repaired.source, PreviewSource::JpegOriginal);
        let raw_after = runtime
            .block_on(service.review(raw_photo.id.clone(), DerivativePriority::Current))
            .unwrap();
        let PreviewRequestResult::Current(raw_after) = raw_after else {
            panic!("expected the RAW Photo to keep its embedded Preview")
        };
        assert_eq!(raw_after.source, PreviewSource::RawEmbeddedJpeg);
        assert!(!raw_after.generated);

        service.shutdown().unwrap();
        library.shutdown().unwrap();
        assert_eq!(original_snapshot(&copied_raw), copied_before);
        assert_eq!(original_snapshot(&sample), source_before);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    #[ignore = "requires SLIPSTREAM_RAW_SAMPLE"]
    fn sony_opt_in_service_uses_largest_embedded_candidate_without_mutating_original() {
        let (sample, source_before) = raw_sample();
        let base = std::env::temp_dir().join(format!(
            "slipstream-preview-sony-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        let originals = base.join("originals");
        fs::create_dir_all(&originals).unwrap();
        let copied_raw = originals.join(sample.file_name().unwrap());
        fs::copy(&sample, &copied_raw).unwrap();
        let copied_before = original_snapshot(&copied_raw);
        let config = LibraryConfig {
            library_root: originals,
            state_directory: base.join("state"),
            database_basename: "library.sqlite".to_owned(),
            limits: ScanLimits::default(),
            command_capacity: NonZeroUsize::new(64).unwrap(),
        };
        let library = Arc::new(Library::open(config).unwrap());
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(library.scan()).unwrap();
        let snapshot = runtime.block_on(library.snapshot()).unwrap();
        let id = snapshot
            .photos
            .iter()
            .find(|photo| {
                snapshot.originals.iter().any(|original| {
                    original.id == photo.original_id && original.kind == crate::OriginalKind::Raw
                })
            })
            .unwrap()
            .id
            .clone();
        let service = PreviewService::new(library.clone(), base.join("cache")).unwrap();
        let result = runtime
            .block_on(service.review(id, DerivativePriority::Current))
            .unwrap();
        let PreviewRequestResult::Current(ready) = result else {
            panic!("expected current Sony Preview")
        };
        assert_eq!(ready.source, PreviewSource::RawEmbeddedJpeg);
        assert_eq!(ready.embedded_candidate_identity.as_deref(), Some("2"));
        assert_eq!((ready.width, ready.height), (2560, 1707));
        service.shutdown().unwrap();
        library.shutdown().unwrap();
        assert_eq!(original_snapshot(&copied_raw), copied_before);
        assert_eq!(original_snapshot(&sample), source_before);
        let _ = fs::remove_dir_all(base);
    }
}
