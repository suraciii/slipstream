use super::*;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

fn facts(exposure_milli_ev: i64) -> PreviewFacts {
    PreviewFacts {
        stage: "develop",
        settings: SETTINGS_CURRENT,
        long_edge: DEVELOPMENT_PREVIEW_LONG_EDGE,
        display_transform: DISPLAY_TRANSFORM_VERSION,
        bundle_sha256: "c".repeat(64),
        source_revision: "source".to_owned(),
        recipe_revision: Some("recipe-1".to_owned()),
        exposure_milli_ev,
        white_balance: WHITE_BALANCE_AS_SHOT,
        source: "original",
        proxy_id: None,
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
        width: 1,
        height: 1,
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
    let owner = EditPreviewOwner::production(None);
    let key = ("photo".to_owned(), "develop", SETTINGS_CURRENT);
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
    let key = ("photo".to_owned(), "develop", SETTINGS_CURRENT);
    let now = SystemTime::now();
    let first = identity(250);
    let second = identity(500);
    assert_eq!(
        owner
            .admit(&key, "photo", "develop", SETTINGS_CURRENT, &first, now)
            .await,
        RenderAdmission::Queued
    );
    // The same full identity coalesces without a second admission.
    assert_eq!(
        owner
            .admit(&key, "photo", "develop", SETTINGS_CURRENT, &first, now)
            .await,
        RenderAdmission::Running
    );
    // A changed identity supersedes the pending intent and cancels the
    // superseded derivation in flight.
    assert_eq!(
        owner
            .admit(&key, "photo", "develop", SETTINGS_CURRENT, &second, now)
            .await,
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
    let key = ("photo".to_owned(), "develop", SETTINGS_CURRENT);
    let identity = identity(250);
    let now = SystemTime::now();
    assert_eq!(
        owner
            .admit(&key, "photo", "develop", SETTINGS_CURRENT, &identity, now)
            .await,
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
        owner
            .admit(&key, "photo", "develop", SETTINGS_CURRENT, &identity, now)
            .await,
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
                SETTINGS_CURRENT,
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
                SETTINGS_CURRENT,
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
    let owner = EditPreviewOwner::production(None);
    let key = ("photo".to_owned(), "develop", SETTINGS_CURRENT);
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
    let owner = EditPreviewOwner::production(None);
    let key = ("photo".to_owned(), "develop", SETTINGS_CURRENT);
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
    let key = ("photo".to_owned(), "develop", SETTINGS_CURRENT);
    let admitted = identity(250);
    assert_eq!(
        owner
            .admit(
                &key,
                "photo",
                "develop",
                SETTINGS_CURRENT,
                &admitted,
                SystemTime::now()
            )
            .await,
        RenderAdmission::Queued
    );
    // Pressure the bound: the pending owner is the least recently
    // touched entry, so it is the eviction victim.
    for index in 0..MAXIMUM_OWNERS {
        let filling_key = (format!("photo-{index}"), "develop", SETTINGS_CURRENT);
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
    let owner = EditPreviewOwner::production(None);
    let key = ("photo".to_owned(), "develop", SETTINGS_CURRENT);
    let permit = owner.derive_permit(&key).await;
    assert_eq!(Arc::strong_count(&permit), 2, "entry and test hold it");
    for index in 0..(MAXIMUM_OWNERS + 10) {
        let evicting_key = (format!("photo-{index}"), "develop", SETTINGS_CURRENT);
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
    let owner = EditPreviewOwner::production(None);
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
    let owner = EditPreviewOwner::production(None);
    for index in 0..(MAXIMUM_OWNERS + 100) {
        let key = (format!("photo-{index}"), "develop", SETTINGS_CURRENT);
        let identity = scripted_identity(&format!("source-{index}"), 250);
        let current = identity.clone();
        publish(&owner, &key, &identity, Some(current)).await;
    }
    assert!(
        owner.owner_count().await <= MAXIMUM_OWNERS,
        "the retained owners never grow past the bound"
    );
    // The most recently touched owner survives; the earliest was evicted.
    let latest = (
        format!("photo-{}", MAXIMUM_OWNERS + 99),
        "develop",
        SETTINGS_CURRENT,
    );
    let latest_identity = scripted_identity(&format!("source-{}", MAXIMUM_OWNERS + 99), 250);
    assert!(owner.current(&latest, &latest_identity).await.is_some());
    let earliest = ("photo-0".to_owned(), "develop", SETTINGS_CURRENT);
    let earliest_identity = scripted_identity("source-0", 250);
    assert!(owner.current(&earliest, &earliest_identity).await.is_none());
}

#[tokio::test]
async fn a_superseded_derivation_is_cancelled_and_never_published() {
    let gate = ScriptedGate::queued(1);
    let owner = EditPreviewOwner::new(Arc::new(UnlandedRetention), gate);
    let key = ("photo".to_owned(), "develop", SETTINGS_CURRENT);
    let signal = owner
        .begin_derivation(&key, &scripted_identity("source", 250))
        .await;
    assert!(!signal.is_cancelled());
    // A newer identity supersedes the in-flight derivation.
    let _admitted = owner
        .admit(
            &key,
            "photo",
            "develop",
            SETTINGS_CURRENT,
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
        derive_preview_display(
            cancelled_record,
            "develop",
            slipstream_core::DerivativeTarget::DevelopmentPreview1224,
            250,
            false,
            signal.token(),
        )
        .await,
        Ok(None)
    ));
}
