//! Bounded selected-step Preview over the local native engine.
//!
//! [`render_selected_step`] renders exactly one selected darktable
//! Processing Step — its complete, validated `darktable-params-1` parameter
//! tree — at a finite disclosed geometry through the native MCP `render`
//! tool, which answers with the bounded display rendition itself. No
//! full-resolution development handoff is produced, downscaled, or required:
//! the rendition never exists above the admitted bound. Nothing beside the
//! selected step's own stack is applied; there is no implicit chaining, no
//! predecessor step, and no camera-preview fallback.
//!
//! The engine child runs with an explicit in-memory library and an explicit
//! never-write sidecar policy — a fresh private configdir carries no
//! darktablerc defaults to inherit — so the Original and any XMP sidecar
//! stay untouched. The caller owns a clean private work directory and
//! serializes admission; the run is bounded by cancellation and timeout
//! through the same guarded process-group supervisor as local development.

use crate::{
    mcp_client::McpClient,
    modules::{DARKTABLE_MODULE, ModuleAvailability, ModuleRegistry, Parameters},
    native_development::{self, Guard},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// The bounded Preview geometry: the qualified Edit Preview long edge. The
/// `render` tool takes a width/height bounding box, so both edges are
/// pinned to this one bound and a rendition can never carry a longer edge
/// than the deployment's disclosed Preview geometry. The integration seam
/// asserts this stays within the application's preview bound.
pub const PREVIEW_LONG_EDGE: u32 = 1224;
/// The bounded size of one Preview rendition. The protocol line bound
/// already caps the base64 response; this pins the decoded bytes a caller
/// may publish.
const PREVIEW_BYTES_MAX: usize = 16 * 1024 * 1024;
/// The engine client identity of local bounded Preview execution.
const CLIENT: &str = "slipstream-local-preview";

/// The finite disclosed geometry of one bounded Preview: both edges of the
/// engine's width/height bounding box.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RenderGeometry {
    pub width: u32,
    pub height: u32,
}

impl RenderGeometry {
    /// The admitted Preview geometry: one bounding box whose both edges
    /// are the disclosed long-edge bound.
    pub const fn preview() -> Self {
        Self {
            width: PREVIEW_LONG_EDGE,
            height: PREVIEW_LONG_EDGE,
        }
    }

    /// Whether one rendition geometry fits inside this bounding box. A
    /// rendition outside the box was not rendered at the admitted geometry
    /// and is refused.
    pub const fn contains(self, width: u32, height: u32) -> bool {
        width != 0 && height != 0 && width <= self.width && height <= self.height
    }
}

/// Validated identity of one locally rendered bounded Preview PNG.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreviewIdentity {
    pub size: u64,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
}

fn cancelled() -> io::Error {
    io::Error::other("bounded preview was cancelled")
}

/// Replace an engine error with its termination cause when the run ended
/// by authority instead of by protocol failure.
fn terminated(error: io::Error, cancellation: &AtomicBool, deadline: Instant) -> io::Error {
    if cancellation.load(Ordering::Relaxed) {
        return cancelled();
    }
    if Instant::now() >= deadline {
        return io::Error::other("bounded preview timed out");
    }
    error
}

