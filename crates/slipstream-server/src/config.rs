use super::*;
use sha2::{Digest, Sha256};
use std::os::unix::fs::MetadataExt;
use std::path::Component;
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
    /// Canonical origins admitted by the HTTP access boundary. Deployment
    /// tooling may provide this internally; the public origin is always kept.
    pub access_origins: Vec<String>,
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
/// The optional standalone SpektraFilm peer runtime. The bundle is independent
/// of the darktable extension: it owns the pinned `spektrafilm-rs` binary,
/// profile-data tree, default parameter tree, and deterministic manifest.
/// `failure` is the truthful unavailable reason a deployment reports when
/// the configured runtime is missing or fails verification.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilmConfig {
    pub(crate) bundle_sha256: String,
    pub(crate) bundle_root: PathBuf,
    /// The pinned `spektrafilm` executable named by the fork manifest.
    pub(crate) binary: PathBuf,
    /// The fork data directory containing profiles, LUTs, and ICC assets.
    pub(crate) data_root: PathBuf,
    /// The runtime-generated complete default parameter tree, verified
    /// against the module boundary's own admission shape at startup.
    pub(crate) parameter_default: serde_json::Value,
    pub(crate) film_profile: String,
    pub(crate) print_profile: String,
    pub(crate) failure: Option<&'static str>,
}

