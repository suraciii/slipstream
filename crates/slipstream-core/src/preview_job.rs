//! Execution of one queued Preview job from published facts to result.
//!
//! The service owns admission, waiting, and shutdown. This module resolves and
//! inspects the source, generates the derivative, and publishes Review facts.

use super::{
    PreviewFacts, PreviewFailure, PreviewFailureKind, PreviewJob, PreviewRequestResult,
    PreviewServiceError, PreviewUnavailable, PreviewUnavailableReason, ServiceInner, ready_result,
};
use crate::{
    CacheError, DerivativeFailureKind, DerivativeIdentity, DerivativePriority, DerivativeResult,
    DerivativeTarget, Library, LibraryError, OriginalCapability, OriginalRecord, PhotoRecord,
    PreviewSeed, PreviewSeedResult, PreviewSource, PreviewState, ScanSnapshot,
    extract_embedded_jpeg, inspect_matching_jpeg, source_revision,
};

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

pub(super) fn process_job(
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
