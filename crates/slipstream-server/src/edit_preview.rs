//! Edit Preview HTTP surface: one display rendition per Photo and stage.
//! The route derives the rendition through the pinned Development display
//! derivative, frames it as response headers followed by the JPEG stream, and
//! schedules preview work latest-intent-wins within its Photo and stage owner
//! per the Photo Development Surface contract.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    response::Response,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use slipstream_processing::photo_profile::{
    APPROVED_EXPOSURE_MILLI_EV_MAX, APPROVED_EXPOSURE_MILLI_EV_MIN,
};
use std::os::fd::AsRawFd;

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

    /// The opaque digest of the full identity, used as the pending intent
    /// identity of the owner.
    fn digest(&self) -> String {
        let mut hasher = Sha256::new();
        let facts = &self.facts;
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
        match &self.evidence {
            ContentEvidence::DevelopmentResult {
                sha256,
                byte_length,
            } => {
                hasher.update(sha256.as_bytes());
                hasher.update([0]);
                hasher.update(byte_length.to_le_bytes());
            }
            ContentEvidence::NotRetained => hasher.update([1]),
        }
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
    /// The workload cannot admit preview-class work in this deployment.
    Unavailable(&'static str),
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
}

/// The service-side admission of preview-class development renders follows the
/// same durable Export lifecycle that owns the production `development-tiff`
/// path. Until that lifecycle lands, no render can be admitted and the route
/// refuses fail-closed with the contract's `processing_unavailable` code; it
/// never reports queued work that cannot run.
pub(crate) struct UnlandedRenderGate;

impl PreviewRenderGate for UnlandedRenderGate {
    fn admit(&self, _request: PreviewRenderRequest<'_>) -> RenderAdmission {
        RenderAdmission::Unavailable("preview-render-admission-unavailable")
    }
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

#[derive(Default)]
struct OwnerEntry {
    published: Option<PublishedRendition>,
    /// The latest admitted render intent, by identity digest. The slot is
    /// latest-intent-wins: a superseded intent never publishes, and a
    /// completed request republishes only while its full identity is current.
    pending: Option<String>,
}

type OwnerKey = (String, &'static str);

/// The Edit Preview owner: the per-server rendition cache and the render
/// intent slots of every Photo and stage owner.
pub(crate) struct EditPreviewOwner {
    retention: Arc<dyn DevelopmentResultRetention>,
    render_gate: Arc<dyn PreviewRenderGate>,
    owners: Mutex<HashMap<OwnerKey, OwnerEntry>>,
    /// Serializes derive-and-publish per owner, so equal concurrent requests
    /// coalesce onto one derivation instead of repeating it.
    derive_permits: Mutex<HashMap<OwnerKey, Arc<tokio::sync::Mutex<()>>>>,
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
            owners: Mutex::new(HashMap::new()),
            derive_permits: Mutex::new(HashMap::new()),
        }
    }

    fn with_entry<R>(&self, key: &OwnerKey, read: impl FnOnce(&mut OwnerEntry) -> R) -> R {
        let mut owners = self.owners.lock().expect("edit preview owners poisoned");
        read(owners.entry(key.clone()).or_default())
    }

    /// The published rendition still current for this full identity, if any.
    /// A rendition under a different display transform, without content
    /// evidence for its source, or past its disclosed expiry is not current.
    fn current(&self, key: &OwnerKey, identity: &PreviewIdentity) -> Option<PublishedRendition> {
        self.with_entry(key, |entry| {
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
    }

    /// The per-owner derive permit. Holding it serializes derive-and-publish
    /// so a concurrent equal request coalesces onto the first derivation.
    fn derive_permit(&self, key: &OwnerKey) -> Arc<tokio::sync::Mutex<()>> {
        self.derive_permits
            .lock()
            .expect("edit preview derive permits poisoned")
            .entry(key.clone())
            .or_default()
            .clone()
    }

    /// Publishes the derived rendition and settles any pending intent.
    fn publish(&self, key: &OwnerKey, rendition: PublishedRendition) {
        self.with_entry(key, |entry| {
            entry.published = Some(rendition);
            entry.pending = None;
        });
    }

    /// Admits one render. A pending request with the same full identity
    /// coalesces; a different identity supersedes the pending intent.
    fn admit(
        &self,
        key: &OwnerKey,
        photo_id: &str,
        stage: &'static str,
        identity: &PreviewIdentity,
    ) -> RenderAdmission {
        let digest = identity.digest();
        let coalesced = self.with_entry(key, |entry| {
            entry.pending.as_deref() == Some(digest.as_str())
        });
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
            self.with_entry(key, |entry| entry.pending = Some(digest));
        }
        admission
    }
}

// ---------------------------------------------------------------- handler

/// One source-class support classification for the preview gates. This is the
/// single adaptation point over the Edit Recipe surface's support derivation.
struct SupportClassification {
    state: &'static str,
    reason: Option<&'static str>,
}

fn classify_support(
    state: &HttpState,
    photo: &slipstream_core::PhotoRead,
    metadata: &slipstream_core::CaptureReviewMetadata,
    source_available: bool,
) -> SupportClassification {
    let support = crate::edit_recipe::derive_support(
        crate::edit_recipe::source_facts(photo, metadata, state),
        source_available,
    );
    SupportClassification {
        // The Edit Recipe support state names an approved class `ready`; the
        // Photo read contract names it `supported`.
        state: if support.state == "ready" {
            "supported"
        } else {
            support.state
        },
        reason: support.reason,
    }
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
    let support = classify_support(&state, &photo, &metadata, read.source_available);
    match support.state {
        "unsupported" => return unsupported_photo(&photo_id),
        "unavailable" => {
            // An operator-disabled deployment is a capability failure of the
            // stage, not missing source facts.
            if support.reason == Some("operator-disabled") {
                return processing_unavailable(stage, "operator-disabled");
            }
            return resource_unavailable(stage, support.reason.unwrap_or("source-unavailable"));
        }
        _ => {}
    }
    if let Err(response) = develop_executable(&state, stage, &read) {
        return *response;
    }
    serve_preview(&state, &photo_id, stage, &read).await
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
    if let Some(rendition) = owner.current(&key, &identity) {
        return rendition_response(photo_id, &rendition);
    }
    // Serialize derive-and-publish per owner: a concurrent request with the
    // same full identity coalesces onto the first derivation.
    let permit = owner.derive_permit(&key);
    let _guard = permit.lock().await;
    if let Some(rendition) = owner.current(&key, &identity) {
        return rendition_response(photo_id, &rendition);
    }
    // Derive only from a retained result whose captured facts are exactly the
    // current identity facts; anything else falls through to admission.
    let Some(record) = retained.filter(|record| record.matches_facts(&facts)) else {
        return admit_render(owner, &key, photo_id, stage, &identity);
    };
    let long_edge = facts.long_edge;
    let path = record.path.clone();
    let derived = tokio::task::spawn_blocking(move || {
        std::fs::File::open(&path).map(|file| process_development_tiff(file.as_raw_fd(), long_edge))
    })
    .await;
    let derivative = match derived {
        Ok(Ok(Ok(derivative))) => derivative,
        Ok(Ok(Err(error))) => return derivative_error(stage, error),
        Ok(Err(_)) => {
            return processing_unavailable(stage, "development-result-unreadable");
        }
        Err(_) => {
            return processing_unavailable(stage, "derivation-unavailable");
        }
    };
    // Publication recheck: the request republishes only while the current
    // owner and the full identity are still the ones it derived under.
    if let Err(response) = recheck_identity(state, photo_id, stage, &identity).await {
        return *response;
    }
    let sha256 = hex(Sha256::digest(&derivative.jpeg).as_slice());
    let rendition = PublishedRendition {
        identity,
        bytes: axum::body::Bytes::from(derivative.jpeg),
        sha256,
        width: derivative.width,
        height: derivative.height,
        expires_at: SystemTime::now() + RENDITION_TTL,
    };
    owner.publish(&key, rendition.clone());
    rendition_response(photo_id, &rendition)
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

/// Re-reads the serialized facts and re-derives the full identity. Any change
/// supersedes the request, and a superseded request never publishes.
async fn recheck_identity(
    state: &HttpState,
    photo_id: &str,
    stage: &'static str,
    identity: &PreviewIdentity,
) -> Result<(), Box<Response<Body>>> {
    let (photo, metadata, read) = match crate::edit_recipe::load_facts(state, photo_id).await {
        Ok(facts) => facts,
        Err(response) => return Err(Box::new(response)),
    };
    let support = classify_support(state, &photo, &metadata, read.source_available);
    match support.state {
        "unsupported" => return Err(Box::new(unsupported_photo(photo_id))),
        "unavailable" => {
            if support.reason == Some("operator-disabled") {
                return Err(Box::new(processing_unavailable(stage, "operator-disabled")));
            }
            return Err(Box::new(resource_unavailable(
                stage,
                support.reason.unwrap_or("source-unavailable"),
            )));
        }
        _ => {}
    }
    develop_executable(state, stage, &read)?;
    let fresh_facts = current_facts(state, stage, &read);
    let fresh_retained = state.edit_preview.retention.resolve(photo_id);
    if PreviewIdentity::build(&fresh_facts, fresh_retained.as_ref()) != *identity {
        return Err(Box::new(resource_unavailable(stage, "preview-superseded")));
    }
    Ok(())
}

/// Admits one preview-class render when no current rendition or usable
/// retained result exists.
fn admit_render(
    owner: &EditPreviewOwner,
    key: &OwnerKey,
    photo_id: &str,
    stage: &'static str,
    identity: &PreviewIdentity,
) -> Response<Body> {
    match owner.admit(key, photo_id, stage, identity) {
        RenderAdmission::Queued => admitted(stage, "queued"),
        RenderAdmission::Running => admitted(stage, "running"),
        RenderAdmission::Indeterminate => outcome_unknown(stage),
        RenderAdmission::Unavailable(reason) => processing_unavailable(stage, reason),
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
    json_accepted(serde_json::json!({
        "state": state,
        "stage": stage,
    }))
}

fn json_accepted(value: Value) -> Response<Body> {
    crate::http::json_response(StatusCode::ACCEPTED, &value)
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

    fn rendition(identity: PreviewIdentity, expires_at: SystemTime) -> PublishedRendition {
        PublishedRendition {
            identity,
            bytes: axum::body::Bytes::from_static(b"jpeg"),
            sha256: "a".repeat(64),
            width: 64,
            height: 64,
            expires_at,
        }
    }

    #[test]
    fn identity_changes_with_every_identity_fact() {
        let base = PreviewIdentity::build(&facts(250), Some(&record(250)));
        let changed = |facts: &PreviewFacts, record: RetainedDevelopmentResult| {
            PreviewIdentity::build(facts, Some(&record))
        };
        // A different display transform, geometry, bundle, source revision,
        // recipe revision, recipe content, or content evidence is a
        // different identity.
        let mut transform = facts(250);
        transform.display_transform = "display-transform-v2";
        assert_ne!(base, changed(&transform, record(250)));
        let mut geometry = facts(250);
        geometry.long_edge = 512;
        assert_ne!(base, changed(&geometry, record(250)));
        let mut bundle = facts(250);
        bundle.bundle_sha256 = "e".repeat(64);
        assert_ne!(base, changed(&bundle, record(250)));
        let mut source = facts(250);
        source.source_revision = "changed".to_owned();
        assert_ne!(base, changed(&source, record(250)));
        let mut revision = facts(250);
        revision.recipe_revision = Some("recipe-2".to_owned());
        assert_ne!(base, changed(&revision, record(250)));
        assert_ne!(base, changed(&facts(500), record(250)));
        assert_ne!(base, changed(&facts(250), record(500)));
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

    #[test]
    fn owner_serves_only_the_current_full_identity() {
        let owner = EditPreviewOwner::production();
        let key = ("photo".to_owned(), "develop");
        let identity = PreviewIdentity::build(&facts(250), Some(&record(250)));
        let expires = SystemTime::now() + RENDITION_TTL;
        owner.publish(&key, rendition(identity.clone(), expires));
        assert!(owner.current(&key, &identity).is_some());
        // A different display transform is not current.
        let mut transform = facts(250);
        transform.display_transform = "display-transform-v2";
        assert!(
            owner
                .current(
                    &key,
                    &PreviewIdentity::build(&transform, Some(&record(250)))
                )
                .is_none()
        );
        // A past expiry is not current.
        let expired_at = SystemTime::now() - Duration::from_secs(1);
        owner.publish(&key, rendition(identity.clone(), expired_at));
        assert!(owner.current(&key, &identity).is_none());
    }

    #[test]
    fn pending_intents_coalesce_and_follow_latest_intent() {
        struct ScriptedGate(Mutex<Vec<RenderAdmission>>);
        impl PreviewRenderGate for ScriptedGate {
            fn admit(&self, _request: PreviewRenderRequest<'_>) -> RenderAdmission {
                self.0
                    .lock()
                    .expect("scripted gate poisoned")
                    .pop()
                    .unwrap_or(RenderAdmission::Unavailable("exhausted"))
            }
        }
        let gate = Arc::new(ScriptedGate(Mutex::new(vec![
            RenderAdmission::Queued,
            RenderAdmission::Queued,
        ])));
        let gate_dyn: Arc<dyn PreviewRenderGate> = gate.clone();
        let owner = EditPreviewOwner::new(Arc::new(UnlandedRetention), gate_dyn);
        let key = ("photo".to_owned(), "develop");
        let first = PreviewIdentity::build(&facts(250), None);
        let second = PreviewIdentity::build(&facts(500), None);
        assert_eq!(
            owner.admit(&key, "photo", "develop", &first),
            RenderAdmission::Queued
        );
        // The same full identity coalesces without a second admission.
        assert_eq!(
            owner.admit(&key, "photo", "develop", &first),
            RenderAdmission::Running
        );
        // A changed identity supersedes the pending intent.
        assert_eq!(
            owner.admit(&key, "photo", "develop", &second),
            RenderAdmission::Queued
        );
        assert_eq!(
            gate.0.lock().unwrap().len(),
            0,
            "exactly two admissions reached the gate"
        );
        // A settled rendition clears the pending slot.
        let settled = PreviewIdentity::build(&facts(500), None);
        owner.publish(
            &key,
            rendition(settled.clone(), SystemTime::now() + RENDITION_TTL),
        );
        assert!(owner.current(&key, &settled).is_some());
    }
}
