//! Edit Preview HTTP surface: one display rendition per Photo and stage.
//! The route derives the rendition through the pinned Development display
//! derivative, frames it as response headers followed by the JPEG stream, and
//! schedules preview work latest-intent-wins within its Photo and stage owner
//! per the Photo Development Surface contract.

use std::{
    collections::HashMap,
    os::fd::AsRawFd,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime},
};

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    response::Response,
};
use sha2::{Digest, Sha256};
use slipstream_processing::photo_profile::{
    APPROVED_EXPOSURE_MILLI_EV_MAX, APPROVED_EXPOSURE_MILLI_EV_MIN,
};
use tokio::sync::{Mutex as AsyncMutex, Semaphore};

use slipstream_core::{
    DEVELOPMENT_PREVIEW_LONG_EDGE, DISPLAY_TRANSFORM_VERSION, EditRecipeRead,
    process_development_tiff,
};

use crate::{
    ProcessingConfig,
    http::{
        CLI_CONTRACT_HEADER, HttpState, cli_error, require_cli_contract, require_published,
        valid_id,
    },
    queries::{format_time, hex_encode},
};

/// The closed stage set of the first version; `stage` is the closed value
/// `develop` until the Film capability is enabled.
const CLOSED_STAGES: [&str; 1] = ["develop"];

/// The disclosed bounded retention of one published rendition. Renditions are
/// rebuildable intermediates, so the bound is short and disclosed through the
/// `expiresAt` response header on every delivery.
const RENDITION_TTL: Duration = Duration::from_secs(300);

/// The bounded patience for one admitted render intent. A render whose
/// completion, failure, or cancellation never settles the intent — a lost
/// receipt — frees its identity again after this bound.
const PENDING_TTL: Duration = Duration::from_secs(300);

/// The longest a derive request waits for the instance-wide conversion slot
/// before falling through to admission. Generous against a whole conversion
/// of a large source, short enough that a stuck conversion cannot hold a
/// request forever.
const DERIVATION_QUEUE_BOUND: Duration = Duration::from_secs(30);

/// The bounded number of Photo and stage owners the server retains. Every
/// owner here holds only rebuildable rendition bytes and one pending intent,
/// and the bound evicts the least recently touched owner under pressure.
const MAXIMUM_OWNERS: usize = 1024;

/// The stored white-balance mode name of the first workload. The core state
/// layer only stores this mode, and the processing baseline is as-shot.
const WHITE_BALANCE_AS_SHOT: &str = "as-shot";

// ---------------------------------------------------------------- identity

/// The identity facts of one stage rendition: the complete fact set whose
/// equality decides cache currency beside the source content evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
struct PreviewFacts {
    stage: &'static str,
    long_edge: u32,
    display_transform: &'static str,
    bundle_sha256: String,
    source_revision: String,
    recipe_revision: Option<String>,
    exposure_milli_ev: i64,
    white_balance: &'static str,
}

/// The source content evidence of one rendition identity. Only a rendition
/// derived from the retained Development Result of the current identity
/// carries that result's evidence; anything else is not current.
#[derive(Clone, Debug, Eq, PartialEq)]
enum ContentEvidence {
    DevelopmentResult { sha256: String, byte_length: u64 },
    NotRetained,
}

/// One stage result identity: the source content evidence, the recipe revision
/// and complete recipe content, the bundle, the stage, the geometry, and the
/// display-transform identity.
#[derive(Clone, Debug, Eq, PartialEq)]
struct PreviewIdentity {
    facts: PreviewFacts,
    evidence: ContentEvidence,
}

impl PreviewIdentity {
    /// Builds the current identity from the current facts and the resolved
    /// retention. Only a retained result captured under exactly the current
    /// facts contributes its content evidence; any other retention is stale
    /// and never serves.
    fn build(facts: &PreviewFacts, retained: Option<&RetainedDevelopmentResult>) -> Self {
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
    fn digest(&self) -> String {
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
            facts.long_edge.to_le_bytes().as_slice(),
            facts.display_transform.as_bytes(),
            facts.bundle_sha256.as_bytes(),
            facts.source_revision.as_bytes(),
            facts.recipe_revision.as_deref().unwrap_or("").as_bytes(),
            facts.exposure_milli_ev.to_le_bytes().as_slice(),
            facts.white_balance.as_bytes(),
        ] {
            hasher.update(part);
            hasher.update([0]);
        }
        hasher.update(evidence_tail);
        hex(&hasher.finalize())
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn milli_ev(exposure_ev: f64) -> i64 {
    (exposure_ev * 1000.0).round() as i64
}

// ---------------------------------------------------------------- seams

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
}

impl RetainedDevelopmentResult {
    /// True when the result was produced under exactly the current recipe
    /// revision and content, source revision, and bundle. A result captured
    /// under any other identity is not current and must not be served.
    fn matches_facts(&self, facts: &PreviewFacts) -> bool {
        self.recipe_revision == facts.recipe_revision
            && self.exposure_milli_ev == facts.exposure_milli_ev
            && self.white_balance == facts.white_balance
            && self.source_revision == facts.source_revision
            && self.bundle_sha256 == facts.bundle_sha256
    }

