use super::*;
pub(crate) async fn download(
    client: &ServiceClient,
    operation: Operation,
    url: Url,
    destination: preview_download::Destination,
    publication: &PublicationState,
    evidence: Option<&Value>,
) -> Result<Value, CommandFailure> {
    let mut response = tokio::time::timeout(
        client.control_timeout,
        client
            .client
            .get(url.clone())
            .header(CONTRACT_HEADER, CLI_CONTRACT_VERSION)
            .bearer_auth(&client.token)
            .send(),
    )
    .await
    .map_err(|_| CommandFailure::transport(operation))?
    .map_err(|_| CommandFailure::transport(operation))?;
    if response.status() != StatusCode::OK {
        let status = response.status();
        let bytes = response_bytes(response, operation).await?;
        return Err(transfer_failure(status, &bytes, operation, &client.token));
    }
    let length = response
        .headers()
        .get("Content-Length")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| CommandFailure::transport(operation))?;
    let digest = response
        .headers()
        .get("X-Sha256")
        .or_else(|| response.headers().get("X-Content-Sha256"))
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .ok_or_else(|| CommandFailure::transport(operation))?;
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        || length == 0
    {
        return Err(CommandFailure::transport(operation));
    }
    let revision = response
        .headers()
        .get("X-Revision")
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| CommandFailure::transport(operation))?;
    if let Some(evidence) = evidence
        && (evidence["sha256"] != digest
            || evidence["byteLength"] != length
            || evidence
                .get("revision")
                .is_some_and(|value| value != &json!(revision)))
    {
        return Err(CommandFailure::transport(operation));
    }
    let header_contract = response
        .headers()
        .get("X-Image-Contract")
        .map(|header| {
            header
                .to_str()
                .ok()
                .and_then(|text| serde_json::from_str::<Value>(text).ok())
                .filter(Value::is_object)
                .ok_or_else(|| CommandFailure::transport(operation))
        })
        .transpose()?;
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    let identity =
        json!({"url":url.as_str(),"revision":revision,"sha256":digest,"byteLength":length});
    let matching = destination
        .private_bytes(".slipstream-part.json")?
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|value| value == identity);
    let mut file = destination.private_file(".slipstream-part", false)?;
    destination.lock_private(&file)?;
    let mut received = if matching {
        file.metadata()
            .await
            .map_err(|_| destination.local_io())?
            .len()
    } else {
        0
    };
    if received >= length {
        received = 0;
    }
    let mut hasher = Sha256::new();
    if received > 0 {
        file.seek(std::io::SeekFrom::Start(0))
            .await
            .map_err(|_| destination.local_io())?;
        let mut remaining = received;
        let mut buffer = [0u8; 65536];
        while remaining > 0 {
            let count = file
                .read(&mut buffer[..remaining.min(65536) as usize])
                .await
                .map_err(|_| destination.local_io())?;
            if count == 0 {
                return Err(destination.local_io());
            }
            hasher.update(&buffer[..count]);
            remaining -= count as u64;
        }
        drop(response);
        response = tokio::time::timeout(
            client.control_timeout,
            client
                .client
                .get(url.clone())
                .header(CONTRACT_HEADER, CLI_CONTRACT_VERSION)
                .bearer_auth(&client.token)
                .header("Range", format!("bytes={received}-"))
                .header("If-Range", format!("\"{revision}\""))
                .send(),
        )
        .await
        .map_err(|_| CommandFailure::transport(operation))?
        .map_err(|_| CommandFailure::transport(operation))?;
        if response.status() == StatusCode::PARTIAL_CONTENT {
            let expected = format!("bytes {received}-{}/{}", length - 1, length);
            if response
                .headers()
                .get("Content-Range")
                .and_then(|value| value.to_str().ok())
                != Some(expected.as_str())
                || response
                    .headers()
                    .get("X-Revision")
                    .and_then(|value| value.to_str().ok())
                    != Some(revision.as_str())
                || response
                    .headers()
                    .get("X-Sha256")
                    .and_then(|value| value.to_str().ok())
                    != Some(digest.as_str())
            {
                return Err(CommandFailure::transport(operation));
            }
        } else if response.status() == StatusCode::OK {
            if response
                .headers()
                .get("X-Revision")
                .and_then(|value| value.to_str().ok())
                != Some(revision.as_str())
                || response
                    .headers()
                    .get("X-Sha256")
                    .and_then(|value| value.to_str().ok())
                    != Some(digest.as_str())
                || response.content_length() != Some(length)
            {
                return Err(CommandFailure::transport(operation));
            }
            received = 0;
            hasher = Sha256::new();
        } else {
            let status = response.status();
            let bytes = response_bytes(response, operation).await?;
            return Err(transfer_failure(status, &bytes, operation, &client.token));
        }
    }
    if received == 0 {
        file.set_len(0).await.map_err(|_| destination.local_io())?;
        file.seek(std::io::SeekFrom::Start(0))
            .await
            .map_err(|_| destination.local_io())?;
    }
    let mut metadata = destination.private_file(".slipstream-part.json", true)?;
    metadata
        .write_all(identity.to_string().as_bytes())
        .await
        .map_err(|_| destination.local_io())?;
    metadata
        .sync_all()
        .await
        .map_err(|_| destination.local_io())?;
    loop {
        let chunk = tokio::time::timeout(client.control_timeout, response.chunk())
            .await
            .map_err(|_| CommandFailure::transport(operation))?
            .map_err(|_| CommandFailure::transport(operation))?;
        let Some(chunk) = chunk else { break };
        received = received
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| CommandFailure::transport(operation))?;
        if received > length {
            return Err(CommandFailure::transport(operation));
        }
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|_| destination.local_io())?;
    }
    if received != length || format!("{:x}", hasher.finalize()) != digest {
        return Err(CommandFailure::transport(operation));
    }
    file.sync_all().await.map_err(|_| destination.local_io())?;
    let image_contract = evidence
        .and_then(|value| {
            value
                .get("imageContract")
                .or_else(|| value.get("outputContract"))
        })
        .filter(|value| !value.is_null())
        .cloned()
        .or(header_contract);
    if let Some(contract) = image_contract {
        let mut retained = file
            .try_clone()
            .await
            .map_err(|_| destination.local_io())?
            .into_std()
            .await;
        let expected_digest = digest.clone();
        tokio::task::spawn_blocking(move || {
            let facts = photo_files::image::inspect_image(&mut retained, length)
                .map_err(|_| CommandFailure::transport(operation))?;
            let format = contract["format"].as_str().unwrap_or("");
            let format = format.strip_prefix("image/").unwrap_or(format);
            let width = contract
                .get("width")
                .or_else(|| contract["geometry"].get("width"));
            let height = contract
                .get("height")
                .or_else(|| contract["geometry"].get("height"));
            if facts.sha256 != expected_digest
                || format != facts.format
                || contract["precision"] != facts.precision
                || width.and_then(Value::as_u64) != Some(u64::from(facts.width))
                || height.and_then(Value::as_u64) != Some(u64::from(facts.height))
            {
                return Err(CommandFailure::transport(operation));
            }
            let color = contract["colorSpace"].as_str();
            let transfer = contract
                .get("transfer")
                .or_else(|| contract.get("transferFunction"))
                .and_then(Value::as_str);
            let expected = match (facts.format, facts.precision, color, transfer) {
                ("tiff", "float32", Some("prophoto-rgb" | "prophoto"), Some("linear")) => {
                    let observed = facts
                        .icc_sha256
                        .as_deref()
                        .filter(|digest| photo_files::image::tiff::is_qualified_profile(digest))
                        .ok_or_else(|| CommandFailure::transport(operation))?;
                    Some(observed)
                }
                ("jpeg", "uint8", Some("srgb"), Some("srgb")) => {
                    Some(photo_files::image::jpeg::OUTPUT_ICC_SHA256)
                }
                _ => contract
                    .get("profileSha256")
                    .or_else(|| contract["profile"].get("sha256"))
                    .and_then(Value::as_str),
            };
            if let Some(expected) = expected {
                if facts.icc_sha256.as_deref() != Some(expected) {
                    return Err(CommandFailure::transport(operation));
                }
            } else if contract
                .get("profile")
                .is_some_and(|profile| !profile.is_null())
                && facts.icc_sha256.is_none()
            {
                return Err(CommandFailure::transport(operation));
            }
            Ok(())
        })
        .await
        .map_err(|_| CommandFailure::transport(operation))??;
    }
    destination.publish(&file)?;
    let result = json!({"path":destination.path(),"revision":revision,"byteLength":length,"sha256":digest,"fileCommitted":true});
    publication.record("file", result.clone());
    if destination.remove_private(".slipstream-part").is_err()
        || destination.remove_private(".slipstream-part.json").is_err()
    {
        return Err(CommandFailure::published_file(result, false, "file"));
    }
    if !destination.fsync_directory() {
        return Err(CommandFailure::published_file(result, false, "file"));
    }
    Ok(result)
}
