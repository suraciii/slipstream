use super::*;
use sha2::{Digest, Sha256};

pub const HEALTH_PATH: &str = "/healthz";

pub(crate) const MAXIMUM_HEADER_BYTES: usize = 16 * 1024;
pub(crate) const MAXIMUM_MUTATION_BODY_BYTES: usize = 64 * 1024;
const DEFAULT_DATABASE_BASENAME: &str = "library.sqlite";
const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 3000;
const DEFAULT_RETAINED_OUTPUT_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// Typed values accepted by the existing `SLIPSTREAM_*` startup contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub library_root: PathBuf,
    pub state_directory: PathBuf,
    pub cache_directory: PathBuf,
    pub database_basename: String,
    pub host: String,
    pub public_origin: String,
    pub port: u16,
    /// Tests and packaged deployments may provide a built Web directory. When
    /// absent, the binary uses the repository's conventional `apps/web/dist`.
    pub web_root: Option<PathBuf>,
    /// Optional local Photo Development capability. The absence of a valid
    /// bundle does not prevent the Library from opening.
    pub processing: Option<ProcessingConfig>,
    /// Finite retained-output allowance for Development TIFF artifacts. The
    /// service refuses a new Export before acceptance when the complete
    /// artifact cannot be reserved inside it.
    pub export_retained_output_bytes: Option<u64>,
    /// Unix socket of the exclusive metadata save supervisor. When absent,
    /// the deployment supports Read Metadata and refuses Save as unavailable.
    pub metadata_supervisor: Option<PathBuf>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessingConfig {
    pub(crate) policy_sha256: String,
    pub(crate) bundle_sha256: String,
    pub(crate) bundle_root: PathBuf,
    pub(crate) failure: Option<&'static str>,
}

pub type StartupConfig = Config;
pub type ServerConfig = Config;

