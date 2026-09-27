//! Edit Preview download: the one-request CLI read of the Photo Development
//! surface. The command never polls: an accepted render intent is a
//! successful pending report, and a ready rendition is validated header for
//! header, by digest, and by JPEG structure before it is published without
//! replacement, exactly as the Camera Preview download publishes.

use super::preview_download::{Destination, complete_jpeg};
use super::{
    CLI_CONTRACT_VERSION, CONTRACT_HEADER, CommandFailure, ErrorResponse, MutationIdentity,
    Operation, PublicationState, ServiceClient, access_boundary_failure, redact_value,
    response_bytes, valid_sha256, valid_utc_time, validated_route_failure, web_url,
};
use clap::{Args, ValueEnum};
use reqwest::{StatusCode, header};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;

const OPERATION: Operation = Operation::PhotosEditPreview;

/// The disclosed bound of one Edit Preview rendition: the same 64 MiB bound
/// the Camera Preview transfer enforces, because a preview is a reduced
/// display derivative, never a full-resolution Export.
const MAXIMUM_PREVIEW_BYTES: u64 = 64 * 1024 * 1024;

/// The bound of one opaque identity fact the response repeats.
const MAXIMUM_IDENTITY_BYTES: usize = 8192;

/// The JPEG frame-dimension bound, so a declared geometry is at least
/// representable in the medium the rendition is served as.
const MAXIMUM_DIMENSION: u32 = 65535;

/// The response headers of a ready rendition, in the server's naming. The
/// source revision travels hex encoded, because header values are text.
const PHOTO_ID_HEADER: &str = "slipstream-edit-preview-photo-id";
const STAGE_HEADER: &str = "slipstream-edit-preview-stage";
const SETTINGS_HEADER: &str = "slipstream-edit-preview-settings";
const WIDTH_HEADER: &str = "slipstream-edit-preview-width";
const HEIGHT_HEADER: &str = "slipstream-edit-preview-height";
const SHA256_HEADER: &str = "slipstream-edit-preview-sha256";
const SOURCE_REVISION_HEADER: &str = "slipstream-edit-preview-source-revision";
const RECIPE_VERSION_HEADER: &str = "slipstream-edit-preview-recipe-version";
const DISPLAY_TRANSFORM_HEADER: &str = "slipstream-edit-preview-display-transform";
const EXPIRES_AT_HEADER: &str = "slipstream-edit-preview-expires-at";

/// The stage of `slipstream photos edit-preview`. The closed stage set of
/// contract version 1 carries the develop stage only.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum EditPreviewStage {
    /// The current development rendered through the pinned display transform.
    Develop,
}

impl EditPreviewStage {
    /// The route segment and header value of this stage.
    fn route(self) -> &'static str {
        match self {
            Self::Develop => "develop",
        }
    }
}

/// The settings selector of `slipstream photos edit-preview`. `current` is
/// the saved Edit Recipe's settings; `baseline` is the as-shot baseline the
/// comparison presents, without saving.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum EditPreviewSettings {
    /// The saved Edit Recipe's settings.
    Current,
    /// The as-shot baseline, without saving.
    Baseline,
}

impl EditPreviewSettings {
    /// The query selector and header value of this settings choice.
    fn query(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Baseline => "baseline",
        }
    }
}

/// The arguments of `slipstream photos edit-preview`.
#[derive(Debug, Args)]
pub struct EditPreviewArgs {
    #[arg(value_name = "PHOTO_ID", value_parser = super::nonempty)]
    pub photo_id: String,
    /// New local file path; an existing file or symbolic link is never replaced.
    #[arg(long, value_name = "PATH", required = true)]
    pub file: PathBuf,
    /// The development stage to preview.
    #[arg(long, value_enum, value_name = "STAGE", required = true)]
    pub stage: EditPreviewStage,
    /// Which settings the rendition renders; the saved recipe by default.
    #[arg(long, value_enum, value_name = "SETTINGS", default_value = "current")]
    pub settings: EditPreviewSettings,
}

/// The typed body of one 202 admission: the route's pending report.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PendingPreview {
    state: String,
    stage: String,
}

impl PendingPreview {
    /// The closed pending state this body reports, when it is this route's
    /// admission for exactly the requested stage. A pending report is a
    /// success; it is never a completed download.
    fn validated_state(&self, stage: &str) -> Option<&'static str> {
        if self.stage != stage {
            return None;
        }
        match self.state.as_str() {
            "queued" => Some("queued"),
            "running" => Some("running"),
            _ => None,
        }
    }
}

