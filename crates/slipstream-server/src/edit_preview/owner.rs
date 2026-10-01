//! The Edit Preview owner: the identity assembly and the per-server rendition
//! cache with the render intent slots of every Photo and stage owner. Identity
//! equality — the complete fact set beside the source content evidence —
//! decides cache currency, admission coalescing, and every supersession and
//! publication decision the owner makes.

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime},
};

use sha2::{Digest, Sha256};
use tokio::sync::{Mutex as AsyncMutex, Semaphore};

use slipstream_core::{DEVELOPMENT_PREVIEW_LONG_EDGE, DISPLAY_TRANSFORM_VERSION};

use crate::{export_manager::ExportManager, preview_render::RenderedIdentity};

use super::renders::{
    DevelopmentResultRetention, PreviewClassRenders, PreviewRenderGate, PreviewRenderRequest,
    RenderAdmission, RenderSettlement, RetainedDevelopmentResult, UnlandedRenderGate,
    UnlandedRetention,
};

use super::{
    DERIVATION_QUEUE_BOUND, MAXIMUM_OWNERS, PENDING_TTL, RENDITION_TTL, WHITE_BALANCE_AS_SHOT,
};

// ---------------------------------------------------------------- identity

/// The identity facts of one stage rendition: the complete fact set whose
/// equality decides cache currency beside the source content evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreviewFacts {
    pub(crate) stage: &'static str,
    /// The settings selector this rendition was requested under: `current`
    /// for the saved recipe's settings, `baseline` for the comparison's
    /// as-shot/baseline development settings.
    pub(crate) settings: &'static str,
    pub(crate) long_edge: u32,
    pub(crate) display_transform: &'static str,
    pub(crate) bundle_sha256: String,
    pub(crate) source_revision: String,
    pub(crate) recipe_revision: Option<String>,
    pub(crate) exposure_milli_ev: i64,
    pub(crate) white_balance: &'static str,
    pub(crate) source: &'static str,
    pub(crate) proxy_id: Option<String>,
}

/// The source content evidence of one rendition identity. Only a rendition
/// derived from the retained Development Result of the current identity
/// carries that result's evidence; anything else is not current.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ContentEvidence {
    DevelopmentResult { sha256: String, byte_length: u64 },
    NotRetained,
}

/// One stage result identity: the source content evidence, the recipe revision
/// and complete recipe content, the bundle, the stage, the geometry, and the
/// display-transform identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PreviewIdentity {
    pub(super) facts: PreviewFacts,
    evidence: ContentEvidence,
}

impl PreviewIdentity {
    /// Builds the current identity from the current facts and the resolved
    /// retention. Only a retained result captured under exactly the current
    /// facts contributes its content evidence; any other retention is stale
    /// and never serves.
    pub(super) fn build(
        facts: &PreviewFacts,
        retained: Option<&RetainedDevelopmentResult>,
    ) -> Self {
        let evidence = retained
            .filter(|record| record.matches_facts(facts))
            .map(RetainedDevelopmentResult::evidence)
            .unwrap_or(ContentEvidence::NotRetained);
        Self {
            facts: facts.clone(),
            evidence,
        }
    }

    /// The digest of the render intent alone: the identity sequence with the
    /// content-evidence tail always unset. A queued admission may carry no
    /// local evidence while a later derive request for the same intent does,
    /// so supersession and settlement compare intents, not evidence.
    fn intent_digest(&self) -> String {
        Self::digest_sequence(&self.facts, &[1])
    }

    /// The opaque digest of the full identity, used as the pending intent
    /// identity of the owner.
    pub(super) fn digest(&self) -> String {
        match &self.evidence {
            ContentEvidence::DevelopmentResult {
                sha256,
                byte_length,
            } => {
                let mut tail = sha256.as_bytes().to_vec();
                tail.push(0);
                tail.extend_from_slice(&byte_length.to_le_bytes());
                Self::digest_sequence(&self.facts, &tail)
            }
            ContentEvidence::NotRetained => Self::digest_sequence(&self.facts, &[1]),
        }
    }

