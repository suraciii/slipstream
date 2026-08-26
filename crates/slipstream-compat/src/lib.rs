//! Executable compatibility contracts for the staged Rust server migration.
//! This crate is not a production server.

#[cfg(test)]
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{ffi::CStr, os::raw::c_char, path::PathBuf};

const CONTRACT_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../compatibility");

unsafe extern "C" {
    fn slipstream_probe_libraw_version() -> *const c_char;
    fn slipstream_probe_jpeg_version() -> i32;
    fn slipstream_probe_thumbnail_api() -> i32;
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
    paired: PairedVector,
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
    edge: u16,
    #[serde(rename = "originalId")]
    original_id: String,
    #[serde(rename = "photoId")]
    photo_id: String,
    #[serde(rename = "sourceRevision")]
    source_revision: String,
    #[serde(rename = "cacheKey")]
    cache_key: String,
    #[serde(rename = "manifestIdentity")]
    manifest_identity: String,
}

#[cfg(test)]
#[derive(Deserialize)]
struct PairedVector {
    #[serde(rename = "rawOriginalId")]
    raw_original_id: String,
    #[serde(rename = "jpegOriginalId")]
    jpeg_original_id: String,
    #[serde(rename = "photoId")]
    photo_id: String,
}

#[cfg(test)]
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CacheKeyInput<'a> {
    version: &'a str,
    photo: &'a str,
    source: &'a str,
    path: &'a str,
    size: u64,
    mtime_ms: f64,
    candidate: &'a Option<String>,
    edge: u16,
}

#[cfg(test)]
#[derive(Serialize)]
struct ManifestKeyInput<'a> {
    photo: &'a str,
    edge: u16,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, routing::get};
    use image::{DynamicImage, GenericImageView, imageops::FilterType};
    use lcms2::Profile;
    use rusqlite::Connection;
    use serde_json::Value;
    use std::fs;
    use tower::ServiceExt;

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
        assert!(vectors.len() >= 9);
        for vector in &vectors {
            assert!(vector["request"]["method"].is_string());
            assert!(
                vector["request"]["path"].is_string() || vector["request"]["target"].is_string()
            );
            assert!(vector["expected"]["status"].is_number());
        }
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
            values
                .iter()
                .any(|v| v["state"] == "ready" && v["stale"] == true)
        );
        assert!(values.iter().any(|v| v["state"] == "failed"));
        assert!(values.iter().any(|v| v["undo"]["field"] == "rating"));
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
    fn shared_identity_vectors_match_javascript_serialization() {
        let contract: IdentityContract =
            serde_json::from_slice(&fs::read(contract_path("identity/vectors.json")).unwrap())
                .unwrap();
        for vector in contract.vectors {
            assert_eq!(original_id(&vector.path), vector.original_id);
            assert_eq!(photo_id(&vector.original_id), vector.photo_id);
            assert_eq!(
                source_revision(&vector.path, vector.size, vector.mtime_ms),
                vector.source_revision
            );
            let cache = CacheKeyInput {
                version: &contract.algorithm_version,
                photo: &vector.photo_id,
                source: &vector.source,
                path: &vector.path,
                size: vector.size,
                mtime_ms: vector.mtime_ms,
                candidate: &vector.candidate,
                edge: vector.edge,
            };
            assert_eq!(
                digest(serde_json::to_string(&cache).unwrap().as_bytes()),
                vector.cache_key
            );
            let manifest = ManifestKeyInput {
                photo: &vector.photo_id,
                edge: vector.edge,
            };
            assert_eq!(
                digest(serde_json::to_string(&manifest).unwrap().as_bytes()),
                vector.manifest_identity
            );
        }
        let pair_seed = format!(
            "{}\0{}",
            contract.paired.raw_original_id, contract.paired.jpeg_original_id
        );
        assert_eq!(photo_id(&pair_seed), contract.paired.photo_id);
    }

    #[test]
    fn canonical_schema_snapshot_executes_with_bundled_sqlite() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(&fs::read_to_string(contract_path("sqlite/schema-v2.sql")).unwrap())
            .unwrap();
        let expected: Value =
            serde_json::from_slice(&fs::read(contract_path("sqlite/schema-v2.json")).unwrap())
                .unwrap();
        assert_eq!(schema_manifest(&connection), expected);
        assert_eq!(
            connection
                .query_row("PRAGMA foreign_key_check", [], |_| Ok(1))
                .optional()
                .unwrap(),
            None
        );
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
        let (raw, jpeg) = native_versions();
        assert!(!raw.is_empty());
        assert!(jpeg > 0);
        assert!(thumbnail_api_available());

        let image = DynamicImage::new_rgb8(9, 5);
        let resized = image.resize(4, 4, FilterType::Lanczos3);
        assert_eq!(resized.dimensions(), (4, 2));
        assert!(Profile::new_srgb().version() > 0.0);
    }

    use rusqlite::OptionalExtension;
}
