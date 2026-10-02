//! Processing Artifact bytes download: one provenance read names the
//! immutable artifact object the transfer must repeat header for header, and
//! the bytes are validated by length and by recomputed SHA-256 digest before
//! the file publishes without replacement, as the Export download does. The
//! artifact is a full-resolution immutable result, never a preview or a
//! legacy stage rendition, and its record travels with the file.

use super::preview_download::Destination;
use super::{
    CLI_CONTRACT_VERSION, CONTRACT_HEADER, CommandFailure, ErrorResponse, Operation,
    PublicationState, ServiceClient, access_boundary_failure, development, response_bytes,
    validated_route_failure, web_url,
};
use reqwest::StatusCode;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

const OPERATION: Operation = Operation::ProcessingArtifactDownload;

/// The closed media mapping of a published Processing Artifact. The
/// provenance record selects the peer's immutable output medium; no client
/// default may turn a JPEG into a TIFF.
fn artifact_content_type(format: &str) -> Option<&'static str> {
    match format.strip_prefix("image/").unwrap_or(format) {
        "tiff" => Some("image/tiff"),
        "jpeg" => Some("image/jpeg"),
        _ => None,
    }
}

/// One value of `name`, as text. A repeated or non-text value is not one
/// fact the provenance record framed.
fn header_value<'a>(headers: &'a reqwest::header::HeaderMap, name: &str) -> Option<&'a str> {
    let mut values = headers.get_all(name).iter();
    values
        .next()
        .and_then(|value| value.to_str().ok())
        .filter(|_| values.next().is_none())
}

/// Recomputes the digest of the received bytes and compares it with the
/// record's published digest, so only the bytes the record named publish.
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

/// Performs one validated Processing Artifact download. The provenance read
/// names the artifact the transfer must repeat field for field; the streamed
/// bytes are checked by declared length and recomputed digest and only then
/// published without replacement. A refusal or an unidentifiable response
/// publishes nothing.
pub(super) async fn download(
    client: &ServiceClient,
    artifact_id: &str,
    destination: Destination,
    publication: &PublicationState,
) -> Result<Value, CommandFailure> {
    // The provenance record is the trusted object the transfer validates
    // against; without it a 200 stream cannot be attributed to anything.
    let artifact = development::processing_artifact(client, artifact_id).await?;
    let output_format = artifact["outputContract"]["format"]
        .as_str()
        .unwrap_or_default();
    let Some(content_type) = artifact_content_type(output_format) else {
        return Err(CommandFailure::transport(OPERATION));
    };
    let byte_length = artifact["byteLength"].as_u64();
    let sha256 = artifact["sha256"].as_str().unwrap_or_default();
    let width = artifact["outputContract"]["geometry"]["width"].as_u64();
    let height = artifact["outputContract"]["geometry"]["height"].as_u64();
    let photo_id = artifact["photoId"].as_str().unwrap_or_default();
    let Some(byte_length) = byte_length.filter(|length| *length > 0) else {
        return Err(CommandFailure::transport(OPERATION));
    };
    if width.is_none() || height.is_none() || sha256.is_empty() {
        return Err(CommandFailure::transport(OPERATION));
    }
    let mut response = client
        .client
        .get(client.endpoint(&["api", "processing-artifacts", artifact_id, "bytes"]))
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
    // A 200 stream without the trusted artifact record read above cannot be
    // validated against anything; the record's own facts are repeated header
    // for header, so a substituted, truncated, or differently sized artifact
    // is refused instead of published.
    let header_is =
        |name: &str, expected: &str| header_value(response.headers(), name) == Some(expected);
    if response.content_length() != Some(byte_length)
        || !header_is("content-type", content_type)
        || !header_is("slipstream-artifact-id", artifact_id)
        || !header_is("slipstream-artifact-photo-id", photo_id)
        || !header_is(
            "slipstream-artifact-filename",
            artifact["filename"].as_str().unwrap_or_default(),
        )
        || !header_is(
            "slipstream-artifact-step-id",
            artifact["stepId"].as_str().unwrap_or_default(),
        )
        || !header_is(
            "slipstream-artifact-module",
            artifact["module"].as_str().unwrap_or_default(),
        )
        || !header_is(
            "slipstream-artifact-adapter-schema-version",
            artifact["adapterSchemaVersion"]
                .as_str()
                .unwrap_or_default(),
        )
        || !header_is(
            "slipstream-artifact-bundle-id",
            artifact["bundleId"].as_str().unwrap_or_default(),
        )
        || !header_is(
            "slipstream-artifact-width",
            &width.unwrap_or_default().to_string(),
        )
        || !header_is(
            "slipstream-artifact-height",
            &height.unwrap_or_default().to_string(),
        )
        || !header_is("slipstream-artifact-byte-length", &byte_length.to_string())
        || !header_is("slipstream-artifact-sha256", sha256)
    {
        return Err(CommandFailure::transport(OPERATION));
    }
    let mut file = destination.anonymous_file()?;
    let mut bytes = Vec::new();
    let maximum = usize::try_from(byte_length).unwrap_or(usize::MAX);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| CommandFailure::transport(OPERATION))?
    {
        if bytes.len().saturating_add(chunk.len()) > maximum {
            return Err(CommandFailure::transport(OPERATION));
        }
        file.write_all(&chunk)
            .await
            .map_err(|_| destination.local_io())?;
        bytes.extend_from_slice(&chunk);
    }
    if bytes.len() != maximum || !digest_matches(&bytes, sha256) {
        return Err(CommandFailure::transport(OPERATION));
    }
    file.sync_all().await.map_err(|_| destination.local_io())?;
    let web_url = web_url(&client.origin, &format!("/?photoId={photo_id}"))
        .map_err(|()| CommandFailure::transport(OPERATION))?;
    let mut data = json!({
        "artifactId": artifact_id,
        "photoId": photo_id,
        "stepId": artifact["stepId"],
        "module": artifact["module"],
        "filename": artifact["filename"],
        "expiresAt": artifact["expiresAt"],
        "bundleId": artifact["bundleId"],
        "adapterSchemaVersion": artifact["adapterSchemaVersion"],
        "width": width,
        "height": height,
        "byteLength": byte_length,
        "sha256": sha256,
        "path": destination.path(),
        "webUrl": web_url,
        "fileCommitted": true,
    });
    super::redact_value(&mut data, &client.token);
    destination.publish(&file)?;
    publication.record("Processing Artifact", data.clone());
    if !destination.fsync_directory() {
        return Err(CommandFailure::published_file(
            data,
            false,
            "Processing Artifact",
        ));
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::{artifact_content_type, digest_matches};

    #[test]
    fn a_matching_digest_confirms_only_the_named_bytes() {
        // sha256 of the empty string; a published record repeats it exactly.
        let empty = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert!(digest_matches(&[], empty));
        assert!(!digest_matches(b"artifact", empty));
        // A digest that is not 64 hex characters is never a match.
        assert!(!digest_matches(
            &[],
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b85"
        ));
    }

    #[test]
    fn artifact_medium_follows_the_published_output_contract() {
        assert_eq!(artifact_content_type("tiff"), Some("image/tiff"));
        assert_eq!(artifact_content_type("image/jpeg"), Some("image/jpeg"));
        assert_eq!(artifact_content_type("png"), None);
    }
}