    fn digest_sequence(facts: &PreviewFacts, evidence_tail: &[u8]) -> String {
        let mut hasher = Sha256::new();
        for part in [
            facts.stage.as_bytes(),
            facts.settings.as_bytes(),
            facts.long_edge.to_le_bytes().as_slice(),
            facts.display_transform.as_bytes(),
            facts.bundle_sha256.as_bytes(),
            facts.source_revision.as_bytes(),
            facts.recipe_revision.as_deref().unwrap_or("").as_bytes(),
            facts.exposure_milli_ev.to_le_bytes().as_slice(),
            facts.white_balance.as_bytes(),
            facts.source.as_bytes(),
            facts.proxy_id.as_deref().unwrap_or("").as_bytes(),
        ] {
            hasher.update(part);
            hasher.update([0]);
        }
        hasher.update(evidence_tail);
        hex(&hasher.finalize())
    }
}

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub(super) fn milli_ev(exposure_ev: f64) -> i64 {
    (exposure_ev * 1000.0).round() as i64
}

/// Maps the render-time identity inputs of one Original preview attempt
/// into this boundary's Preview facts. The rendering executor reports plain
/// evidence; only this boundary assembles Preview identity.
pub(super) fn preview_facts(identity: &RenderedIdentity) -> PreviewFacts {
    PreviewFacts {
        stage: identity.stage,
        settings: identity.settings,
        long_edge: DEVELOPMENT_PREVIEW_LONG_EDGE,
        display_transform: DISPLAY_TRANSFORM_VERSION,
        bundle_sha256: identity.bundle_sha256.clone(),
        source_revision: identity.source_revision.clone(),
        recipe_revision: identity.recipe_revision.clone(),
        exposure_milli_ev: identity.exposure_milli_ev,
        white_balance: WHITE_BALANCE_AS_SHOT,
        source: "original",
        proxy_id: None,
    }
}

// ---------------------------------------------------------------- owner

/// One published rendition with the full identity it was derived under.
#[derive(Clone, Debug)]
pub(crate) struct PublishedRendition {
    pub(super) identity: PreviewIdentity,
    pub(super) bytes: axum::body::Bytes,
    pub(super) sha256: String,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) expires_at: SystemTime,
}

/// One admitted render intent: the identity digest it was admitted under and
/// the moment of admission, which bounds how long an unsettled intent answers
/// `running` before the identity is free again.
#[derive(Clone, Debug)]
struct PendingIntent {
    digest: String,
    since: SystemTime,
}

/// The cancellable intent of one derivation, shared by requests for the same
/// identity so polling cannot cancel work it is waiting for.
#[derive(Clone)]
pub(super) struct DerivationSignal {
    cancelled: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
    identity_digest: String,
}

impl DerivationSignal {
    fn start(identity_digest: &str) -> Self {
        Self {
            cancelled: Arc::default(),
            notify: Arc::new(tokio::sync::Notify::new()),
            identity_digest: identity_digest.to_owned(),
        }
    }

    /// The token the native conversion polls around its opaque call.
    pub(super) fn token(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancelled)
    }

    pub(super) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.notify.notify_waiters();
    }

    /// Resolves as soon as the intent is cancelled. The waiter is registered
    /// before each flag check, so a concurrent cancel is never lost.
    pub(super) async fn cancelled(&self) {
        let mut notified = std::pin::pin!(self.notify.notified());
        loop {
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.as_mut().await;
            notified.set(self.notify.notified());
        }
    }
}

#[derive(Default)]
struct OwnerEntry {
    published: Option<PublishedRendition>,
    pending: Option<PendingIntent>,
    /// The cancellable intent of the derivation in flight, if any.
    inflight: Option<DerivationSignal>,
    /// The per-owner derive serialization, owned by the entry so owner
    /// eviction releases it with the rest of the owner state.
    derive_permit: Arc<AsyncMutex<()>>,
    last_used: u64,
}

/// One rendition owner: the Photo, the stage, and the settings selector. The
/// comparison of a stage owns its rendition separately, so a baseline request
/// never publishes over, or supersedes, the current rendition of that stage.
pub(super) type OwnerKey = (String, &'static str, &'static str);

/// Evicts the least recently touched other owner when the retained-owner
/// bound is full; every owner insert path runs this first.
fn evict_if_full(
    owners: &mut HashMap<OwnerKey, OwnerEntry>,
    key: &OwnerKey,
    render_gate: &Arc<dyn PreviewRenderGate>,
) {
    if !owners.contains_key(key) && owners.len() >= MAXIMUM_OWNERS {
        let victim = owners
            .iter()
            .filter(|(existing, _)| *existing != key)
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(existing, _)| existing.clone());
        let evicted = victim.and_then(|victim| owners.remove(&victim).map(|entry| (victim, entry)));
        if let Some((victim, entry)) = evicted {
            // An evicted owner can no longer cancel or settle its own work:
            // its queued admission settles as cancelled so the gate never
            // answers `running` forever, and its in-flight token flips so a
            // conversion that already started discards its result.
            if let Some(pending) = entry.pending {
                render_gate.settle(
                    PreviewRenderRequest {
                        photo_id: &victim.0,
                        stage: victim.1,
                        settings: victim.2,
                        identity_digest: &pending.digest,
                    },
                    RenderSettlement::Cancelled,
                );
            }
            if let Some(inflight) = &entry.inflight {
                inflight.cancel();
            }
        }
    }
}

