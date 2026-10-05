use super::*;
use std::os::unix::fs::PermissionsExt;

/// Writes one structurally valid film bundle fixture below `base` and
/// returns its root. Every digest is real, so only the identity the
/// verification pins can differ between a good and a hostile bundle.
fn write_bundle(base: &Path) -> PathBuf {
    let root = base.join("slipstream-film");
    let data = root.join("data");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(data.join("profile.json"), b"{\"profile\":\"fixture\"}\n").unwrap();
    let binary = root.join("spektrafilm");
    std::fs::write(&binary, b"#!/bin/sh\n").unwrap();
    let mut permissions = std::fs::metadata(&binary).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&binary, permissions).unwrap();
    let parameters = root.join("parameters-default.json");
    std::fs::write(
        &parameters,
        serde_json::to_vec(&slipstream_processing::modules::spektrafilm_default_tree()).unwrap(),
    )
    .unwrap();
    let document = serde_json::json!({
        "format": 2,
        "implementation": "spektrafilm-rs",
        "forkCommit": slipstream_processing::modules::SPEKTRAFILM_FORK_COMMIT,
        "adapterVersion": "spektrafilm-rs-adapter-1",
        "parameterSchemaVersion": "spektrafilm-rs-params-1",
        "binary": "/opt/slipstream-film/spektrafilm",
        "dataRoot": "/opt/slipstream-film/data",
        "parametersDefault": "/opt/slipstream-film/parameters-default.json",
        "filmProfile": "kodak_portra_400",
        "printProfile": "kodak_portra_endura",
        "files": {
            "/opt/slipstream-film/spektrafilm": digest_of(&binary),
            "/opt/slipstream-film/parameters-default.json": digest_of(&parameters),
        },
        "data": {
            "profile.json": digest_of(&data.join("profile.json")),
        },
    });
    let encoded = serde_json::to_vec(&document).unwrap();
    std::fs::write(root.join("bundle-manifest.json"), &encoded).unwrap();
    let bundle = format!("{:x}", Sha256::digest(&encoded));
    std::fs::write(root.join("bundle"), &bundle).unwrap();
    root
}

fn digest_of(path: &Path) -> String {
    format!("{:x}", Sha256::digest(std::fs::read(path).unwrap()))
}

fn fresh_base() -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "slipstream-film-bundle-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    std::fs::create_dir_all(&base).unwrap();
    base
}

#[test]
fn a_truthful_bundle_verifies_and_carries_the_pinned_identity() {
    let base = fresh_base();
    let root = write_bundle(&base);
    let (config, failure) = verify_film_bundle(&root);
    assert_eq!(failure, None);
    let config = config.expect("the fixture bundle verifies");
    assert!(config.ready());
    assert_eq!(config.bundle_sha256.len(), 64);
    assert!(config.binary.ends_with("slipstream-film/spektrafilm"));
    assert!(config.data_root.ends_with("slipstream-film/data"));
    assert_eq!(config.film_profile, "kodak_portra_400");
    assert!(config.parameter_default.is_object());
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_tampered_runtime_or_recipe_identity_is_refused() {
    let base = fresh_base();
    let root = write_bundle(&base);
    // A runtime file changed after the manifest was recorded.
    let engine = base.join("slipstream-film/spektrafilm");
    std::fs::write(&engine, b"#!/bin/sh\ntampered").unwrap();
    let (_, failure) = verify_film_bundle(&root);
    assert_eq!(failure, Some("film-bundle-unavailable"));

    let base2 = fresh_base();
    let root2 = write_bundle(&base2);
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root2.join("bundle-manifest.json")).unwrap())
            .unwrap();
    let mut hostile = manifest.clone();
    hostile["forkCommit"] = serde_json::json!("0".repeat(40));
    let encoded = serde_json::to_vec(&hostile).unwrap();
    std::fs::write(root2.join("bundle-manifest.json"), &encoded).unwrap();
    std::fs::write(
        root2.join("bundle"),
        format!("{:x}", Sha256::digest(&encoded)),
    )
    .unwrap();
    let (_, failure) = verify_film_bundle(&root2);
    assert_eq!(failure, Some("film-bundle-unavailable"));
    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::remove_dir_all(&base2);
}

