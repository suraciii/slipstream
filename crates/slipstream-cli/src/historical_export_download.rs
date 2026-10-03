//! Read-only delivery of image Exports retained from the historical surface.
use super::preview_download::Destination;
use super::{
    CLI_CONTRACT_VERSION, CONTRACT_HEADER, CommandFailure, ErrorResponse,
    MAXIMUM_SOURCE_REVISION_BYTES, Method, Operation, PublicationState, ServiceClient, StatusCode,
    Value, access_boundary_failure, json, response_bytes, valid_sha256, valid_utc_time,
    validated_route_failure, web_url,
};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

const OPERATION: Operation = Operation::PhotosHistoricalExportDownload;

pub(crate) fn record_valid(record: &Value, photo_id: &str) -> bool {
    let text = |key: &str, max: usize| {
        record[key]
            .as_str()
            .is_some_and(|value| !value.is_empty() && value.len() <= max)
    };
    let state = record["state"].as_str();
    let terminal = matches!(state, Some("succeeded" | "failed" | "cancelled"));
    text("exportId", 128)
        && text("photoId", 128)
        && (photo_id.is_empty() || record["photoId"] == json!(photo_id))
        && text("recipeVersion", 128)
        && text("sourceRevision", MAXIMUM_SOURCE_REVISION_BYTES)
        && text("bundleId", 128)
        && matches!(
            record["target"].as_str(),
            Some("development-tiff" | "film-jpeg")
        )
        && matches!(
            state,
            Some("queued" | "running" | "succeeded" | "failed" | "cancelled")
        )
        && record["createdAt"].as_str().is_some_and(valid_utc_time)
        && match record.get("settledAt") {
            Some(Value::Null) => !terminal,
            Some(Value::String(time)) => terminal && valid_utc_time(time),
            _ => false,
        }
        && match record.get("terminalOutcome") {
            Some(Value::Null) => !terminal,
            Some(Value::String(outcome)) => terminal && Some(outcome.as_str()) == state,
            _ => false,
        }
        && matches!(
            record.get("failureReason"),
            Some(Value::Null) | Some(Value::String(_))
        )
        && match record.get("receiptExpiresAt") {
            Some(Value::Null) => true,
            Some(Value::String(time)) => valid_utc_time(time),
            _ => false,
        }
        && match record.get("artifact") {
            Some(Value::Null) => true,
            Some(artifact) => state == Some("succeeded") && artifact_valid(artifact, record),
            None => false,
        }
}

fn artifact_valid(artifact: &Value, record: &Value) -> bool {
    let text = |key: &str| {
        artifact[key]
            .as_str()
            .is_some_and(|value| !value.is_empty())
    };
    let medium = match record["target"].as_str() {
        Some("development-tiff") => ("develop", "image/tiff"),
        Some("film-jpeg") => ("film", "image/jpeg"),
        _ => return false,
    };
    artifact["exportId"] == record["exportId"]
        && artifact["target"] == record["target"]
        && artifact["stage"] == json!(medium.0)
        && artifact["contentType"] == json!(medium.1)
        && [
            "filename",
            "orientation",
            "sampleFormat",
            "colorSpace",
            "profileIdentity",
        ]
        .iter()
        .all(|key| text(key))
        && artifact["filename"]
            .as_str()
            .is_some_and(|name| !name.contains(['/', '\\']) && !name.chars().any(char::is_control))
        && artifact["iccEmbedded"].is_boolean()
        && artifact["width"].as_u64().is_some_and(|value| value > 0)
        && artifact["height"].as_u64().is_some_and(|value| value > 0)
        && artifact["byteLength"]
            .as_u64()
            .is_some_and(|value| value > 0)
        && artifact["sha256"].as_str().is_some_and(valid_sha256)
        && artifact["expiresAt"].as_str().is_some_and(valid_utc_time)
}

pub(super) async fn download(
    client: &ServiceClient,
    export_id: &str,
    destination: Destination,
    publication: &PublicationState,
) -> Result<Value, CommandFailure> {
    let record: Value = client
        .json(
            OPERATION,
            Method::GET,
            client.endpoint(&["api", "exports", export_id]),
            None,
        )
        .await?;
    if !record_valid(&record, "") || record["exportId"] != json!(export_id) {
        return Err(CommandFailure::transport(OPERATION));
    }
    let artifact = &record["artifact"];
    // The bytes route supplies the retained service refusal for absent or expired output.
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
    let Some(length) = artifact["byteLength"].as_u64() else {
        return Err(CommandFailure::transport(OPERATION));
    };
    let header_is = |name: &str, expected: &str| {
        let mut values = response.headers().get_all(name).iter();
        values.next().and_then(|value| value.to_str().ok()) == Some(expected)
            && values.next().is_none()
    };
    if response.content_length() != Some(length)
        || !header_is(
            "content-type",
            artifact["contentType"].as_str().unwrap_or_default(),
        )
        || ![
            ("slipstream-artifact-export-id", "exportId"),
            ("slipstream-artifact-target", "target"),
            ("slipstream-artifact-stage", "stage"),
            ("slipstream-artifact-content-type", "contentType"),
            ("slipstream-artifact-filename", "filename"),
            ("slipstream-artifact-orientation", "orientation"),
            ("slipstream-artifact-sample-format", "sampleFormat"),
            ("slipstream-artifact-color-space", "colorSpace"),
            ("slipstream-artifact-profile-identity", "profileIdentity"),
            ("slipstream-artifact-sha256", "sha256"),
            ("slipstream-artifact-expires-at", "expiresAt"),
        ]
        .iter()
        .all(|(header, key)| header_is(header, artifact[*key].as_str().unwrap_or_default()))
        || ![
            ("slipstream-artifact-width", "width"),
            ("slipstream-artifact-height", "height"),
            ("slipstream-artifact-byte-length", "byteLength"),
            ("slipstream-artifact-icc-embedded", "iccEmbedded"),
        ]
        .iter()
        .all(|(header, key)| header_is(header, &artifact[*key].to_string()))
    {
        return Err(CommandFailure::transport(OPERATION));
    }
    let mut file = destination.anonymous_file()?;
    let mut received = 0_u64;
    let mut digest = Sha256::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| CommandFailure::transport(OPERATION))?
    {
        received = received
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| CommandFailure::transport(OPERATION))?;
        if received > length {
            return Err(CommandFailure::transport(OPERATION));
        }
        digest.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|_| destination.local_io())?;
    }
    if received != length
        || format!("{:x}", digest.finalize()) != artifact["sha256"].as_str().unwrap_or_default()
    {
        return Err(CommandFailure::transport(OPERATION));
    }
    file.sync_all().await.map_err(|_| destination.local_io())?;
    let photo_id = record["photoId"].as_str().unwrap_or_default();
    let mut data = json!({"exportId": export_id, "photoId": photo_id, "historical": true,
        "artifact": artifact, "path": destination.path(), "fileCommitted": true,
        "webUrl": web_url(&client.origin, &format!("/?photoId={photo_id}")).map_err(|_| CommandFailure::transport(OPERATION))?});
    super::redact_value(&mut data, &client.token);
    destination.publish(&file)?;
    publication.record("Historical Export", data.clone());
    if !destination.fsync_directory() {
        return Err(CommandFailure::published_file(
            data,
            false,
            "Historical Export",
        ));
    }
    Ok(data)
}
