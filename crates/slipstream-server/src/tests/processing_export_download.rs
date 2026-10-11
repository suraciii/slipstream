use super::*;
/// One settled artifact's published bytes download under a finite lease
/// with the record's identity field for field in the response headers.
#[tokio::test]
async fn artifact_bytes_download_under_a_lease_with_provenance_headers() {
    let (base, config) = export_fixture();
    let (application, router) = export_application(&config).await;
    let photo_id = first_photo_id(&application).await;
    let (recipe_revision, source_revision) =
        save_selected_step(&application, &photo_id, "recipe-1").await;

    let body = submit_body(
        "export-bytes",
        "develop-1",
        &recipe_revision,
        &source_revision,
    );
    settle_historical_export(
        &application,
        &photo_id,
        "export-bytes",
        &recipe_revision,
        &source_revision,
    )
    .await;
    let (status, created) = post_processing_export(&router, &photo_id, &body).await;
    assert_eq!(status, StatusCode::CREATED);
    let artifact = created["artifact"].clone();
    let artifact_id = artifact["artifactId"].as_str().unwrap().to_owned();
    let byte_length = artifact["byteLength"].as_u64().unwrap();
    let sha256 = artifact["sha256"].as_str().unwrap().to_owned();
    assert_eq!(
        artifact["filename"],
        format!("darktable-develop-1-{artifact_id}.tif")
    );
    let (_, settled) = export_status(&router, &photo_id, "export-bytes").await;
    let timestamp = |seconds: u64| {
        time::OffsetDateTime::from_unix_timestamp(seconds as i64)
            .unwrap()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap()
    };
    assert_eq!(
        artifact["publishedAt"],
        timestamp(settled["terminalAt"].as_u64().unwrap())
    );
    assert_eq!(
        artifact["expiresAt"],
        timestamp(settled["retainUntil"].as_u64().unwrap())
    );
    assert_eq!(artifact["orientation"], "top-left");
    assert_eq!(artifact["iccEmbedded"], true);
    let reseeded = save_selected_step(&application, &photo_id, "recipe-after-publication").await;
    assert_eq!(reseeded, (recipe_revision.clone(), source_revision.clone()));
    assert_eq!(artifact["sampleFormat"], "float32");
    let router = create_router_with_processing(
        Arc::clone(&application),
        crate::http::open_web_root(config.web_root()),
        None,
    );
    let (status, replayed) = post_processing_export(&router, &photo_id, &body).await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(replayed["replayed"], true);
    assert_eq!(replayed["artifact"], artifact);

    let response = send(
        &router,
        authenticated_request()
            .uri(format!(
                "https://camera.local/api/processing-artifacts/{artifact_id}/bytes"
            ))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers();
    assert_eq!(headers.get("content-type").unwrap(), "image/tiff");
    let header = |name: &str| headers.get(name).unwrap().to_str().unwrap();
    assert_eq!(headers.get(header::ACCEPT_RANGES).unwrap(), "bytes");
    assert_eq!(
        headers.get(header::ETAG).unwrap().to_str().unwrap(),
        format!("\"{sha256}\"")
    );
    assert_eq!(header("content-length"), byte_length.to_string());
    assert_eq!(header("slipstream-artifact-id"), artifact_id);
    assert_eq!(header("slipstream-artifact-sha256"), sha256);
    assert_eq!(
        header("slipstream-artifact-filename"),
        artifact["filename"].as_str().unwrap()
    );
    assert_eq!(
        header("slipstream-artifact-published-at"),
        artifact["publishedAt"].as_str().unwrap()
    );
    assert_eq!(
        header("slipstream-artifact-expires-at"),
        artifact["expiresAt"].as_str().unwrap()
    );
    assert_eq!(header("slipstream-artifact-orientation"), "top-left");
    assert_eq!(header("slipstream-artifact-icc-embedded"), "true");
    assert_eq!(header("slipstream-artifact-sample-format"), "float32");
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(bytes.len() as u64, byte_length);

    let (status, retained) = get_json(
        &router,
        &format!("/api/photos/{photo_id}/processing-exports"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        retained["exports"]
            .as_array()
            .unwrap()
            .iter()
            .any(|work| work["requestId"] == "export-bytes" && work["state"] == "succeeded")
    );
    assert_eq!(retained["artifacts"], serde_json::json!([artifact.clone()]));
    use sha2::{Digest, Sha256};
    assert_eq!(format!("{:x}", Sha256::digest(&bytes)), sha256);

    // One contiguous range is served against the record's strong identity; a
    // stale or unreadable If-Range validator falls back to the full
    // representation, and an unsatisfiable range names the full length.
    let uri = format!("https://camera.local/api/processing-artifacts/{artifact_id}/bytes");
    async fn artifact_bytes(
        router: &Router,
        uri: &str,
        range: Option<&str>,
        if_range: Option<header::HeaderValue>,
    ) -> Response<Body> {
        let mut request = authenticated_request().uri(uri);
        if let Some(range) = range {
            request = request.header(header::RANGE, range);
        }
        if let Some(if_range) = if_range {
            request = request.header(header::IF_RANGE, if_range);
        }
        send(router, request.body(Body::empty()).unwrap()).await
    }
    let header_of = |response: &Response<Body>, name: header::HeaderName| {
        response
            .headers()
            .get(name)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned()
    };

    let range_response = artifact_bytes(
        &router,
        &uri,
        Some("bytes=1-3"),
        Some(header::HeaderValue::from_str(&format!("\"{sha256}\"")).unwrap()),
    )
    .await;
    assert_eq!(range_response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        header_of(&range_response, header::CONTENT_RANGE),
        format!("bytes 1-3/{byte_length}")
    );
    assert_eq!(header_of(&range_response, header::CONTENT_LENGTH), "3");
    let range_bytes = axum::body::to_bytes(range_response.into_body(), 64 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(&range_bytes[..], &bytes[1..4]);

    for if_range in [
        header::HeaderValue::from_static("\"different\""),
        header::HeaderValue::from_bytes(&[0xff]).unwrap(),
    ] {
        let fallback = artifact_bytes(&router, &uri, Some("bytes=1-3"), Some(if_range)).await;
        assert_eq!(fallback.status(), StatusCode::OK);
        assert_eq!(
            header_of(&fallback, header::CONTENT_LENGTH),
            byte_length.to_string()
        );
        let _ = axum::body::to_bytes(fallback.into_body(), 64 * 1024 * 1024)
            .await
            .unwrap();
    }

    let invalid = artifact_bytes(&router, &uri, Some("bytes=999999999-"), None).await;
    assert_eq!(invalid.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(
        header_of(&invalid, header::CONTENT_RANGE),
        format!("bytes */{byte_length}")
    );

    // Unknown bytes identities answer the same closed refusal as the
    // provenance route.
    let (status, body) = get_json(&router, "/api/processing-artifacts/pa-none/bytes").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"]["code"], "unknown_artifact");

    application.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