/// Validate the selected step's complete `darktable-params-1` envelope and
/// extract its render stack verbatim from the pinned schema. The tree is
/// one object of at most `stack` and
/// `output`; every stack entry carries exactly the declared fields, and
/// `output`, when present, is exactly the pinned development handoff the
/// step's Export publishes — the bounded Preview renders the stack, never
/// that handoff. Anything else is refused before the engine starts.
pub(crate) fn selected_step_stack(parameters: &Parameters) -> io::Result<Vec<Value>> {
    if parameters.module != DARKTABLE_MODULE {
        return Err(io::Error::other(
            "selected step parameters are not the admitted darktable envelope",
        ));
    }
    ModuleRegistry::new(
        ModuleAvailability::unavailable("validation only"),
        ModuleAvailability::unavailable("validation only"),
    )
    .validate_parameters(parameters)
    .map_err(|error| io::Error::other(error.message))?;
    Ok(parameters
        .tree
        .get("stack")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}

/// Extract the selected stack for native automatic evaluation. Retained
/// automatic modes are structurally valid editing intent; execution validation
/// is deliberately deferred until the engine returns concrete parameters.
pub(crate) fn automatic_step_stack(parameters: &Parameters) -> io::Result<Vec<Value>> {
    if parameters.module != DARKTABLE_MODULE {
        return Err(io::Error::other(
            "selected step parameters are not the admitted darktable envelope",
        ));
    }
    ModuleRegistry::new(
        ModuleAvailability::unavailable("validation only"),
        ModuleAvailability::unavailable("validation only"),
    )
    .validate_saved_parameters(parameters)
    .map_err(|error| io::Error::other(error.message))?;
    Ok(parameters
        .tree
        .get("stack")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default())
}
/// The render tool's engine spelling for one admitted module-owned stack.
/// The module contract uses `multiPriority`; the pinned native bridge consumes
/// `multi_priority`. This is a wire adaptation, not a change to the captured
/// parameter tree.
pub(crate) fn engine_stack(stack: &[Value]) -> Vec<Value> {
    stack
        .iter()
        .map(|entry| {
            let Some(object) = entry.as_object() else {
                return entry.clone();
            };
            let mut adapted = object.clone();
            if let Some(priority) = adapted.remove("multiPriority") {
                adapted.insert("multi_priority".to_owned(), priority);
            }
            Value::Object(adapted)
        })
        .collect()
}

/// The `render` tool arguments of one bounded Preview: the staged Original
/// by path, the finite disclosed bounding box, and the selected step's
/// complete stack with only the pinned engine-key spelling adapted.
pub(crate) fn render_arguments(input: &str, geometry: RenderGeometry, stack: &[Value]) -> Value {
    json!({
        "input": {"path": input},
        "width": geometry.width,
        "height": geometry.height,
        "stack": engine_stack(stack),
    })
}

/// The PNG signature every rendition must carry.
const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

/// The dimensions of one PNG byte stream from its mandatory leading IHDR
/// chunk. Only the signature, chunk header, and big-endian width/height
/// are read; the pixel data stays the engine's answer.
pub(crate) fn png_dimensions(bytes: &[u8]) -> io::Result<(u32, u32)> {
    let header = bytes
        .get(..24)
        .ok_or_else(|| io::Error::other("preview rendition is truncated"))?;
    if header[..8] != PNG_SIGNATURE {
        return Err(io::Error::other("preview rendition is not a PNG"));
    }
    if header[12..16] != *b"IHDR"
        || u32::from_be_bytes([header[8], header[9], header[10], header[11]]) != 13
    {
        return Err(io::Error::other(
            "preview rendition has no leading IHDR chunk",
        ));
    }
    let width = u32::from_be_bytes([header[16], header[17], header[18], header[19]]);
    let height = u32::from_be_bytes([header[20], header[21], header[22], header[23]]);
    if width == 0 || height == 0 {
        return Err(io::Error::other("preview rendition declares no geometry"));
    }
    Ok((width, height))
}

/// Validate one engine-rendered rendition against the bounded contract and
/// return its identity facts. A rendition above the disclosed geometry or
/// size bound was not a bounded render and is refused whole.
fn rendition_identity(png: &[u8], geometry: RenderGeometry) -> io::Result<PreviewIdentity> {
    if png.is_empty() || png.len() > PREVIEW_BYTES_MAX {
        return Err(io::Error::other(
            "preview rendition is empty or exceeds the bounded size",
        ));
    }
    let (width, height) = png_dimensions(png)?;
    if !geometry.contains(width, height) {
        return Err(io::Error::other(
            "preview rendition exceeds the admitted geometry",
        ));
    }
    Ok(PreviewIdentity {
        size: png.len() as u64,
        sha256: format!("{:x}", Sha256::digest(png)),
        width,
        height,
    })
}

/// Derive the engine-private tree below the caller's work directory. Only
/// the fixed skeleton is created; nothing outside `work` is written, and no
/// output profile is staged because the bounded rendition never leaves the
/// engine as a development handoff.
fn prepare(work: &Path) -> io::Result<()> {
    for directory in ["config", "cache", "tmp", "xdg"] {
        let path = work.join(directory);
        fs::create_dir_all(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Write the validated rendition bytes to the caller's private output path
/// and sync them before the identity is reported.
fn write_rendition(output: &Path, png: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(output)?;
    file.write_all(png)?;
    file.sync_all()
}

/// The bounded render sequence over the private native engine: a fresh
/// in-memory-catalog child whose only output is the `render` tool's own
/// bounded PNG answer. The bundle's tool, module, and schema identities
/// are pinned exactly like local development before the selected step's
/// stack is forwarded.
fn render_at(
    engine: &Path,
    metadata: &Path,
    work: &Path,
    input: &Path,
    geometry: RenderGeometry,
    stack: &[Value],
    guard: Guard,
) -> io::Result<Vec<u8>> {
    let approved = native_development::approved_metadata(metadata)?;
    let config = work.join("config");
    let cache = work.join("cache");
    let tmp = work.join("tmp");
    let xdg = work.join("xdg");
    // The render child keeps no catalog, writes no sidecar, and crawls no
    // directory: an explicit in-memory library and an explicit sidecar
    // policy, because the fresh private configdir carries no defaults to
    // inherit. The workflow matches the development baseline the step's
    // Export handoff pins, so exactly the selected stack differs.
    let args: Vec<String> = [
        "--core",
        "--disable-opencl",
        "--configdir",
        native_development::strict(&config)?,
        "--cachedir",
        native_development::strict(&cache)?,
        "--tmpdir",
        native_development::strict(&tmp)?,
        "--library",
        ":memory:",
        "--conf",
        "plugins/darkroom/workflow=none",
        "--conf",
        "write_sidecar_files=never",
        "--conf",
        "run_crawler_on_start=FALSE",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    let env = [
        (
            "PATH".to_string(),
            "/opt/darktable/bin:/usr/local/bin:/usr/bin:/bin".to_string(),
        ),
        (
            "HOME".to_string(),
            native_development::strict(&xdg)?.to_string(),
        ),
        (
            "XDG_CONFIG_HOME".to_string(),
            native_development::strict(&xdg)?.to_string(),
        ),
        (
            "XDG_CACHE_HOME".to_string(),
            native_development::strict(&cache)?.to_string(),
        ),
        (
            "TMPDIR".to_string(),
            native_development::strict(&tmp)?.to_string(),
        ),
        ("OMP_NUM_THREADS".to_string(), "4".to_string()),
    ];
    let mut engine = McpClient::spawn_guarded(
        native_development::strict(engine)?,
        &args,
        &env,
        guard.cancellation,
        guard.deadline,
    )?;
    engine.initialize(CLIENT)?;
    native_development::verify_engine_contract(&mut engine, &approved)?;
    let png = engine.call_image_png(
        "render",
        render_arguments(native_development::strict(input)?, geometry, stack),
    )?;
    engine.shutdown()?;
    Ok(png)
}

/// Execute one bounded selected-step Preview over the caller-owned paths.
///
/// `engine` is the pinned `darktable-mcp` binary, `metadata` its bundle
/// engine-metadata contract, `work` a clean private directory this call
/// may own, `input` the staged Original, `output` the path the validated
/// rendition is written to, and `parameters` the selected step's complete
/// module-owned parameter snapshot. The run is bounded by `cancellation`
/// and `timeout`: either kills the whole engine process group and returns
/// an error naming the cause. The returned identity describes the bounded
/// rendition actually rendered, never a larger intermediate.
#[allow(clippy::too_many_arguments)]
pub fn render_selected_step(
    engine: &Path,
    metadata: &Path,
    work: &Path,
    input: &Path,
    output: &Path,
    parameters: &Parameters,
    cancellation: Arc<AtomicBool>,
    timeout: Duration,
) -> io::Result<PreviewIdentity> {
    if timeout.is_zero() {
        return Err(io::Error::other("bounded preview requires a timeout"));
    }
    if cancellation.load(Ordering::Relaxed) {
        return Err(cancelled());
    }
    let stack = selected_step_stack(parameters)?;
    let geometry = RenderGeometry::preview();
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| io::Error::other("bounded preview deadline overflow"))?;
    prepare(work)?;
    if cancellation.load(Ordering::Relaxed) {
        return Err(cancelled());
    }
    let png = render_at(
        engine,
        metadata,
        work,
        input,
        geometry,
        &stack,
        Guard {
            cancellation: cancellation.clone(),
            deadline,
        },
    )
    .map_err(|error| terminated(error, &cancellation, deadline))?;
    let identity = rendition_identity(&png, geometry)?;
    write_rendition(output, &png)?;
    Ok(identity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::{DARKTABLE_MODULE, DARKTABLE_PARAMETER_VERSION};

    /// One complete admitted darktree envelope of the pinned schema.
    fn parameters(tree: Value) -> Parameters {
        Parameters {
            module: DARKTABLE_MODULE.to_owned(),
            version: DARKTABLE_PARAMETER_VERSION.to_owned(),
            tree,
        }
    }

    fn pinned_tree() -> Value {
        json!({
            "stack": [{
                "operation": "exposure",
                "multiPriority": 0,
                "enabled": true,
                "params": {"exposure": 0.25},
                "before": "colorin",
                "after": "colorout"
            }],
            "output": {
                "format": "tiff",
                "precisionBits": 32,
                "colorSpace": "prophoto-rgb",
                "transferFunction": "linear"
            }
        })
    }

    /// One minimal PNG byte stream whose mandatory leading IHDR chunk
    /// declares the given geometry.
    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = PNG_SIGNATURE.to_vec();
        bytes.extend(13u32.to_be_bytes());
        bytes.extend(b"IHDR");
        bytes.extend(width.to_be_bytes());
        bytes.extend(height.to_be_bytes());
        bytes.extend([8, 6, 0, 0, 0]);
        bytes.extend([0, 0, 0, 0]);
        bytes
    }

    /// The admitted geometry is finite, disclosed, and never above the
    /// qualified Preview long edge; renditions outside it are refused.
    #[test]
    fn preview_geometry_is_finite_and_within_the_disclosed_bound() {
        assert_eq!(PREVIEW_LONG_EDGE, 1224);
        let geometry = RenderGeometry::preview();
        assert_eq!(geometry.width, PREVIEW_LONG_EDGE);
        assert_eq!(geometry.height, PREVIEW_LONG_EDGE);
        assert!(geometry.contains(1224, 1));
        assert!(geometry.contains(1, 1224));
        assert!(geometry.contains(640, 480));
        assert!(!geometry.contains(1225, 1));
        assert!(!geometry.contains(1, 1225));
        assert!(!geometry.contains(0, 480));
        assert!(!geometry.contains(640, 0));
    }

    /// The render call carries the finite bounding box and the selected
    /// step's exact stack, byte for byte, over the staged Original path.
    #[test]
    fn render_arguments_carry_the_bounded_box_and_the_exact_stack() {
        let stack = pinned_tree()["stack"].as_array().unwrap().clone();
        let arguments = render_arguments("/input/original.ARW", RenderGeometry::preview(), &stack);
        assert_eq!(
            arguments,
            json!({
                "input": {"path": "/input/original.ARW"},
                "width": 1224,
                "height": 1224,
                "stack": [{
                    "operation": "exposure",
                    "multi_priority": 0,
                    "enabled": true,
                    "params": {"exposure": 0.25},
                    "before": "colorin",
                    "after": "colorout"
                }]
            })
        );
        // The adapter changes only the pinned engine spelling; the captured
        // module tree remains in its caller-owned camelCase form.
        assert_eq!(arguments["stack"][0]["multi_priority"], json!(0));
        assert!(arguments["stack"][0].get("multiPriority").is_none());
        assert!(arguments.get("export").is_none());
        assert!(arguments.get("history_end").is_none());
    }

    /// The pinned tree validates and its stack is extracted verbatim,
    /// including every optional placement field.
    #[test]
    fn selected_step_stack_extracts_the_pinned_tree_verbatim() {
        let tree = pinned_tree();
        let extracted = selected_step_stack(&parameters(tree.clone())).unwrap();
        assert_eq!(extracted, tree["stack"].as_array().unwrap().clone());
        // A tree without a stack admits the empty exact stack.
        assert!(
            selected_step_stack(&parameters(json!({
                "output": pinned_tree()["output"]
            })))
            .unwrap()
            .is_empty()
        );
    }

    /// Every foreign module, unknown field, unpinned output, or malformed
    /// stack entry is refused before any engine work.
    #[test]
    fn selected_step_stack_refuses_every_unadmitted_tree() {
        let refused = |tree: Value| selected_step_stack(&parameters(tree)).is_err();
        let mut foreign = parameters(pinned_tree());
        foreign.module = "spektrafilm".to_owned();
        assert!(selected_step_stack(&foreign).is_err());
        let mut future = parameters(pinned_tree());
        future.version = "darktable-params-2".to_owned();
        assert!(selected_step_stack(&future).is_err());
        assert!(refused(json!("not an object")));
        assert!(refused(json!({"stack": [], "surprise": 1})));
        // The output block, when present, is exactly the pinned handoff.
        assert!(refused(json!({"stack": [], "output": {"format": "jpeg"}})));
        assert!(refused(json!({"stack": [], "output": {
            "format": "tiff", "precisionBits": 16,
            "colorSpace": "prophoto-rgb", "transferFunction": "linear"
        }})));
        assert!(refused(json!({"stack": "not an array"})));
        assert!(refused(json!({"stack": ["not an object"]})));
        assert!(refused(json!({"stack": [{
            "operation": "exposure", "enabled": true, "params": {}
        }]})));
        assert!(refused(json!({"stack": [{
            "operation": "exposure", "multiPriority": 0, "enabled": true
        }]})));
        assert!(refused(json!({"stack": [{
            "operation": "exposure", "multiPriority": 0, "enabled": true,
            "params": [], "unknown": 1
        }]})));
        assert!(refused(json!({"stack": [{
            "operation": "", "multiPriority": 0, "enabled": true, "params": {}
        }]})));
        // The stack length stays finite.
        let oversized = json!({"stack": (0..129).map(|_| {
            json!({"operation": "exposure", "multiPriority": 0, "enabled": true, "params": {}})
        }).collect::<Vec<_>>()});
        assert!(refused(oversized));
        // A full-length stack stays admitted when the envelope itself
        // fits the bounded frame the admission predicate pins.
        let bounded = json!({"stack": (0..128).map(|_| {
            json!({"operation": "e", "multiPriority": 0, "enabled": true, "params": {}})
        }).collect::<Vec<_>>()});
        assert_eq!(
            selected_step_stack(&parameters(bounded)).unwrap().len(),
            128
        );
    }

    /// The leading IHDR chunk is the only header read, and renditions that
    /// are not bounded PNGs are refused whole.
    #[test]
    fn rendition_identity_accepts_only_bounded_pngs() {
        let admitted = rendition_identity(&png(1224, 801), RenderGeometry::preview()).unwrap();
        assert_eq!(admitted.width, 1224);
        assert_eq!(admitted.height, 801);
        assert_eq!(
            admitted.sha256,
            format!("{:x}", Sha256::digest(png(1224, 801)))
        );
        assert_eq!(admitted.size, png(1224, 801).len() as u64);

        // Geometry above the disclosed bound, empty bytes, and non-PNG or
        // truncated payloads are refused.
        assert!(rendition_identity(&png(1225, 1), RenderGeometry::preview()).is_err());
        assert!(rendition_identity(&png(640, 2000), RenderGeometry::preview()).is_err());
        assert!(rendition_identity(&[], RenderGeometry::preview()).is_err());
        assert!(rendition_identity(b"not a png at all", RenderGeometry::preview()).is_err());
        assert!(rendition_identity(PNG_SIGNATURE.as_ref(), RenderGeometry::preview()).is_err());
        let mut zeroed = png(640, 480);
        zeroed.splice(16..24, [0u8; 8]);
        assert!(rendition_identity(&zeroed, RenderGeometry::preview()).is_err());
        // A payload above the size bound is refused without being parsed.
        let oversized = {
            let mut bytes = png(640, 480);
            bytes.resize(PREVIEW_BYTES_MAX + 1, 0);
            bytes
        };
        assert!(rendition_identity(&oversized, RenderGeometry::preview()).is_err());
    }

    /// A cancelled or unbounded call refuses before the engine is named or
    /// spawned: the authority checks answer first.
    #[test]
    fn a_cancelled_or_unbounded_call_refuses_before_the_engine() {
        let nowhere = Path::new("/nowhere");
        assert!(
            render_selected_step(
                nowhere,
                nowhere,
                nowhere,
                nowhere,
                nowhere,
                &parameters(pinned_tree()),
                Arc::new(AtomicBool::new(true)),
                Duration::from_secs(1),
            )
            .is_err()
        );
        assert!(
            render_selected_step(
                nowhere,
                nowhere,
                nowhere,
                nowhere,
                nowhere,
                &parameters(pinned_tree()),
                Arc::new(AtomicBool::new(false)),
                Duration::ZERO,
            )
            .is_err()
        );
        // A malformed envelope refuses without any engine contact too.
        assert!(
            render_selected_step(
                nowhere,
                nowhere,
                nowhere,
                nowhere,
                nowhere,
                &parameters(json!({"stack": [{"surprise": true}]})),
                Arc::new(AtomicBool::new(false)),
                Duration::from_secs(1),
            )
            .is_err()
        );
    }

    /// The bounded write publishes exactly the validated bytes under the
    /// caller's private path.
    #[test]
    fn write_rendition_publishes_the_validated_bytes() {
        let base =
            std::env::temp_dir().join(format!("slipstream-bounded-preview-{}", std::process::id()));
        fs::create_dir_all(&base).unwrap();
        let output = base.join("preview.png");
        let bytes = png(640, 480);
        write_rendition(&output, &bytes).unwrap();
        assert_eq!(fs::read(&output).unwrap(), bytes);
        let _ = fs::remove_dir_all(&base);
    }
}