impl Config {
    pub fn from_env(
        environment: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, ConfigError> {
        let values = environment
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>();
        Self::from_lookup(|name| values.get(name).cloned())
    }

    pub fn from_process_environment() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| env::var(name).ok())
    }

    fn from_lookup(mut get: impl FnMut(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let library_root =
            absolute_path(get("SLIPSTREAM_LIBRARY_ROOT"), "SLIPSTREAM_LIBRARY_ROOT")?;
        let state_directory = absolute_path(
            get("SLIPSTREAM_STATE_DIRECTORY"),
            "SLIPSTREAM_STATE_DIRECTORY",
        )?;
        let cache_directory = absolute_path(
            get("SLIPSTREAM_CACHE_DIRECTORY"),
            "SLIPSTREAM_CACHE_DIRECTORY",
        )?;
        let database_basename = get("SLIPSTREAM_DATABASE_BASENAME")
            .unwrap_or_else(|| DEFAULT_DATABASE_BASENAME.to_owned());
        if !is_valid_database_basename(&database_basename) {
            return Err(ConfigError::Invalid("SLIPSTREAM_DATABASE_BASENAME"));
        }
        let host = get("SLIPSTREAM_HOST").unwrap_or_else(|| DEFAULT_HOST.to_owned());
        if host.is_empty()
            || host.len() > 255
            || host.chars().any(char::is_whitespace)
            || host.contains('/')
        {
            return Err(ConfigError::Invalid("SLIPSTREAM_HOST"));
        }
        let port = match get("SLIPSTREAM_PORT") {
            None => DEFAULT_PORT,
            Some(value) => value
                .parse::<u16>()
                .ok()
                .filter(|port| *port != 0)
                .ok_or(ConfigError::Invalid("SLIPSTREAM_PORT"))?,
        };
        let web_root = get("SLIPSTREAM_WEB_ROOT").map(PathBuf::from);
        if let Some(path) = &web_root
            && !path.is_absolute()
        {
            return Err(ConfigError::Invalid("SLIPSTREAM_WEB_ROOT"));
        }
        let public_origin = crate::access::canonical_origin(
            &get("SLIPSTREAM_PUBLIC_ORIGIN").unwrap_or_else(|| format!("http://localhost:{port}")),
        )
        .ok_or(ConfigError::Invalid("SLIPSTREAM_PUBLIC_ORIGIN"))?;
        let photo_development =
            get("SLIPSTREAM_PHOTO_DEVELOPMENT").unwrap_or_else(|| "auto".to_owned());
        let processing = match photo_development.as_str() {
            "disabled" => None,
            "auto" | "enabled" => {
                let bundle_root = get("SLIPSTREAM_PHOTO_BUNDLE_DIRECTORY")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("/opt/slipstream-photo"));
                if !bundle_root.is_absolute() {
                    return Err(ConfigError::Invalid("SLIPSTREAM_PHOTO_BUNDLE_DIRECTORY"));
                }
                let (bundle_sha256, failure) = local_bundle_identity(&bundle_root);
                Some(ProcessingConfig {
                    policy_sha256: LOCAL_PHOTO_POLICY_SHA256.to_owned(),
                    bundle_sha256,
                    bundle_root,
                    failure,
                })
            }
            _ => return Err(ConfigError::Invalid("SLIPSTREAM_PHOTO_DEVELOPMENT")),
        };
        let export_retained_output_bytes = match get("SLIPSTREAM_EXPORT_RETAINED_OUTPUT_BYTES") {
            None => Some(DEFAULT_RETAINED_OUTPUT_BYTES),
            Some(value) => {
                let Ok(bytes) = value.parse::<u64>() else {
                    return Err(ConfigError::Invalid(
                        "SLIPSTREAM_EXPORT_RETAINED_OUTPUT_BYTES",
                    ));
                };
                if bytes == 0 {
                    return Err(ConfigError::Invalid(
                        "SLIPSTREAM_EXPORT_RETAINED_OUTPUT_BYTES",
                    ));
                }
                Some(bytes)
            }
        };
        let metadata_supervisor = match get("SLIPSTREAM_METADATA_SUPERVISOR") {
            None => None,
            Some(value) => {
                let path = PathBuf::from(&value);
                if !path.is_absolute() {
                    return Err(ConfigError::Invalid("SLIPSTREAM_METADATA_SUPERVISOR"));
                }
                Some(path)
            }
        };
        Ok(Self {
            public_origin,
            library_root,
            state_directory,
            cache_directory,
            database_basename,
            host,
            port,
            web_root,
            processing,
            export_retained_output_bytes,
            metadata_supervisor,
        })
    }

    pub fn web_root(&self) -> PathBuf {
        self.web_root
            .clone()
            .unwrap_or_else(|| PathBuf::from("apps/web/dist"))
    }
}

const LOCAL_PHOTO_POLICY_SHA256: &str =
    "f349c72c07b6ff4a77563a9170892639e75a46578ea3d947d322ca3b9ba66ad2";
const ENGINE_METADATA_BYTES_MAX: u64 = 16 * 1024 * 1024;

fn local_bundle_identity(root: &Path) -> (String, Option<&'static str>) {
    let bundle = read_bounded_text(&root.join("bundle"), 128)
        .map(|value| value.trim().to_owned())
        .filter(|value| is_lower_hex(value, 64));
    let valid = bundle
        .as_ref()
        .is_some_and(|bundle| verify_local_bundle(root, bundle).is_some());
    (
        bundle.unwrap_or_default(),
        (!valid).then_some("bundle-unavailable"),
    )
}

fn read_bounded_text(path: &Path, maximum: u64) -> Option<String> {
    use std::io::Read;

    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(maximum + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= maximum)
        .then(|| String::from_utf8(bytes).ok())
        .flatten()
}