/// The derived bytes of one rendition, awaiting the publication decision.
pub(super) struct DerivedRendition {
    pub(super) bytes: axum::body::Bytes,
    pub(super) sha256: String,
    pub(super) width: u32,
    pub(super) height: u32,
}

/// The outcome of a conditional publication.
#[derive(Debug)]
pub(crate) enum PublishOutcome {
    Published(Box<PublishedRendition>),
    Superseded,
}

/// The Edit Preview owner: the per-server rendition cache and the render
/// intent slots of every Photo and stage owner.
pub(crate) struct EditPreviewOwner {
    pub(super) retention: Arc<dyn DevelopmentResultRetention>,
    render_gate: Arc<dyn PreviewRenderGate>,
    owners: AsyncMutex<HashMap<OwnerKey, OwnerEntry>>,
    /// One heavy native conversion at a time, instance-wide: the same bound
    /// the closed workload applies to processing jobs. Native conversion
    /// memory scales with source size, so unbounded concurrency across
    /// Photos would scale resident memory with request fan-out.
    derivation_permits: Arc<Semaphore>,
    /// How long a request may wait for the conversion slot before falling
    /// through to the admission path instead of hanging behind a conversion.
    derivation_queue_bound: Duration,
    touches: AtomicU64,
    derivations_started: AtomicUsize,
}

impl EditPreviewOwner {
    /// The production owner. A deployment with the Export lifecycle shares
    /// one preview-class registry between retention and render admission, so
    /// an unretained identity is rendered through the same serialized
    /// production workload. A deployment without processing remains
    /// fail-closed through the unlanded seams.
    pub(crate) fn production(exports: Option<Arc<ExportManager>>) -> Self {
        let Some(exports) = exports else {
            return Self::new(Arc::new(UnlandedRetention), Arc::new(UnlandedRenderGate));
        };
        let renders = Arc::new(PreviewClassRenders::new(exports));
        let retention: Arc<dyn DevelopmentResultRetention> = renders.clone();
        let render_gate: Arc<dyn PreviewRenderGate> = renders;
        Self::new(retention, render_gate)
    }

    pub(crate) fn new(
        retention: Arc<dyn DevelopmentResultRetention>,
        render_gate: Arc<dyn PreviewRenderGate>,
    ) -> Self {
        Self {
            retention,
            render_gate,
            owners: AsyncMutex::new(HashMap::new()),
            derivation_permits: Arc::new(Semaphore::new(1)),
            derivation_queue_bound: DERIVATION_QUEUE_BOUND,
            touches: AtomicU64::new(0),
            derivations_started: AtomicUsize::new(0),
        }
    }

    /// Touches one owner entry under the owner lock, evicting the least
    /// recently touched other owner when the retained-owner bound is full.
    async fn touch_entry<R>(&self, key: &OwnerKey, read: impl FnOnce(&mut OwnerEntry) -> R) -> R {
        let mut owners = self.owners.lock().await;
        let touch = self.touches.fetch_add(1, Ordering::Relaxed) + 1;
        evict_if_full(&mut owners, key, &self.render_gate);
        let entry = owners.entry(key.clone()).or_default();
        entry.last_used = touch;
        read(entry)
    }

    /// The published rendition still current for this full identity, if any.
    /// A rendition under a different display transform, without content
    /// evidence for its source, or past its disclosed expiry is not current.
    pub(super) async fn current(
        &self,
        key: &OwnerKey,
        identity: &PreviewIdentity,
    ) -> Option<PublishedRendition> {
        self.touch_entry(key, |entry| {
            let hit = entry
                .published
                .as_ref()
                .filter(|rendition| {
                    rendition.identity == *identity && rendition.expires_at > SystemTime::now()
                })
                .cloned();
            // A current rendition settles any pending intent of the same
            // identity.
            if hit.is_some() {
                entry.pending = None;
            }
            hit
        })
        .await
    }

