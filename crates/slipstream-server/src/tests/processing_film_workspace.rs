use super::*;
use crate::http::create_router_with_processing;
use std::os::unix::fs::PermissionsExt;

fn film_engine(config: &mut Config, base: &Path) {
    let photo = FakePhotoEngine::install(base);
    let mut processing = photo.processing_config();
    let root = base.join("film-test-runtime");
    fs::create_dir_all(root.join("data")).unwrap();
    let rgb = image::RgbImage::from_pixel(2, 1, image::Rgb([80, 100, 120]));
    fs::write(
        root.join("preview.png"),
        [
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 1,
            8, 2, 0, 0, 0, 123, 64, 232, 221, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 8, 72,
            169, 0, 34, 0, 7, 235, 2, 89, 147, 249, 235, 236, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66,
            96, 130,
        ],
    )
    .unwrap();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 85)
        .encode_image(&rgb)
        .unwrap();
    let icc = include_bytes!("../../../slipstream-core/assets/srgb-iec61966-2-1.icc");
    let mut profile = b"ICC_PROFILE\0\x01\x01".to_vec();
    profile.extend_from_slice(icc);
    let mut encoded = jpeg[..2].to_vec();
    encoded.extend_from_slice(b"\xff\xe2");
    encoded.extend_from_slice(&u16::try_from(profile.len() + 2).unwrap().to_be_bytes());
    encoded.extend_from_slice(&profile);
    encoded.extend_from_slice(&jpeg[2..]);
    fs::write(root.join("output.jpg"), encoded).unwrap();
    let binary = root.join("spektrafilm");
    fs::write(
        &binary,
        br#"#!/bin/sh
while [ "$#" -gt 0 ]; do
case "$1" in
--input) input="$2"; shift 2;;
--recipe) recipe="$2"; shift 2;;
--output) output="$2"; shift 2;;
*) shift;;
esac
done
[ -r "$input" ] || exit 5
if /bin/grep -q '"format":"png"' "$recipe"; then
 /bin/cp "$(/usr/bin/dirname "$0")/preview.png" "$output"
else
 /bin/cp "$(/usr/bin/dirname "$0")/output.jpg" "$output"
fi
"#,
    )
    .unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    processing.film = Some(crate::config::FilmConfig {
        bundle_sha256: "a".repeat(64),
        bundle_root: root.clone(),
        binary,
        data_root: root.join("data"),
        parameter_default: slipstream_processing::modules::spektrafilm_default_tree(),
        film_profile: "kodak_portra_400".into(),
        print_profile: "kodak_portra_endura".into(),
        failure: None,
    });
    config.processing = Some(processing);
    config.export_retained_output_bytes = Some(slipstream_core::MAXIMUM_EXPORT_BYTES);
}

