//! Preview-class render admission and ephemeral Development TIFF retention:
//! the seams where the Edit Preview route resolves retained Development
//! Results and admits renders through the same closed production workload an
//! Export uses. One registry entry exists per Photo and stage, staging is
//! service-private and never outlives its owner, and every admission settles.

use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc, Mutex as SyncMutex,
        atomic::{AtomicU64, Ordering},
    },
    time::SystemTime,
};

use crate::{
    export_manager::{ExportManager, RetainedDevelopmentIdentity},
    preview_render::{PreviewCancellation, PreviewRender, RenderedPreview},
};

use super::owner::{ContentEvidence, PreviewFacts, PreviewIdentity, preview_facts};
use super::{
    MAXIMUM_OWNERS, PREVIEW_SWEEP_PERIOD, RENDITION_TTL, SETTINGS_BASELINE, WHITE_BALANCE_AS_SHOT,
};

/// One retained Development Result with the identity facts its publication
/// captured. This is the seam where the durable Development Result retention
/// of the Export lifecycle plugs in; `path` is a server-private artifact
/// location and never appears on the wire.
#[derive(Clone, Debug)]
pub(crate) struct RetainedDevelopmentResult {
    /// The content evidence established when the result was published.
    pub(crate) sha256: String,
    pub(crate) byte_length: u64,
    pub(crate) recipe_revision: Option<String>,
    pub(crate) exposure_milli_ev: i64,
    pub(crate) white_balance: &'static str,
    pub(crate) source_revision: String,
    pub(crate) bundle_sha256: String,
    pub(crate) path: PathBuf,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

impl RetainedDevelopmentResult {
    /// True when the result was produced under exactly the current recipe
    /// revision and content, source revision, and bundle. A result captured
    /// under any other identity is not current and must not be served.
    ///
    /// A baseline identity is defined by its settings alone: it names no saved
    /// recipe, so a retained result produced under exactly those settings is
    /// the same development whatever revision captured them.
    pub(super) fn matches_facts(&self, facts: &PreviewFacts) -> bool {
        let revision_matches =
            facts.settings == SETTINGS_BASELINE || self.recipe_revision == facts.recipe_revision;
        revision_matches
            && self.exposure_milli_ev == facts.exposure_milli_ev
            && self.white_balance == facts.white_balance
            && self.source_revision == facts.source_revision
            && self.bundle_sha256 == facts.bundle_sha256
    }

    pub(super) fn evidence(&self) -> ContentEvidence {
        ContentEvidence::DevelopmentResult {
            sha256: self.sha256.clone(),
            byte_length: self.byte_length,
        }
    }
}

/// Resolves the retained Development Result of one Photo whose captured
/// identity is exactly the current facts, or `None` when no such result is
/// retained. Resolution reads the durable Export lifecycle, so it is
/// asynchronous; a read that cannot be answered resolves as "not retained"
/// and the caller refuses fail-closed.
pub(crate) trait DevelopmentResultRetention: Send + Sync {
    fn resolve<'a>(
        &'a self,
        photo_id: &'a str,
        facts: &'a PreviewFacts,
    ) -> Pin<Box<dyn Future<Output = Option<RetainedDevelopmentResult>> + Send + 'a>>;
}

/// The durable Development Result retention of the Export lifecycle: the
/// published Development TIFF of a succeeded Export is the retained result,
/// and its disclosed artifact expiry is its retention. A Photo without a
/// matching retained result resolves to `None`; production wraps this in
/// `PreviewClassRenders`, which falls through to an ephemeral preview result
/// and admits a render when neither exists.
pub(crate) struct RetainedExportDevelopmentResults {
    exports: Arc<ExportManager>,
}

impl RetainedExportDevelopmentResults {
    pub(crate) fn new(exports: Arc<ExportManager>) -> Self {
        Self { exports }
    }
}