    fn evidence(&self) -> ContentEvidence {
        ContentEvidence::DevelopmentResult {
            sha256: self.sha256.clone(),
            byte_length: self.byte_length,
        }
    }
}

/// Resolves the retained Development Result of one Photo, or `None` when no
/// result of the current identity is retained.
pub(crate) trait DevelopmentResultRetention: Send + Sync {
    fn resolve(&self, photo_id: &str) -> Option<RetainedDevelopmentResult>;
}

/// The durable Development Result retention lands with the Export lifecycle.
/// Until that lifecycle retains results, the production resolver retains
/// nothing and every render request refuses fail-closed instead of claiming
/// queued work it cannot run.
pub(crate) struct UnlandedRetention;

impl DevelopmentResultRetention for UnlandedRetention {
    fn resolve(&self, _photo_id: &str) -> Option<RetainedDevelopmentResult> {
        None
    }
}

/// One preview-class render admission against the same closed production
/// workload an Export uses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RenderAdmission {
    /// Admitted behind the owner's current work.
    // The production workload cannot express a bounded preview render yet
    // (see `UnlandedRenderGate`), so only the route tests construct the
    // queued and indeterminate outcomes; the closed contract keeps them.
    #[allow(dead_code)]
    Queued,
    /// The same full identity is already admitted; the request coalesced.
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
    /// The service-side admission of preview-class renders has not landed.
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

/// The settlement of one admitted render, reported when the attempt completes,
/// fails, or is cancelled. A completed render publishes through the owner; a
/// failed or cancelled one frees its identity to be admitted again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RenderSettlement {
    #[allow(dead_code)]
    Completed,
    #[allow(dead_code)]
    Failed,
    Cancelled,
}

/// The admission request of one preview-class render. A production gate binds
/// the attempt to the identity digest; today's fail-closed gate ignores it.
#[allow(dead_code)]
pub(crate) struct PreviewRenderRequest<'a> {
    pub(crate) photo_id: &'a str,
    pub(crate) stage: &'static str,
    pub(crate) identity_digest: &'a str,
}

