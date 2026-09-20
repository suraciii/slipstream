//! Executable compatibility contracts for the Rust server and Web boundary.
//! This crate is not a production server.

#[cfg(test)]
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{ffi::CStr, os::raw::c_char, path::PathBuf};

const CONTRACT_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../compatibility");

unsafe extern "C" {
    fn slipstream_probe_libraw_version() -> *const c_char;
    fn slipstream_probe_jpeg_version() -> i32;
    fn slipstream_probe_thumbnail_api() -> i32;
    #[cfg(test)]
    fn slipstream_probe_vips_lifecycle() -> i32;
}

pub fn contract_path(relative: &str) -> PathBuf {
    PathBuf::from(CONTRACT_ROOT).join(relative)
}

pub fn native_versions() -> (String, i32) {
    // SAFETY: the probe returns LibRaw's process-lifetime version string and an integer.
    unsafe {
        let raw = CStr::from_ptr(slipstream_probe_libraw_version())
            .to_string_lossy()
            .into_owned();
        (raw, slipstream_probe_jpeg_version())
    }
}

pub fn thumbnail_api_available() -> bool {
    // SAFETY: the probe owns and closes its temporary LibRaw handle.
    unsafe { slipstream_probe_thumbnail_api() == 1 }
}

fn digest(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

pub fn original_id(path: &str) -> String {
    digest(
        [b"original\0".as_slice(), path.as_bytes()]
            .concat()
            .as_slice(),
    )
}

pub fn photo_id(seed: &str) -> String {
    digest([b"photo\0".as_slice(), seed.as_bytes()].concat().as_slice())
}

pub fn source_revision(path: &str, size: u64, mtime_ms: f64) -> String {
    format!(
        "{path}\0{size}\0{}",
        serde_json::Number::from_f64(mtime_ms).unwrap()
    )
}

#[cfg(test)]
#[derive(Deserialize)]
struct IdentityContract {
    #[serde(rename = "algorithmVersion")]
    algorithm_version: String,
    vectors: Vec<IdentityVector>,
}

#[cfg(test)]
#[derive(Deserialize)]
struct IdentityVector {
    path: String,
    source: String,
    size: u64,
    #[serde(rename = "mtimeMs")]
    mtime_ms: f64,
    candidate: Option<String>,
    edge: u32,
    #[serde(rename = "originalId")]
    original_id: String,
    #[serde(rename = "photoId")]
    photo_id: String,
    #[serde(rename = "sourceRevision")]
    source_revision: String,
    #[serde(rename = "historicalCacheKey")]
    historical_cache_key: String,
    #[serde(rename = "historicalManifestIdentity")]
    historical_manifest_identity: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, routing::get};
    use image::{
        DynamicImage, ExtendedColorType, GenericImageView, ImageDecoder, ImageEncoder,
        codecs::jpeg::{JpegDecoder, JpegEncoder},
        imageops::FilterType,
        metadata::Orientation,
    };
    use lcms2::Profile;
    use rusqlite::Connection;
    use serde_json::Value;
    use slipstream_core::{DerivativeProfile, DerivativeTarget, process_jpeg};
    use std::{
        collections::BTreeSet,
        fs,
        io::Cursor,
        path::Path,
        sync::{Mutex, OnceLock},
    };
    static VIPS_TEST_LEASE: OnceLock<Mutex<()>> = OnceLock::new();
    use tower::ServiceExt;

    #[test]
    fn preview_fixture_contract_is_deterministic_and_complete() {
        let contract: Value =
            serde_json::from_slice(&fs::read(contract_path("preview/fixtures.json")).unwrap())
                .unwrap();
        assert_eq!(contract["schemaVersion"], 1);
        assert_eq!(
            contract["algorithmVersionGate"]["required"],
            "fixture-matrix-pass"
        );
        assert_eq!(
            contract["algorithmVersionGate"]["selectedRustVersion"],
            "rust-vips-v1"
        );
        assert_eq!(contract["targets"], serde_json::json!([512, 2560]));
        assert_eq!(
            contract["limits"],
            serde_json::json!({
                "inputBytes": 128 * 1024 * 1024u64,
                "decodedPixels": 100_000_000u64,
                "outputBytes": 64 * 1024 * 1024u64,
                "librawMemoryMb": 256,
                "queueConcurrency": 2
            })
        );
        assert_eq!(
            contract["cases"][0]["orientations"],
            serde_json::json!([1, 2, 3, 4, 5, 6, 7, 8])
        );
        for name in [
            "rgb-small-orientation-1-8",
            "rgb-large",
            "rgb-valid-icc",
            "rgb-invalid-icc",
            "cmyk",
            "corrupt-jpeg",
            "truncated-jpeg",
        ] {
            assert!(
                contract["cases"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|case| case["name"] == name)
            );
        }
    }

    #[test]
    fn preview_fixture_matrix_runs_rust_derivative_for_every_recipe() {
        let _lease = VIPS_TEST_LEASE
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap();
        let contract: Value =
            serde_json::from_slice(&fs::read(contract_path("preview/fixtures.json")).unwrap())
                .unwrap();
        let targets = contract["targets"].as_array().unwrap();
        let cases = contract["cases"].as_array().unwrap();
        for case in cases {
            let kind = case["kind"].as_str().unwrap();
            let orientations = case["orientations"].as_array().unwrap();
            let mut recipes = Vec::new();
            if kind == "corrupt" {
                recipes.push((0, vec![0xff, 0xd8, 0xff, 0xd9]));
            } else if kind == "truncated" {
                recipes.push((0, test_jpeg(12, 8)[..8].to_vec()));
            } else {
                for orientation in orientations {
                    recipes.push((
                        orientation.as_u64().unwrap() as u16,
                        oriented_jpeg(
                            case["width"].as_u64().unwrap() as u32,
                            case["height"].as_u64().unwrap() as u32,
                            orientation.as_u64().unwrap() as u16,
                            case["profile"].as_str().unwrap(),
                            kind,
                        ),
                    ));
                }
            }
            for (orientation, source) in recipes {
                for target in targets {
                    let target = match target.as_u64().unwrap() {
                        512 => DerivativeTarget::Thumbnail512,
                        2560 => DerivativeTarget::Review2560,
                        value => panic!("unsupported fixture target {value}"),
                    };
                    let result = process_jpeg(&source, target);
                    if kind == "corrupt" || kind == "truncated" {
                        assert!(result.is_err(), "{kind} unexpectedly decoded");
                        continue;
                    }
                    let derivative = result.unwrap_or_else(|error| {
                        panic!("fixture {kind} failed at {:?}: {error:?}", target)
                    });
                    assert!(derivative.width > 0 && derivative.height > 0);
                    assert!(
                        std::cmp::max(derivative.width, derivative.height) <= target.long_edge()
                    );
                    assert!(derivative.jpeg.len() <= 64 * 1024 * 1024);
                    let mut decoder = JpegDecoder::new(Cursor::new(&derivative.jpeg)).unwrap();
                    let source_width = case["width"].as_u64().unwrap() as u32;
                    let source_height = case["height"].as_u64().unwrap() as u32;
                    let oriented = if matches!(orientation, 5..=8) {
                        (source_height, source_width)
                    } else {
                        (source_width, source_height)
                    };
                    let scale = target.long_edge().min(oriented.0.max(oriented.1)) as f32
                        / oriented.0.max(oriented.1) as f32;
                    let expected_dimensions = (
                        ((oriented.0 as f32 * scale).round() as u32).max(1),
                        ((oriented.1 as f32 * scale).round() as u32).max(1),
                    );
                    assert_eq!(
                        (derivative.width, derivative.height),
                        expected_dimensions,
                        "{kind} orientation {orientation} target {:?}",
                        target
                    );
                    assert_eq!(decoder.dimensions(), (derivative.width, derivative.height));
                    assert_eq!(decoder.orientation().unwrap(), Orientation::NoTransforms);
                    let expected_profile = if case["profile"] == "srgb" {
                        DerivativeProfile::PreservedIcc
                    } else {
                        DerivativeProfile::Srgb
                    };
                    assert_eq!(derivative.profile, expected_profile);
                    if kind == "cmyk" {
                        assert_eq!(decoder.color_type().channel_count(), 3);
                        assert!(
                            !derivative
                                .jpeg
                                .windows(12)
                                .any(|window| window == b"ICC_PROFILE\0")
                        );
                    }
                    let channels = decoder.color_type().channel_count() as usize;
                    let mut pixels = vec![0_u8; decoder.total_bytes() as usize];
                    decoder.read_image(&mut pixels).unwrap();
                    assert!(!pixels.is_empty());
                    let pixel = |x: u32, y: u32| {
                        let offset = (y * derivative.width + x) as usize * channels;
                        &pixels[offset..offset + channels]
                    };
                    assert!(pixel(0, 0).iter().any(|channel| *channel != 0));
                    assert!(
                        pixel(derivative.width - 1, derivative.height - 1)
                            .iter()
                            .any(|channel| *channel != 0)
                    );
                    if kind == "rgb" && case["name"] == "rgb-small-orientation-1-8" {
                        let expected = match orientation {
                            1 => [230, 20, 20],
                            2 => [20, 230, 20],
                            3 => [230, 230, 20],
                            4 => [20, 20, 230],
                            5 => [230, 20, 20],
                            6 => [20, 20, 230],
                            7 => [230, 230, 20],
                            8 => [20, 230, 20],
                            _ => unreachable!(),
                        };
                        let corner = pixel(0, 0);
                        assert!(
                            corner[0].abs_diff(expected[0]) < 45
                                && corner[1].abs_diff(expected[1]) < 45
                                && corner[2].abs_diff(expected[2]) < 45,
                            "orientation {orientation} target {:?}: corner {corner:?}, expected {expected:?}",
                            target
                        );
                    }
                }
            }
        }
    }

    fn orientation_pattern(width: u32, height: u32) -> Vec<u8> {
        let mut pixels = vec![0_u8; width as usize * height as usize * 3];
        for y in 0..height {
            for x in 0..width {
                let horizontal = x >= width / 2;
                let vertical = y >= height / 2;
                let color = match (horizontal, vertical) {
                    (false, false) => [230, 20, 20],
                    (true, false) => [20, 230, 20],
                    (false, true) => [20, 20, 230],
                    (true, true) => [230, 230, 20],
                };
                pixels[((y * width + x) * 3) as usize..((y * width + x) * 3 + 3) as usize]
                    .copy_from_slice(&color);
            }
        }
        pixels
    }

    fn test_jpeg(width: u32, height: u32) -> Vec<u8> {
        let pixels = vec![127_u8; width as usize * height as usize * 3];
        let mut bytes = Vec::new();
        JpegEncoder::new_with_quality(&mut bytes, 85)
            .encode(&pixels, width, height, ExtendedColorType::Rgb8)
            .unwrap();
        bytes
    }

    fn oriented_jpeg(
        width: u32,
        height: u32,
        orientation: u16,
        profile: &str,
        kind: &str,
    ) -> Vec<u8> {
        if kind == "cmyk" {
            return cmyk_jpeg();
        }
        assert_eq!(kind, "rgb");
        let bytes = if profile == "srgb" {
            let pixels = orientation_pattern(width, height);
            let mut encoded = Vec::new();
            let mut encoder = JpegEncoder::new_with_quality(&mut encoded, 85);
            encoder
                .set_icc_profile(Profile::new_srgb().icc().unwrap())
                .unwrap();
            encoder
                .encode(&pixels, width, height, ExtendedColorType::Rgb8)
                .unwrap();
            encoded
        } else {
            let pixels = orientation_pattern(width, height);
            let mut encoded = Vec::new();
            JpegEncoder::new_with_quality(&mut encoded, 100)
                .encode(&pixels, width, height, ExtendedColorType::Rgb8)
                .unwrap();
            encoded
        };
        // Keep this as actual EXIF bytes, not a Rust byte-string containing the
        // literal characters `\\`/`x`; libvips only recognizes the TIFF payload.
        let mut exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x01\0\0\0\x00\0\0\0\0\0\0\0\0\0\0".to_vec();
        exif[24..26].copy_from_slice(&orientation.to_le_bytes());
        let mut result = Vec::with_capacity(bytes.len() + exif.len() + 24);
        result.extend_from_slice(&bytes[..2]);
        let length = u16::try_from(exif.len() + 2).unwrap();
        result.extend_from_slice(&[0xff, 0xe1]);
        result.extend_from_slice(&length.to_be_bytes());
        result.extend_from_slice(&exif);
        if profile == "invalid" {
            // A malformed APP2 profile must be ignored rather than copied.
            let payload = b"ICC_PROFILE\0\x01\x01bad";
            let marker_length = u16::try_from(payload.len() + 2).unwrap();
            result.extend_from_slice(&[0xff, 0xe2]);
            result.extend_from_slice(&marker_length.to_be_bytes());
            result.extend_from_slice(payload);
        }
        result.extend_from_slice(&bytes[2..]);
        result
    }

    fn cmyk_jpeg() -> Vec<u8> {
        vec![
            0xff, 0xd8, 0xff, 0xee, 0x00, 0x0e, 0x41, 0x64, 0x6f, 0x62, 0x65, 0x00, 0x64, 0x00,
            0x00, 0x00, 0x00, 0x00, 0xff, 0xdb, 0x00, 0x43, 0x00, 0x03, 0x02, 0x02, 0x03, 0x02,
            0x02, 0x03, 0x03, 0x03, 0x03, 0x04, 0x03, 0x03, 0x04, 0x05, 0x08, 0x05, 0x05, 0x04,
            0x04, 0x05, 0x0a, 0x07, 0x07, 0x06, 0x08, 0x0c, 0x0a, 0x0c, 0x0c, 0x0b, 0x0a, 0x0b,
            0x0b, 0x0d, 0x0e, 0x12, 0x10, 0x0d, 0x0e, 0x11, 0x0e, 0x0b, 0x0b, 0x10, 0x16, 0x10,
            0x11, 0x13, 0x14, 0x15, 0x15, 0x15, 0x0c, 0x0f, 0x17, 0x18, 0x16, 0x14, 0x18, 0x12,
            0x14, 0x15, 0x14, 0xff, 0xc0, 0x00, 0x14, 0x08, 0x00, 0x0c, 0x00, 0x18, 0x04, 0x43,
            0x11, 0x00, 0x4d, 0x11, 0x00, 0x59, 0x11, 0x00, 0x4b, 0x11, 0x00, 0xff, 0xc4, 0x00,
            0x1f, 0x00, 0x00, 0x01, 0x05, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09,
            0x0a, 0x0b, 0xff, 0xc4, 0x00, 0xb5, 0x10, 0x00, 0x02, 0x01, 0x03, 0x03, 0x02, 0x04,
            0x03, 0x05, 0x05, 0x04, 0x04, 0x00, 0x00, 0x01, 0x7d, 0x01, 0x02, 0x03, 0x00, 0x04,
            0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71, 0x14,
            0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0, 0x24,
            0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27,
            0x28, 0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46,
            0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59, 0x5a, 0x63, 0x64,
            0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a,
            0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97,
            0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3,
            0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8,
            0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2, 0xe3,
            0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7,
            0xf8, 0xf9, 0xfa, 0xff, 0xda, 0x00, 0x0e, 0x04, 0x43, 0x00, 0x4d, 0x00, 0x59, 0x00,
            0x4b, 0x00, 0x00, 0x3f, 0x00, 0xfd, 0x53, 0xa6, 0x57, 0xe5, 0x55, 0x7e, 0xa9, 0xd1,
            0x45, 0x14, 0x51, 0x45, 0x14, 0x51, 0x45, 0x14, 0x51, 0x45, 0x14, 0x51, 0x45, 0x14,
            0x57, 0xff, 0xd9,
        ]
    }

    #[test]
    fn every_compatibility_file_has_an_executing_consumer() {
        let root = Path::new(CONTRACT_ROOT);
        fn files_under(root: &Path, path: &Path, files: &mut BTreeSet<String>) {
            for entry in fs::read_dir(path).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if path.is_dir() {
                    files_under(root, &path, files);
                } else if path.is_file() {
                    files.insert(
                        path.strip_prefix(root)
                            .unwrap()
                            .to_str()
                            .unwrap()
                            .replace('\\', "/"),
                    );
                }
            }
        }
        let mut actual = BTreeSet::new();
        files_under(root, root, &mut actual);
        let consumed = BTreeSet::from([
            "identity/vectors.json",
            "metadata/capture-order.json",
            "metadata/capture-time.json",
            "preview/fixtures.json",
            "protocol/batch-workflows.json",
            "protocol/browse-vectors.json",
            "protocol/cache-vectors.json",
            "protocol/capture-order-omission.json",
            "protocol/responses.json",
            "protocol/vectors.json",
            "sqlite/malformed-v2.sql",
            "sqlite/rejections.json",
            "sqlite/schema-v1.json",
            "sqlite/schema-v1.sql",
            "sqlite/schema-v2.json",
            "sqlite/schema-v2.sql",
            "sqlite/schema-v3.json",
            "sqlite/schema-v3.sql",
            "sqlite/schema-v4.json",
            "sqlite/schema-v4.sql",
            "sqlite/schema-v5.json",
            "sqlite/schema-v5.sql",
            "sqlite/schema-v6.json",
            "sqlite/schema-v6.sql",
            "sqlite/v0.sql",
            "sqlite/v1.sql",
            "startup/vectors.json",
        ])
        .into_iter()
        .map(str::to_owned)
        .collect::<BTreeSet<_>>();
        assert_eq!(
            actual, consumed,
            "each compatibility file must be added to an executing consumer and this inventory"
        );
        let repository = root.parent().unwrap();
        for (file, source) in [
            (
                "identity/vectors.json",
                "crates/slipstream-core/src/identity.rs",
            ),
            (
                "metadata/capture-time.json",
                "crates/slipstream-core/src/capture.rs",
            ),
            (
                "metadata/capture-order.json",
                "crates/slipstream-core/src/capture.rs",
            ),
            (
                "protocol/batch-workflows.json",
                "crates/slipstream-compat/src/lib.rs",
            ),
            (
                "protocol/browse-vectors.json",
                "crates/slipstream-server/src/tests.rs",
            ),
            (
                "protocol/cache-vectors.json",
                "crates/slipstream-server/src/tests.rs",
            ),
            (
                "protocol/capture-order-omission.json",
                "crates/slipstream-server/src/tests.rs",
            ),
            (
                "protocol/responses.json",
                "crates/slipstream-server/src/tests.rs",
            ),
            (
                "protocol/vectors.json",
                "crates/slipstream-server/src/tests.rs",
            ),
            (
                "startup/vectors.json",
                "crates/slipstream-server/src/tests.rs",
            ),
            (
                "sqlite/malformed-v2.sql",
                "crates/slipstream-core/src/persistence/owner.rs",
            ),
            (
                "sqlite/rejections.json",
                "crates/slipstream-core/src/persistence/owner.rs",
            ),
            (
                "sqlite/v0.sql",
                "crates/slipstream-core/src/persistence/owner.rs",
            ),
            (
                "sqlite/v1.sql",
                "crates/slipstream-core/src/persistence/owner.rs",
            ),
        ] {
            assert!(
                fs::read_to_string(repository.join(source))
                    .unwrap()
                    .contains(file),
                "{file} lost its executing consumer {source}"
            );
        }
        for version in 1..=5 {
            let schema = fs::read_to_string(
                repository.join("crates/slipstream-core/src/persistence/schema.rs"),
            )
            .unwrap();
            for extension in ["sql", "json"] {
                let file = format!("sqlite/schema-v{version}.{extension}");
                assert!(schema.contains(&file), "{file} lost its schema consumer");
            }
        }
        let preview =
            fs::read_to_string(repository.join("crates/slipstream-compat/src/lib.rs")).unwrap();
        assert!(
            preview.contains("fn preview_fixture_matrix_runs_rust_derivative_for_every_recipe")
        );
        assert!(preview.contains("contract_path(\"preview/fixtures.json\")"));
    }

    #[test]
    fn shared_protocol_goldens_preserve_absence_instead_of_null() {
        let mut vectors: Vec<Value> =
            serde_json::from_slice(&fs::read(contract_path("protocol/vectors.json")).unwrap())
                .unwrap();
        vectors.extend(
            serde_json::from_slice::<Vec<Value>>(
                &fs::read(contract_path("protocol/cache-vectors.json")).unwrap(),
            )
            .unwrap(),
        );
        vectors.extend(
            serde_json::from_slice::<Vec<Value>>(
                &fs::read(contract_path("protocol/browse-vectors.json")).unwrap(),
            )
            .unwrap(),
        );
        assert!(vectors.len() >= 20);
        for vector in &vectors {
            assert!(vector["request"]["method"].is_string());
            assert!(
                vector["request"]["path"].is_string() || vector["request"]["target"].is_string()
            );
            assert!(vector["expected"]["status"].is_number());
        }
        let browse: Vec<Value> = serde_json::from_slice(
            &fs::read(contract_path("protocol/browse-vectors.json")).unwrap(),
        )
        .unwrap();
        assert!(browse.iter().any(|vector| {
            vector["request"]["method"] == "POST"
                && vector["request"]["path"] == "/api/browse"
                && vector["expected"]["body"]["token"] == "$token"
        }));
        assert!(
            browse
                .iter()
                .any(|vector| vector["expected"]["status"] == 404)
        );
        let values: Vec<Value> =
            serde_json::from_slice(&fs::read(contract_path("protocol/responses.json")).unwrap())
                .unwrap();
        assert!(values.len() >= 6);
        for value in &values {
            assert!(
                !contains_null(value),
                "golden responses omit optional fields"
            );
        }
        assert!(
            values.iter().any(|v| {
                v["photos"][0]["preview"]["url"]
                    .as_str()
                    .is_some_and(|url| url == "$reviewUrl" || is_review_derivative_url(url))
            }),
            "browse photo summaries document the hydrated current preview URL"
        );
        assert!(
            values.iter().any(|v| {
                v["photos"][0]["preview"]["thumbnailUrl"]
                    .as_str()
                    .is_some_and(|url| {
                        url == "$thumbnailUrl"
                            || (url.starts_with("/api/derivatives/") && url.contains("/thumbnail/"))
                    })
            }),
            "browse photo summaries document the hydrated current thumbnail URL"
        );
        let ready_previews: Vec<&Value> = values
            .iter()
            .filter(|value| value["state"] == "ready")
            .collect();
        assert_eq!(
            ready_previews.len(),
            2,
            "the current top-level ready Preview response set is exact"
        );
        assert!(
            ready_previews.iter().any(|value| value["stale"] == false),
            "direct Preview responses include a current ready example"
        );
        assert!(
            ready_previews.iter().any(|value| value["stale"] == true),
            "direct Preview responses include a stale ready example"
        );
        for preview in ready_previews {
            assert!(
                preview["source"].is_string(),
                "top-level ready Preview response has a source"
            );
            let url = preview["url"]
                .as_str()
                .expect("top-level ready Preview response has a URL");
            assert!(
                url == "$secondReviewUrl" || is_review_derivative_url(url),
                "top-level ready Preview response uses the target-qualified review URL"
            );
            let retired_url = url.replacen("/review/", "/", 1);
            assert!(
                !is_review_derivative_url(&retired_url),
                "the retired no-target direct Preview URL stays rejected"
            );
        }
        assert!(
            values
                .iter()
                .any(|v| v["state"] == "ready" && v["stale"] == true)
        );
        assert!(values.iter().any(|v| v["state"] == "failed"));
        assert!(values.iter().any(|v| v["undo"]["field"] == "rating"));
    }

    #[test]
    fn batch_workflow_contract_examples_match_wire_rules() {
        fn keys(value: &Value) -> BTreeSet<String> {
            value
                .as_object()
                .expect("contract value is an object")
                .keys()
                .cloned()
                .collect()
        }

        let contract: Value = serde_json::from_slice(
            &fs::read(contract_path("protocol/batch-workflows.json")).unwrap(),
        )
        .unwrap();
        assert!(!contains_null(&contract));

        let photo_state_cases = contract["photoStateBatch"]
            .as_array()
            .expect("photo state batch examples");
        assert_eq!(photo_state_cases.len(), 2);
        let photo_state_response_keys = BTreeSet::from([
            "applied".to_owned(),
            "changedElsewhere".to_owned(),
            "missing".to_owned(),
        ]);
        for case in photo_state_cases {
            let request = &case["request"];
            assert_eq!(request["method"], "POST");
            assert_eq!(request["path"], "/api/photos/state");
            assert_eq!(
                keys(&request["body"]),
                BTreeSet::from(["selectionState".to_owned(), "photos".to_owned()])
            );
            assert!(matches!(
                request["body"]["selectionState"].as_str(),
                Some("selected" | "rejected")
            ));
            let photos = request["body"]["photos"]
                .as_array()
                .expect("batch request photos");
            assert!(!photos.is_empty());
            let mut requested_ids = BTreeSet::new();
            for photo in photos {
                assert_eq!(
                    keys(photo),
                    BTreeSet::from(["photoId".to_owned(), "expectedCurrent".to_owned()])
                );
                assert!(requested_ids.insert(photo["photoId"].as_str().unwrap()));
                assert!(matches!(
                    photo["expectedCurrent"].as_str(),
                    Some("undecided" | "selected" | "rejected")
                ));
            }

            let response = &case["response"];
            assert_eq!(keys(response), photo_state_response_keys);
            let applied = response["applied"].as_array().unwrap();
            let changed = response["changedElsewhere"].as_array().unwrap();
            let missing = response["missing"].as_array().unwrap();
            assert_eq!(applied.len() + changed.len() + missing.len(), photos.len());
            let mut outcome_ids = BTreeSet::new();
            for item in applied {
                assert_eq!(
                    keys(item),
                    BTreeSet::from(["photoId".to_owned(), "priorValue".to_owned()])
                );
                assert!(outcome_ids.insert(item["photoId"].as_str().unwrap()));
                assert!(matches!(
                    item["priorValue"].as_str(),
                    Some("undecided" | "selected" | "rejected")
                ));
            }
            for item in changed {
                assert_eq!(
                    keys(item),
                    BTreeSet::from(["photoId".to_owned(), "currentValue".to_owned()])
                );
                assert!(outcome_ids.insert(item["photoId"].as_str().unwrap()));
                assert!(matches!(
                    item["currentValue"].as_str(),
                    Some("undecided" | "selected" | "rejected")
                ));
            }
            for item in missing {
                assert_eq!(keys(item), BTreeSet::from(["photoId".to_owned()]));
                assert!(outcome_ids.insert(item["photoId"].as_str().unwrap()));
            }
            assert_eq!(outcome_ids, requested_ids);
        }

        let album_cases = contract["albumMembership"]
            .as_array()
            .expect("Album membership examples");
        assert_eq!(album_cases.len(), 2);
        for case in album_cases {
            let request = &case["request"];
            assert_eq!(request["method"], "POST");
            assert!(request["path"].as_str().unwrap().contains("$albumId"));
            assert!(request["body"]["photoIds"].as_array().unwrap().len() <= 100);
            let response = &case["response"];
            assert!(response["albumId"].is_string());
            let albums = response["albums"].as_array().unwrap();
            assert!(!albums.is_empty());
            assert!(albums[0]["id"].is_string());
            assert!(albums[0]["name"].is_string());
            assert!(albums[0]["photoCount"].is_number());
            assert!(albums[0]["hasSavedPosition"].is_boolean());
            match case["name"].as_str().unwrap() {
                "add-result" => assert_eq!(
                    keys(response),
                    BTreeSet::from([
                        "albumId".to_owned(),
                        "addedPhotoIds".to_owned(),
                        "alreadyMemberPhotoIds".to_owned(),
                        "albums".to_owned(),
                    ])
                ),
                "remove-added-result" => assert_eq!(
                    keys(response),
                    BTreeSet::from([
                        "albumId".to_owned(),
                        "removedPhotoIds".to_owned(),
                        "alreadyAbsentPhotoIds".to_owned(),
                        "albums".to_owned(),
                    ])
                ),
                other => panic!("unexpected Album contract example: {other}"),
            }
        }

        let invalid = contract["invalidRequests"]
            .as_array()
            .expect("invalid request examples");
        assert_eq!(invalid.len(), 8);
        for example in invalid {
            assert!(example["name"].is_string());
            let route = example["route"].as_str().expect("invalid request route");
            assert!(example["body"].is_object());
            let expected_error = if route.ends_with("/members/batch-remove") {
                "Invalid membership compensation"
            } else if route.starts_with("/api/albums/") {
                "Invalid membership batch"
            } else {
                "Invalid Photo state batch"
            };
            assert_eq!(example["error"], expected_error);
        }
        let over_limit = invalid
            .iter()
            .find(|example| example["name"] == "photo-state-batch-over-limit")
            .expect("over-limit invalid request example");
        let over_limit_photos = over_limit["body"]["photos"].as_array().unwrap();
        assert_eq!(over_limit_photos.len(), 101);
        let over_limit_ids = over_limit_photos
            .iter()
            .filter_map(|photo| photo["photoId"].as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(over_limit_ids.len(), 101);
        assert!(over_limit_ids.iter().all(|id| {
            id.len() == 36
                && id.starts_with("00000000-0000-4000-8000-")
                && id[24..].chars().all(|character| character.is_ascii_digit())
        }));

        let malformed = contract["malformedResponses"]
            .as_array()
            .expect("malformed response examples");
        assert_eq!(malformed.len(), 7);
        for example in malformed {
            assert!(example["name"].is_string());
            assert!(matches!(
                example["kind"].as_str(),
                Some("photoStateBatch" | "albumMembership")
            ));
            assert!(example["response"].is_object());
            assert_eq!(example["classification"], "malformed");
        }
        assert!(malformed[0]["response"].get("missing").is_none());
        assert!(malformed[1]["response"].get("conflicts").is_some());
        assert!(malformed[2]["response"].get("addedCount").is_some());
        assert_eq!(
            malformed[3]["response"]["applied"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            malformed[4]["response"]["applied"][0]["photoId"],
            "$unrequestedPhotoId"
        );
        assert!(
            malformed[5]["response"]["applied"][0]
                .get("unexpected")
                .is_some()
        );
        assert_eq!(
            malformed[6]["response"]["applied"][0]["photoId"],
            malformed[6]["response"]["missing"][0]["photoId"]
        );
    }

    #[test]
    fn review_derivative_url_validator_matches_server_identifier_grammar() {
        let uuid_photo_id = "a0000000-0000-4000-8000-000000000020";
        let hash_photo_id = "22aabb24fc11ac401ef1989dd4f0579579b4928169040cde3546d89c4cea7255";
        let cache_key = "e58bdfea56e2b8808ee7ff77350635351d084ec8a10f657e2512360751396105";
        let invalid_photo_id = format!("{}g", &hash_photo_id[..63]);
        let invalid_cache_key = format!("{}g", &cache_key[..63]);

        for url in [
            format!("/api/derivatives/{uuid_photo_id}/review/{cache_key}.jpg"),
            format!("/api/derivatives/{hash_photo_id}/review/{cache_key}.jpg"),
        ] {
            assert!(is_review_derivative_url(&url), "accepted URL: {url}");
        }

        for url in [
            format!(
                "/api/derivatives/{}/review/{cache_key}.jpg",
                uuid_photo_id.to_ascii_uppercase()
            ),
            format!(
                "/api/derivatives/{uuid_photo_id}/review/{}.jpg",
                cache_key.to_ascii_uppercase()
            ),
            format!("/api/derivatives/{}/review/{cache_key}.jpg", "a".repeat(35)),
            format!("/api/derivatives/{}/review/{cache_key}.jpg", "a".repeat(65)),
            format!("/api/derivatives/{invalid_photo_id}/review/{cache_key}.jpg"),
            format!(
                "/api/derivatives/{uuid_photo_id}/review/{}.jpg",
                "a".repeat(63)
            ),
            format!("/api/derivatives/{uuid_photo_id}/review/{invalid_cache_key}.jpg"),
            format!("/api/derivatives/{uuid_photo_id}/{cache_key}.jpg"),
            format!("/api/derivatives/{uuid_photo_id}/review/{cache_key}.jpg/extra"),
            format!("/api/derivatives/{uuid_photo_id}/review/{cache_key}.jpeg"),
        ] {
            assert!(!is_review_derivative_url(&url), "rejected URL: {url}");
        }
    }

    fn is_review_derivative_url(url: &str) -> bool {
        let Some(path) = url.strip_prefix("/api/derivatives/") else {
            return false;
        };
        let mut segments = path.split('/');
        let (Some(photo_id), Some("review"), Some(filename), None) = (
            segments.next(),
            segments.next(),
            segments.next(),
            segments.next(),
        ) else {
            return false;
        };
        let Some(cache_key) = filename.strip_suffix(".jpg") else {
            return false;
        };
        is_valid_photo_id(photo_id) && is_valid_cache_key(cache_key)
    }

    fn is_valid_photo_id(value: &str) -> bool {
        value.len() >= 36
            && value.len() <= 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte) || byte == b'-')
    }

    fn is_valid_cache_key(value: &str) -> bool {
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }

    fn contains_null(value: &Value) -> bool {
        match value {
            Value::Null => true,
            Value::Array(values) => values.iter().any(contains_null),
            Value::Object(values) => values.values().any(contains_null),
            _ => false,
        }
    }

    #[test]
    fn shared_identity_vectors_match_contract_serialization() {
        let contract: IdentityContract =
            serde_json::from_slice(&fs::read(contract_path("identity/vectors.json")).unwrap())
                .unwrap();
        assert_eq!(contract.algorithm_version, "sharp-v2");
        for vector in contract.vectors {
            assert!(matches!(
                vector.source.as_str(),
                "matching-jpeg" | "embedded-raw-jpeg"
            ));
            assert!(matches!(vector.edge, 512 | 2560));
            if let Some(candidate) = vector.candidate {
                assert!(!candidate.is_empty());
                assert!(candidate.parse::<u32>().is_ok());
            }
            assert!(is_valid_cache_key(&vector.historical_cache_key));
            assert!(is_valid_cache_key(&vector.historical_manifest_identity));
            assert_eq!(original_id(&vector.path), vector.original_id);
            assert_eq!(photo_id(&vector.original_id), vector.photo_id);
            assert_eq!(
                source_revision(&vector.path, vector.size, vector.mtime_ms),
                vector.source_revision
            );
        }
    }

    #[test]
    fn canonical_schema_snapshots_execute_with_bundled_sqlite() {
        for version in ["v4", "v5", "v6"] {
            let connection = Connection::open_in_memory().unwrap();
            connection
                .execute_batch(
                    &fs::read_to_string(contract_path(&format!("sqlite/schema-{version}.sql")))
                        .unwrap(),
                )
                .unwrap();
            let expected: Value = serde_json::from_slice(
                &fs::read(contract_path(&format!("sqlite/schema-{version}.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(schema_manifest(&connection), expected, "{version}");
            assert_eq!(
                connection
                    .query_row("PRAGMA foreign_key_check", [], |_| Ok(1))
                    .optional()
                    .unwrap(),
                None,
                "{version}"
            );
        }
        assert!(!rusqlite::version().is_empty());
    }

    fn compact_sql(value: &str) -> String {
        value
            .to_lowercase()
            .chars()
            .filter(|character| !character.is_whitespace() && !"\"`[]".contains(*character))
            .collect()
    }

    fn schema_manifest(connection: &Connection) -> Value {
        let version: u32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        let mut objects = Vec::new();
        let mut statement = connection
            .prepare(
                "SELECT type,name,sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type,name",
            )
            .unwrap();
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .unwrap();
        for row in rows {
            let (kind, name, sql) = row.unwrap();
            let mut object = serde_json::json!({
                "type": kind,
                "name": name,
                "sql": compact_sql(&sql),
            });
            if kind == "table" {
                let columns = connection
                    .prepare(&format!("PRAGMA table_info(\"{name}\")"))
                    .unwrap()
                    .query_map([], |row| {
                        Ok(serde_json::json!({
                            "name": row.get::<_, String>(1)?,
                            "type": row.get::<_, String>(2)?,
                            "notNull": row.get::<_, bool>(3)?,
                            "default": row.get::<_, Option<String>>(4)?,
                            "primaryKey": row.get::<_, u32>(5)?,
                        }))
                    })
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                let foreign_keys = connection
                    .prepare(&format!("PRAGMA foreign_key_list(\"{name}\")"))
                    .unwrap()
                    .query_map([], |row| {
                        Ok(serde_json::json!({
                            "table": row.get::<_, String>(2)?,
                            "from": row.get::<_, String>(3)?,
                            "to": row.get::<_, String>(4)?,
                            "onUpdate": row.get::<_, String>(5)?,
                            "onDelete": row.get::<_, String>(6)?,
                        }))
                    })
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap();
                object["columns"] = columns.into();
                object["foreignKeys"] = foreign_keys.into();
            }
            objects.push(object);
        }
        serde_json::json!({ "userVersion": version, "objects": objects })
    }

    #[test]
    fn startup_vectors_are_parseable_and_cover_defaults_and_explicit_binding() {
        let values: Vec<Value> =
            serde_json::from_slice(&fs::read(contract_path("startup/vectors.json")).unwrap())
                .unwrap();
        assert_eq!(values.len(), 2);
        assert_eq!(values[0]["expected"]["host"], "127.0.0.1");
        assert_eq!(values[0]["expected"]["port"], 3000);
        assert_eq!(values[1]["expected"]["host"], "0.0.0.0");
        assert_eq!(values[1]["expected"]["port"], 8080);
        for value in values {
            for key in [
                "libraryRoot",
                "stateDirectory",
                "cacheDirectory",
                "databaseBasename",
                "host",
                "port",
            ] {
                assert!(value["expected"].get(key).is_some(), "missing {key}");
            }
        }
    }

    #[tokio::test]
    async fn selected_axum_stack_compiles_and_serves_an_owned_state() {
        let response = Router::new()
            .route("/probe", get(|| async { "ok" }))
            .oneshot(
                http::Request::builder()
                    .uri("/probe")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), http::StatusCode::OK);
    }

    #[test]
    fn native_shim_and_rust_image_color_candidates_link_and_run() {
        let _lease = VIPS_TEST_LEASE
            .get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap();
        let (raw, jpeg) = native_versions();
        assert!(!raw.is_empty());
        assert!(jpeg > 0);
        assert!(thumbnail_api_available());
        assert!(unsafe { slipstream_probe_vips_lifecycle() == 1 });

        let image = DynamicImage::new_rgb8(9, 5);
        let resized = image.resize(4, 4, FilterType::Lanczos3);
        assert_eq!(resized.dimensions(), (4, 2));
        assert!(Profile::new_srgb().version() > 0.0);
    }

    use rusqlite::OptionalExtension;
}