#[tokio::test]
async fn film_workspace_input_survives_original_loss_and_retains_preview_and_export_identity() {
    let (base, mut config) = prepare_fixture();
    let original = config.library_root.join("approved.ARW");
    approved_raw_fixture(&original);
    film_engine(&mut config, &base);
    let application = Application::open(&config).await.unwrap();
    wait_for_scan_settled(&application).await;
    let photo_id = browse_photo_ids(&application, BrowseSourceRequest::Library)
        .await
        .remove(0);
    let binding = super::processing_recipe::completed_film_binding(&application, &photo_id);
    fs::remove_file(&original).unwrap();
    application.rescan().await.unwrap();
    wait_for_scan_settled(&application).await;
    let router = create_router_with_processing(
        Arc::clone(&application),
        crate::http::open_web_root(config.web_root()),
        config.processing.clone(),
    );
    application.access.seed_test_token();
    let uri = format!("/api/photos/{photo_id}/processing-recipe");
    let (_, read) = get_json(&router, &uri).await;
    assert_eq!(read["sourceRevision"], "");
    let response = post_json(
        &router,
        &uri,
        super::processing_recipe::film_save(
            binding.clone(),
            read["sourceRevision"].clone(),
            "film-save",
        ),
        Some("https://camera.local"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let saved = response_json(response).await;
    let preview = send(
        &router,
        authenticated_request()
            .uri(format!("/api/photos/{photo_id}/processing-preview/film"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    let preview_status = preview.status();
    if preview_status != StatusCode::OK {
        panic!(
            "Film Preview {preview_status}: {}",
            response_json(preview).await
        );
    }
    assert_eq!(preview.headers()["content-type"], "image/png");
    let preview_identity = preview.headers()["slipstream-processing-preview-identity"]
        .to_str()
        .unwrap()
        .to_owned();
    let preview_bytes = axum::body::to_bytes(preview.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap();
    let preview_path = base.join("film-preview-response.png");
    fs::write(&preview_path, &preview_bytes).unwrap();
    let facts = photo_files::image::inspect_image(
        &mut fs::File::open(&preview_path).unwrap(),
        16 * 1024 * 1024,
    )
    .unwrap();
    assert_eq!((facts.format, facts.width), ("png", 2));
    let export_uri = format!("/api/photos/{photo_id}/processing-exports");
    let request = serde_json::json!({"requestId":"film-export","stepId":"film","expectedRecipeRevision":saved["recipe"]["revision"],"expectedSourceRevision":""});
    let mut stale = request.clone();
    stale["requestId"] = serde_json::json!("film-stale-refusal");
    stale["expectedRecipeRevision"] = serde_json::json!("stale-recipe");
    let refused = post_json(&router, &export_uri, stale, Some("https://camera.local")).await;
    let refused_status = refused.status();
    if refused_status != StatusCode::CONFLICT {
        panic!(
            "Film stale Export {refused_status}: {}",
            response_json(refused).await
        );
    }
    assert!(
        !application
            .workspaces
            .processing_export_dependency_owners()
            .unwrap()
            .contains(&"processing-export:film-stale-refusal".to_owned())
    );
    let accepted = post_json(
        &router,
        &export_uri,
        request.clone(),
        Some("https://camera.local"),
    )
    .await;
    let accepted_status = accepted.status();
    if accepted_status != StatusCode::ACCEPTED {
        panic!(
            "Film Export {accepted_status}: {}",
            response_json(accepted).await
        );
    }
    drop(accepted);
    drop(router);
    let router = create_router_with_processing(
        Arc::clone(&application),
        crate::http::open_web_root(config.web_root()),
        config.processing.clone(),
    );
    let mut artifact = None;
    for _ in 0..200 {
        let (_, status) = get_json(&router, &format!("{export_uri}/film-export")).await;
        if status["state"] == "succeeded" {
            artifact = status["artifactId"].as_str().map(str::to_owned);
            break;
        }
        assert_ne!(status["state"], "failed", "{status}");
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let artifact_id = artifact.expect("Film settles supervised work");
    assert!(
        !application
            .workspaces
            .processing_export_dependency_owners()
            .unwrap()
            .contains(&"processing-export:film-export".to_owned())
    );
    let (_, inspected) =
        get_json(&router, &format!("/api/processing-artifacts/{artifact_id}")).await;
    assert!(inspected["expiresAt"].is_null());
    assert_eq!(inspected["input"]["binding"], binding);
    assert_eq!(
        inspected["outputContract"]["encoding"],
        "quality-85-baseline"
    );
    let alias = application
        .workspaces
        .resolve_artifact_alias(&artifact_id)
        .unwrap()
        .unwrap();
    assert_eq!(alias["workspaceId"], binding["workspaceId"]);
    let retained_output = application
        .workspaces
        .inspect(
            alias["workspaceId"].as_str().unwrap(),
            alias["artifactId"].as_str().unwrap(),
        )
        .unwrap();
    assert_eq!(
        retained_output["imageContract"],
        inspected["outputContract"]
    );
    let files = application
        .workspaces
        .files(
            Some(binding["workspaceId"].as_str().unwrap()),
            None,
            60,
            None,
        )
        .unwrap();
    let items = files["items"].as_array().unwrap();
    assert!(items.iter().any(|item| item["kind"] == "preview"
        && item["provenance"]["handle"]["identity"] == preview_identity));
    let download = send(
        &router,
        authenticated_request()
            .uri(format!("/api/processing-artifacts/{artifact_id}/bytes"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(download.status(), StatusCode::OK);
    assert_eq!(download.headers()["slipstream-artifact-expires-at"], "null");
    let bytes = axum::body::to_bytes(download.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        bytes.as_ref(),
        fs::read(base.join("film-test-runtime/output.jpg")).unwrap()
    );
    let replay = post_json(&router, &export_uri, request, Some("https://camera.local")).await;
    assert_eq!(
        response_json(replay).await["artifact"]["artifactId"],
        artifact_id
    );
    application.shutdown().await.unwrap();
    drop(router);
    drop(application);
    let reopened = Application::open(&config).await.unwrap();
    let alias_after_restart = reopened
        .workspaces
        .resolve_artifact_alias(&artifact_id)
        .unwrap()
        .unwrap();
    assert_eq!(alias_after_restart, alias);
    let router = create_router_with_processing(
        Arc::clone(&reopened),
        crate::http::open_web_root(config.web_root()),
        config.processing.clone(),
    );
    reopened.access.seed_test_token();
    let download = send(
        &router,
        authenticated_request()
            .uri(format!("/api/processing-artifacts/{artifact_id}/bytes"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(download.status(), StatusCode::OK);
    assert_eq!(
        axum::body::to_bytes(download.into_body(), 1024 * 1024)
            .await
            .unwrap(),
        bytes
    );
    let status_uri = format!("{export_uri}/film-export");
    let input_owner = "processing-export:film-export";
    let retain_input = || {
        reopened
            .workspaces
            .register_dependency(
                binding["workspaceId"].as_str().unwrap(),
                binding["artifactId"].as_str().unwrap(),
                binding["revision"].as_str().unwrap(),
                input_owner,
                "job",
            )
            .unwrap();
    };
    retain_input();
    let (status, work) = get_json(&router, &status_uri).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(work["state"], "succeeded");
    assert!(
        !reopened
            .workspaces
            .processing_export_dependency_owners()
            .unwrap()
            .contains(&input_owner.to_owned())
    );
    retain_input();
    let output_path = Path::new(retained_output["path"].as_str().unwrap());
    fs::set_permissions(output_path, fs::Permissions::from_mode(0o600)).unwrap();
    let mut changed = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut changed, 85)
        .encode_image(&image::RgbImage::from_pixel(2, 1, image::Rgb([20, 40, 60])))
        .unwrap();
    for condition in ["changed", "truncated", "missing"] {
        match condition {
            "changed" => fs::write(output_path, &changed).unwrap(),
            "truncated" => fs::write(output_path, &bytes[..32]).unwrap(),
            "missing" => fs::remove_file(output_path).unwrap(),
            _ => unreachable!(),
        }
        let (status, refusal) = get_json(&router, &status_uri).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{condition}");
        assert_eq!(refusal["error"]["code"], "outcome_unknown", "{condition}");
        assert!(refusal.get("state").is_none(), "{condition}");
        let (download_status, _) = get_json(
            &router,
            &format!("/api/processing-artifacts/{artifact_id}/bytes"),
        )
        .await;
        assert_eq!(
            download_status,
            StatusCode::INTERNAL_SERVER_ERROR,
            "{condition}"
        );
        assert!(
            reopened
                .workspaces
                .processing_export_dependency_owners()
                .unwrap()
                .contains(&input_owner.to_owned()),
            "{condition} must preserve the unreconciled input pin"
        );
        assert_eq!(
            reopened
                .workspaces
                .resolve_artifact_alias(&artifact_id)
                .unwrap(),
            Some(alias.clone()),
            "{condition} must not retarget the permanent alias"
        );
        let historical = reopened
            .library
            .processing_export_work("film-export")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            historical.state,
            slipstream_core::ProcessingExportWorkState::Succeeded
        );
    }
    assert!(
        !output_path.exists(),
        "Status must not recreate removed output bytes"
    );
    reopened.shutdown().await.unwrap();
    let _ = fs::remove_dir_all(base);
}
