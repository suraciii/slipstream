//! The semantic development request over the private native engine.
//!
//! [`develop_at`] runs the pinned `development-tiff` protocol over
//! caller-owned paths for local single-container execution: the engine
//! binary, bundle metadata, staged output profile, and private work tree
//! all belong to the calling executor.

use crate::mcp_client::McpClient;
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{self, Read},
    path::Path,
    sync::{Arc, atomic::AtomicBool},
    time::Instant,
};

const METADATA_BYTES_MAX: u64 = 16 * 1024 * 1024;

/// The external termination authority of one local execution: a cancelled
/// flag polled together with the absolute deadline by the engine watchdog.
pub(crate) struct Guard {
    pub cancellation: Arc<AtomicBool>,
    pub deadline: Instant,
}

fn module_list(value: &Value) -> io::Result<&[Value]> {
    match value {
        Value::Array(entries) => Ok(entries),
        Value::Object(object) => object
            .get("modules")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .ok_or_else(|| io::Error::other("engine module list is missing")),
        _ => Err(io::Error::other("engine module list is not an array")),
    }
}

/// One strict UTF-8 engine path argument: the pinned bundle has no
/// non-UTF-8 paths, and a caller-supplied one must not become an ambient
/// encoding surprise.
pub(crate) fn strict(path: &Path) -> io::Result<&str> {
    path.to_str()
        .ok_or_else(|| io::Error::other("engine path is not valid UTF-8"))
}

/// Read and bound-check the bundle engine-metadata contract.
pub(crate) fn approved_metadata(metadata_path: &Path) -> io::Result<Value> {
    let metadata = File::open(metadata_path)?;
    if metadata.metadata()?.len() > METADATA_BYTES_MAX {
        return Err(io::Error::other("engine metadata exceeds the bundle bound"));
    }
    let mut bytes = Vec::new();
    metadata
        .take(METADATA_BYTES_MAX + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > METADATA_BYTES_MAX {
        return Err(io::Error::other("engine metadata exceeds the bundle bound"));
    }
    serde_json::from_slice(&bytes).map_err(|_| io::Error::other("engine metadata is invalid"))
}

/// Pin one running engine to the bundle contract before any payload work:
/// the tool inventory, the module list, and every recorded module schema
/// must match the approved metadata exactly. Discovery here grants
/// nothing; a differing engine is refused.
pub(crate) fn verify_engine_contract(engine: &mut McpClient, approved: &Value) -> io::Result<()> {
    let modules = engine.call("list_modules", json!({}))?;
    if engine.tools_list()? != approved["tools"]
        || module_list(&modules)? != module_list(&approved["modules"])?
    {
        return Err(io::Error::other(
            "engine tools or module identities differ from the bundle",
        ));
    }
    let schemas = approved["schemas"]
        .as_object()
        .ok_or_else(|| io::Error::other("engine schema inventory is missing"))?;
    // One generic metadata contract; no C parameter layouts or module encoders here.
    for (operation, schema) in schemas {
        if engine.call("module_schema", json!({"operation": operation}))? != *schema {
            return Err(io::Error::other(format!(
                "engine schema differs for {operation}"
            )));
        }
    }
    Ok(())
}
/// Parameterized development for local, single-container execution: the
/// engine binary, the bundle metadata, the staged output profile and the
/// private work tree are all caller-owned, and every engine-private path
/// is derived below `work`. The optional guard binds the run to the
/// caller's cancellation flag and deadline.
#[allow(clippy::too_many_arguments)]
pub(crate) fn develop_at(
    engine: &Path,
    metadata_path: &Path,
    profile: &Path,
    work: &Path,
    input: &Path,
    output: &Path,
    exposure_milli_ev: i64,
    client: &str,
    guard: Option<Guard>,
) -> io::Result<()> {
    let approved = approved_metadata(metadata_path)?;
    let config = work.join("config");
    let cache = work.join("cache");
    let tmp = work.join("tmp");
    let xdg = work.join("xdg");
    let library = work.join("library.db");
    let args: Vec<String> = [
        "--core",
        "--disable-opencl",
        "--configdir",
        strict(&config)?,
        "--cachedir",
        strict(&cache)?,
        "--tmpdir",
        strict(&tmp)?,
        "--library",
        strict(&library)?,
        "--conf",
        "plugins/darkroom/workflow=none",
        "--conf",
        "write_sidecar_files=never",
        "--conf",
        "run_crawler_on_start=FALSE",
        "--conf",
        "plugins/imageio/format/tiff/bpp=32",
        "--conf",
        "plugins/imageio/format/tiff/compress=1",
        "--conf",
        "plugins/imageio/format/tiff/compresslevel=6",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    let env = [
        (
            "PATH".to_string(),
            "/opt/darktable/bin:/usr/local/bin:/usr/bin:/bin".to_string(),
        ),
        ("HOME".to_string(), strict(&xdg)?.to_string()),
        ("XDG_CONFIG_HOME".to_string(), strict(&xdg)?.to_string()),
        ("XDG_CACHE_HOME".to_string(), strict(&cache)?.to_string()),
        ("TMPDIR".to_string(), strict(&tmp)?.to_string()),
        ("OMP_NUM_THREADS".to_string(), "4".to_string()),
    ];
    let program = strict(engine)?;
    let mut engine = match guard {
        Some(Guard {
            cancellation,
            deadline,
        }) => McpClient::spawn_guarded(program, &args, &env, cancellation, deadline)?,
        None => McpClient::spawn(program, &args, &env)?,
    };
    engine.initialize(client)?;
    verify_engine_contract(&mut engine, &approved)?;
    let result = engine.call(
        "export_images",
        json!({
            "input": {"path": input},
            "out_path": output,
            "format": "scene-linear-tiff",
            "icc_file": profile,
            "baseline": "raw-development",
            "width": 0,
            "height": 0,
            "upscale": false,
            "high_quality": true,
            "stack": [{
                "operation": "exposure",
                "multi_priority": 0,
                "enabled": true,
                "params": {
                    "mode": "EXPOSURE_MODE_MANUAL",
                    "black": 0.0,
                    "exposure": exposure_milli_ev as f64 / 1000.0,
                    "compensate_exposure_bias": false,
                    "compensate_hilite_pres": false
                }
            }]
        }),
    )?;
    if result["paths"]
        .as_array()
        .is_none_or(|paths| paths.as_slice() != [json!(output)])
        || result["skipped"].as_u64() != Some(0)
        || result["exported"].as_u64() != Some(1)
        || result["ok"].as_bool() != Some(true)
    {
        return Err(io::Error::other(
            "engine did not write the requested artifact",
        ));
    }
    engine.shutdown()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_list_accepts_the_supported_wire_shapes() {
        let entries = json!([{"operation": "exposure"}]);
        assert_eq!(module_list(&entries).unwrap(), entries.as_array().unwrap());
        let wrapped = json!({"modules": entries});
        assert_eq!(
            module_list(&wrapped).unwrap(),
            wrapped["modules"].as_array().unwrap(),
        );
    }

    #[test]
    fn module_list_rejects_missing_or_non_array_payloads() {
        for value in [json!(null), json!({}), json!({"modules": "invalid"})] {
            assert!(module_list(&value).is_err());
        }
    }
}