impl FilmConfig {
    /// Whether this deployment's standalone SpektraFilm runtime is
    /// installed and verified. Availability belongs to this module alone
    /// and never derives from the darktable stage.
    pub(crate) fn ready(&self) -> bool {
        self.failure.is_none()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProcessingConfig {
    pub(crate) policy_sha256: String,
    pub(crate) bundle_sha256: String,
    pub(crate) bundle_root: PathBuf,
    /// The independently verified standalone SpektraFilm peer runtime.
    /// `None` means the deployment explicitly disabled the module; a
    /// `Some` value with `failure` set is a configured-but-unavailable
    /// runtime with its truthful reason.
    pub(crate) film: Option<FilmConfig>,
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
        let access_origins = crate::access::access_origins(
            get("SLIPSTREAM_ACCESS_ORIGINS").as_deref(),
            &host,
            port,
            &public_origin,
        )
        .map_err(|_| ConfigError::Invalid("SLIPSTREAM_ACCESS_ORIGINS"))?;
        let photo_development =
            get("SLIPSTREAM_PHOTO_DEVELOPMENT").unwrap_or_else(|| "auto".to_owned());
        let film = film_bundle_config(
            &mut get,
            "SLIPSTREAM_FILM_MODULE",
            "SLIPSTREAM_FILM_BUNDLE_DIRECTORY",
        )?;
        let processing = match photo_development.as_str() {
            // The darktable stage stays disabled while an independently
            // configured SpektraFilm runtime still opens the processing
            // extension: the config carries the truthful darktable
            // failure and empty darktable identity, and only the film
            // paths are admissible.
            "disabled" => film.map(|film| ProcessingConfig {
                policy_sha256: LOCAL_PHOTO_POLICY_SHA256.to_owned(),
                bundle_sha256: String::new(),
                bundle_root: PathBuf::from("/opt/slipstream-photo"),
                film: Some(film),
                failure: Some("darktable-disabled"),
            }),
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
                    film,
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
            access_origins,
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

/// The standalone SpektraFilm bundle directory's fixed manifest name.
const FILM_MANIFEST_BYTES_MAX: u64 = 4 * 1024 * 1024;

/// Resolve the optional standalone SpektraFilm peer from its typed
/// `SLIPSTREAM_FILM_*` startup contract. `None` (module disabled) never
/// fails the Library open; a configured-but-missing runtime reports the
/// truthful `film-runtime-missing` reason, and a present bundle that fails
/// verification reports `film-bundle-unavailable`.
fn film_bundle_config(
    get: &mut dyn FnMut(&str) -> Option<String>,
    module_key: &'static str,
    directory_key: &'static str,
) -> Result<Option<FilmConfig>, ConfigError> {
    let mode = get(module_key).unwrap_or_else(|| "auto".to_owned());
    if mode == "disabled" {
        return Ok(None);
    }
    if mode != "auto" && mode != "enabled" {
        return Err(ConfigError::Invalid(module_key));
    }
    let bundle_root = get(directory_key)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/opt/slipstream-film"));
    if !bundle_root.is_absolute() {
        return Err(ConfigError::Invalid(directory_key));
    }
    if !bundle_root.is_dir() {
        return Ok(Some(FilmConfig {
            bundle_sha256: String::new(),
            bundle_root,
            binary: PathBuf::new(),
            data_root: PathBuf::new(),
            parameter_default: serde_json::Value::Null,
            film_profile: String::new(),
            print_profile: String::new(),
            failure: Some("film-runtime-missing"),
        }));
    }
    let (config, failure) = verify_film_bundle(&bundle_root);
    Ok(Some(match config {
        Some(mut config) => {
            config.failure = failure;
            config
        }
        None => FilmConfig {
            bundle_sha256: String::new(),
            bundle_root,
            binary: PathBuf::new(),
            data_root: PathBuf::new(),
            parameter_default: serde_json::Value::Null,
            film_profile: String::new(),
            print_profile: String::new(),
            failure,
        },
    }))
}

/// Verify the standalone SpektraFilm bundle's deterministic manifest and
/// every asset it names. The pinned recipe, handoff, and finished-output
/// identities must equal the module boundary's own constants, so a changed
/// runtime can never silently change saved looks.
fn verify_film_bundle(root: &Path) -> (Option<FilmConfig>, Option<&'static str>) {
    let unavailable = "film-bundle-unavailable";
    let manifest_path = root.join("bundle-manifest.json");
    let Some(manifest) = read_bounded_json(&manifest_path, FILM_MANIFEST_BYTES_MAX) else {
        return (None, Some(unavailable));
    };
    let Some(bundle) = read_bounded_text(&root.join("bundle"), 128)
        .map(|value| value.trim().to_owned())
        .filter(|value| is_lower_hex(value, 64))
    else {
        return (None, Some(unavailable));
    };
    let digest_matches = read_bounded_bytes(&manifest_path, FILM_MANIFEST_BYTES_MAX)
        .is_some_and(|bytes| format!("{:x}", Sha256::digest(&bytes)) == bundle);
    if !digest_matches
        || manifest["format"].as_u64() != Some(2)
        || manifest["implementation"].as_str()
            != Some(slipstream_processing::modules::SPEKTRAFILM_IMPLEMENTATION)
        || manifest["forkCommit"].as_str()
            != Some(slipstream_processing::modules::SPEKTRAFILM_FORK_COMMIT)
        || manifest["adapterVersion"].as_str()
            != Some(slipstream_processing::modules::SPEKTRAFILM_ADAPTER_VERSION)
        || manifest["parameterSchemaVersion"].as_str()
            != Some(slipstream_processing::modules::SPEKTRAFILM_PARAMETER_VERSION)
    {
        return (None, Some(unavailable));
    }
    let Some(binary) = bundle_manifest_path(root, manifest["binary"].as_str()) else {
        return (None, Some(unavailable));
    };
    let Some(data_root) = bundle_manifest_path(root, manifest["dataRoot"].as_str()) else {
        return (None, Some(unavailable));
    };
    let Some(binary_metadata) = fs::metadata(&binary).ok() else {
        return (None, Some(unavailable));
    };
    if !binary_metadata.is_file() || binary_metadata.mode() & 0o111 == 0 || !data_root.is_dir() {
        return (None, Some(unavailable));
    }

    for (key, digest) in manifest["files"].as_object().into_iter().flatten() {
        let Some(digest) = digest.as_str() else {
            return (None, Some(unavailable));
        };
        let Some(path) = bundle_manifest_path(root, Some(key)) else {
            return (None, Some(unavailable));
        };
        if verify_bundle_file(&path, digest).is_none() {
            return (None, Some(unavailable));
        }
    }
    let Some(data_entries) = manifest["data"]
        .as_object()
        .filter(|entries| !entries.is_empty())
    else {
        return (None, Some(unavailable));
    };
    for (relative, digest) in data_entries {
        let Some(digest) = digest.as_str() else {
            return (None, Some(unavailable));
        };
        let Some(path) = bundle_relative_path(&data_root, relative) else {
            return (None, Some(unavailable));
        };
        if verify_bundle_file(&path, digest).is_none() {
            return (None, Some(unavailable));
        }
    }
    let Some(parameters_path) = bundle_manifest_path(root, manifest["parametersDefault"].as_str())
    else {
        return (None, Some(unavailable));
    };
    let Some(parameter_default) = read_bounded_json(&parameters_path, 1024 * 1024) else {
        return (None, Some(unavailable));
    };
    let admitted = slipstream_processing::modules::validate_spektrafilm_parameters(
        &slipstream_processing::modules::Parameters {
            module: slipstream_processing::modules::SPEKTRAFILM_MODULE.to_owned(),
            version: slipstream_processing::modules::SPEKTRAFILM_PARAMETER_VERSION.to_owned(),
            tree: parameter_default.clone(),
        },
    );
    if admitted.is_err() {
        return (None, Some(unavailable));
    }
    let Some(film_profile) = manifest["filmProfile"]
        .as_str()
        .filter(|value| !value.is_empty())
    else {
        return (None, Some(unavailable));
    };
    let Some(print_profile) = manifest["printProfile"]
        .as_str()
        .filter(|value| !value.is_empty())
    else {
        return (None, Some(unavailable));
    };
    (
        Some(FilmConfig {
            bundle_sha256: bundle,
            bundle_root: root.to_path_buf(),
            binary,
            data_root,
            parameter_default,
            film_profile: film_profile.to_owned(),
            print_profile: print_profile.to_owned(),
            failure: None,
        }),
        None,
    )
}

fn bundle_manifest_path(root: &Path, value: Option<&str>) -> Option<PathBuf> {
    let relative = value?.strip_prefix("/opt/slipstream-film/")?;
    bundle_relative_path(root, relative)
}

fn bundle_relative_path(root: &Path, value: &str) -> Option<PathBuf> {
    let canonical_root = fs::canonicalize(root).ok()?;
    let path = safe_relative_path(&canonical_root, value)?;
    let canonical = fs::canonicalize(path).ok()?;
    canonical.starts_with(&canonical_root).then_some(canonical)
}
fn safe_relative_path(root: &Path, value: &str) -> Option<PathBuf> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return None;
    }
    Some(root.join(path))
}

fn read_bounded_json(path: &Path, maximum: u64) -> Option<serde_json::Value> {
    serde_json::from_slice(&read_bounded_bytes(path, maximum)?).ok()
}

fn read_bounded_bytes(path: &Path, maximum: u64) -> Option<Vec<u8>> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(maximum + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    ((bytes.len() as u64) <= maximum).then_some(bytes)
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

#[cfg(test)]
#[path = "config_film_tests.rs"]
mod film_bundle_tests;
