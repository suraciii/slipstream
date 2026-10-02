//! Selected-step Processing Preview download: the one-request CLI read of
//! the composable Processing Step surface. The command never polls: an
//! accepted render intent is a successful pending report, and a ready
//! rendition is validated identity fact for identity fact, by digest, and
//! by PNG structure before it is published without replacement, exactly as
//! the Edit Preview download publishes. The route never substitutes the
//! Camera Preview, a legacy stage rendition, or another module's result.
use super::preview_download::{Destination, complete_png};
use super::{
    CLI_CONTRACT_VERSION, CONTRACT_HEADER, CommandFailure, ErrorResponse, Operation,
    PublicationState, ServiceClient, access_boundary_failure, redact_value, response_bytes,
    valid_sha256, validated_route_failure, web_url,
};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

const OPERATION: Operation = Operation::PhotosProcessingPreview;

/// The disclosed bound of one selected-step Preview rendition: the same
/// 64 MiB bound the framed preview transfers enforce, because a Preview is
/// a bounded display derivative, never a full-resolution Export.
const MAXIMUM_PREVIEW_BYTES: u64 = 64 * 1024 * 1024;

/// The bound of one opaque identity fact the response repeats.
const MAXIMUM_IDENTITY_BYTES: usize = 8192;

/// The frame-dimension bound, so a declared geometry is at least
/// representable in the medium the rendition is served as.
const MAXIMUM_DIMENSION: u32 = 65535;

/// The response headers of a ready rendition, in the route's naming. The
/// source revision travels hex encoded, because header values are text.
/// These names are the selected-step route's own contract, kept beside each
/// other so a change on the service side is one change here.
const PHOTO_ID_HEADER: &str = "slipstream-processing-preview-photo-id";
const STEP_ID_HEADER: &str = "slipstream-processing-preview-step-id";
const WIDTH_HEADER: &str = "slipstream-processing-preview-width";
const HEIGHT_HEADER: &str = "slipstream-processing-preview-height";
const SHA256_HEADER: &str = "slipstream-processing-preview-sha256";
const SOURCE_REVISION_HEADER: &str = "slipstream-processing-preview-source-revision";
const RECIPE_REVISION_HEADER: &str = "slipstream-processing-preview-recipe-revision";
const MODULE_HEADER: &str = "slipstream-processing-preview-module";
const ADAPTER_SCHEMA_VERSION_HEADER: &str = "slipstream-processing-preview-adapter-schema-version";
const PARAMETER_DIGEST_HEADER: &str = "slipstream-processing-preview-parameter-digest";
const OUTPUT_CONTRACT_HEADER: &str = "slipstream-processing-preview-output-contract";
const DISPLAY_CONVERSION_HEADER: &str = "slipstream-processing-preview-display-conversion";
const IDENTITY_HEADER: &str = "slipstream-processing-preview-identity";

/// The bounded native render's own display rendition medium.
const RENDITION_CONTENT_TYPE: &str = "image/png";

/// The typed body of one 202 admission: the route's pending report.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PendingPreview {
    state: String,
    step_id: String,
}

impl PendingPreview {
    /// The closed pending state this body reports, when it is this route's
    /// admission for exactly the requested step. A pending report is a
    /// success; it is never a completed download.
    fn validated_state(&self, step_id: &str) -> Option<&'static str> {
        if self.step_id != step_id {
            return None;
        }
        match self.state.as_str() {
            "queued" => Some("queued"),
            "running" => Some("running"),
            _ => None,
        }
    }
}

/// The identity facts of one ready selected-step rendition.
struct RenditionMetadata {
    source_revision: String,
    recipe_revision: String,
    width: u32,
    height: u32,
    sha256: String,
}

/// The one value of `name`, as text. A repeated or non-text value is not one
/// fact the contract framed.
fn header_value<'a>(headers: &'a reqwest::header::HeaderMap, name: &str) -> Option<&'a str> {
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

