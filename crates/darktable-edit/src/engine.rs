mod bundle;
mod transport;
use crate::protocol::ToolError;
use bundle::Bundle;
use photo_files::{Directory, EntryKind, FileLease, LeaseMode};
use serde_json::{Value, json};
use std::{
    io,
    os::{fd::AsRawFd, unix::fs::symlink},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
pub use transport::run_supervisor;
use transport::{Client, OUTPUT_MAX};

/// The standalone native bundle and its authoritative semantic contract.
pub struct Engine {
    bundle: Arc<Bundle>,
    supervisor: PathBuf,
    resources: Value,
}
fn error(code: &str, error: impl std::fmt::Display) -> ToolError {
    ToolError::new(code, error.to_string(), 6)
}
fn strict(path: &Path) -> io::Result<&str> {
    path.to_str()
        .ok_or_else(|| io::Error::other("native path is not UTF-8"))
}
struct Scratch {
    parent: Directory,
    directory: Directory,
    name: String,
}
fn remove_contents(directory: &Directory) -> io::Result<()> {
    for entry in directory.entries(20_000)? {
        match entry.kind {
            EntryKind::Directory => {
                remove_contents(&directory.child(&entry.name)?)?;
                directory.remove_directory(&entry.name)?;
            }
            _ => directory.remove_file(&entry.name)?,
        }
    }
    Ok(())
}
impl Scratch {
    fn create() -> io::Result<Self> {
        let parent = Directory::open("/tmp")?;
        let name = photo_files::identity("darktable-edit")?;
        let directory = parent.create_directory(&name)?;
        for name in ["config", "cache", "tmp", "home"] {
            directory.create_directory(name)?;
        }
        Ok(Self {
            parent,
            directory,
            name,
        })
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = remove_contents(&self.directory);
        let _ = self.parent.remove_directory(&self.name);
    }
}
impl Engine {
    pub fn open() -> Result<Self, ToolError> {
        let path = std::env::var_os("DARKTABLE_EDIT_BUNDLE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/opt/darktable-edit"));
        let bundle = Bundle::open(&path).map_err(|e| error("engine_unavailable", e))?;
        let executable = std::env::current_exe().map_err(|e| error("engine_unavailable", e))?;
        let directory = executable
            .parent()
            .ok_or_else(|| error("engine_unavailable", "executable has no parent"))?;
        let directory = if directory.file_name().and_then(|n| n.to_str()) == Some("deps") {
            directory.parent().unwrap_or(directory)
        } else {
            directory
        };
        let supervisor = directory.join("darktable-edit-supervisor");
        if !supervisor.is_file() {
            return Err(error(
                "engine_unavailable",
                "darktable-edit-supervisor must be installed beside darktable-edit",
            ));
        }
        Ok(Self {
            bundle: Arc::new(bundle),
            supervisor,
            resources: json!([]),
        })
    }
    pub fn bundle_id(&self) -> &str {
        &self.bundle.id
    }
    pub fn schema_version(&self) -> &str {
        &self.bundle.manifest.schema_version
    }
    pub fn discover(&self) -> Result<Value, ToolError> {
        Ok(self.bundle.manifest.discovery.clone())
    }
    pub fn with_resources(&self, resources: &Value) -> Self {
        Self {
            bundle: self.bundle.clone(),
            supervisor: self.supervisor.clone(),
            resources: resources.clone(),
        }
    }
    fn invoke(
        &self,
        input: Option<&Path>,
        name: &str,
        mut arguments: Value,
        output: bool,
    ) -> Result<(Value, Scratch), ToolError> {
        let work = Scratch::create().map_err(|e| error("engine_failed", e))?;
        if !self.resources.is_array() {
            return Err(error(
                "resource_unavailable",
                "resource evidence must be an array",
            ));
        }
        if !self.resources.as_array().is_some_and(Vec::is_empty) {
            if !arguments["options"].is_object() {
                arguments["options"] = json!({});
            }
            arguments["options"]["resources"] = self.resources.clone();
        }
        let mut resource_leases = Vec::new();
        let mut input_ancestors = Vec::new();
        if let Some(resources) = arguments
            .pointer_mut("/options/resources")
            .and_then(Value::as_array_mut)
        {
            if resources.len() > 128 {
                return Err(error(
                    "resource_unavailable",
                    "resource count exceeds bound",
                ));
            }
            for (index, resource) in resources.iter_mut().enumerate() {
                let path = PathBuf::from(
                    resource["path"]
                        .as_str()
                        .ok_or_else(|| error("resource_unavailable", "resource path missing"))?,
                );
                input_ancestors.extend(
                    photo_files::ancestor_leases(&path, Duration::from_secs(5))
                        .map_err(|e| error("resource_unavailable", e))?,
                );
                let parent = Directory::open(
                    path.parent()
                        .ok_or_else(|| error("resource_unavailable", "resource parent missing"))?,
                )
                .map_err(|e| error("resource_unavailable", e))?;
                let file = parent
                    .open_file(Path::new(path.file_name().ok_or_else(|| {
                        error("resource_unavailable", "resource filename missing")
                    })?))
                    .map_err(|e| error("resource_unavailable", e))?;
                let lease = FileLease::acquire(file, LeaseMode::Shared, Duration::from_secs(5))
                    .map_err(|e| error("resource_unavailable", e))?;
                let evidence = photo_files::fingerprint(
                    &mut lease
                        .file()
                        .try_clone()
                        .map_err(|e| error("resource_unavailable", e))?,
                    64 * 1024 * 1024,
                )
                .map_err(|e| error("resource_unavailable", e))?;
                if resource["sha256"] != evidence.sha256
                    || resource["byteLength"].as_u64() != Some(evidence.byte_length)
                {
                    return Err(error(
                        "resource_unavailable",
                        "resource bytes differ from admitted identity",
                    ));
                }
                let kind = resource["kind"]
                    .as_str()
                    .ok_or_else(|| error("resource_unavailable", "resource kind missing"))?;
                let extension = match kind {
                    "icc-profile" => "icc",
                    "lut" => "cube",
                    "raster-mask" => match resource["mediaType"].as_str() {
                        Some("image/png") => "png",
                        Some("image/tiff") => "tiff",
                        _ => {
                            return Err(error(
                                "resource_unavailable",
                                "raster mask media type missing",
                            ));
                        }
                    },
                    _ => {
                        return Err(error(
                            "resource_unavailable",
                            "resource kind is not admitted",
                        ));
                    }
                };
                let private = work
                    .directory
                    .path()
                    .join(format!("resource-{index}.{extension}"));
                symlink(
                    format!("/proc/self/fd/{}", lease.file().as_raw_fd()),
                    &private,
                )
                .map_err(|e| error("resource_unavailable", e))?;
                resource["path"] = json!(private);
                resource_leases.push((lease, evidence.observation));
            }
        }
        let lease = if let Some(input) = input {
            input_ancestors.extend(
                photo_files::ancestor_leases(input, Duration::from_secs(5))
                    .map_err(|e| error("input_unavailable", e))?,
            );
            let parent = Directory::open(
                input
                    .parent()
                    .ok_or_else(|| error("input_unavailable", "input parent missing"))?,
            )
            .map_err(|e| error("input_unavailable", e))?;
            let filename = input
                .file_name()
                .ok_or_else(|| error("input_unavailable", "input filename missing"))?;
            let file = parent
                .open_file(Path::new(filename))
                .map_err(|e| error("input_unavailable", e))?;
            if file
                .metadata()
                .map_err(|e| error("input_unavailable", e))?
                .len()
                > OUTPUT_MAX
            {
                return Err(error(
                    "input_unavailable",
                    "input exceeds native byte bound",
                ));
            }
            let lease = FileLease::acquire(file, LeaseMode::Shared, Duration::from_secs(5))
                .map_err(|e| error("input_unavailable", e))?;
            let extension = input
                .extension()
                .and_then(|v| v.to_str())
                .filter(|v| v.len() <= 16 && v.bytes().all(|c| c.is_ascii_alphanumeric()))
                .ok_or_else(|| {
                    error("input_unavailable", "input extension is missing or invalid")
                })?;
            let path = work.directory.path().join(format!("input.{extension}"));
            symlink(format!("/proc/self/fd/{}", lease.file().as_raw_fd()), &path)
                .map_err(|e| error("input_unavailable", e))?;
            arguments["input"] = json!({"path":path});
            Some(lease)
        } else {
            None
        };
        if output {
            arguments["destination"] = json!(work.directory.path().join("rendered"));
        }
        self.bundle
            .directory
            .verify_path()
            .map_err(|e| error("engine_incompatible", e))?;
        let root = work.directory.path();
        let data = self
            .bundle
            .directory
            .path()
            .join(&self.bundle.manifest.data_directory);
        let arguments_native = [
            "--read-only".to_owned(),
            "--core".to_owned(),
            "--disable-opencl".to_owned(),
            "--library".to_owned(),
            ":memory:".to_owned(),
            "--configdir".to_owned(),
            strict(&root.join("config"))
                .map_err(|e| error("engine_failed", e))?
                .to_owned(),
            "--cachedir".to_owned(),
            strict(&root.join("cache"))
                .map_err(|e| error("engine_failed", e))?
                .to_owned(),
            "--tmpdir".to_owned(),
            strict(&root.join("tmp"))
                .map_err(|e| error("engine_failed", e))?
                .to_owned(),
            "--datadir".to_owned(),
            strict(&data)
                .map_err(|e| error("engine_failed", e))?
                .to_owned(),
            "--conf".to_owned(),
            "write_sidecar_files=never".to_owned(),
            "--conf".to_owned(),
            "run_crawler_on_start=FALSE".to_owned(),
            "--conf".to_owned(),
            "plugins/darkroom/workflow=none".to_owned(),
        ];
        let environment = vec![
            (
                "HOME".into(),
                strict(&root.join("home"))
                    .map_err(|e| error("engine_failed", e))?
                    .into(),
            ),
            (
                "XDG_CONFIG_HOME".into(),
                strict(&root.join("config"))
                    .map_err(|e| error("engine_failed", e))?
                    .into(),
            ),
            (
                "XDG_CACHE_HOME".into(),
                strict(&root.join("cache"))
                    .map_err(|e| error("engine_failed", e))?
                    .into(),
            ),
            (
                "TMPDIR".into(),
                strict(&root.join("tmp"))
                    .map_err(|e| error("engine_failed", e))?
                    .into(),
            ),
            ("OMP_NUM_THREADS".into(), "4".into()),
            ("OMP_THREAD_LIMIT".into(), "4".into()),
            ("OPENBLAS_NUM_THREADS".into(), "1".into()),
            ("LANG".into(), "C.UTF-8".into()),
            ("LC_ALL".into(), "C.UTF-8".into()),
            (
                "LD_LIBRARY_PATH".into(),
                strict(&self.bundle.directory.path().join("lib"))
                    .map_err(|e| error("engine_failed", e))?
                    .into(),
            ),
        ];
        let observation = lease
            .as_ref()
            .map(|v| photo_files::Observation::of(v.file()))
            .transpose()
            .map_err(|e| error("input_unavailable", e))?;
        let fds = lease
            .iter()
            .map(|v| v.file().as_raw_fd())
            .chain(
                resource_leases
                    .iter()
                    .map(|(lease, _)| lease.file().as_raw_fd()),
            )
            .collect();
        let mut native = Client::spawn(
            &self.supervisor,
            &self
                .bundle
                .directory
                .path()
                .join(&self.bundle.manifest.binary),
            &arguments_native,
            &environment,
            root,
            fds,
        )
        .map_err(|e| error("engine_failed", e))?;
        native
            .initialize(&self.bundle.manifest.protocol_version)
            .map_err(|e| error("engine_incompatible", e))?;
        if native
            .tools()
            .map_err(|e| error("engine_incompatible", e))?
            != self.bundle.manifest.tools
            || native
                .call("edit_discover", json!({}))
                .map_err(|e| error("engine_incompatible", e))?
                != self.bundle.manifest.discovery
        {
            return Err(error(
                "engine_incompatible",
                "native metadata/schema differs from pinned bundle",
            ));
        }
        let result = native
            .call(name, arguments)
            .map_err(|e| error("engine_failed", e))?;
        drop(native);
        if let (Some(observation), Some(lease)) = (&observation, &lease) {
            observation
                .verify(lease.file())
                .map_err(|e| error("input_changed", e))?;
        }
        for (lease, observation) in &resource_leases {
            observation
                .verify(lease.file())
                .map_err(|e| error("resource_changed", e))?;
        }
        drop(lease);
        Ok((result, work))
    }
    pub fn initialize(&self, input: &Path) -> Result<Value, ToolError> {
        self.invoke(Some(input), "edit_initialize", json!({}), false)
            .map(|(value, _)| value)
    }
    pub fn validate(&self, input: &Path, snapshot: &Value) -> Result<Value, ToolError> {
        self.invoke(
            Some(input),
            "edit_validate",
            json!({"snapshot":snapshot}),
            false,
        )
        .map(|(value, _)| value)
    }
    /// Builds an intermediate atomic candidate; callers must validate before saving.
    pub(crate) fn construction_action(
        &self,
        input: &Path,
        snapshot: &Value,
        action: &Value,
    ) -> Result<Value, ToolError> {
        self.invoke(
            Some(input),
            "edit_action",
            json!({"snapshot":snapshot,"action":action,"construction":true}),
            false,
        )
        .map(|(value, _)| value)
    }
    pub fn import(&self, input: &Path, legacy: &Value) -> Result<Value, ToolError> {
        self.invoke(Some(input), "edit_import", json!({"legacy":legacy}), false)
            .map(|(value, _)| value)
    }
    pub fn state_file(&self, input: &Path, snapshot: &Value) -> Result<Vec<u8>, ToolError> {
        let (value, _) = self.invoke(
            Some(input),
            "edit_state_file",
            json!({"snapshot":snapshot}),
            false,
        )?;
        let xmp = value["xmp"]
            .as_str()
            .filter(|v| !v.is_empty() && v.len() <= 16 * 1024 * 1024)
            .ok_or_else(|| error("engine_failed", "native XMP missing or oversized"))?;
        Ok(xmp.as_bytes().to_vec())
    }
    pub fn render(
        &self,
        input: &Path,
        snapshot: &Value,
        destination: &Path,
        preview: bool,
        options: &Value,
    ) -> Result<Value, ToolError> {
        let (mut native, work) = self.invoke(
            Some(input),
            "edit_render",
            json!({"snapshot":snapshot,"preview":preview,"options":options}),
            true,
        )?;
        let format = native["format"]
            .as_str()
            .ok_or_else(|| error("engine_failed", "native output format missing"))?;
        let profile = native
            .pointer("/profile/id")
            .and_then(Value::as_str)
            .ok_or_else(|| error("engine_failed", "native output profile missing"))?;
        let icc = self
            .bundle
            .manifest
            .output_profiles
            .get(profile)
            .ok_or_else(|| error("engine_incompatible", "output ICC is not pinned"))?;
        let source = work.directory.path().join("rendered");
        let observed = photo_files::image::validate(&source, OUTPUT_MAX, icc)
            .map_err(|e| error("output_invalid", e))?;
        if observed.format != format
            || native["precision"] != observed.precision
            || native["width"].as_u64() != Some(u64::from(observed.width))
            || native["height"].as_u64() != Some(u64::from(observed.height))
            || native["bytes"].as_u64() != Some(observed.bytes)
        {
            return Err(error(
                "output_invalid",
                "native output claims differ from decoded file",
            ));
        }
        let parent = Directory::open(
            destination
                .parent()
                .ok_or_else(|| error("output_invalid", "output parent missing"))?,
        )
        .map_err(|e| error("output_invalid", e))?;
        let name = destination
            .file_name()
            .ok_or_else(|| error("output_invalid", "output filename missing"))?;
        let mut pending =
            photo_files::PendingFile::create(&parent).map_err(|e| error("output_invalid", e))?;
        let mut source = work
            .directory
            .open_file("rendered")
            .map_err(|e| error("output_invalid", e))?;
        let copied =
            io::copy(&mut source, pending.file_mut()).map_err(|e| error("output_invalid", e))?;
        if copied != observed.bytes {
            return Err(error("output_invalid", "output changed during copy"));
        }
        let evidence = photo_files::fingerprint(pending.file_mut(), OUTPUT_MAX)
            .map_err(|e| error("output_invalid", e))?;
        if evidence.sha256 != observed.sha256 || evidence.byte_length != observed.bytes {
            return Err(error(
                "output_invalid",
                "output byte identity changed during copy",
            ));
        }
        pending
            .publish(Path::new(name), OUTPUT_MAX)
            .map_err(|e| error("output_invalid", e))?;
        native["path"] = json!(destination);
        native["sha256"] = json!(observed.sha256);
        native["iccSha256"] = json!(observed.icc_sha256);
        native["length"] = json!(observed.bytes);
        native["dimensions"] = json!({"width":observed.width,"height":observed.height});
        Ok(native)
    }
}
