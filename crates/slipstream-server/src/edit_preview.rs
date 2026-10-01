//! Edit Preview HTTP surface: one display rendition per Photo and stage.
//! The route derives the rendition through the pinned Development display
//! derivative, frames it as response headers followed by the JPEG stream, and
//! schedules preview work latest-intent-wins within its Photo and stage owner
//! per the Photo Development Surface contract.

mod owner;
mod renders;

pub(crate) use owner::{EditPreviewOwner, PreviewFacts, PublishOutcome, PublishedRendition};
#[allow(unused_imports)]
pub(crate) use renders::{
    DevelopmentResultRetention, PreviewClassRenders, PreviewRenderGate, PreviewRenderRequest,
    RenderAdmission, RenderSettlement, RetainedDevelopmentResult, UnlandedRenderGate,
    UnlandedRetention,
};

use std::{
    os::fd::AsRawFd,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
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

use slipstream_core::{
    DEVELOPMENT_PREVIEW_LONG_EDGE, DISPLAY_TRANSFORM_VERSION, EditRecipeRead, WhiteBalanceIntent,
    process_development_proxy, process_development_tiff,
};

use owner::{DerivedRendition, OwnerKey, PreviewIdentity, hex, milli_ev};

use crate::{
    ProcessingConfig,
    http::{
        CLI_CONTRACT_HEADER, HttpState, cli_error, require_cli_contract, require_published,
        valid_id,
    },
    queries::{format_time, hex_encode},
};

/// The closed stage set of the Edit Preview route.
const CLOSED_STAGES: [&str; 2] = ["develop", "film"];

/// The closed `settings` selector of the route. `current` is the saved Edit
/// Recipe's settings and the default. `baseline` is the as-shot/baseline
/// development settings the comparison presents — the processing baseline of
/// 0 EV against the documented baseline and as-shot white balance — and it is
/// derived from the Photo rather than from the saved recipe.
const CLOSED_SETTINGS: [&str; 2] = [SETTINGS_CURRENT, SETTINGS_BASELINE];
const SETTINGS_CURRENT: &str = "current";
const SETTINGS_BASELINE: &str = "baseline";

/// The disclosed bounded retention of one published rendition. Renditions are
/// rebuildable intermediates, so the bound is short and disclosed through the
/// `expiresAt` response header on every delivery.
const RENDITION_TTL: Duration = Duration::from_secs(300);

/// How often an idle service sweeps expired preview staging. The disclosed
/// retention window governs serving; this bounds how long the private output
/// of an expired rendition outlives that window when no request touches it.
const PREVIEW_SWEEP_PERIOD: Duration = Duration::from_secs(60);

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
pub(crate) const WHITE_BALANCE_AS_SHOT: &str = "as-shot";

// ---------------------------------------------------------------- handler

/// Classifies one Photo's source class through the Edit Recipe surface's
/// support derivation: the closed contract states (`supported`,
/// `unavailable`, `unsupported`) with one closed `supportReason`.
fn classify_support(
    photo: &slipstream_core::PhotoRead,
    read: &EditRecipeRead,
) -> crate::edit_recipe::SupportClassification {
    crate::edit_recipe::derive_support(
        crate::edit_recipe::source_facts(photo),
        read.source_available,
        photo.original_available,
        read.current_source_revision.as_deref(),
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
    let Some(settings) = closed_settings(request.uri().query()) else {
        return settings_outside_closed_set();
    };
    if !valid_id(&photo_id) {
        return unknown_photo(&photo_id);
    }
    let (photo, read) = match crate::edit_recipe::load_facts(&state, &photo_id).await {
        Ok(facts) => facts,
        Err(response) => return response,
    };
    let proxy = if !read.source_available {
        match state.application.proxies.as_ref() {
            Some(manager) => manager.current_artifact(&photo_id).await,
            None => None,
        }
    } else {
        None
    };
    if let Some(response) = support_refusal(&photo, &read, stage, proxy.is_some()) {
        return response;
    }
    if let Err(response) = develop_executable(&state, stage, settings, &read) {
        return *response;
    }
    serve_preview(&state, &photo_id, stage, settings, &read, proxy.as_ref()).await
}

/// The support refusal of one Photo, if its class or facts refuse the route.
/// The deployment's processing enablement is the capability boundary and is
/// checked separately by `develop_executable`.
fn support_refusal(
    photo: &slipstream_core::PhotoRead,
    read: &EditRecipeRead,
    stage: &'static str,
    proxy_current: bool,
) -> Option<Response<Body>> {
    if proxy_current {
        return None;
    }
    let support = classify_support(photo, read);
    match support.state {
        "unsupported" => Some(unsupported_photo(&photo.id, stage)),
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

/// The closed `settings` selector of the route. An absent selector is the
/// saved recipe's settings; `baseline` is the as-shot/baseline development
/// settings the comparison presents. Any other value is outside the closed
/// set and refuses the request rather than serving a rendition the client did
/// not ask for.
fn closed_settings(query: Option<&str>) -> Option<&'static str> {
    let mut selected = SETTINGS_CURRENT;
    for part in query.unwrap_or_default().split('&') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        if key == "settings" {
            selected = CLOSED_SETTINGS
                .iter()
                .copied()
                .find(|closed| *closed == value)?;
        }
    }
    Some(selected)
}

/// The Film stage has no production admission authority. The local engine
/// qualifies only the closed Development workload; admitting Film remains
/// independent qualification work.
fn develop_executable(
    state: &HttpState,
    stage: &'static str,
    settings: &'static str,
    read: &EditRecipeRead,
) -> Result<(), Box<Response<Body>>> {
    let Some(_config) = state.processing.as_ref() else {
        return Err(Box::new(processing_unavailable(stage, "operator-disabled")));
    };
    if stage == "film" {
        return Err(Box::new(processing_unavailable(
            stage,
            "film-not-qualified",
        )));
    }
    if settings == SETTINGS_BASELINE {
        return Ok(());
    }
    if let Some(recipe) = read.recipe.as_ref() {
        let milli = recipe.settings.exposure_ev * 1000.0;
        let rounded = milli.round();
        let representable = milli.is_finite()
            && (milli - rounded).abs() < 1e-6
            && rounded >= APPROVED_EXPOSURE_MILLI_EV_MIN as f64
            && rounded <= APPROVED_EXPOSURE_MILLI_EV_MAX as f64
            // The closed execution payload carries the approved mode only, so
            // a stored mode the capability does not admit is retained intent
            // that no admitted render can execute: it refuses here instead of
            // admitting an attempt that cannot produce a result.
            && recipe.settings.white_balance == WhiteBalanceIntent::AsShot;
        if !representable {
            return Err(Box::new(processing_unavailable(
                stage,
                "recipe-not-representable",
            )));
        }
    }
    Ok(())
}

/// One constructed Preview pass: the proxy-aware current facts, the retained
/// evidence resolved behind them, and the full identity that evidence
/// builds. The initial serve and the fresh post-derivation read share this
/// one construction, so the two passes can never disagree about what the
/// facts mean.
struct ConstructedPreview {
    facts: PreviewFacts,
    proxy_retained: Option<RetainedDevelopmentResult>,
    retained: Option<RetainedDevelopmentResult>,
    identity: PreviewIdentity,
}

/// Builds the proxy-aware facts of one request, resolves the retained
/// evidence behind them, and assembles the full Preview identity. The
/// caller owns when the serialized reads happen; this owns what they mean.
/// A Film request over a proxy never takes retained evidence: its Film
/// rendition must come from a render over that proxy, not from the
/// Development TIFF the proxy already retained.
async fn construct_preview(
    state: &HttpState,
    owner: &EditPreviewOwner,
    photo_id: &str,
    stage: &'static str,
    settings: &'static str,
    read: &EditRecipeRead,
    proxy: Option<&(slipstream_core::DevelopmentProxyRecord, std::path::PathBuf)>,
) -> Result<ConstructedPreview, Response<Body>> {
    let Some(source_revision) = proxy
        .map(|(record, _)| record.source_revision.clone())
        .or_else(|| read.current_source_revision.clone())
    else {
        return Err(resource_unavailable(
            stage,
            crate::edit_recipe::READ_PENDING,
        ));
    };
    let mut facts = current_facts(state, stage, settings, &source_revision, read);
    if let Some((record, _)) = proxy {
        facts.source = "development-proxy";
        facts.proxy_id = Some(record.identity_digest());
        facts.bundle_sha256 = record.bundle_sha256.clone();
    }
    let proxy_retained =
        proxy
            .filter(|_| stage != "film")
            .map(|(record, path)| RetainedDevelopmentResult {
                sha256: record.artifact_sha256.clone(),
                byte_length: record.artifact_bytes,
                recipe_revision: facts.recipe_revision.clone(),
                exposure_milli_ev: facts.exposure_milli_ev,
                white_balance: facts.white_balance,
                source_revision: record.source_revision.clone(),
                bundle_sha256: record.bundle_sha256.clone(),
                path: path.clone(),
                width: record.width,
                height: record.height,
            });
    let retained = owner
        .retention
        .resolve(photo_id, &facts)
        .await
        .or(proxy_retained.clone());
    let identity = PreviewIdentity::build(
        &facts,
        if proxy.is_some() && stage == "film" {
            None
        } else {
            retained.as_ref()
        },
    );
    Ok(ConstructedPreview {
        facts,
        proxy_retained,
        retained,
        identity,
    })
}

/// Serves one Edit Preview: the current rendition, the derivation of a
/// retained Development Result, or the render admission.
async fn serve_preview(
    state: &HttpState,
    photo_id: &str,
    stage: &'static str,
    settings: &'static str,
    read: &EditRecipeRead,
    proxy: Option<&(slipstream_core::DevelopmentProxyRecord, std::path::PathBuf)>,
) -> Response<Body> {
    let owner = &state.edit_preview;
    let key = (photo_id.to_owned(), stage, settings);
    let constructed = construct_preview(state, owner, photo_id, stage, settings, read, proxy).await;
    let preview = match constructed {
        Ok(preview) => preview,
        Err(response) => return response,
    };
    let ConstructedPreview {
        facts,
        proxy_retained,
        retained,
        identity,
    } = preview;
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
        return admit_render(
            owner,
            &key,
            photo_id,
            stage,
            settings,
            &identity,
            SystemTime::now(),
        )
        .await;
    }
    // Serialize derive-and-publish per owner: a concurrent request with the
    // same full identity coalesces onto the first derivation.
    let permit = owner.derive_permit(&key).await;
    let _guard = permit.lock().await;
    if let Some(rendition) = owner.current(&key, &identity).await {
        return rendition_response(photo_id, &rendition);
    }
    // The retention may have moved while this request waited for the permit;
    // a current proxy remains the authoritative retained source offline.
    let retained = owner
        .retention
        .resolve(photo_id, &facts)
        .await
        .or(proxy_retained.clone());
    let Some(record) = retained.filter(|record| record.matches_facts(&facts)) else {
        return admit_render(
            owner,
            &key,
            photo_id,
            stage,
            settings,
            &identity,
            SystemTime::now(),
        )
        .await;
    };
    // A newer intent registered while this request waited for the mutex
    // supersedes it: refuse without starting any conversion.
    if owner.intent_superseded(&key, &identity).await {
        return superseded_during_derivation(owner, &key, photo_id, stage, &identity).await;
    }
    // Register the intent before queuing for the heavy conversion: a newer
    // admission can then cancel this signal while it waits, and the queued
    // request discovers the supersession before any native work starts.
    let signal = owner.begin_derivation(&key, &identity).await;
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
            return admit_render(owner, &key, photo_id, stage, settings, &identity, SystemTime::now())
                .await;
        }
        permit = acquisition => permit,
    };
    owner.note_derivation_started();
    let derived = derive_preview_display(
        record,
        stage,
        slipstream_core::DerivativeTarget::DevelopmentPreview1224,
        facts.exposure_milli_ev,
        proxy.is_some(),
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
            || async { fresh_identity(state, photo_id, stage, settings).await.ok() },
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
                    fresh_identity(state, photo_id, stage, settings).await.ok()
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

/// Reads a retained Film JPEG directly, or converts a retained Development
/// TIFF through the pinned display transform.
async fn derive_preview_display(
    record: RetainedDevelopmentResult,
    stage: &'static str,
    target: slipstream_core::DerivativeTarget,
    exposure_milli_ev: i64,
    proxy_backed: bool,
    cancelled: Arc<AtomicBool>,
) -> Result<Option<slipstream_core::Derivative>, slipstream_core::DerivativeError> {
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let path = record.path;
    let width = record.width;
    let height = record.height;
    let derived = tokio::task::spawn_blocking(move || {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        if stage == "film" {
            if width == 0 || height == 0 {
                return Err(slipstream_core::DerivativeError::Internal);
            }
            let metadata =
                std::fs::metadata(&path).map_err(|_| slipstream_core::DerivativeError::Internal)?;
            if metadata.len() > 128 * 1024 * 1024 {
                return Err(slipstream_core::DerivativeError::OutputLimit);
            }
            let jpeg =
                std::fs::read(&path).map_err(|_| slipstream_core::DerivativeError::Internal)?;
            return Ok(Some(slipstream_core::Derivative {
                width,
                height,
                profile: slipstream_core::DerivativeProfile::Srgb,
                jpeg,
            }));
        }
        std::fs::File::open(&path)
            .map_err(|_| slipstream_core::DerivativeError::Internal)
            .and_then(|file| {
                if proxy_backed {
                    process_development_proxy(file.as_raw_fd(), exposure_milli_ev, target).map(Some)
                } else {
                    process_development_tiff(file.as_raw_fd(), target).map(Some)
                }
            })
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
    settings: &'static str,
) -> Result<PreviewIdentity, Response<Body>> {
    let (photo, read) = match crate::edit_recipe::load_facts(state, photo_id).await {
        Ok(facts) => facts,
        Err(response) => return Err(response),
    };
    let proxy = if !read.source_available {
        match state.application.proxies.as_ref() {
            Some(manager) => manager.current_artifact(photo_id).await,
            None => None,
        }
    } else {
        None
    };
    if let Some(response) = support_refusal(&photo, &read, stage, proxy.is_some()) {
        return Err(response);
    }
    develop_executable(state, stage, settings, &read).map_err(|response| *response)?;
    let constructed = construct_preview(
        state,
        &state.edit_preview,
        photo_id,
        stage,
        settings,
        &read,
        proxy.as_ref(),
    )
    .await?;
    Ok(constructed.identity)
}

/// Admits one preview-class render when no current rendition or usable
/// retained result exists.
async fn admit_render(
    owner: &EditPreviewOwner,
    key: &OwnerKey,
    photo_id: &str,
    stage: &'static str,
    settings: &'static str,
    identity: &PreviewIdentity,
    now: SystemTime,
) -> Response<Body> {
    match owner
        .admit(key, photo_id, stage, settings, identity, now)
        .await
    {
        RenderAdmission::Queued => admitted(stage, "queued"),
        RenderAdmission::Running => admitted(stage, "running"),
        RenderAdmission::Indeterminate => outcome_unknown(stage),
        RenderAdmission::Unavailable(reason) => processing_unavailable(stage, reason.reason()),
    }
}

/// The current identity facts of one Photo owner: the source revision and
/// settings facts of the serialized read, the observed bundle, the stage, the
/// qualified preview geometry, and the pinned display-transform identity. The
/// baseline selector names the processing baseline itself — 0 EV against the
/// documented baseline and as-shot white balance — so its facts do not depend
/// on the saved recipe.
fn current_facts(
    state: &HttpState,
    stage: &'static str,
    settings: &'static str,
    source_revision: &str,
    read: &EditRecipeRead,
) -> PreviewFacts {
    let bundle_sha256 = state
        .processing
        .as_ref()
        .map(|config: &ProcessingConfig| config.bundle_sha256.clone())
        .unwrap_or_else(|| "disabled".to_owned());
    let (recipe_revision, exposure_milli_ev) = match read.recipe.as_ref() {
        // The baseline selector and a Photo without a saved recipe both name
        // the processing baseline: 0 EV and as-shot.
        Some(recipe) if settings != SETTINGS_BASELINE => (
            Some(recipe.revision.clone()),
            milli_ev(recipe.settings.exposure_ev),
        ),
        _ => (None, 0),
    };
    PreviewFacts {
        stage,
        settings,
        long_edge: DEVELOPMENT_PREVIEW_LONG_EDGE,
        display_transform: DISPLAY_TRANSFORM_VERSION,
        bundle_sha256,
        source_revision: source_revision.to_owned(),
        recipe_revision,
        exposure_milli_ev,
        white_balance: WHITE_BALANCE_AS_SHOT,
        source: "original",
        proxy_id: None,
    }
}

/// `photoId`, `stage`, `settings`, `contentType`, `width`, `height`,
/// `byteLength`, `sha256`, `sourceRevision`, `recipeVersion`,
/// `displayTransform`, and `expiresAt` travel as the response headers, and the
/// stream follows them. A baseline rendition reports the empty
/// `recipeVersion`: no saved recipe produced it.
fn rendition_response(photo_id: &str, rendition: &PublishedRendition) -> Response<Body> {
    let facts = &rendition.identity.facts;
    let mut builder = Response::builder()
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
        .header("slipstream-edit-preview-settings", facts.settings)
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
        .header("slipstream-edit-preview-source", facts.source)
        .header(
            "slipstream-edit-preview-expires-at",
            format_time(rendition.expires_at),
        );
    if facts.source == "development-proxy" {
        builder = builder.header(
            "slipstream-edit-preview-proxy-id",
            facts.proxy_id.as_deref().unwrap_or(""),
        );
    }
    builder
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
// `invalid_settings` for a stage or settings selector outside its closed set,
// 422 `unsupported_photo`, 503 `processing_unavailable`, 503
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

fn settings_outside_closed_set() -> Response<Body> {
    cli_error(
        StatusCode::UNPROCESSABLE_ENTITY,
        "invalid_settings",
        "The requested settings are outside the closed selector set.",
        serde_json::json!({
            "argument": "settings",
            "reason": "settings-outside-closed-set",
            "closedSet": CLOSED_SETTINGS,
        }),
    )
}

fn unsupported_photo(photo_id: &str, stage: &'static str) -> Response<Body> {
    cli_error(
        StatusCode::UNPROCESSABLE_ENTITY,
        "unsupported_photo",
        "This Photo's source class has no approved profile.",
        serde_json::json!({"photoId": photo_id, "stage": stage}),
    )
}

fn processing_unavailable(stage: &'static str, reason: &'static str) -> Response<Body> {
    cli_error(
        StatusCode::SERVICE_UNAVAILABLE,
        "processing_unavailable",
        if stage == "film" {
            "The film stage cannot execute for this Photo right now."
        } else {
            "The develop stage cannot execute for this Photo right now."
        },
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
mod tests;