/// Validates one ready response's identity against the requested selectors.
/// A missing, repeated, malformed, or foreign fact refuses the whole
/// response: a rendition the client cannot identify as the selected step's
/// own result is never published.
fn rendition_metadata(
    headers: &reqwest::header::HeaderMap,
    photo_id: &str,
    step_id: &str,
) -> Option<RenditionMetadata> {
    let identifies = |name: &str, expected: &str| header_value(headers, name) == Some(expected);
    if !identifies(PHOTO_ID_HEADER, photo_id) || !identifies(STEP_ID_HEADER, step_id) {
        return None;
    }
    let width = dimension(header_value(headers, WIDTH_HEADER)?)?;
    let height = dimension(header_value(headers, HEIGHT_HEADER)?)?;
    let sha256 = header_value(headers, SHA256_HEADER)?.to_owned();
    if !valid_sha256(&sha256) {
        return None;
    }
    let source_revision = decode_source_revision(header_value(headers, SOURCE_REVISION_HEADER)?)?;
    let recipe_revision = bounded_text(header_value(headers, RECIPE_REVISION_HEADER)?)?;
    let _module = bounded_text(header_value(headers, MODULE_HEADER)?)?;
    let _adapter_schema_version =
        bounded_text(header_value(headers, ADAPTER_SCHEMA_VERSION_HEADER)?)?;
    let parameter_digest = header_value(headers, PARAMETER_DIGEST_HEADER)?.to_owned();
    if !valid_sha256(&parameter_digest) {
        return None;
    }
    let output_contract = header_value(headers, OUTPUT_CONTRACT_HEADER)?.to_owned();
    if !valid_sha256(&output_contract) {
        return None;
    }
    let _display_conversion = bounded_text(header_value(headers, DISPLAY_CONVERSION_HEADER)?)?;
    let identity = header_value(headers, IDENTITY_HEADER)?.to_owned();
    if !valid_sha256(&identity) {
        return None;
    }
    Some(RenditionMetadata {
        source_revision,
        recipe_revision,
        width,
        height,
        sha256,
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

/// Performs the one selected-step Preview read of this command. An accepted
/// render intent is reported as pending, and a ready rendition is streamed
/// into the staged anonymous file, validated, and only then published. The
/// command never polls; a later invocation observes the finished work.
pub(super) async fn download(
    client: &ServiceClient,
    photo_id: &str,
    step_id: &str,
    destination: Destination,
    publication: &PublicationState,
) -> Result<Value, CommandFailure> {
    let mut response = client
        .client
        .get(client.endpoint(&["api", "photos", photo_id, "processing-preview", step_id]))
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
        let Some(state) = pending.validated_state(step_id) else {
            return Err(CommandFailure::transport(OPERATION));
        };
        let web_url = web_url(&client.origin, &format!("/?photoId={photo_id}"))
            .map_err(|_| CommandFailure::transport(OPERATION))?;
        let mut data = json!({
            "photoId": photo_id,
            "stepId": step_id,
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
        return Err(validated_route_failure(error, OPERATION, &client.token)
            .unwrap_or_else(|| CommandFailure::transport(OPERATION)));
    }
    // The route serves the bounded native render's own display rendition as
    // PNG bytes; any other medium is not the contract this client validates.
    if response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some(RENDITION_CONTENT_TYPE)
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
    let Some(metadata) = rendition_metadata(response.headers(), photo_id, step_id) else {
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
    let complete = tokio::task::spawn_blocking(move || complete_png(&bytes, width, height))
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
        "stepId": step_id,
        "state": "ready",
        "sourceRevision": metadata.source_revision,
        "recipeRevision": metadata.recipe_revision,
        "contentType": RENDITION_CONTENT_TYPE,
        "width": metadata.width,
        "height": metadata.height,
        "byteLength": byte_length,
        "sha256": metadata.sha256,
        "path": destination.path(),
        "webUrl": web_url,
        "fileCommitted": true,
    });
    redact_value(&mut data, &client.token);
    destination.publish(&file)?;
    publication.record("Processing Preview", data.clone());
    if !destination.fsync_directory() {
        return Err(CommandFailure::published_file(
            data,
            false,
            "Processing Preview",
        ));
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> reqwest::header::HeaderMap {
        let mut headers = reqwest::header::HeaderMap::new();
        for (name, value) in pairs {
            headers.append(
                reqwest::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                reqwest::header::HeaderValue::from_str(value).unwrap(),
            );
        }
        headers
    }

    fn ready_headers() -> Vec<(&'static str, &'static str)> {
        const SHA256: &str = concat!(
            "aaaaaaaa", "aaaaaaaa", "aaaaaaaa", "aaaaaaaa", "aaaaaaaa", "aaaaaaaa", "aaaaaaaa",
            "aaaaaaaa",
        );
        vec![
            (PHOTO_ID_HEADER, "p1"),
            (STEP_ID_HEADER, "develop-1"),
            (WIDTH_HEADER, "8"),
            (HEIGHT_HEADER, "4"),
            (SHA256_HEADER, SHA256),
            (SOURCE_REVISION_HEADER, "736f757263652d33"),
            (RECIPE_REVISION_HEADER, "recipe-9"),
            (MODULE_HEADER, "darktable"),
            (
                ADAPTER_SCHEMA_VERSION_HEADER,
                "darktable-adapter-1:darktable-params-1",
            ),
            (PARAMETER_DIGEST_HEADER, SHA256),
            (OUTPUT_CONTRACT_HEADER, SHA256),
            (DISPLAY_CONVERSION_HEADER, "display-transform-v1"),
            (IDENTITY_HEADER, SHA256),
        ]
    }

    #[test]
    fn a_ready_rendition_reports_its_validated_identity() {
        let metadata =
            rendition_metadata(&headers(&ready_headers()), "p1", "develop-1").expect("identity");
        assert_eq!(metadata.source_revision, "source-3");
        assert_eq!(metadata.recipe_revision, "recipe-9");
        assert_eq!((metadata.width, metadata.height), (8, 4));
        assert_eq!(metadata.sha256, "a".repeat(64));
    }

    #[test]
    fn a_foreign_photo_or_step_fact_refuses_the_whole_response() {
        let mut foreign_photo = ready_headers();
        foreign_photo[0] = (PHOTO_ID_HEADER, "p2");
        assert!(rendition_metadata(&headers(&foreign_photo), "p1", "develop-1").is_none());
        let mut foreign_step = ready_headers();
        foreign_step[1] = (STEP_ID_HEADER, "film-1");
        assert!(rendition_metadata(&headers(&foreign_step), "p1", "develop-1").is_none());
    }

    #[test]
    fn a_missing_repeated_or_malformed_fact_refuses_the_response() {
        let base = ready_headers();
        let missing = base[..base.len() - 1].to_vec();
        assert!(rendition_metadata(&headers(&missing), "p1", "develop-1").is_none());
        let mut malformed = base.clone();
        malformed[2] = (WIDTH_HEADER, "0");
        assert!(rendition_metadata(&headers(&malformed), "p1", "develop-1").is_none());
        let mut bad_digest = base.clone();
        bad_digest[4] = (SHA256_HEADER, "not-a-digest");
        assert!(rendition_metadata(&headers(&bad_digest), "p1", "develop-1").is_none());
        let mut odd_hex = base.clone();
        odd_hex[5] = (SOURCE_REVISION_HEADER, "736");
        assert!(rendition_metadata(&headers(&odd_hex), "p1", "develop-1").is_none());
        let mut repeated = headers(&base);
        repeated.append(
            reqwest::header::HeaderName::from_bytes(STEP_ID_HEADER.as_bytes()).unwrap(),
            reqwest::header::HeaderValue::from_str("develop-1").unwrap(),
        );
        assert!(rendition_metadata(&repeated, "p1", "develop-1").is_none());
    }

    #[test]
    fn the_digest_admits_only_the_bytes_the_receipt_names() {
        let bytes = b"rendition bytes";
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        let claimed = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert!(digest_matches(bytes, &claimed));
        assert!(!digest_matches(b"other bytes", &claimed));
        assert!(!digest_matches(bytes, "a"));
    }

    #[test]
    fn a_pending_admission_is_accepted_only_for_the_requested_step() {
        let pending = |state: &str, step_id: &str| PendingPreview {
            state: state.to_owned(),
            step_id: step_id.to_owned(),
        };
        assert_eq!(
            pending("queued", "develop-1").validated_state("develop-1"),
            Some("queued")
        );
        assert_eq!(
            pending("running", "develop-1").validated_state("develop-1"),
            Some("running")
        );
        assert_eq!(
            pending("queued", "film-1").validated_state("develop-1"),
            None
        );
        assert_eq!(
            pending("settled", "develop-1").validated_state("develop-1"),
            None
        );
    }
}