impl DevelopmentResultRetention for RetainedExportDevelopmentResults {
    fn resolve<'a>(
        &'a self,
        photo_id: &'a str,
        facts: &'a PreviewFacts,
    ) -> Pin<Box<dyn Future<Output = Option<RetainedDevelopmentResult>> + Send + 'a>> {
        Box::pin(async move {
            let identity = RetainedDevelopmentIdentity {
                matches_baseline: facts.settings == SETTINGS_BASELINE,
                recipe_revision: facts.recipe_revision.as_deref(),
                exposure_milli_ev: facts.exposure_milli_ev,
                source_revision: &facts.source_revision,
                bundle_sha256: &facts.bundle_sha256,
            };
            let retained = self
                .exports
                .retained_development_result(photo_id, &identity)
                .await?;
            Some(RetainedDevelopmentResult {
                sha256: retained.sha256,
                byte_length: retained.byte_length,
                recipe_revision: Some(retained.recipe_revision),
                exposure_milli_ev: retained.exposure_milli_ev,
                white_balance: WHITE_BALANCE_AS_SHOT,
                source_revision: retained.source_revision,
                path: retained.path,
                width: 0,
                height: 0,
                bundle_sha256: retained.bundle_id,
            })
        })
    }
}

/// The retention of a deployment without the Export lifecycle: nothing is
/// retained, so every render request refuses fail-closed.
pub(crate) struct UnlandedRetention;

impl DevelopmentResultRetention for UnlandedRetention {
    fn resolve<'a>(
        &'a self,
        _photo_id: &'a str,
        _facts: &'a PreviewFacts,
    ) -> Pin<Box<dyn Future<Output = Option<RetainedDevelopmentResult>> + Send + 'a>> {
        Box::pin(async { None })
    }
}

/// One preview-class render admission against the same closed production
/// workload an Export uses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RenderAdmission {
    /// A new attempt was admitted behind the serialized processing slot.
    Queued,
    /// The same identity is already running or has a live ephemeral result.
    Running,
    /// The admission may have reached the workload, but its outcome is
    /// unknowable from this side.
    #[allow(dead_code)]
    Indeterminate,
    /// The workload cannot admit preview-class work in this deployment, for
    /// one of the closed reasons.
    Unavailable(RenderUnavailable),
}

/// The closed reasons a render gate can be unavailable. The wire reason is
/// part of the refusal contract, so the set is closed here instead of an
/// open `&'static str` a gate adapter could extend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RenderUnavailable {
    /// The service-side admission of preview-class renders is not available
    /// in this deployment.
    AdmissionNotLanded,
}

impl RenderUnavailable {
    /// The wire `reason` value of the `processing_unavailable` refusal.
    pub(crate) fn reason(self) -> &'static str {
        match self {
            Self::AdmissionNotLanded => "preview-render-admission-unavailable",
        }
    }
}

/// The settlement of one admitted render. A completed render becomes a
/// bounded ephemeral retained result; failure and cancellation release the
/// identity so a later request admits a new attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RenderSettlement {
    #[allow(dead_code)]
    Completed,
    #[allow(dead_code)]
    Failed,
    Cancelled,
}

/// The admission request of one preview-class render. The production gate
/// binds the attempt to this opaque identity digest.
#[allow(dead_code)]
pub(crate) struct PreviewRenderRequest<'a> {
    pub(crate) photo_id: &'a str,
    pub(crate) stage: &'static str,
    pub(crate) settings: &'static str,
    pub(crate) identity_digest: &'a str,
}

