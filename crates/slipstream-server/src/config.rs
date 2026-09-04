use super::*;
use ::http::uri::Authority;
pub const HEALTH_PATH: &str = "/healthz";

pub(crate) const MAXIMUM_HEADER_BYTES: usize = 16 * 1024;
pub(crate) const MAXIMUM_MUTATION_BODY_BYTES: usize = 64 * 1024;
const DEFAULT_DATABASE_BASENAME: &str = "library.sqlite";
const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 3000;

/// The one browser-visible authority an online Slipstream server admits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublicOrigin {
    scheme: &'static str,
    host: String,
    port: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PublicOriginError;

impl fmt::Display for PublicOriginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid public origin")
    }
}

impl std::error::Error for PublicOriginError {}

impl PublicOrigin {
    pub fn parse(value: &str) -> Result<Self, PublicOriginError> {
        let (scheme, authority) = value.split_once("://").ok_or(PublicOriginError)?;
        let scheme = if scheme.eq_ignore_ascii_case("http") {
            "http"
        } else if scheme.eq_ignore_ascii_case("https") {
            "https"
        } else {
            return Err(PublicOriginError);
        };
        if authority.is_empty()
            || authority.contains(['/', '?', '#', '@'])
            || authority.contains("://")
        {
            return Err(PublicOriginError);
        }
        let (host, port) = parse_authority(authority, scheme)?;
        Ok(Self { scheme, host, port })
    }

    pub(crate) fn matches_authority(&self, value: &str) -> Result<bool, PublicOriginError> {
        let (host, port) = parse_authority(value, self.scheme)?;
        Ok(self.host == host && self.port == port)
    }

    pub(crate) fn matches_absolute_uri(
        &self,
        uri: &::http::Uri,
    ) -> Result<bool, PublicOriginError> {
        let scheme = uri.scheme_str().ok_or(PublicOriginError)?;
        let authority = uri.authority().ok_or(PublicOriginError)?;
        let supported_scheme =
            scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https");
        if !supported_scheme {
            return Ok(false);
        }
        Ok(scheme.eq_ignore_ascii_case(self.scheme)
            && self.matches_authority(authority.as_str())?)
    }
}

impl fmt::Display for PublicOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let host = if self.host.starts_with('[') {
            self.host.as_str().to_owned()
        } else if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        write!(formatter, "{}://{host}", self.scheme)?;
        if self.port != default_port(self.scheme) {
            write!(formatter, ":{}", self.port)?;
        }
        Ok(())
    }
}

fn parse_authority(value: &str, scheme: &str) -> Result<(String, u16), PublicOriginError> {
    if value.is_empty() || value.contains('@') {
        return Err(PublicOriginError);
    }
    let authority = value.parse::<Authority>().map_err(|_| PublicOriginError)?;
    let host = authority.host();
    if host.is_empty() {
        return Err(PublicOriginError);
    }
    let port = match explicit_port(authority.as_str())? {
        Some(port) if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) => {
            port.parse::<u16>().map_err(|_| PublicOriginError)?
        }
        Some(_) => return Err(PublicOriginError),
        None => default_port(scheme),
    };
    if port == 0 {
        return Err(PublicOriginError);
    }
    Ok((host.to_ascii_lowercase(), port))
}

fn explicit_port(value: &str) -> Result<Option<&str>, PublicOriginError> {
    if value.starts_with('[') {
        let closing = value.find(']').ok_or(PublicOriginError)?;
        let suffix = &value[closing + 1..];
        if suffix.is_empty() {
            Ok(None)
        } else {
            suffix.strip_prefix(':').map(Some).ok_or(PublicOriginError)
        }
    } else {
        Ok(value.rsplit_once(':').map(|(_, port)| port))
    }
}

fn default_port(scheme: &str) -> u16 {
    if scheme.eq_ignore_ascii_case("https") {
        443
    } else {
        80
    }
}

/// Typed values accepted by the existing `SLIPSTREAM_*` startup contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub library_root: PathBuf,
    pub state_directory: PathBuf,
    pub cache_directory: PathBuf,
    pub database_basename: String,
    pub host: String,
    pub port: u16,
    pub public_origin: PublicOrigin,
    /// Tests and packaged deployments may provide a built Web directory. When
    /// absent, the binary uses the repository's conventional `apps/web/dist`.
    pub web_root: Option<PathBuf>,
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
        let public_origin = get("SLIPSTREAM_PUBLIC_ORIGIN")
            .ok_or(ConfigError::Missing("SLIPSTREAM_PUBLIC_ORIGIN"))
            .and_then(|value| {
                PublicOrigin::parse(&value)
                    .map_err(|_| ConfigError::Invalid("SLIPSTREAM_PUBLIC_ORIGIN"))
            })?;
        let web_root = get("SLIPSTREAM_WEB_ROOT").map(PathBuf::from);
        if let Some(path) = &web_root
            && !path.is_absolute()
        {
            return Err(ConfigError::Invalid("SLIPSTREAM_WEB_ROOT"));
        }
        Ok(Self {
            library_root,
            state_directory,
            cache_directory,
            database_basename,
            host,
            port,
            public_origin,
            web_root,
        })
    }

    pub fn web_root(&self) -> PathBuf {
        self.web_root
            .clone()
            .unwrap_or_else(|| PathBuf::from("apps/web/dist"))
    }
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
    WebUnavailable,
    StorageLayout,
    Io(io::Error),
    Cache(String),
    BrowseNotFound,
    BrowseLimit,
    Join(String),
    NotPublished,
    FileLocationsExpired,
    FolderInvalid,
    FolderNotFound,
    FileLocationWindow,
}

impl fmt::Display for ServerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => error.fmt(formatter),
            Self::Library(error) => error.fmt(formatter),
            Self::Preview(error) => formatter.write_str(error),
            Self::PreviewUnavailable => formatter.write_str("Preview service is unavailable"),
            Self::WebUnavailable => formatter.write_str("Web application is not built"),
            Self::StorageLayout => {
                formatter.write_str("Photo Library, state, and cache directories must not overlap")
            }
            Self::Io(error) => error.fmt(formatter),
            Self::Cache(error) => formatter.write_str(error),
            Self::BrowseNotFound => formatter.write_str("Browse source is no longer available"),
            Self::BrowseLimit => formatter.write_str("Browse window is invalid"),
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
pub(crate) const MAX_BROWSE_SNAPSHOTS: usize = 8;
pub(crate) const BROWSE_SNAPSHOT_IDLE: Duration = Duration::from_secs(30 * 60);

pub(crate) static NEXT_BROWSE_NAMESPACE: AtomicU64 = AtomicU64::new(0);
