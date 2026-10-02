use super::*;
use std::os::unix::fs::PermissionsExt;

/// Writes one structurally valid film bundle fixture below `base` and
/// returns its root. Every digest is real, so only the identity the
/// verification pins can differ between a good and a hostile bundle.
fn write_bundle(base: &Path) -> PathBuf {
    let root = base.join("slipstream-film");
    let runtime = base.join("runtime");
    let source = base.join("spektrafilm/src");
    let probe = base.join("probe");
    std::fs::create_dir_all(root.join("runner")).unwrap();
    std::fs::create_dir_all(runtime.join("bin")).unwrap();
    std::fs::create_dir_all(&source).unwrap();
    std::fs::create_dir_all(&probe).unwrap();
    let engine = runtime.join("bin/python");
    std::fs::write(&engine, b"#!/bin/sh\n").unwrap();
    let mut permissions = std::fs::metadata(&engine).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&engine, permissions).unwrap();
    let runner = root.join("runner/film_runner.py");
    std::fs::write(&runner, b"runner").unwrap();
    let source_file = source.join("sim.py");
    std::fs::write(&source_file, b"source").unwrap();
    let probe_file = probe.join("film.py");
    std::fs::write(&probe_file, b"probe").unwrap();
    let parameters = root.join("parameters-default.json");
    std::fs::write(
        &parameters,
        serde_json::to_vec(&slipstream_processing::modules::spektrafilm_default_tree()).unwrap(),
    )
    .unwrap();
    let document = serde_json::json!({
        "format": 1,
        "spektrafilm_commit": "3bb2c2d2801ff68b92019cf1dbcbb133d60832bc",
        "engine": engine,
        "runner": runner,
        "source_root": source,
        "probe_root": probe,
        "recipe_sha256":
            slipstream_processing::modules::SPEKTRAFILM_RECIPE_SHA256,
        "input_icc_sha256":
            slipstream_processing::modules::SPEKTRAFILM_INPUT_ICC_SHA256,
        "output_icc_sha256":
            slipstream_processing::modules::SPEKTRAFILM_OUTPUT_ICC_SHA256,
        "finished_jpeg_quality": 85,
        "procedure": "film-once-empty-cache-v1",
        "files": {
            "/opt/slipstream-film/runner/film_runner.py": digest_of(&runner),
            "/opt/slipstream-film/parameters-default.json": digest_of(&parameters),
        },
        "runtime": {"bin/python": digest_of(&engine)},
        "source": {"sim.py": digest_of(&source_file)},
        "probe": {"film.py": digest_of(&probe_file)},
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
    assert!(config.engine.ends_with("runtime/bin/python"));
    assert!(config.runner.ends_with("runner/film_runner.py"));
    assert!(config.source_root.ends_with("spektrafilm/src"));
    assert!(config.parameter_default.is_object());
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_tampered_runtime_or_recipe_identity_is_refused() {
    let base = fresh_base();
    let root = write_bundle(&base);
    // A runtime file changed after the manifest was recorded.
    let engine = base.join("runtime/bin/python");
    std::fs::write(&engine, b"#!/bin/sh\ntampered").unwrap();
    let (_, failure) = verify_film_bundle(&root);
    assert_eq!(failure, Some("film-bundle-unavailable"));

    let base2 = fresh_base();
    let root2 = write_bundle(&base2);
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root2.join("bundle-manifest.json")).unwrap())
            .unwrap();
    let mut hostile = manifest.clone();
    hostile["recipe_sha256"] = serde_json::json!("0".repeat(64));
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