fn verify_local_bundle(root: &Path, bundle: &str) -> Option<()> {
    use std::io::Read;
    use std::os::unix::fs::PermissionsExt;
    let mut bytes = Vec::new();
    fs::File::open(root.join("bundle-manifest.json"))
        .ok()?
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 4 * 1024 * 1024 || format!("{:x}", Sha256::digest(&bytes)) != bundle {
        return None;
    }
    let manifest: Value = serde_json::from_slice(&bytes).ok()?;
    if manifest["format"].as_u64()? != 1
        || manifest["engine"].as_str()? != "/opt/darktable/bin/darktable-mcp"
        || !is_lower_hex(manifest["darktable_commit"].as_str()?, 40)
    {
        return None;
    }
    let native = manifest["native"].as_object()?;
    native.get("bin/darktable-mcp")?;
    for (name, digest) in native {
        let path = Path::new(name);
        if path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
            || path.as_os_str().is_empty()
        {
            return None;
        }
        verify_bundle_file(&root.join("darktable").join(path), digest.as_str()?)?;
    }
    let files = manifest["files"].as_object()?;
    let required = [
        (
            "/opt/slipstream-photo/engine-metadata.json",
            "engine-metadata.json",
        ),
        (
            "/opt/slipstream-photo/icc/LargeRGB-elle-V2-g10.icc",
            "icc/LargeRGB-elle-V2-g10.icc",
        ),
        ("/opt/slipstream-photo/darktable-commit", "darktable-commit"),
        ("/opt/os-packages.txt", "os-packages.txt"),
    ];
    if files.len() != required.len() {
        return None;
    }
    for (name, relative) in required {
        verify_bundle_file(&root.join(relative), files.get(name)?.as_str()?)?;
    }
    let icc = files
        .get("/opt/slipstream-photo/icc/LargeRGB-elle-V2-g10.icc")?
        .as_str()?;
    if manifest["icc"].as_str()? != icc
        || icc != slipstream_processing::local_photo::ICC_ASSET_SHA256
        || manifest["metadata"].as_str()?
            != files
                .get("/opt/slipstream-photo/engine-metadata.json")?
                .as_str()?
        || read_bounded_text(&root.join("darktable-commit"), 41)?.trim()
            != manifest["darktable_commit"].as_str()?
    {
        return None;
    }
    let engine = fs::metadata(root.join("darktable/bin/darktable-mcp")).ok()?;
    if engine.permissions().mode() & 0o111 == 0 {
        return None;
    }
    let metadata_path = root.join("engine-metadata.json");
    let metadata_file = fs::metadata(&metadata_path).ok()?;
    if !metadata_file.is_file() || metadata_file.len() > ENGINE_METADATA_BYTES_MAX {
        return None;
    }
    let mut metadata_bytes = Vec::new();
    fs::File::open(&metadata_path)
        .ok()?
        .take(ENGINE_METADATA_BYTES_MAX + 1)
        .read_to_end(&mut metadata_bytes)
        .ok()?;
    if metadata_bytes.len() as u64 > ENGINE_METADATA_BYTES_MAX {
        return None;
    }
    let metadata: Value = serde_json::from_slice(&metadata_bytes).ok()?;
    if !metadata["tools"].is_array()
        || !(metadata["modules"].is_array() || metadata["modules"]["modules"].is_array())
        || !metadata["schemas"].is_object()
    {
        return None;
    }
    Some(())
}

fn verify_bundle_file(path: &Path, digest: &str) -> Option<()> {
    if !is_lower_hex(digest, 64) {
        return None;
    }
    let mut file = fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher).ok()?;
    (format!("{:x}", hasher.finalize()) == digest).then_some(())
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Configuration for the offline Library Expansion command. It deliberately
/// omits HTTP settings because the command never opens an HTTP listener.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpansionConfig {
    pub library_root: PathBuf,
    pub state_directory: PathBuf,
    pub cache_directory: PathBuf,
    pub database_basename: String,
}