pub(crate) trait PreviewRenderGate: Send + Sync {
    fn admit(&self, request: PreviewRenderRequest<'_>) -> RenderAdmission;
    /// Settles one admitted render. Every admission must eventually settle:
    /// completion, failure, and cancellation all free the identity, so a
    /// failed or lost render is retried by the next request instead of
    /// answering `running` forever.
    fn settle(&self, request: PreviewRenderRequest<'_>, settlement: RenderSettlement);
}

/// The service-side admission of preview-class development renders follows the
/// same durable Export lifecycle that owns the production `development-tiff`
/// path. Until that lifecycle lands, no render can be admitted and the route
/// refuses fail-closed with the contract's `processing_unavailable` code; it
/// never reports queued work that cannot run, and there is nothing to settle.
pub(crate) struct UnlandedRenderGate;

impl PreviewRenderGate for UnlandedRenderGate {
    fn admit(&self, _request: PreviewRenderRequest<'_>) -> RenderAdmission {
        RenderAdmission::Unavailable(RenderUnavailable::AdmissionNotLanded)
    }

    fn settle(&self, _request: PreviewRenderRequest<'_>, _settlement: RenderSettlement) {}
}

// ---------------------------------------------------------------- owner

/// One published rendition with the full identity it was derived under.
#[derive(Clone, Debug)]
pub(crate) struct PublishedRendition {
    identity: PreviewIdentity,
    bytes: axum::body::Bytes,
    sha256: String,
    width: u32,
    height: u32,
    expires_at: SystemTime,
}

/// One admitted render intent: the identity digest it was admitted under and
/// the moment of admission, which bounds how long an unsettled intent answers
/// `running` before the identity is free again.
#[derive(Clone, Debug)]
struct PendingIntent {
    digest: String,
    since: SystemTime,
}

/// The cancellable intent of one derivation: the token the conversion
/// polls, plus the wakeup that releases a request still queued for the
/// conversion slot the moment its intent is cancelled.
#[derive(Clone)]
struct DerivationSignal {
    cancelled: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl DerivationSignal {
    fn start() -> Self {
        Self {
            cancelled: Arc::default(),
            notify: Arc::new(tokio::sync::Notify::new()),
        }
    }

    /// The token the native conversion polls around its opaque call.
    fn token(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancelled)
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
        self.notify.notify_waiters();
    }

    /// Resolves as soon as the intent is cancelled. The waiter is registered
    /// before each flag check, so a concurrent cancel is never lost.
    async fn cancelled(&self) {
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

type OwnerKey = (String, &'static str);

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
struct DerivedRendition {
    bytes: axum::body::Bytes,
    sha256: String,
    width: u32,
    height: u32,
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
    retention: Arc<dyn DevelopmentResultRetention>,
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
    /// The production owner: no durable Development Result retention and no
    /// preview-class render admission exist yet, so renders refuse
    /// fail-closed until the Export lifecycle lands.
    pub(crate) fn production() -> Self {
        Self::new(Arc::new(UnlandedRetention), Arc::new(UnlandedRenderGate))
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
    async fn current(
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
    async fn derive_permit(&self, key: &OwnerKey) -> Arc<AsyncMutex<()>> {
        self.touch_entry(key, |entry| entry.derive_permit.clone())
            .await
    }

    /// The instance-wide heavy-conversion permit. Held across one derivation,
    /// so at most one native display conversion runs at a time.
    fn derivation_queue_bound(&self) -> Duration {
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

    fn note_derivation_started(&self) {
        self.derivations_started.fetch_add(1, Ordering::Relaxed);
    }

    /// Registers one in-flight derivation and returns its cancellation token.
    /// A previous in-flight derivation of this owner is cancelled: a
    /// superseded request must not publish, and its native result is
    /// discarded instead of racing the newer identity.
    async fn begin_derivation(&self, key: &OwnerKey) -> DerivationSignal {
        self.touch_entry(key, |entry| {
            if let Some(previous) = entry.inflight.take() {
                previous.cancel();
            }
            let signal = DerivationSignal::start();
            entry.inflight = Some(signal.clone());
            signal
        })
        .await
    }

    /// Clears the in-flight slot when this derivation leaves without
    /// publishing, so a cancelled slot never lingers on the entry.
    async fn end_derivation(&self, key: &OwnerKey, signal: &DerivationSignal) {
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
    async fn intent_superseded(&self, key: &OwnerKey, identity: &PreviewIdentity) -> bool {
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
    async fn admit(
        &self,
        key: &OwnerKey,
        photo_id: &str,
        stage: &'static str,
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
        if coalesced {
            return RenderAdmission::Running;
        }
        let admission = self.render_gate.admit(PreviewRenderRequest {
            photo_id,
            stage,
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
    async fn settle_render(
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
    async fn publish_if_current<F, Fut>(
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
    async fn confirm_publication<F, Fut>(
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

// ---------------------------------------------------------------- handler

/// Classifies one Photo's source class through the Edit Recipe surface's
/// support derivation: the closed contract states (`supported`,
/// `unavailable`, `unsupported`) with one closed `supportReason`.
fn classify_support(
    photo: &slipstream_core::PhotoRead,
    metadata: &slipstream_core::CaptureReviewMetadata,
    read: &EditRecipeRead,
) -> crate::edit_recipe::SupportClassification {
    crate::edit_recipe::derive_support(
        crate::edit_recipe::source_facts(photo, metadata),
        read.source_available,
        photo.original_available,
    )
}

/// `GET /api/photos/{id}/edit-preview/{stage}`
pub(crate) async fn get_edit_preview(
    State(state): State<HttpState>,
    axum::extract::Path((photo_id, stage)): axum::extract::Path<(String, String)>,
    request: Request<Body>,
) -> Response<Body> {
    if request.headers().contains_key(CLI_CONTRACT_HEADER)
        && let Err(response) = require_cli_contract(&request)
    {
        return *response;
    }
    if let Err(response) = require_published(&state.application) {
        return *response;
    }
    let Some(stage) = closed_stage(&stage) else {
        return stage_outside_closed_set();
    };
    if !valid_id(&photo_id) {
        return unknown_photo(&photo_id);
    }
    // One serialized recipe read owns every identity fact, so a rescan
    // between reads cannot mix an old recipe with newer source facts.
    let (photo, metadata, read) = match crate::edit_recipe::load_facts(&state, &photo_id).await {
        Ok(facts) => facts,
        Err(response) => return response,
    };
    if let Some(response) = support_refusal(&photo, &metadata, &read, stage) {
        return response;
    }
    if let Err(response) = develop_executable(&state, stage, &read) {
        return *response;
    }
    serve_preview(&state, &photo_id, stage, &read).await
}

/// The support refusal of one Photo, if its class or facts refuse the route.
/// The deployment's processing enablement is the capability boundary and is
/// checked separately by `develop_executable`.
fn support_refusal(
    photo: &slipstream_core::PhotoRead,
    metadata: &slipstream_core::CaptureReviewMetadata,
    read: &EditRecipeRead,
    stage: &'static str,
) -> Option<Response<Body>> {
    let support = classify_support(photo, metadata, read);
    match support.state {
        "unsupported" => Some(unsupported_photo(&photo.id)),
        "unavailable" => Some(resource_unavailable(
            stage,
            support.reason.unwrap_or("original-missing"),
        )),
        _ => None,
    }
}

/// The closed stage set of the route.
fn closed_stage(stage: &str) -> Option<&'static str> {
    CLOSED_STAGES
        .iter()
        .copied()
        .find(|closed| *closed == stage)
}

/// The develop stage executes only when the deployment admits processing and
/// the stored recipe is representable by the qualified execution payload.
fn develop_executable(
    state: &HttpState,
    stage: &'static str,
    read: &EditRecipeRead,
) -> Result<(), Box<Response<Body>>> {
    let Some(_config) = state.processing.as_ref() else {
        return Err(Box::new(processing_unavailable(stage, "operator-disabled")));
    };
    if let Some(recipe) = read.recipe.as_ref() {
        let milli = recipe.settings.exposure_ev * 1000.0;
        let rounded = milli.round();
        let representable = milli.is_finite()
            && (milli - rounded).abs() < 1e-6
            && rounded >= APPROVED_EXPOSURE_MILLI_EV_MIN as f64
            && rounded <= APPROVED_EXPOSURE_MILLI_EV_MAX as f64;
        if !representable {
            return Err(Box::new(processing_unavailable(
                stage,
                "recipe-not-representable",
            )));
        }
    }
    Ok(())
}

/// Serves one Edit Preview: the current rendition, the derivation of a
/// retained Development Result, or the render admission.
async fn serve_preview(
    state: &HttpState,
    photo_id: &str,
    stage: &'static str,
    read: &EditRecipeRead,
) -> Response<Body> {
    let owner = &state.edit_preview;
    let key = (photo_id.to_owned(), stage);
    let retained = owner.retention.resolve(photo_id);
    let facts = current_facts(state, stage, read);
    let identity = PreviewIdentity::build(&facts, retained.as_ref());
    if let Some(rendition) = owner.current(&key, &identity).await {
        return rendition_response(photo_id, &rendition);
    }
    // The admission path must stay reachable while another request's
    // derivation runs, so a newer intent can supersede and cancel in-flight
    // work without waiting behind the native conversion. Only the derive
    // path — a retained result of exactly the current identity — takes the
    // per-owner derive permit and the instance-wide conversion permit.
    if !retained
        .as_ref()
        .is_some_and(|record| record.matches_facts(&facts))
    {
        return admit_render(owner, &key, photo_id, stage, &identity, SystemTime::now()).await;
    }
    // Serialize derive-and-publish per owner: a concurrent request with the
    // same full identity coalesces onto the first derivation.
    let permit = owner.derive_permit(&key).await;
    let _guard = permit.lock().await;
    if let Some(rendition) = owner.current(&key, &identity).await {
        return rendition_response(photo_id, &rendition);
    }
    // The retention may have moved while this request waited for the permit.
    let retained = owner.retention.resolve(photo_id);
    let Some(record) = retained.filter(|record| record.matches_facts(&facts)) else {
        return admit_render(owner, &key, photo_id, stage, &identity, SystemTime::now()).await;
    };
    // A newer intent registered while this request waited for the mutex
    // supersedes it: refuse without starting any conversion.
    if owner.intent_superseded(&key, &identity).await {
        return superseded_during_derivation(owner, &key, photo_id, stage, &identity).await;
    }
    // Register the intent before queuing for the heavy conversion: a newer
    // admission can then cancel this signal while it waits, and the queued
    // request discovers the supersession before any native work starts.
    let signal = owner.begin_derivation(&key).await;
    // One heavy native conversion at a time, instance-wide. The wait is
    // observable: cancellation wakes the queued request, and the wait is
    // bounded, so a stuck conversion cannot hold a request forever.
    let acquisition = owner.derivation_permit();
    // Biased so a cancellation that is already resolved always wins the
    // poll over an acquisition that completed in the same wake: superseded
    // intent never starts work.
    let _derivation_permit = tokio::select! {
        biased;
        _ = signal.cancelled() => {
            owner.end_derivation(&key, &signal).await;
            return superseded_during_derivation(owner, &key, photo_id, stage, &identity).await;
        }
        _ = tokio::time::sleep(owner.derivation_queue_bound()) => {
            owner.end_derivation(&key, &signal).await;
            // The bounded wait fell through to the admission path: the
            // stage is reported as queued render work instead of hanging
            // on the slot.
            return admit_render(owner, &key, photo_id, stage, &identity, SystemTime::now())
                .await;
        }
        permit = acquisition => permit,
    };
    owner.note_derivation_started();
    let derived = derive_development_display(
        record,
        slipstream_core::DerivativeTarget::DevelopmentPreview1224,
        signal.token(),
    )
    .await;
    let derivative = match derived {
        Ok(Some(derivative)) => derivative,
        Ok(None) => {
            return superseded_during_derivation(owner, &key, photo_id, stage, &identity).await;
        }
        Err(error) => return derivative_error(stage, error),
    };
    if signal.is_cancelled() {
        return superseded_during_derivation(owner, &key, photo_id, stage, &identity).await;
    }
    let sha256 = hex(Sha256::digest(&derivative.jpeg).as_slice());
    match owner
        .publish_if_current(
            &key,
            &identity,
            || async { fresh_identity(state, photo_id, stage).await.ok() },
            DerivedRendition {
                bytes: axum::body::Bytes::from(derivative.jpeg),
                sha256,
                width: derivative.width,
                height: derivative.height,
            },
        )
        .await
    {
        PublishOutcome::Published(rendition) => {
            // Acceptance is ordered against persistence: a second serialized
            // facts read runs after the store, and a save that committed
            // before it evicts the rendition, so a stale rendition is never
            // knowingly served. A save committing after that read is the
            // same point-in-time skew every read response in the service
            // has, and the next request serves the newer identity.
            if owner
                .confirm_publication(&key, &identity, || async {
                    fresh_identity(state, photo_id, stage).await.ok()
                })
                .await
            {
                rendition_response(photo_id, &rendition)
            } else {
                resource_unavailable(stage, "preview-superseded")
            }
        }
        PublishOutcome::Superseded => resource_unavailable(stage, "preview-superseded"),
    }
}

/// One native display conversion of a retained result, cancellable: a token
/// set before the conversion skips the native call entirely, and a token set
/// during it discards the finished result. The conversion itself is one
/// opaque FFI call and cannot be interrupted once started.
async fn derive_development_display(
    record: RetainedDevelopmentResult,
    target: slipstream_core::DerivativeTarget,
    cancelled: Arc<AtomicBool>,
) -> Result<Option<slipstream_core::Derivative>, slipstream_core::DerivativeError> {
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let path = record.path;
    let derived = tokio::task::spawn_blocking(move || {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        std::fs::File::open(&path)
            .map_err(|_| slipstream_core::DerivativeError::Internal)
            .and_then(|file| process_development_tiff(file.as_raw_fd(), target).map(Some))
    })
    .await;
    match derived {
        Ok(result) => result,
        Err(_) => Err(slipstream_core::DerivativeError::Internal),
    }
}

/// A derivation whose identity was superseded while it ran: its admission is
/// cancelled, and the request is refused instead of served stale.
async fn superseded_during_derivation(
    owner: &EditPreviewOwner,
    key: &OwnerKey,
    photo_id: &str,
    stage: &'static str,
    identity: &PreviewIdentity,
) -> Response<Body> {
    owner
        .settle_render(key, photo_id, stage, identity, RenderSettlement::Cancelled)
        .await;
    resource_unavailable(stage, "preview-superseded")
}

/// The fresh full identity of one Photo owner, re-read through the same
/// serialized gates as the first pass.
async fn fresh_identity(
    state: &HttpState,
    photo_id: &str,
    stage: &'static str,
) -> Result<PreviewIdentity, Response<Body>> {
    let (photo, metadata, read) = match crate::edit_recipe::load_facts(state, photo_id).await {
        Ok(facts) => facts,
        Err(response) => return Err(response),
    };
    if let Some(response) = support_refusal(&photo, &metadata, &read, stage) {
        return Err(response);
    }
    develop_executable(state, stage, &read).map_err(|response| *response)?;
    let facts = current_facts(state, stage, &read);
    let retained = state.edit_preview.retention.resolve(photo_id);
    Ok(PreviewIdentity::build(&facts, retained.as_ref()))
}

/// Admits one preview-class render when no current rendition or usable
/// retained result exists.
async fn admit_render(
    owner: &EditPreviewOwner,
    key: &OwnerKey,
    photo_id: &str,
    stage: &'static str,
    identity: &PreviewIdentity,
    now: SystemTime,
) -> Response<Body> {
    match owner.admit(key, photo_id, stage, identity, now).await {
        RenderAdmission::Queued => admitted(stage, "queued"),
        RenderAdmission::Running => admitted(stage, "running"),
        RenderAdmission::Indeterminate => outcome_unknown(stage),
        RenderAdmission::Unavailable(reason) => processing_unavailable(stage, reason.reason()),
    }
}

/// The current identity facts of one Photo owner: the source revision and
/// recipe facts of the serialized read, the observed bundle, the stage, the
/// qualified preview geometry, and the pinned display-transform identity.
fn current_facts(state: &HttpState, stage: &'static str, read: &EditRecipeRead) -> PreviewFacts {
    let bundle_sha256 = state
        .processing
        .as_ref()
        .map(|config: &ProcessingConfig| config.bundle_sha256.clone())
        .unwrap_or_else(|| "disabled".to_owned());
    let (recipe_revision, exposure_milli_ev) = match read.recipe.as_ref() {
        // No saved recipe is the processing baseline: 0 EV and as-shot.
        None => (None, 0),
        Some(recipe) => (
            Some(recipe.revision.clone()),
            milli_ev(recipe.settings.exposure_ev),
        ),
    };
    PreviewFacts {
        stage,
        long_edge: DEVELOPMENT_PREVIEW_LONG_EDGE,
        display_transform: DISPLAY_TRANSFORM_VERSION,
        bundle_sha256,
        source_revision: read.current_source_revision.clone(),
        recipe_revision,
        exposure_milli_ev,
        white_balance: WHITE_BALANCE_AS_SHOT,
    }
}

/// `photoId`, `stage`, `contentType`, `width`, `height`, `byteLength`,
/// `sha256`, `sourceRevision`, `recipeVersion`, `displayTransform`, and
/// `expiresAt` travel as the response headers, and the stream follows them.
fn rendition_response(photo_id: &str, rendition: &PublishedRendition) -> Response<Body> {
    let facts = &rendition.identity.facts;
    Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, "image/jpeg")
        .header(
            axum::http::header::CONTENT_LENGTH,
            rendition.bytes.len().to_string(),
        )
        .header(axum::http::header::CACHE_CONTROL, "no-store")
        .header("x-content-type-options", "nosniff")
        .header("slipstream-edit-preview-photo-id", photo_id)
        .header("slipstream-edit-preview-stage", facts.stage)
        .header("slipstream-edit-preview-width", rendition.width.to_string())
        .header(
            "slipstream-edit-preview-height",
            rendition.height.to_string(),
        )
        .header("slipstream-edit-preview-sha256", &rendition.sha256)
        .header(
            "slipstream-edit-preview-source-revision",
            hex_encode(facts.source_revision.as_bytes()),
        )
        .header(
            "slipstream-edit-preview-recipe-version",
            facts.recipe_revision.as_deref().unwrap_or(""),
        )
        .header(
            "slipstream-edit-preview-display-transform",
            facts.display_transform,
        )
        .header(
            "slipstream-edit-preview-expires-at",
            format_time(rendition.expires_at),
        )
        .body(Body::from(rendition.bytes.clone()))
        .expect("valid edit preview response")
}

fn admitted(stage: &'static str, state: &'static str) -> Response<Body> {
    crate::http::json_response(
        StatusCode::ACCEPTED,
        &serde_json::json!({
            "state": state,
            "stage": stage,
        }),
    )
}

// ---------------------------------------------------------------- refusals

// The closed refusal set of the route: 404 `unknown_photo`, 422
// `invalid_settings` for a stage outside the closed set, 422
// `unsupported_photo`, 503 `processing_unavailable`, 503
// `resource_unavailable`, and 500 `outcome_unknown`.

fn unknown_photo(photo_id: &str) -> Response<Body> {
    cli_error(
        StatusCode::NOT_FOUND,
        "unknown_photo",
        "Query Photos and use a current Photo ID.",
        serde_json::json!({"resource": "photo", "reference": photo_id}),
    )
}

fn stage_outside_closed_set() -> Response<Body> {
    cli_error(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_settings",
        "The requested stage is outside the closed stage set.",
        serde_json::json!({
            "argument": "stage",
            "reason": "stage-outside-closed-set",
            "closedSet": CLOSED_STAGES,
        }),
    )
}

fn unsupported_photo(photo_id: &str) -> Response<Body> {
    cli_error(
        StatusCode::UNPROCESSABLE_ENTITY,
        "unsupported_photo",
        "This Photo's source class has no approved profile.",
        serde_json::json!({"photoId": photo_id, "stage": CLOSED_STAGES[0]}),
    )
}

fn processing_unavailable(stage: &'static str, reason: &'static str) -> Response<Body> {
    cli_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "processing_unavailable",
        "The develop stage cannot execute for this Photo right now.",
        serde_json::json!({"stage": stage, "reason": reason}),
    )
}

fn resource_unavailable(stage: &'static str, reason: &'static str) -> Response<Body> {
    cli_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "resource_unavailable",
        "The service lacks the facts or capacity to serve this preview.",
        serde_json::json!({"stage": stage, "reason": reason}),
    )
}

fn outcome_unknown(stage: &'static str) -> Response<Body> {
    cli_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "outcome_unknown",
        "The render admission outcome is unknown; request the preview again.",
        serde_json::json!({"stage": stage}),
    )
}

fn derivative_error(
    stage: &'static str,
    error: slipstream_core::DerivativeError,
) -> Response<Body> {
    match error {
        slipstream_core::DerivativeError::ResourceLimit
        | slipstream_core::DerivativeError::OutputLimit => {
            resource_unavailable(stage, "derivative-resource-limit")
        }
        // A retained result the display derivative cannot decode is a
        // processing failure of the stage, not a resource refusal.
        slipstream_core::DerivativeError::Unsupported
        | slipstream_core::DerivativeError::Malformed => {
            processing_unavailable(stage, "development-result-unusable")
        }
        slipstream_core::DerivativeError::Internal => {
            processing_unavailable(stage, "derivative-internal")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    fn facts(exposure_milli_ev: i64) -> PreviewFacts {
        PreviewFacts {
            stage: "develop",
            long_edge: DEVELOPMENT_PREVIEW_LONG_EDGE,
            display_transform: DISPLAY_TRANSFORM_VERSION,
            bundle_sha256: "c".repeat(64),
            source_revision: "source".to_owned(),
            recipe_revision: Some("recipe-1".to_owned()),
            exposure_milli_ev,
            white_balance: WHITE_BALANCE_AS_SHOT,
        }
    }

    fn record(exposure_milli_ev: i64) -> RetainedDevelopmentResult {
        RetainedDevelopmentResult {
            sha256: "d".repeat(64),
            byte_length: 42,
            recipe_revision: Some("recipe-1".to_owned()),
            exposure_milli_ev,
            white_balance: WHITE_BALANCE_AS_SHOT,
            source_revision: "source".to_owned(),
            bundle_sha256: "c".repeat(64),
            path: PathBuf::from("/tmp/unused.tiff"),
        }
    }

    fn identity(exposure_milli_ev: i64) -> PreviewIdentity {
        PreviewIdentity::build(&facts(exposure_milli_ev), Some(&record(exposure_milli_ev)))
    }

    /// A gate whose admissions and settlements a test scripts in order.
    struct ScriptedGate {
        admissions: Mutex<Vec<RenderAdmission>>,
        settlements: Mutex<Vec<RenderSettlement>>,
    }

    impl ScriptedGate {
        fn queued(times: usize) -> Arc<Self> {
            Arc::new(Self {
                admissions: Mutex::new(vec![RenderAdmission::Queued; times]),
                settlements: Mutex::new(Vec::new()),
            })
        }
    }

    impl PreviewRenderGate for ScriptedGate {
        fn admit(&self, _request: PreviewRenderRequest<'_>) -> RenderAdmission {
            self.admissions
                .lock()
                .expect("scripted gate poisoned")
                .pop()
                .expect("admission script exhausted")
        }

        fn settle(&self, _request: PreviewRenderRequest<'_>, settlement: RenderSettlement) {
            self.settlements
                .lock()
                .expect("scripted gate poisoned")
                .push(settlement);
        }
    }

    fn scripted_identity(source: &str, exposure_milli_ev: i64) -> PreviewIdentity {
        let mut varied = facts(exposure_milli_ev);
        varied.source_revision = source.to_owned();
        PreviewIdentity::build(&varied, None)
    }

    async fn publish(
        owner: &EditPreviewOwner,
        key: &OwnerKey,
        identity: &PreviewIdentity,
        current: Option<PreviewIdentity>,
    ) -> PublishOutcome {
        owner
            .publish_if_current(
                key,
                identity,
                || std::future::ready(current.clone()),
                DerivedRendition {
                    bytes: axum::body::Bytes::from_static(b"jpeg"),
                    sha256: "a".repeat(64),
                    width: 64,
                    height: 64,
                },
            )
            .await
    }

    #[test]
    fn identity_changes_with_every_identity_fact() {
        let base = identity(250);
        let mut transform = facts(250);
        transform.display_transform = "display-transform-v2";
        assert_ne!(base, PreviewIdentity::build(&transform, Some(&record(250))));
        let mut geometry = facts(250);
        geometry.long_edge = 512;
        assert_ne!(base, PreviewIdentity::build(&geometry, Some(&record(250))));
        let mut bundle = facts(250);
        bundle.bundle_sha256 = "e".repeat(64);
        assert_ne!(base, PreviewIdentity::build(&bundle, Some(&record(250))));
        let mut source = facts(250);
        source.source_revision = "changed".to_owned();
        assert_ne!(base, PreviewIdentity::build(&source, Some(&record(250))));
        let mut revision = facts(250);
        revision.recipe_revision = Some("recipe-2".to_owned());
        assert_ne!(base, PreviewIdentity::build(&revision, Some(&record(250))));
        assert_ne!(base, identity(500));
        // A stale record contributes no evidence, so the identity falls back
        // to not-retained and can never equal a current-evidence identity.
        assert_ne!(
            base,
            PreviewIdentity::build(&facts(500), Some(&record(250)))
        );
        assert_eq!(
            PreviewIdentity::build(&facts(500), Some(&record(250))),
            PreviewIdentity::build(&facts(500), None)
        );
    }

    #[tokio::test]
    async fn owner_serves_only_the_current_full_identity() {
        let owner = EditPreviewOwner::production();
        let key = ("photo".to_owned(), "develop");
        let first = identity(250);
        assert!(matches!(
            publish(&owner, &key, &first, Some(first.clone())).await,
            PublishOutcome::Published(_)
        ));
        assert!(owner.current(&key, &first).await.is_some());
        // A different display transform is not current.
        let mut transform = facts(250);
        transform.display_transform = "display-transform-v2";
        let transformed = PreviewIdentity::build(&transform, Some(&record(250)));
        assert!(owner.current(&key, &transformed).await.is_none());
    }

    #[tokio::test]
    async fn pending_intents_coalesce_and_follow_latest_intent() {
        let gate = ScriptedGate::queued(2);
        let gate_dyn: Arc<dyn PreviewRenderGate> = gate.clone();
        let owner = EditPreviewOwner::new(Arc::new(UnlandedRetention), gate_dyn);
        let key = ("photo".to_owned(), "develop");
        let now = SystemTime::now();
        let first = identity(250);
        let second = identity(500);
        assert_eq!(
            owner.admit(&key, "photo", "develop", &first, now).await,
            RenderAdmission::Queued
        );
        // The same full identity coalesces without a second admission.
        assert_eq!(
            owner.admit(&key, "photo", "develop", &first, now).await,
            RenderAdmission::Running
        );
        // A changed identity supersedes the pending intent and cancels the
        // superseded derivation in flight.
        assert_eq!(
            owner.admit(&key, "photo", "develop", &second, now).await,
            RenderAdmission::Queued
        );
        assert_eq!(
            gate.admissions.lock().unwrap().len(),
            0,
            "exactly two admissions reached the gate"
        );
        // A settled rendition clears the pending slot.
        assert!(matches!(
            publish(&owner, &key, &second, Some(second.clone())).await,
            PublishOutcome::Published(_)
        ));
        assert!(owner.current(&key, &second).await.is_some());
    }

    #[tokio::test]
    async fn a_failed_or_lost_render_is_retried_instead_of_running_forever() {
        let gate = ScriptedGate::queued(3);
        let gate_dyn: Arc<dyn PreviewRenderGate> = gate.clone();
        let owner = EditPreviewOwner::new(Arc::new(UnlandedRetention), gate_dyn);
        let key = ("photo".to_owned(), "develop");
        let identity = identity(250);
        let now = SystemTime::now();
        assert_eq!(
            owner.admit(&key, "photo", "develop", &identity, now).await,
            RenderAdmission::Queued
        );
        // An explicit failure settles the intent, and the next request is a
        // new admission instead of coalescing into the dead one.
        owner
            .settle_render(
                &key,
                "photo",
                "develop",
                &identity,
                RenderSettlement::Failed,
            )
            .await;
        assert_eq!(
            owner.admit(&key, "photo", "develop", &identity, now).await,
            RenderAdmission::Queued,
            "a failed render frees its identity for retry"
        );
        // A lost receipt — an intent that never settles — frees its identity
        // once its patience expires instead of answering running forever.
        assert_eq!(
            owner
                .admit(
                    &key,
                    "photo",
                    "develop",
                    &identity,
                    now + PENDING_TTL - Duration::from_secs(1)
                )
                .await,
            RenderAdmission::Running,
            "an intent inside its patience still coalesces"
        );
        assert_eq!(
            owner
                .admit(
                    &key,
                    "photo",
                    "develop",
                    &identity,
                    now + PENDING_TTL + Duration::from_secs(1)
                )
                .await,
            RenderAdmission::Queued,
            "an expired intent is re-admitted"
        );
        assert_eq!(gate.settlements.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn publication_is_conditional_on_the_identity_current_at_publish_time() {
        let owner = EditPreviewOwner::production();
        let key = ("photo".to_owned(), "develop");
        let derived = identity(250);
        let newer = identity(500);
        // The late completion of a superseded request never publishes: the
        // final serialized read returns the newer identity.
        assert!(matches!(
            publish(&owner, &key, &derived, Some(newer.clone())).await,
            PublishOutcome::Superseded
        ));
        assert!(owner.current(&key, &derived).await.is_none());
        // Unreadable fresh facts refuse too.
        assert!(matches!(
            publish(&owner, &key, &derived, None).await,
            PublishOutcome::Superseded
        ));
        // The current identity publishes and serves.
        assert!(matches!(
            publish(&owner, &key, &derived, Some(derived.clone())).await,
            PublishOutcome::Published(_)
        ));
        assert!(owner.current(&key, &derived).await.is_some());
    }

    #[tokio::test]
    async fn publication_is_confirmed_against_persistence_after_publishing() {
        let owner = EditPreviewOwner::production();
        let key = ("photo".to_owned(), "develop");
        let published = identity(250);
        let newer = identity(500);
        assert!(matches!(
            publish(&owner, &key, &published, Some(published.clone())).await,
            PublishOutcome::Published(_)
        ));
        // The acceptance read names a newer identity: the rendition is
        // evicted and the publication is not confirmed.
        assert!(
            !owner
                .confirm_publication(&key, &published, {
                    let newer = newer.clone();
                    move || std::future::ready(Some(newer.clone()))
                })
                .await
        );
        assert!(owner.current(&key, &published).await.is_none());
        // The acceptance read still naming the identity confirms it.
        assert!(matches!(
            publish(&owner, &key, &published, Some(published.clone())).await,
            PublishOutcome::Published(_)
        ));
        assert!(
            owner
                .confirm_publication(&key, &published, {
                    let published = published.clone();
                    move || std::future::ready(Some(published.clone()))
                })
                .await
        );
        assert!(owner.current(&key, &published).await.is_some());
        // An unreadable acceptance read refuses instead of serving.
        assert!(
            !owner
                .confirm_publication(&key, &published, || std::future::ready(None))
                .await
        );
        assert!(owner.current(&key, &published).await.is_none());
    }

    #[tokio::test]
    async fn evicting_a_pending_owner_settles_its_admission_as_cancelled() {
        let gate = ScriptedGate::queued(1);
        let gate_dyn: Arc<dyn PreviewRenderGate> = gate.clone();
        let owner = EditPreviewOwner::new(Arc::new(UnlandedRetention), gate_dyn);
        let key = ("photo".to_owned(), "develop");
        let admitted = identity(250);
        assert_eq!(
            owner
                .admit(&key, "photo", "develop", &admitted, SystemTime::now())
                .await,
            RenderAdmission::Queued
        );
        // Pressure the bound: the pending owner is the least recently
        // touched entry, so it is the eviction victim.
        for index in 0..MAXIMUM_OWNERS {
            let filling_key = (format!("photo-{index}"), "develop");
            let filling_identity = scripted_identity(&format!("source-{index}"), 250);
            let current = filling_identity.clone();
            publish(&owner, &filling_key, &filling_identity, Some(current)).await;
        }
        assert!(owner.owner_count().await <= MAXIMUM_OWNERS);
        let settlements = gate.settlements.lock().unwrap();
        assert_eq!(settlements.len(), 1, "the evicted admission settles once");
        assert!(matches!(settlements[0], RenderSettlement::Cancelled));
    }

    #[tokio::test]
    async fn owner_eviction_releases_the_derive_permit() {
        let owner = EditPreviewOwner::production();
        let key = ("photo".to_owned(), "develop");
        let permit = owner.derive_permit(&key).await;
        assert_eq!(Arc::strong_count(&permit), 2, "entry and test hold it");
        for index in 0..(MAXIMUM_OWNERS + 10) {
            let evicting_key = (format!("photo-{index}"), "develop");
            let evicting_identity = scripted_identity(&format!("source-{index}"), 250);
            let current = evicting_identity.clone();
            publish(&owner, &evicting_key, &evicting_identity, Some(current)).await;
        }
        assert!(owner.owner_count().await <= MAXIMUM_OWNERS);
        assert_eq!(
            Arc::strong_count(&permit),
            1,
            "owner eviction released the entry's permit handle"
        );
    }

    #[tokio::test]
    async fn heavy_derivations_are_bounded_instance_wide() {
        let owner = EditPreviewOwner::production();
        let first = owner.try_derivation_permit();
        assert!(first.is_some(), "the first heavy conversion is admitted");
        assert!(
            owner.try_derivation_permit().is_none(),
            "a second concurrent heavy conversion waits for the instance bound"
        );
        drop(first);
        assert!(owner.try_derivation_permit().is_some());
    }

    #[tokio::test]
    async fn owners_are_bounded_and_evict_the_least_recently_touched() {
        let owner = EditPreviewOwner::production();
        for index in 0..(MAXIMUM_OWNERS + 100) {
            let key = (format!("photo-{index}"), "develop");
            let identity = scripted_identity(&format!("source-{index}"), 250);
            let current = identity.clone();
            publish(&owner, &key, &identity, Some(current)).await;
        }
        assert!(
            owner.owner_count().await <= MAXIMUM_OWNERS,
            "the retained owners never grow past the bound"
        );
        // The most recently touched owner survives; the earliest was evicted.
        let latest = (format!("photo-{}", MAXIMUM_OWNERS + 99), "develop");
        let latest_identity = scripted_identity(&format!("source-{}", MAXIMUM_OWNERS + 99), 250);
        assert!(owner.current(&latest, &latest_identity).await.is_some());
        let earliest = ("photo-0".to_owned(), "develop");
        let earliest_identity = scripted_identity("source-0", 250);
        assert!(owner.current(&earliest, &earliest_identity).await.is_none());
    }

    #[tokio::test]
    async fn a_superseded_derivation_is_cancelled_and_never_published() {
        let gate = ScriptedGate::queued(1);
        let owner = EditPreviewOwner::new(Arc::new(UnlandedRetention), gate);
        let key = ("photo".to_owned(), "develop");
        let signal = owner.begin_derivation(&key).await;
        assert!(!signal.is_cancelled());
        // A newer identity supersedes the in-flight derivation.
        let _admitted = owner
            .admit(
                &key,
                "photo",
                "develop",
                &scripted_identity("newer", 500),
                SystemTime::now(),
            )
            .await;
        assert!(
            signal.is_cancelled(),
            "the superseded derivation is cancelled"
        );
        // A cancelled conversion skips the native call entirely: the record
        // points at a path that does not exist, and the result is still a
        // clean cancellation, not an open failure.
        let cancelled_record = RetainedDevelopmentResult {
            path: PathBuf::from("/nonexistent/development-result.tif"),
            ..record(250)
        };
        assert!(matches!(
            derive_development_display(
                cancelled_record,
                slipstream_core::DerivativeTarget::DevelopmentPreview1224,
                signal.token(),
            )
            .await,
            Ok(None)
        ));
    }
}
