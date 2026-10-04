use super::*;
use std::collections::{HashMap, VecDeque};
#[derive(Default)]
pub(super) struct ReviewWarmupState {
    pending: VecDeque<String>,
    queued: HashMap<String, bool>,
    running: bool,
    closed: bool,
    task: Option<JoinHandle<()>>,
}

pub(super) struct ReviewWarmupRequest {
    pub(super) photo_id: String,
    pub(super) retry: bool,
}

impl ReviewWarmupState {
    fn add(&mut self, request: ReviewWarmupRequest) {
        let retry = request.retry;
        if let Some(existing) = self.queued.get_mut(&request.photo_id) {
            // A later publication owns the current source. Its retry intent
            // must replace, rather than merge with, an older source's intent.
            *existing = retry;
        } else {
            self.pending.push_back(request.photo_id.clone());
            self.queued.insert(request.photo_id, retry);
        }
    }
}

pub(super) struct ReviewWarmup;

impl ReviewWarmup {
    pub(super) fn enqueue(application: &Arc<Application>, requests: Vec<ReviewWarmupRequest>) {
        let mut state = application
            .review_warmup
            .lock()
            .expect("review warmup poisoned");
        if state.closed {
            return;
        }
        for request in requests {
            state.add(request);
        }
        if state.running || state.pending.is_empty() {
            return;
        }
        state.running = true;
        let application = Arc::downgrade(application);
        state.task = Some(tokio::spawn(async move {
            Self::run(application).await;
        }));
    }

    async fn run(application: std::sync::Weak<Application>) {
        tokio::time::sleep(Duration::from_millis(100)).await;
        loop {
            let Some(application) = application.upgrade() else {
                return;
            };
            let closed = application
                .review_warmup
                .lock()
                .expect("review warmup poisoned")
                .closed;
            if closed {
                return;
            }
            let next = {
                let mut state = application
                    .review_warmup
                    .lock()
                    .expect("review warmup poisoned");
                let Some(photo_id) = state.pending.pop_front() else {
                    state.running = false;
                    return;
                };
                let retry = state
                    .queued
                    .remove(&photo_id)
                    .expect("queued warmup request");
                (photo_id, retry)
            };
            let _ = application
                .preview_target(
                    &next.0,
                    DerivativeTarget::Review2560,
                    slipstream_core::DerivativePriority::Background,
                    next.1,
                )
                .await;
        }
    }

    pub(super) async fn close(application: &Application) {
        let task = {
            let mut state = application
                .review_warmup
                .lock()
                .expect("review warmup poisoned");
            state.closed = true;
            state.pending.clear();
            state.queued.clear();
            state.task.take()
        };
        if let Some(task) = task {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Application {
    pub(super) fn published_preview_facts(&self, photo_id: &str) -> Option<PreviewFacts> {
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
        self.preview_target(photo_id, DerivativeTarget::Review2560, priority, false)
            .await
    }

    pub async fn thumbnail(&self, photo_id: &str) -> Result<PreviewResponse, ServerError> {
        self.preview_target(
            photo_id,
            DerivativeTarget::Thumbnail512,
            slipstream_core::DerivativePriority::VisibleGrid,
            false,
        )
        .await
    }

    async fn preview_target(
        &self,
        photo_id: &str,
        target: DerivativeTarget,
        priority: slipstream_core::DerivativePriority,
        retry: bool,
    ) -> Result<PreviewResponse, ServerError> {
        if !valid_id(photo_id) {
            return Ok(PreviewResponse::unavailable("Unknown Photo"));
        }
        let published_facts = self.published_preview_facts(photo_id);
        let result = if let Some(facts) = published_facts.clone() {
            if retry {
                self.preview.retry_with_facts(facts, target, priority).await
            } else {
                self.preview
                    .request_with_facts(facts, target, priority)
                    .await
            }
        } else if retry {
            self.preview
                .retry(photo_id.to_owned(), target, priority)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_warmup_publication_replaces_retry_intent() {
        let mut state = ReviewWarmupState::default();
        state.add(ReviewWarmupRequest {
            photo_id: "photo".to_owned(),
            retry: true,
        });
        state.add(ReviewWarmupRequest {
            photo_id: "photo".to_owned(),
            retry: false,
        });

        assert_eq!(state.pending.len(), 1);
        assert_eq!(state.queued.get("photo"), Some(&false));
    }
}