pub(crate) trait PreviewRenderGate: Send + Sync {
    fn admit(&self, request: PreviewRenderRequest<'_>) -> RenderAdmission;
    /// Whether the gate still owns a live attempt or unexpired result for the
    /// request. The default keeps the existing scripted test seam's local
    /// coalescing behavior; production gates report their registry truth.
    fn is_live(&self, _request: PreviewRenderRequest<'_>) -> bool {
        true
    }
    /// Settles one admitted render. Every admission eventually settles:
    /// completion, failure, and cancellation all release a dead identity.
    fn settle(&self, request: PreviewRenderRequest<'_>, settlement: RenderSettlement);
}

/// The service-side admission of preview-class development renders for a
/// deployment without the Export lifecycle.
pub(crate) struct UnlandedRenderGate;

impl PreviewRenderGate for UnlandedRenderGate {
    fn admit(&self, _request: PreviewRenderRequest<'_>) -> RenderAdmission {
        RenderAdmission::Unavailable(RenderUnavailable::AdmissionNotLanded)
    }

    fn settle(&self, _request: PreviewRenderRequest<'_>, _settlement: RenderSettlement) {}
}

/// One preview-class render registry key: the Photo, the stage, and the
/// settings selector. The comparison of a stage is its own owner, so a
/// baseline render never publishes over, or is superseded by, the current
/// rendition of the same stage.
type PreviewRenderKey = (String, &'static str, &'static str);

enum PreviewRenderState {
    Running,
    Ready {
        identity: Box<PreviewFacts>,
        output_path: PathBuf,
        attempt_key: String,
        size: u64,
        sha256: String,
        facts: Box<crate::export_manager::DevelopmentTiffFacts>,
        deadline: SystemTime,
    },
    #[allow(dead_code)]
    Failed,
}

struct PreviewRenderEntry {
    identity_digest: String,
    state: PreviewRenderState,
    cancellation: PreviewCancellation,
    last_used: u64,
}

struct PreviewClassRendersInner {
    render: Arc<PreviewRender>,
    exports: Arc<ExportManager>,
    retained: RetainedExportDevelopmentResults,
    entries: SyncMutex<HashMap<PreviewRenderKey, PreviewRenderEntry>>,
    touches: AtomicU64,
}

/// Production preview-class admission and ephemeral Development TIFF
/// retention. One registry entry exists per Photo and stage, and all heavy
/// attempts run through the ExportManager's serialized launcher slot.
pub(crate) struct PreviewClassRenders {
    inner: Arc<PreviewClassRendersInner>,
}

impl PreviewClassRenders {
    pub(crate) fn new(exports: Arc<ExportManager>) -> Self {
        let inner = Arc::new(PreviewClassRendersInner {
            render: Arc::new(PreviewRender::new(
                Arc::clone(exports.library()),
                Arc::clone(&exports),
            )),
            retained: RetainedExportDevelopmentResults::new(Arc::clone(&exports)),
            exports,
            entries: SyncMutex::new(HashMap::new()),
            touches: AtomicU64::new(0),
        });
        // Ephemeral staging is deleted when its retention window elapses, not
        // only when a later request touches the entry: an idle service must
        // not hold a rendition past the retention it discloses. Starting the
        // sweep needs a Tokio runtime, like every other spawned service task.
        let swept = Arc::clone(&inner);
        tokio::spawn(async move {
            let mut period = tokio::time::interval(PREVIEW_SWEEP_PERIOD);
            period.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                period.tick().await;
                PreviewClassRenders::sweep_expired(&swept);
            }
        });
        Self { inner }
    }

    /// Deletes the private output of every rendition whose retention window
    /// has elapsed. Called on a later admission or resolve, and by the sweep
    /// that bounds how long an idle service keeps expired staging.
    fn sweep_expired(inner: &PreviewClassRendersInner) {
        let mut entries = inner
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        Self::purge_expired_locked(inner, &mut entries, SystemTime::now());
    }

    fn remove_output(inner: &PreviewClassRendersInner, entry: &PreviewRenderEntry) {
        if let PreviewRenderState::Ready { attempt_key, .. } = &entry.state {
            inner.exports.delete_preview_output(attempt_key);
        }
    }

    fn purge_expired_locked(
        inner: &PreviewClassRendersInner,
        entries: &mut HashMap<PreviewRenderKey, PreviewRenderEntry>,
        now: SystemTime,
    ) {
        let expired = entries
            .iter()
            .filter_map(|(key, entry)| {
                matches!(
                    &entry.state,
                    PreviewRenderState::Ready { deadline, .. } if *deadline <= now
                )
                .then_some(key.clone())
            })
            .collect::<Vec<_>>();
        for key in expired {
            if let Some(entry) = entries.remove(&key) {
                Self::remove_output(inner, &entry);
            }
        }
    }

    fn settle_inner(
        inner: &PreviewClassRendersInner,
        photo_id: &str,
        stage: &'static str,
        settings: &'static str,
        identity_digest: &str,
        settlement: RenderSettlement,
    ) {
        let key = (photo_id.to_owned(), stage, settings);
        let mut entries = inner
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let matches = entries
            .get(&key)
            .is_some_and(|entry| entry.identity_digest == identity_digest);
        if !matches {
            return;
        }
        if matches!(settlement, RenderSettlement::Completed) {
            // A task that has not installed its Ready payload has no output
            // to retain; release that dead running identity rather than
            // leaving a permanent `running` answer.
            if let Some(entry) = entries.get(&key)
                && matches!(&entry.state, PreviewRenderState::Running)
                && let Some(entry) = entries.remove(&key)
            {
                entry.cancellation.cancel();
            }
            return;
        }
        let Some(mut entry) = entries.remove(&key) else {
            return;
        };
        Self::remove_output(inner, &entry);
        entry.state = PreviewRenderState::Failed;
        entry.cancellation.cancel();
    }

    fn complete(
        inner: &PreviewClassRendersInner,
        photo_id: &str,
        stage: &'static str,
        settings: &'static str,
        identity_digest: &str,
        result: RenderedPreview,
    ) {
        let key = (photo_id.to_owned(), stage, settings);
        let mut entries = inner
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(entry) = entries.get_mut(&key) else {
            inner.exports.delete_preview_output(&result.attempt_key);
            return;
        };
        if entry.identity_digest != identity_digest
            || !matches!(&entry.state, PreviewRenderState::Running)
        {
            inner.exports.delete_preview_output(&result.attempt_key);
            return;
        }
        let deadline = SystemTime::now() + RENDITION_TTL;
        entry.state = PreviewRenderState::Ready {
            identity: Box::new(preview_facts(&result.identity)),
            output_path: result.path,
            attempt_key: result.attempt_key,
            size: result.size,
            sha256: result.sha256,
            facts: Box::new(result.output_facts),
            deadline,
        };
        entry.last_used = inner.touches.fetch_add(1, Ordering::Relaxed) + 1;
    }

    fn evict_one_locked(
        inner: &PreviewClassRendersInner,
        entries: &mut HashMap<PreviewRenderKey, PreviewRenderEntry>,
    ) {
        let victim = entries
            .iter()
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(key, _)| key.clone());
        if let Some(victim) = victim
            && let Some(entry) = entries.remove(&victim)
        {
            entry.cancellation.cancel();
            Self::remove_output(inner, &entry);
        }
    }
}
impl PreviewRenderGate for PreviewClassRenders {
    fn admit(&self, request: PreviewRenderRequest<'_>) -> RenderAdmission {
        let key = (request.photo_id.to_owned(), request.stage, request.settings);
        let now = SystemTime::now();
        let mut entries = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        Self::purge_expired_locked(&self.inner, &mut entries, now);
        if let Some(entry) = entries.get_mut(&key) {
            if entry.identity_digest == request.identity_digest {
                match &entry.state {
                    PreviewRenderState::Running => {
                        entry.last_used = self.inner.touches.fetch_add(1, Ordering::Relaxed) + 1;
                        return RenderAdmission::Running;
                    }
                    PreviewRenderState::Ready { deadline, .. } if *deadline > now => {
                        entry.last_used = self.inner.touches.fetch_add(1, Ordering::Relaxed) + 1;
                        return RenderAdmission::Running;
                    }
                    PreviewRenderState::Ready { .. } | PreviewRenderState::Failed => {}
                }
            }
            if let Some(entry) = entries.remove(&key) {
                entry.cancellation.cancel();
                Self::remove_output(&self.inner, &entry);
            }
        }
        if entries.len() >= MAXIMUM_OWNERS {
            Self::evict_one_locked(&self.inner, &mut entries);
        }
        let cancellation = PreviewCancellation::new();
        entries.insert(
            key.clone(),
            PreviewRenderEntry {
                identity_digest: request.identity_digest.to_owned(),
                state: PreviewRenderState::Running,
                cancellation: cancellation.clone(),
                last_used: self.inner.touches.fetch_add(1, Ordering::Relaxed) + 1,
            },
        );
        drop(entries);

        if tokio::runtime::Handle::try_current().is_err() {
            Self::settle_inner(
                &self.inner,
                request.photo_id,
                request.stage,
                request.settings,
                request.identity_digest,
                RenderSettlement::Failed,
            );
            return RenderAdmission::Unavailable(RenderUnavailable::AdmissionNotLanded);
        }
        let inner = Arc::clone(&self.inner);
        let photo_id = request.photo_id.to_owned();
        let identity_digest = request.identity_digest.to_owned();
        let stage = request.stage;
        let settings = request.settings;
        tokio::spawn(async move {
            let result = inner
                .render
                .render(&photo_id, stage, settings, cancellation)
                .await;
            match result {
                Ok(result)
                    if PreviewIdentity::build(&preview_facts(&result.identity), None).digest()
                        == identity_digest =>
                {
                    Self::complete(&inner, &photo_id, stage, settings, &identity_digest, result);
                }
                Ok(result) => {
                    inner.exports.delete_preview_output(&result.attempt_key);
                    Self::settle_inner(
                        &inner,
                        &photo_id,
                        stage,
                        settings,
                        &identity_digest,
                        RenderSettlement::Failed,
                    );
                }
                Err(_) => Self::settle_inner(
                    &inner,
                    &photo_id,
                    stage,
                    settings,
                    &identity_digest,
                    RenderSettlement::Failed,
                ),
            }
        });
        RenderAdmission::Queued
    }