impl ExpansionConfig {
    pub fn from_env(
        environment: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, ConfigError> {
        let values = environment
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>();
        Self::from_lookup(|name| values.get(name).cloned())
    }

    pub fn from_process_environment() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| env::var(name).ok())
    }

    fn from_lookup(mut get: impl FnMut(&str) -> Option<String>) -> Result<Self, ConfigError> {
        let library_root =
            absolute_path(get("SLIPSTREAM_LIBRARY_ROOT"), "SLIPSTREAM_LIBRARY_ROOT")?;
        let state_directory = absolute_path(
            get("SLIPSTREAM_STATE_DIRECTORY"),
            "SLIPSTREAM_STATE_DIRECTORY",
        )?;
        let cache_directory = absolute_path(
            get("SLIPSTREAM_CACHE_DIRECTORY"),
            "SLIPSTREAM_CACHE_DIRECTORY",
        )?;
        let database_basename = get("SLIPSTREAM_DATABASE_BASENAME")
            .unwrap_or_else(|| DEFAULT_DATABASE_BASENAME.to_owned());
        if !is_valid_database_basename(&database_basename) {
            return Err(ConfigError::Invalid("SLIPSTREAM_DATABASE_BASENAME"));
        }
        Ok(Self {
            library_root,
            state_directory,
            cache_directory,
            database_basename,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigError {
    Missing(&'static str),
    NotAbsolute(&'static str),
    Invalid(&'static str),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing(name) => write!(formatter, "{name} must be set"),
            Self::NotAbsolute(name) => write!(formatter, "{name} must be an absolute path"),
            Self::Invalid(name) => write!(formatter, "{name} is invalid"),
        }
    }
}

impl std::error::Error for ConfigError {}

fn absolute_path(value: Option<String>, name: &'static str) -> Result<PathBuf, ConfigError> {
    let value = value.ok_or(ConfigError::Missing(name))?;
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(ConfigError::NotAbsolute(name));
    }
    Ok(path)
}

fn is_valid_database_basename(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphanumeric()
                || byte == b'.'
                || byte == b'_'
                || byte == b'-' && index > 0
        })
        && value.as_bytes()[0].is_ascii_alphanumeric()
}

#[derive(Debug)]
pub enum ServerError {
    Config(ConfigError),
    Library(LibraryError),
    Preview(String),
    PreviewUnavailable,
    Export(String),
    WebUnavailable,
    StorageLayout,
    Io(io::Error),
    Cache(String),
    BrowseNotFound,
    PhotoNotFound,
    BrowseLimit,
    BrowseOrder,
    Join(String),
    NotPublished,
    FileLocationsExpired,
    FolderInvalid,
    FolderNotFound,
    FileLocationWindow,
    FolderAlbumLimit,
    QueryCapacity,
    RemovalFilter,
    RemovedWindow,
    RestorationInvalid,
    /// One Folder-prefix recovery review would evaluate more mappings than
    /// the advertised bound.
    RecoveryScope {
        evaluated: usize,
    },
}

impl fmt::Display for ServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => error.fmt(formatter),
            Self::Library(error) => error.fmt(formatter),
            Self::Preview(error) => formatter.write_str(error),
            Self::PreviewUnavailable => formatter.write_str("Preview service is unavailable"),
            Self::Export(error) => formatter.write_str(error),
            Self::WebUnavailable => formatter.write_str("Web application is not built"),
            Self::StorageLayout => {
                formatter.write_str("Photo Library, state, and cache directories must not overlap")
            }
            Self::Io(error) => error.fmt(formatter),
            Self::Cache(error) => formatter.write_str(error),
            Self::BrowseNotFound => formatter.write_str("Browse source is no longer available"),
            Self::PhotoNotFound => formatter.write_str("Photo is no longer available"),
            Self::BrowseLimit => formatter.write_str("Browse window is invalid"),
            Self::BrowseOrder => {
                formatter.write_str("Browse view order is invalid for this source")
            }
            Self::Join(error) => formatter.write_str(error),
            Self::NotPublished => formatter.write_str(
                "Library is initializing; the first completed scan has not published a Library yet",
            ),
            Self::FileLocationsExpired => {
                formatter.write_str("File Locations changed with a newer Library publication")
            }
            Self::FolderInvalid => formatter.write_str("Original Folder location is invalid"),
            Self::FolderNotFound => {
                formatter.write_str("Original Folder is not part of this publication")
            }
            Self::FileLocationWindow => formatter.write_str("File Location window is invalid"),
            Self::FolderAlbumLimit => formatter
                .write_str("This Original Folder contains too many Photos for one Album operation"),
            Self::QueryCapacity => formatter.write_str("Retained query capacity is unavailable"),
            Self::RemovalFilter => {
                formatter.write_str("Removal requires a Browse Snapshot filtered to Rejected")
            }
            Self::RemovedWindow => formatter.write_str("Removed Photos window is invalid"),
            Self::RestorationInvalid => {
                formatter.write_str("Restore names exactly one operation or a bounded Photo list")
            }
            Self::RecoveryScope { evaluated } => write!(
                formatter,
                "Recovery review scope of {evaluated} mappings exceeds the advertised bound"
            ),
        }
    }
}