/// The validated metadata of one ready rendition: exactly the typed response
/// headers the wire contract frames ahead of the JPEG stream.
struct RenditionMetadata {
    source_revision: String,
    recipe_version: String,
    display_transform: String,
    width: u32,
    height: u32,
    sha256: String,
    expires_at: String,
}

/// The one value of `name`, as text. A repeated or non-text value is not one
/// fact the contract framed.
fn header_value<'a>(headers: &'a header::HeaderMap, name: &'static str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    let value = values.next().and_then(|value| value.to_str().ok());
    match values.next() {
        Some(_) => None,
        None => value,
    }
}

/// One positive bounded dimension of the rendition geometry.
fn dimension(value: &str) -> Option<u32> {
    value
        .parse::<u32>()
        .ok()
        .filter(|value| (1..=MAXIMUM_DIMENSION).contains(value))
}

/// A nonempty bounded opaque text fact of the rendition identity.
fn bounded_text(value: &str) -> Option<String> {
    (!value.is_empty() && value.len() <= MAXIMUM_IDENTITY_BYTES).then(|| value.to_owned())
}

/// Decodes the source revision the service hex encodes for the response
/// headers back to its opaque UTF-8 text.
fn decode_source_revision(value: &str) -> Option<String> {
    if value.is_empty()
        || !value.len().is_multiple_of(2)
        || value.len() > 2 * MAXIMUM_IDENTITY_BYTES
    {
        return None;
    }
    let decoded = value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = pair[0] as char;
            let low = pair[1] as char;
            u8::try_from(high.to_digit(16)? * 16 + low.to_digit(16)?).ok()
        })
        .collect::<Option<Vec<u8>>>()?;
    String::from_utf8(decoded)
        .ok()
        .filter(|revision| !revision.is_empty())
}

/// The recipe version a rendition may report. A baseline rendition is not the
/// product of any saved recipe, so it must report none; a current rendition
/// reports none when no saved recipe exists.
fn validated_recipe_version(value: &str, settings: &str) -> Option<String> {
    match settings {
        "baseline" => value.is_empty().then_some(String::new()),
        _ if value.len() <= MAXIMUM_IDENTITY_BYTES => Some(value.to_owned()),
        _ => None,
    }
}

/// Validates one ready response's metadata against the requested selectors.
/// A missing, repeated, malformed, or foreign fact refuses the whole
/// response: a rendition the client cannot identify is not published.
fn rendition_metadata(
    headers: &header::HeaderMap,
    photo_id: &str,
    stage: &str,
    settings: &str,
) -> Option<RenditionMetadata> {
    let identifies =
        |name: &'static str, expected: &str| header_value(headers, name) == Some(expected);
    if !identifies(PHOTO_ID_HEADER, photo_id)
        || !identifies(STAGE_HEADER, stage)
        || !identifies(SETTINGS_HEADER, settings)
    {
        return None;
    }
    let width = dimension(header_value(headers, WIDTH_HEADER)?)?;
    let height = dimension(header_value(headers, HEIGHT_HEADER)?)?;
    let sha256 = header_value(headers, SHA256_HEADER)?.to_owned();
    if !valid_sha256(&sha256) {
        return None;
    }
    let source_revision = decode_source_revision(header_value(headers, SOURCE_REVISION_HEADER)?)?;
    let recipe_version =
        validated_recipe_version(header_value(headers, RECIPE_VERSION_HEADER)?, settings)?;
    let display_transform = bounded_text(header_value(headers, DISPLAY_TRANSFORM_HEADER)?)?;
    let expires_at = header_value(headers, EXPIRES_AT_HEADER)?;
    if !valid_utc_time(expires_at) {
        return None;
    }
    Some(RenditionMetadata {
        source_revision,
        recipe_version,
        display_transform,
        width,
        height,
        sha256,
        expires_at: expires_at.to_owned(),
    })
}

/// Recomputes the digest of the received bytes and compares it with the
/// declared digest, so only bytes the receipt names are published.
fn digest_matches(bytes: &[u8], claimed: &str) -> bool {
    let digest = Sha256::digest(bytes);
    claimed.len() == 64
        && digest
            .iter()
            .zip(claimed.as_bytes().chunks_exact(2))
            .all(|(byte, pair)| {
                let high = (pair[0] as char).to_digit(16);
                let low = (pair[1] as char).to_digit(16);
                high.zip(low)
                    .is_some_and(|(high, low)| u32::from(*byte) == high * 16 + low)
            })
}