    fn is_live(&self, request: PreviewRenderRequest<'_>) -> bool {
        let key = (request.photo_id.to_owned(), request.stage, request.settings);
        let now = SystemTime::now();
        let mut entries = self
            .inner
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        Self::purge_expired_locked(&self.inner, &mut entries, now);
        let live = entries.get(&key).is_some_and(|entry| {
            entry.identity_digest == request.identity_digest
                && match &entry.state {
                    PreviewRenderState::Running => true,
                    PreviewRenderState::Ready {
                        output_path,
                        deadline,
                        ..
                    } => *deadline > now && fs::metadata(output_path).is_ok(),
                    PreviewRenderState::Failed => false,
                }
        });
        if !live
            && entries
                .get(&key)
                .is_some_and(|entry| entry.identity_digest == request.identity_digest)
            && let Some(entry) = entries.remove(&key)
        {
            Self::remove_output(&self.inner, &entry);
        }
        live
    }

    fn settle(&self, request: PreviewRenderRequest<'_>, settlement: RenderSettlement) {
        Self::settle_inner(
            &self.inner,
            request.photo_id,
            request.stage,
            request.settings,
            request.identity_digest,
            settlement,
        );
    }
}

impl DevelopmentResultRetention for PreviewClassRenders {
    fn resolve<'a>(
        &'a self,
        photo_id: &'a str,
        facts: &'a PreviewFacts,
    ) -> Pin<Box<dyn Future<Output = Option<RetainedDevelopmentResult>> + Send + 'a>> {
        Box::pin(async move {
            if let Some(retained) = self.inner.retained.resolve(photo_id, facts).await {
                return Some(retained);
            }
            let key = (photo_id.to_owned(), facts.stage, facts.settings);
            let now = SystemTime::now();
            let mut entries = self
                .inner
                .entries
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            Self::purge_expired_locked(&self.inner, &mut entries, now);
            let entry = entries.get_mut(&key)?;
            let (identity, output_path, size, sha256, width, height, deadline) = match &entry.state
            {
                PreviewRenderState::Ready {
                    identity,
                    output_path,
                    size,
                    sha256,
                    facts,
                    deadline,
                    ..
                } => (
                    identity.as_ref().clone(),
                    output_path.clone(),
                    *size,
                    sha256.clone(),
                    facts.width,
                    facts.height,
                    *deadline,
                ),
                _ => return None,
            };
            if deadline <= now || identity != *facts {
                return None;
            }
            if fs::metadata(&output_path).is_err() {
                if let Some(entry) = entries.remove(&key) {
                    Self::remove_output(&self.inner, &entry);
                }
                return None;
            }
            entry.last_used = self.inner.touches.fetch_add(1, Ordering::Relaxed) + 1;
            Some(RetainedDevelopmentResult {
                sha256,
                byte_length: size,
                recipe_revision: identity.recipe_revision.clone(),
                exposure_milli_ev: identity.exposure_milli_ev,
                white_balance: WHITE_BALANCE_AS_SHOT,
                source_revision: identity.source_revision.clone(),
                bundle_sha256: identity.bundle_sha256.clone(),
                path: output_path,
                width,
                height,
            })
        })
    }
}