impl std::error::Error for ServerError {}
impl From<ConfigError> for ServerError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}
impl From<LibraryError> for ServerError {
    fn from(error: LibraryError) -> Self {
        Self::Library(error)
    }
}
impl From<io::Error> for ServerError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<slipstream_core::CacheError> for ServerError {
    fn from(error: slipstream_core::CacheError) -> Self {
        Self::Cache(error.to_string())
    }
}

pub(crate) fn validate_storage_layout(config: &Config) -> Result<(), ServerError> {
    validate_storage_paths([
        config.library_root.as_path(),
        config.state_directory.as_path(),
        config.cache_directory.as_path(),
    ])
}

pub(crate) fn validate_expansion_storage_layout(
    config: &ExpansionConfig,
) -> Result<(), ServerError> {
    validate_storage_paths([
        config.library_root.as_path(),
        config.state_directory.as_path(),
        config.cache_directory.as_path(),
    ])
}

fn validate_storage_paths(paths: [&Path; 3]) -> Result<(), ServerError> {
    let canonical = paths
        .iter()
        .map(|path| canonicalize_layout_path(path).map_err(|_| ServerError::StorageLayout))
        .collect::<Result<Vec<_>, _>>()?;
    for (index, left) in canonical.iter().enumerate() {
        for right in canonical.iter().skip(index + 1) {
            if left == right || left.starts_with(right) || right.starts_with(left) {
                return Err(ServerError::StorageLayout);
            }
        }
    }
    Ok(())
}

pub(crate) fn canonicalize_layout_path(path: &Path) -> io::Result<PathBuf> {
    if !path.is_absolute() {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    let mut components = Vec::<OsString>::new();
    for component in path.components() {
        match component {
            Component::RootDir => {}
            Component::CurDir => {}
            Component::Normal(value) => components.push(value.to_owned()),
            Component::ParentDir => {
                components
                    .pop()
                    .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
            }
            Component::Prefix(_) => {
                return Err(io::Error::from(io::ErrorKind::InvalidInput));
            }
        }
    }

    let mut existing = PathBuf::from("/");
    let mut first_missing = components.len();
    for (index, component) in components.iter().enumerate() {
        if first_missing != components.len() {
            break;
        }
        let candidate = existing.join(component);
        match fs::canonicalize(&candidate) {
            Ok(canonical) => existing = canonical,
            Err(error) if error.kind() == io::ErrorKind::NotFound => first_missing = index,
            Err(error) => return Err(error),
        }
    }
    for component in components.iter().skip(first_missing) {
        existing.push(component);
    }
    Ok(existing)
}

pub(crate) const MAX_BROWSE_WINDOW: usize = 60;
/// Maximum removed Photos one bounded Removed Photos listing page returns.
pub(crate) const MAX_REMOVED_WINDOW: usize = 60;
/// Maximum Photos one explicit restore request may name. Undo restores one
/// operation instead, so this bound only limits machine clients.
pub(crate) const MAX_RESTORATION_PHOTOS: usize = 100;
pub(crate) const MAX_BROWSE_SNAPSHOTS: usize = 8;
pub(crate) const BROWSE_SNAPSHOT_IDLE: Duration = Duration::from_secs(30 * 60);

pub(crate) static NEXT_BROWSE_NAMESPACE: AtomicU64 = AtomicU64::new(0);
