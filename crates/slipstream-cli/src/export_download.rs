//! Export artifact download: one inspect read names the artifact object the
//! download must repeat header for header, and the transfer publishes a
//! complete file without replacement, as the Preview download does. Both
//! closed targets stream the same way; the target only fixes the validated
//! stage and media type.

use super::preview_download::Destination;
use super::{
    CLI_CONTRACT_VERSION, CONTRACT_HEADER, CommandFailure, ErrorResponse, ExportInspectWire,
    Operation, PublicationState, ServiceClient, access_boundary_failure, response_bytes,
    validated_export_inspect, validated_route_failure,
};
use reqwest::{StatusCode, header};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

const OPERATION: Operation = Operation::PhotosExportDownload;

pub(super) async fn download(
    client: &ServiceClient,
    export_id: &str,
    destination: Destination,
    publication: &PublicationState,
) -> Result<Value, CommandFailure> {
    // The receipt read names the artifact object the download must repeat
    // header for header. When nothing validated is retained, the artifact
    // route below owns the closed per-state refusal instead.
    let inspect: ExportInspectWire = client
        .json(
            OPERATION,
            reqwest::Method::GET,
            client.endpoint(&["api", "exports", export_id]),
            None,
        )
        .await?;
    let inspect = validated_export_inspect(inspect, export_id, OPERATION)?;
    let mut response = client
        .client
        .get(client.endpoint(&["api", "exports", export_id, "artifact"]))
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
    // A 200 stream without the trusted artifact object read above cannot be
    // validated against anything.
    let Some(artifact) = inspect.artifact else {
        return Err(CommandFailure::transport(OPERATION));
    };
    let header_is = |name: &str, expected: &str| {
        let mut values = response.headers().get_all(name).iter();
        values.next().and_then(|value| value.to_str().ok()) == Some(expected)
            && values.next().is_none()
    };
    if response.content_length() != Some(artifact.byte_length)
        || response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            != Some(artifact.content_type.as_str())
        || !header_is("slipstream-artifact-export-id", &artifact.export_id)
        || !header_is("slipstream-artifact-target", &artifact.target)
        || !header_is("slipstream-artifact-stage", &artifact.stage)
        || !header_is("slipstream-artifact-content-type", &artifact.content_type)
        || !header_is("slipstream-artifact-filename", &artifact.filename)
        || !header_is("slipstream-artifact-orientation", &artifact.orientation)
        || !header_is("slipstream-artifact-sample-format", &artifact.sample_format)
        || !header_is("slipstream-artifact-color-space", &artifact.color_space)
        || !header_is(
            "slipstream-artifact-icc-embedded",
            &artifact.icc_embedded.to_string(),
        )
        || !header_is("slipstream-artifact-width", &artifact.width.to_string())
        || !header_is("slipstream-artifact-height", &artifact.height.to_string())
        || !header_is(
            "slipstream-artifact-profile-identity",
            &artifact.profile_identity,
        )
        || !header_is(
            "slipstream-artifact-byte-length",
            &artifact.byte_length.to_string(),
        )
        || !header_is("slipstream-artifact-sha256", &artifact.sha256)
        || !header_is("slipstream-artifact-expires-at", &artifact.expires_at)
    {
        return Err(CommandFailure::transport(OPERATION));
    }
    let mut file = destination.anonymous_file()?;
    let mut received: u64 = 0;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| CommandFailure::transport(OPERATION))?
    {
        received = received.saturating_add(chunk.len() as u64);
        if received > artifact.byte_length {
            return Err(CommandFailure::transport(OPERATION));
        }
        file.write_all(&chunk)
            .await
            .map_err(|_| destination.local_io())?;
    }
    if received != artifact.byte_length {
        return Err(CommandFailure::transport(OPERATION));
    }
    file.sync_all().await.map_err(|_| destination.local_io())?;
    let mut data = json!({
        "exportId": artifact.export_id,
        "path": destination.path(),
        "target": artifact.target,
        "stage": artifact.stage,
        "contentType": artifact.content_type,
        "filename": artifact.filename,
        "orientation": artifact.orientation,
        "sampleFormat": artifact.sample_format,
        "colorSpace": artifact.color_space,
        "iccEmbedded": artifact.icc_embedded,
        "width": artifact.width,
        "height": artifact.height,
        "profileIdentity": artifact.profile_identity,
        "byteLength": artifact.byte_length,
        "sha256": artifact.sha256,
        "expiresAt": artifact.expires_at,
        "fileCommitted": true,
    });
    super::redact_value(&mut data, &client.token);
    destination.publish(&file)?;
    publication.record("Export", data.clone());
    if !destination.fsync_directory() {
        return Err(CommandFailure::published_file(data, false, "Export"));
    }
    Ok(data)
}
