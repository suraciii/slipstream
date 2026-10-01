//! The Photo worker's semantic development request over the private native engine.

use crate::mcp_client::McpClient;
use serde_json::{Value, json};
use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

const ENGINE: &str = "/opt/darktable/bin/darktable-mcp";
const METADATA: &str = "/opt/slipstream-photo/engine-metadata.json";
const PROFILE: &str = "/work/config/color/out/linear-prophoto.icc";
const METADATA_BYTES_MAX: u64 = 16 * 1024 * 1024;

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

pub fn develop(input: &Path, output: &Path, exposure_milli_ev: i64) -> io::Result<()> {
    let metadata = File::open(METADATA)?;
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
    let approved: Value = serde_json::from_slice(&bytes)
        .map_err(|_| io::Error::other("engine metadata is invalid"))?;
    let args: Vec<String> = [
        "--core",
        "--disable-opencl",
        "--configdir",
        "/work/config",
        "--cachedir",
        "/work/cache",
        "--tmpdir",
        "/work/tmp",
        "--library",
        "/work/library.db",
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
        ("HOME".to_string(), "/work/xdg".to_string()),
        ("XDG_CONFIG_HOME".to_string(), "/work/xdg".to_string()),
        ("XDG_CACHE_HOME".to_string(), "/work/cache".to_string()),
        ("TMPDIR".to_string(), "/work/tmp".to_string()),
        ("OMP_NUM_THREADS".to_string(), "4".to_string()),
    ];
    let mut engine = McpClient::spawn(ENGINE, &args, &env)?;
    engine.initialize("slipstream-photo-worker")?;
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
    let result = engine.call(
        "export_images",
        json!({
            "input": {"path": input},
            "out_path": output,
            "format": "scene-linear-tiff",
            "icc_file": PROFILE,
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
