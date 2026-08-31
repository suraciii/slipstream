use super::*;
pub const HEALTH_PATH: &str = "/healthz";

pub(crate) const MAXIMUM_HEADER_BYTES: usize = 16 * 1024;
pub(crate) const MAXIMUM_MUTATION_BODY_BYTES: usize = 64 * 1024;
const DEFAULT_DATABASE_BASENAME: &str = "library.sqlite";
const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 3000;

/// Typed values accepted by the existing `SLIPSTREAM_*` startup contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Config {
    pub library_root: PathBuf,
    pub state_directory: PathBuf,
    pub cache_directory: PathBuf,
    pub database_basename: String,
    pub host: String,
    pub port: u16,
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
            web_root,
        })
    }

    pub fn web_root(&self) -> PathBuf {
        self.web_root
            .clone()
            .unwrap_or_else(|| PathBuf::from("apps/web/dist"))
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
    let paths = [
        config.library_root.as_path(),
        config.state_directory.as_path(),
        config.cache_directory.as_path(),
    ];
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
