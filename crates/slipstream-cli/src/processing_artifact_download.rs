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
use reqwest::{StatusCode, header};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::SeekFrom;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

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
#[cfg(test)]
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
const PART: &str = ".slipstream-part";
const META: &str = ".slipstream-part.json";

fn clear_private_state(destination: &Destination) {
    let _ = destination.remove_private(PART);
    let _ = destination.remove_private(META);
}

fn is_terminal_artifact_failure(failure: &CommandFailure) -> bool {
    matches!(
        failure.payload.code.as_str(),
        "artifact_expired" | "unknown_artifact"
    )
}

/// Downloads into a private resumable file. Only the completed private inode
/// is linked into the requested destination after every byte is validated.
pub(super) async fn download(
    client: &ServiceClient,
    artifact_id: &str,
    destination: Destination,
    publication: &PublicationState,
    idle_timeout: Duration,
) -> Result<Value, CommandFailure> {
    let artifact = match tokio::time::timeout(
        idle_timeout,
        development::processing_artifact(client, artifact_id),
    )
    .await
    {
        Err(_) => return Err(CommandFailure::transport(OPERATION)),
        Ok(Ok(artifact)) => artifact,
        Ok(Err(failure)) => {
            if is_terminal_artifact_failure(&failure) {
                clear_private_state(&destination);
            }
            return Err(failure);
        }
    };
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
    let filename = artifact["filename"].as_str().unwrap_or_default();
    let Some(byte_length) = byte_length.filter(|length| *length > 0) else {
        return Err(CommandFailure::transport(OPERATION));
    };
    if width.is_none() || height.is_none() || sha256.len() != 64 || filename.is_empty() {
        return Err(CommandFailure::transport(OPERATION));
    }
    let identity = json!({
        "artifactId": artifact_id, "filename": filename, "byteLength": byte_length,
        "sha256": sha256, "contentType": content_type, "outputFormat": output_format,
    });
    let matching = destination
        .private_bytes(META)?
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|value| value == identity);
    let mut partial = destination.private_file(PART, false)?;
    destination.lock_private(&partial)?;
    let mut offset = if matching {
        partial
            .metadata()
            .await
            .map_err(|_| destination.local_io())?
            .len()
    } else {
        0
    };
    if !matching || offset > byte_length {
        partial
            .set_len(0)
            .await
            .map_err(|_| destination.local_io())?;
        offset = 0;
    }
    if !matching || offset == 0 {
        let mut sidecar = destination.private_file(META, true)?;
        sidecar
            .write_all(identity.to_string().as_bytes())
            .await
            .map_err(|_| destination.local_io())?;
        sidecar
            .sync_all()
            .await
            .map_err(|_| destination.local_io())?;
    }
    let mut hasher = Sha256::new();
    if offset > 0 {
        partial
            .seek(SeekFrom::Start(0))
            .await
            .map_err(|_| destination.local_io())?;
        let mut remaining = offset;
        let mut buf = [0_u8; 64 * 1024];
        while remaining > 0 {
            let read = partial
                .read(&mut buf)
                .await
                .map_err(|_| destination.local_io())?;
            if read == 0 {
                break;
            }
            let take = read.min(remaining as usize);
            hasher.update(&buf[..take]);
            remaining -= take as u64;
        }
        if remaining != 0 || offset == byte_length {
            // A complete partial is never trusted as a cache hit; always
            // validate by downloading the immutable object again.
            partial
                .set_len(0)
                .await
                .map_err(|_| destination.local_io())?;
            offset = 0;
            hasher = Sha256::new();
        }
    }
    let mut request = client
        .client
        .get(client.endpoint(&["api", "processing-artifacts", artifact_id, "bytes"]))
        .header(CONTRACT_HEADER, CLI_CONTRACT_VERSION)
        .bearer_auth(&client.token);
    if offset > 0 && offset < byte_length {
        request = request
            .header(header::RANGE, format!("bytes={offset}-"))
            .header(header::IF_RANGE, format!("\"{sha256}\""));
    }
    let mut response = tokio::time::timeout(idle_timeout, request.send())
        .await
        .map_err(|_| CommandFailure::transport(OPERATION))?
        .map_err(|_| CommandFailure::transport(OPERATION))?;
    let status = response.status();
    if status.is_redirection() {
        return Err(CommandFailure::transport(OPERATION));
    }
    if let Some(failure) = access_boundary_failure(status, None, &[], OPERATION) {
        return Err(failure);
    }
    if status != StatusCode::OK && status != StatusCode::PARTIAL_CONTENT {
        let bytes = tokio::time::timeout(idle_timeout, response_bytes(response, OPERATION))
            .await
            .map_err(|_| CommandFailure::transport(OPERATION))??;
        let error = serde_json::from_slice::<ErrorResponse>(&bytes)
            .map_err(|_| CommandFailure::transport(OPERATION))?
            .error;
        let failure = validated_route_failure(error, OPERATION, &client.token)
            .unwrap_or_else(|| CommandFailure::transport(OPERATION));
        if is_terminal_artifact_failure(&failure) {
            clear_private_state(&destination);
        }
        return Err(failure);
    }
    let ranged = offset > 0 && status == StatusCode::PARTIAL_CONTENT;
    if offset > 0 && status == StatusCode::OK {
        partial
            .set_len(0)
            .await
            .map_err(|_| destination.local_io())?;
        offset = 0;
        hasher = Sha256::new();
    }
    let expected_length = byte_length - offset;
    let valid_range = if ranged {
        response
            .headers()
            .get(header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("bytes "))
            .and_then(|v| v.strip_suffix(&format!("/{byte_length}")))
            .and_then(|v| v.split_once('-'))
            .and_then(|(start, end)| Some((start.parse::<u64>().ok()?, end.parse::<u64>().ok()?)))
            .is_some_and(|(start, end)| start == offset && end.checked_add(1) == Some(byte_length))
    } else {
        status == StatusCode::OK
    };
    let header_is =
        |name: &str, expected: &str| header_value(response.headers(), name) == Some(expected);
    let etag = format!("\"{sha256}\"");
    if !header_is("etag", &etag)
        || !header_is("accept-ranges", "bytes")
        || !valid_range
        || response.content_length() != Some(expected_length)
        || !header_is("content-type", content_type)
        || !header_is("slipstream-artifact-id", artifact_id)
        || !header_is("slipstream-artifact-photo-id", photo_id)
        || !header_is("slipstream-artifact-filename", filename)
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
        clear_private_state(&destination);
        return Err(CommandFailure::transport(OPERATION));
    }
    partial
        .seek(SeekFrom::Start(offset))
        .await
        .map_err(|_| destination.local_io())?;
    let mut received = offset;
    while let Some(chunk) = tokio::time::timeout(idle_timeout, response.chunk())
        .await
        .map_err(|_| CommandFailure::transport(OPERATION))?
        .map_err(|_| CommandFailure::transport(OPERATION))?
    {
        if received.saturating_add(chunk.len() as u64) > byte_length {
            clear_private_state(&destination);
            return Err(CommandFailure::transport(OPERATION));
        }
        partial
            .write_all(&chunk)
            .await
            .map_err(|_| destination.local_io())?;
        partial
            .sync_data()
            .await
            .map_err(|_| destination.local_io())?;
        hasher.update(&chunk);
        received = received.saturating_add(chunk.len() as u64);
    }
    if received != byte_length || format!("{:x}", hasher.finalize()) != sha256 {
        if received == byte_length {
            // Every expected byte arrived but the immutable identity does not
            // match, so this partial can never settle; discard it instead of
            // letting future retries resume the same corrupt prefix.
            clear_private_state(&destination);
        }
        return Err(CommandFailure::transport(OPERATION));
    }
    partial
        .sync_all()
        .await
        .map_err(|_| destination.local_io())?;
    let web_url = web_url(&client.origin, &format!("/?photoId={photo_id}"))
        .map_err(|()| CommandFailure::transport(OPERATION))?;
    let mut data = json!({
        "artifactId": artifact_id, "photoId": photo_id, "stepId": artifact["stepId"],
        "module": artifact["module"], "filename": filename, "expiresAt": artifact["expiresAt"],
        "bundleId": artifact["bundleId"], "adapterSchemaVersion": artifact["adapterSchemaVersion"],
        "width": width, "height": height, "byteLength": byte_length, "sha256": sha256,
        "path": destination.path(), "webUrl": web_url, "fileCommitted": true,
    });
    super::redact_value(&mut data, &client.token);
    destination.publish(&partial)?;
    publication.record("Processing Artifact", data.clone());
    clear_private_state(&destination);
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