    /// The per-owner derive permit, created with and evicted alongside its
    /// owner entry. Holding it serializes derive-and-publish so a concurrent
    /// equal request coalesces onto the first derivation.
    pub(super) async fn derive_permit(&self, key: &OwnerKey) -> Arc<AsyncMutex<()>> {
        self.touch_entry(key, |entry| entry.derive_permit.clone())
            .await
    }

    /// The instance-wide heavy-conversion permit. Held across one derivation,
    /// so at most one native display conversion runs at a time.
    pub(super) fn derivation_queue_bound(&self) -> Duration {
        self.derivation_queue_bound
    }

    /// Binds a shorter conversion-slot wait, for tests that observe the
    /// bound without waiting out the production constant.
    #[cfg(test)]
    pub(crate) fn with_derivation_queue_bound(mut self, bound: Duration) -> Self {
        self.derivation_queue_bound = bound;
        self
    }

    pub(crate) async fn derivation_permit(&self) -> tokio::sync::OwnedSemaphorePermit {
        Arc::clone(&self.derivation_permits)
            .acquire_owned()
            .await
            .expect("derivation permits never close")
    }

    /// A heavy-conversion permit without waiting; the test observation of the
    /// instance-wide bound.
    #[allow(dead_code)]
    pub(crate) fn try_derivation_permit(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        self.derivation_permits.clone().try_acquire_owned().ok()
    }

    /// The number of derivations this owner actually started, after every
    /// coalescing check.
    #[allow(dead_code)]
    pub(crate) fn derivations_started(&self) -> usize {
        self.derivations_started.load(Ordering::Relaxed)
    }

    pub(super) fn note_derivation_started(&self) {
        self.derivations_started.fetch_add(1, Ordering::Relaxed);
    }

    /// Registers one in-flight derivation. Identical requests share its signal;
    /// a changed identity cancels prior work before replacing it.
    pub(super) async fn begin_derivation(
        &self,
        key: &OwnerKey,
        identity: &PreviewIdentity,
    ) -> DerivationSignal {
        let identity_digest = identity.digest();
        self.touch_entry(key, |entry| {
            if let Some(previous) = entry.inflight.as_ref() {
                if previous.identity_digest == identity_digest {
                    return previous.clone();
                }
                previous.cancel();
            }
            let signal = DerivationSignal::start(&identity_digest);
            entry.inflight = Some(signal.clone());
            signal
        })
        .await
    }

    /// Clears the in-flight slot when this derivation leaves without
    /// publishing, so a cancelled slot never lingers on the entry.
    pub(super) async fn end_derivation(&self, key: &OwnerKey, signal: &DerivationSignal) {
        self.touch_entry(key, |entry| {
            if entry
                .inflight
                .as_ref()
                .is_some_and(|installed| Arc::ptr_eq(&installed.cancelled, &signal.cancelled))
            {
                entry.inflight = None;
            }
        })
        .await
    }

    /// Whether a newer intent has been registered for this owner while this
    /// request waited: its pending digest names a different identity, so
    /// this request must not start native work of its own.
    pub(super) async fn intent_superseded(
        &self,
        key: &OwnerKey,
        identity: &PreviewIdentity,
    ) -> bool {
        let intent = identity.intent_digest();
        self.touch_entry(key, |entry| {
            entry
                .pending
                .as_ref()
                .is_some_and(|pending| pending.digest != intent)
        })
        .await
    }