#[test]
fn manifest_paths_must_resolve_inside_the_bundle_root() {
    let base = fresh_base();
    let root = write_bundle(&base);
    let outside = base.join("outside-engine");
    std::fs::write(&outside, b"#!/bin/sh\n").unwrap();
    let mut permissions = std::fs::metadata(&outside).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&outside, permissions).unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("bundle-manifest.json")).unwrap()).unwrap();
    manifest["binary"] = serde_json::json!(outside);
    let encoded = serde_json::to_vec(&manifest).unwrap();
    std::fs::write(root.join("bundle-manifest.json"), &encoded).unwrap();
    std::fs::write(
        root.join("bundle"),
        format!("{:x}", Sha256::digest(&encoded)),
    )
    .unwrap();

    let (_, failure) = verify_film_bundle(&root);
    assert_eq!(failure, Some("film-bundle-unavailable"));
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_missing_directory_reports_the_runtime_missing_reason() {
    let film = film_bundle_config(
        &mut |key: &str| match key {
            "SLIPSTREAM_FILM_MODULE" => Some("enabled".to_owned()),
            "SLIPSTREAM_FILM_BUNDLE_DIRECTORY" => Some("/nonexistent-slipstream-film".to_owned()),
            _ => None,
        },
        "SLIPSTREAM_FILM_MODULE",
        "SLIPSTREAM_FILM_BUNDLE_DIRECTORY",
    )
    .unwrap()
    .expect("enabled");
    assert_eq!(film.failure, Some("film-runtime-missing"));
    assert!(!film.ready());
}

#[test]
fn a_darktable_disabled_deployment_with_film_opens_processing_independently() {
    let config = Config::from_lookup(|key: &str| match key {
        "SLIPSTREAM_LIBRARY_ROOT" => Some("/tmp/library".to_owned()),
        "SLIPSTREAM_STATE_DIRECTORY" => Some("/tmp/state".to_owned()),
        "SLIPSTREAM_CACHE_DIRECTORY" => Some("/tmp/cache".to_owned()),
        "SLIPSTREAM_PHOTO_DEVELOPMENT" => Some("disabled".to_owned()),
        "SLIPSTREAM_FILM_MODULE" => Some("enabled".to_owned()),
        "SLIPSTREAM_FILM_BUNDLE_DIRECTORY" => Some("/nonexistent-slipstream-film".to_owned()),
        _ => None,
    })
    .unwrap();
    // disabled + film enabled: the processing extension opens with only
    // the film stage admissible and the truthful darktable failure.
    let processing = config.processing.expect("film opens the extension");
    assert_eq!(processing.failure, Some("darktable-disabled"));
    assert_eq!(processing.bundle_sha256, "");
    let film = processing.film.as_ref().expect("film configured");
    assert_eq!(film.failure, Some("film-runtime-missing"));
    assert!(!film.ready());

    // disabled + film disabled: the extension stays fully closed.
    let both_disabled = Config::from_lookup(|key: &str| match key {
        "SLIPSTREAM_LIBRARY_ROOT" => Some("/tmp/library".to_owned()),
        "SLIPSTREAM_STATE_DIRECTORY" => Some("/tmp/state".to_owned()),
        "SLIPSTREAM_CACHE_DIRECTORY" => Some("/tmp/cache".to_owned()),
        "SLIPSTREAM_PHOTO_DEVELOPMENT" => Some("disabled".to_owned()),
        "SLIPSTREAM_FILM_MODULE" => Some("disabled".to_owned()),
        _ => None,
    })
    .unwrap();
    assert!(both_disabled.processing.is_none());
}