/// Performs the one Edit Preview read of this command. An accepted render
/// intent is reported as pending, and a ready rendition is streamed into the
/// staged anonymous file, validated, and only then published. The command
/// never polls; a later invocation observes the finished work.
pub(super) async fn download(
    client: &ServiceClient,
    args: &EditPreviewArgs,
    destination: Destination,
    publication: &PublicationState,
) -> Result<Value, CommandFailure> {
    let photo_id = args.photo_id.as_str();
    let stage = args.stage.route();
    let settings = args.settings.query();
    let mut url = client.endpoint(&["api", "photos", photo_id, "edit-preview", stage]);
    url.query_pairs_mut().append_pair("settings", settings);
    let mut response = client
        .client
        .get(url)
        .header(CONTRACT_HEADER, CLI_CONTRACT_VERSION)
        .bearer_auth(&client.token)
        .send()
        .await
        .map_err(|_| CommandFailure::transport(OPERATION))?;
    let status = response.status();
    if status.is_redirection() {
        return Err(CommandFailure::transport(OPERATION));
    }
    if let Some(failure) = access_boundary_failure(status, None, &[], OPERATION) {
        return Err(failure);
    }
    if status == StatusCode::ACCEPTED {
        let bytes = response_bytes(response, OPERATION).await?;
        let pending: PendingPreview =
            serde_json::from_slice(&bytes).map_err(|_| CommandFailure::transport(OPERATION))?;
        let Some(state) = pending.validated_state(stage) else {
            return Err(CommandFailure::transport(OPERATION));
        };
        let web_url = web_url(&client.origin, &format!("/?photoId={photo_id}"))
            .map_err(|_| CommandFailure::transport(OPERATION))?;
        let mut data = json!({
            "photoId": photo_id,
            "stage": stage,
            "settings": settings,
            "state": state,
            "webUrl": web_url,
            "fileCommitted": false,
        });
        redact_value(&mut data, &client.token);
        return Ok(data);
    }
    if status != StatusCode::OK {
        let bytes = response_bytes(response, OPERATION).await?;
        if let Some(failure) = access_boundary_failure(status, None, &bytes, OPERATION) {
            return Err(failure);
        }
        let error = serde_json::from_slice::<ErrorResponse>(&bytes)
            .map_err(|_| CommandFailure::transport(OPERATION))?
            .error;
        if status == StatusCode::INTERNAL_SERVER_ERROR
            && error.code == "outcome_unknown"
            && error.details.get("stage").and_then(Value::as_str) == Some(stage)
        {
            return Err(CommandFailure::unknown(&MutationIdentity {
                operation: OPERATION,
                photo_ids: vec![photo_id.to_owned()],
                album_id: None,
                album_name: None,
                mappings: Vec::new(),
            }));
        }
        return Err(validated_route_failure(error, OPERATION, &client.token)
            .unwrap_or_else(|| CommandFailure::transport(OPERATION)));
    }
    if response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some("image/jpeg")
    {
        return Err(CommandFailure::transport(OPERATION));
    }
    let Some(byte_length) = response
        .content_length()
        .filter(|length| (1..=MAXIMUM_PREVIEW_BYTES).contains(length))
        .and_then(|length| usize::try_from(length).ok())
    else {
        return Err(CommandFailure::transport(OPERATION));
    };
    let Some(metadata) = rendition_metadata(response.headers(), photo_id, stage, settings) else {
        return Err(CommandFailure::transport(OPERATION));
    };
    let mut file = destination.anonymous_file()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| CommandFailure::transport(OPERATION))?
    {
        if bytes.len().saturating_add(chunk.len()) > byte_length {
            return Err(CommandFailure::transport(OPERATION));
        }
        file.write_all(&chunk)
            .await
            .map_err(|_| destination.local_io())?;
        bytes.extend_from_slice(&chunk);
    }
    if bytes.len() != byte_length || !digest_matches(&bytes, &metadata.sha256) {
        return Err(CommandFailure::transport(OPERATION));
    }
    let width = metadata.width;
    let height = metadata.height;
    let complete = tokio::task::spawn_blocking(move || complete_jpeg(&bytes, width, height))
        .await
        .map_err(|_| CommandFailure::transport(OPERATION))?;
    if !complete {
        return Err(CommandFailure::transport(OPERATION));
    }
    file.sync_all().await.map_err(|_| destination.local_io())?;
    let web_url = web_url(&client.origin, &format!("/?photoId={photo_id}"))
        .map_err(|_| CommandFailure::transport(OPERATION))?;
    let mut data = json!({
        "photoId": photo_id,
        "stage": stage,
        "settings": settings,
        "state": "ready",
        "sourceRevision": metadata.source_revision,
        "recipeVersion": metadata.recipe_version,
        "displayTransform": metadata.display_transform,
        "contentType": "image/jpeg",
        "width": metadata.width,
        "height": metadata.height,
        "byteLength": byte_length,
        "sha256": metadata.sha256,
        "expiresAt": metadata.expires_at,
        "detailLimited": true,
        "path": destination.path(),
        "webUrl": web_url,
        "fileCommitted": true,
    });
    redact_value(&mut data, &client.token);
    destination.publish(&file)?;
    publication.record("Edit Preview", data.clone());
    if !destination.fsync_directory() {
        return Err(CommandFailure::published_file(data, false, "Edit Preview"));
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lowercase hex encoding the service applies to header facts.
    fn hex(text: &str) -> String {
        text.bytes().map(|byte| format!("{byte:02x}")).collect()
    }

    fn header_map(pairs: Vec<(&'static str, String)>) -> header::HeaderMap {
        let mut map = header::HeaderMap::new();
        for (name, value) in pairs {
            let name = header::HeaderName::from_bytes(name.as_bytes()).unwrap();
            map.insert(name, header::HeaderValue::from_str(&value).unwrap());
        }
        map
    }

    /// The response headers of one valid ready rendition, in the server's
    /// naming.
    fn ready_headers(settings: &str, recipe_version: &str) -> header::HeaderMap {
        header_map(vec![
            (PHOTO_ID_HEADER, "photo".to_owned()),
            (STAGE_HEADER, "develop".to_owned()),
            (SETTINGS_HEADER, settings.to_owned()),
            (WIDTH_HEADER, "816".to_owned()),
            (HEIGHT_HEADER, "1224".to_owned()),
            (SHA256_HEADER, "a".repeat(64)),
            (SOURCE_REVISION_HEADER, hex("rev-1")),
            (RECIPE_VERSION_HEADER, recipe_version.to_owned()),
            (DISPLAY_TRANSFORM_HEADER, "display-transform-v1".to_owned()),
            (EXPIRES_AT_HEADER, "2026-09-27T12:00:00Z".to_owned()),
        ])
    }

    #[test]
    fn ready_metadata_matches_the_request_and_decodes_the_revision() {
        let metadata = rendition_metadata(
            &ready_headers("current", "recipe-7"),
            "photo",
            "develop",
            "current",
        )
        .expect("a valid ready response");
        assert_eq!(metadata.source_revision, "rev-1");
        assert_eq!(metadata.recipe_version, "recipe-7");
        assert_eq!(metadata.width, 816);
        assert_eq!(metadata.height, 1224);
        assert_eq!(metadata.display_transform, "display-transform-v1");
        assert_eq!(metadata.expires_at, "2026-09-27T12:00:00Z");
        assert_eq!(metadata.sha256, "a".repeat(64));
    }

    #[test]
    fn current_may_report_no_saved_recipe_but_baseline_must_not() {
        let current =
            rendition_metadata(&ready_headers("current", ""), "photo", "develop", "current")
                .expect("an empty recipe version is a valid current rendition");
        assert_eq!(current.recipe_version, "");
        assert!(
            rendition_metadata(
                &ready_headers("baseline", ""),
                "photo",
                "develop",
                "baseline"
            )
            .is_some(),
            "a baseline rendition reports no recipe version"
        );
        assert!(
            rendition_metadata(
                &ready_headers("baseline", "recipe-7"),
                "photo",
                "develop",
                "baseline"
            )
            .is_none(),
            "a baseline rendition is not the product of a saved recipe"
        );
    }

    #[test]
    fn ready_metadata_refuses_foreign_selectors() {
        let headers = ready_headers("current", "recipe-7");
        assert!(rendition_metadata(&headers, "other", "develop", "current").is_none());
        assert!(rendition_metadata(&headers, "photo", "film", "current").is_none());
        assert!(rendition_metadata(&headers, "photo", "develop", "baseline").is_none());
    }

    #[test]
    fn ready_metadata_refuses_a_repeated_fact() {
        let mut headers = ready_headers("current", "recipe-7");
        let name = header::HeaderName::from_bytes(SHA256_HEADER.as_bytes()).unwrap();
        headers.append(
            name,
            header::HeaderValue::from_str(&"b".repeat(64)).unwrap(),
        );
        assert!(
            rendition_metadata(&headers, "photo", "develop", "current").is_none(),
            "a repeated header is not one contract fact"
        );
    }

    #[test]
    fn ready_metadata_refuses_malformed_facts() {
        let tamper = |name: &'static str, value: String| {
            let mut headers = ready_headers("current", "recipe-7");
            headers.insert(
                header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                header::HeaderValue::from_str(&value).unwrap(),
            );
            headers
        };
        for (name, value) in [
            (SHA256_HEADER, "a".repeat(63)),
            (SHA256_HEADER, "g".repeat(64)),
            (WIDTH_HEADER, "0".to_owned()),
            (WIDTH_HEADER, "65536".to_owned()),
            (WIDTH_HEADER, "wide".to_owned()),
            (HEIGHT_HEADER, "0".to_owned()),
            (SOURCE_REVISION_HEADER, format!("{}g", &hex("rev-1")[..2])),
            (SOURCE_REVISION_HEADER, hex("rev-1")[..1].to_owned()),
            (SOURCE_REVISION_HEADER, String::new()),
            (DISPLAY_TRANSFORM_HEADER, String::new()),
            (EXPIRES_AT_HEADER, "yesterday".to_owned()),
            (EXPIRES_AT_HEADER, "2026-09-27 12:00:00Z".to_owned()),
        ] {
            let headers = tamper(name, value.clone());
            assert!(
                rendition_metadata(&headers, "photo", "develop", "current").is_none(),
                "{name}={value:?} must refuse"
            );
        }
        assert!(
            rendition_metadata(&header::HeaderMap::new(), "photo", "develop", "current").is_none(),
            "a response without the framed facts refuses"
        );
    }

    #[test]
    fn source_revision_decoding_is_the_inverse_of_the_service_encoding() {
        assert_eq!(
            decode_source_revision(&hex("rev-1")).as_deref(),
            Some("rev-1")
        );
        assert_eq!(
            decode_source_revision("7265762D31").as_deref(),
            Some("rev-1"),
            "hex digits decode in either case"
        );
        assert_eq!(
            decode_source_revision(&hex("ein Spielerstück")).as_deref(),
            Some("ein Spielerstück"),
            "non-ASCII revisions round-trip through UTF-8"
        );
        assert!(decode_source_revision("").is_none());
        assert!(decode_source_revision("726").is_none(), "odd length");
        assert!(decode_source_revision("zz").is_none(), "not hex");
        assert!(
            decode_source_revision("ff").is_none(),
            "bytes that are not UTF-8 text are not a revision"
        );
        assert!(
            decode_source_revision(&"7".repeat(2 * MAXIMUM_IDENTITY_BYTES + 2)).is_none(),
            "the revision stays bounded"
        );
    }

    #[test]
    fn dimensions_are_positive_and_within_the_medium_bound() {
        assert_eq!(dimension("1"), Some(1));
        assert_eq!(dimension("65535"), Some(65535));
        assert_eq!(dimension("65536"), None);
        assert_eq!(dimension("0"), None);
        assert_eq!(dimension("-1"), None);
        assert_eq!(dimension("+1"), Some(1));
    }

    #[test]
    fn the_digest_names_exactly_the_received_bytes() {
        let received = b"slipstream edit preview bytes";
        let digest: String = Sha256::digest(received)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert!(digest_matches(received, &digest));
        let tampered = format!(
            "{}{}",
            &digest[..63],
            if digest.ends_with('0') { "1" } else { "0" }
        );
        assert!(!digest_matches(received, &tampered));
        assert!(!digest_matches(b"other bytes", &digest));
    }

    #[test]
    fn pending_states_are_typed_and_stage_bound() {
        let queued = PendingPreview {
            state: "queued".to_owned(),
            stage: "develop".to_owned(),
        };
        assert_eq!(queued.validated_state("develop"), Some("queued"));
        let running = PendingPreview {
            state: "running".to_owned(),
            stage: "develop".to_owned(),
        };
        assert_eq!(running.validated_state("develop"), Some("running"));
        assert_eq!(
            queued.validated_state("film"),
            None,
            "a foreign stage is not this route's admission"
        );
        assert_eq!(
            PendingPreview {
                state: "failed".to_owned(),
                stage: "develop".to_owned(),
            }
            .validated_state("develop"),
            None,
            "a failed render is not a pending success"
        );
        assert_eq!(
            PendingPreview {
                state: "ready".to_owned(),
                stage: "develop".to_owned(),
            }
            .validated_state("develop"),
            None,
            "a ready report is not a pending admission"
        );
    }

    #[test]
    fn pending_bodies_parse_the_closed_wire_shape() {
        let pending: PendingPreview =
            serde_json::from_slice(br#"{"state":"queued","stage":"develop"}"#)
                .expect("the admission shape");
        assert_eq!(pending.validated_state("develop"), Some("queued"));
        let pending: PendingPreview =
            serde_json::from_slice(br#"{"state":"running","stage":"develop"}"#)
                .expect("the running shape");
        assert_eq!(pending.validated_state("develop"), Some("running"));
    }
}