    /// Admits one render. A pending request with the same full identity
    /// coalesces until it settles or its patience expires; a different
    /// identity supersedes the pending intent and cancels the superseded
    /// derivation in flight.
    pub(super) async fn admit(
        &self,
        key: &OwnerKey,
        photo_id: &str,
        stage: &'static str,
        settings: &'static str,
        identity: &PreviewIdentity,
        now: SystemTime,
    ) -> RenderAdmission {
        let digest = identity.digest();
        let intent = identity.intent_digest();
        let coalesced = self
            .touch_entry(key, |entry| {
                matches!(
                    entry.pending.as_ref(),
                    Some(pending)
                        if pending.digest == intent
                            && now
                                .duration_since(pending.since)
                                .unwrap_or_default()
                                < PENDING_TTL
                )
            })
            .await;
        if coalesced
            && self.render_gate.is_live(PreviewRenderRequest {
                photo_id,
                stage,
                settings,
                identity_digest: &digest,
            })
        {
            return RenderAdmission::Running;
        }
        if coalesced {
            // The gate settled or expired the attempt while the owner still
            // held its local pending receipt; make the gate authoritative.
            self.touch_entry(key, |entry| {
                if entry
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.digest == intent)
                {
                    entry.pending = None;
                }
            })
            .await;
        }
        let admission = self.render_gate.admit(PreviewRenderRequest {
            photo_id,
            stage,
            settings,
            identity_digest: &digest,
        });
        if matches!(
            admission,
            RenderAdmission::Queued | RenderAdmission::Running
        ) {
            self.touch_entry(key, |entry| {
                // Any identity change supersedes whatever ran before it:
                // its in-flight derivation must not publish, and its native
                // work is released.
                let differing = entry
                    .pending
                    .as_ref()
                    .is_none_or(|pending| pending.digest != intent);
                if differing && let Some(inflight) = &entry.inflight {
                    inflight.cancel();
                }
                entry.pending = Some(PendingIntent {
                    digest: intent,
                    since: now,
                });
            })
            .await;
        }
        admission
    }

    /// Settles one admitted render: the gate records the outcome and the
    /// pending intent of that identity is freed, so the next request retries
    /// instead of coalescing into a dead intent.
    pub(super) async fn settle_render(
        &self,
        key: &OwnerKey,
        photo_id: &str,
        stage: &'static str,
        identity: &PreviewIdentity,
        settlement: RenderSettlement,
    ) {
        let digest = identity.digest();
        let intent = identity.intent_digest();
        self.render_gate.settle(
            PreviewRenderRequest {
                photo_id,
                stage,
                settings: key.2,
                identity_digest: &digest,
            },
            settlement,
        );
        self.touch_entry(key, |entry| {
            if entry
                .pending
                .as_ref()
                .is_some_and(|pending| pending.digest == intent)
            {
                entry.pending = None;
            }
        })
        .await;
    }

    /// Publishes the derived rendition, conditional on the identity that is
    /// current at publish time: the owner lock is held across one final
    /// serialized facts read, and the rendition is stored only while the
    /// identity that read returns is still the identity the request derived
    /// under. Publication is ordered after that read; a save that commits
    /// after it is reflected by the next request, exactly like every other
    /// serialized read in the service.
    pub(super) async fn publish_if_current<F, Fut>(
        &self,
        key: &OwnerKey,
        identity: &PreviewIdentity,
        read_current: F,
        derived: DerivedRendition,
    ) -> PublishOutcome
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Option<PreviewIdentity>>,
    {
        let mut owners = self.owners.lock().await;
        let Some(current) = read_current().await else {
            // The fresh facts are unreadable; refuse instead of publishing.
            return PublishOutcome::Superseded;
        };
        if current != *identity {
            return PublishOutcome::Superseded;
        }
        let touch = self.touches.fetch_add(1, Ordering::Relaxed) + 1;
        evict_if_full(&mut owners, key, &self.render_gate);
        let entry = owners.entry(key.clone()).or_default();
        entry.last_used = touch;
        entry.pending = None;
        entry.inflight = None;
        let rendition = PublishedRendition {
            identity: identity.clone(),
            bytes: derived.bytes,
            sha256: derived.sha256,
            width: derived.width,
            height: derived.height,
            expires_at: SystemTime::now() + RENDITION_TTL,
        };
        entry.published = Some(rendition.clone());
        PublishOutcome::Published(Box::new(rendition))
    }

    /// Accepts a published rendition against persistence. The serialized
    /// facts read runs after the store; when it no longer names the
    /// rendition's identity — or cannot be read — the rendition is evicted
    /// and the requesting response is refused instead of served stale.
    pub(super) async fn confirm_publication<F, Fut>(
        &self,
        key: &OwnerKey,
        identity: &PreviewIdentity,
        read_current: F,
    ) -> bool
    where
        F: Fn() -> Fut,
        Fut: Future<Output = Option<PreviewIdentity>>,
    {
        match read_current().await {
            Some(current) if current == *identity => true,
            _ => {
                self.evict_published(key, identity).await;
                false
            }
        }
    }

    /// Removes the published rendition of exactly this identity, leaving any
    /// newer publication alone.
    async fn evict_published(&self, key: &OwnerKey, identity: &PreviewIdentity) {
        self.touch_entry(key, |entry| {
            if entry
                .published
                .as_ref()
                .is_some_and(|rendition| rendition.identity == *identity)
            {
                entry.published = None;
            }
        })
        .await;
    }

    /// The bounded number of owners currently retained.
    #[allow(dead_code)]
    pub(crate) async fn owner_count(&self) -> usize {
        self.owners.lock().await.len()
    }
}
