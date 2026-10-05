use super::*;
#[path = "auth_input.rs"]
mod auth_input;
impl CommandFailure {
    pub(crate) fn storage(operation: Operation) -> Self {
        Self::from_payload(
            6,
            ErrorPayload {
                code: "storage_failed".to_owned(),
                message: "The local credential store could not be read or written.".to_owned(),
                effect: "none".to_owned(),
                details: json!({ "operation": operation.wire() }),
            },
        )
    }
}
#[cfg(unix)]
use std::os::unix::{
    fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    io::AsRawFd,
};
use std::{
    collections::BTreeMap,
    io::Write,
    process::{Command as ProcessCommand, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::Instant as StdInstant,
};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

const MAXIMUM_CREDENTIAL_BYTES: usize = 45;
const MAXIMUM_AUTH_STORE_BYTES: usize = 64 * 1024;
const AUTH_STORE_VERSION: u8 = 1;
const KEYRING_SERVICE: &str = "slipstream";
const KEYRING_TIMEOUT: Duration = Duration::from_secs(2);
static NEXT_KEYRING_CREDENTIAL: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AuthStore {
    version: u8,
    active_origin: Option<String>,
    entries: BTreeMap<String, AuthEntry>,
}

impl Default for AuthStore {
    fn default() -> Self {
        Self {
            version: AUTH_STORE_VERSION,
            active_origin: None,
            entries: BTreeMap::new(),
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AuthEntry {
    storage: CredentialStorage,
    #[serde(default)]
    credential_id: Option<String>,
    token: Option<String>,
    created_at: String,
    last_login_at: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum CredentialStorage {
    File,
    Keyring,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthEntryView {
    origin: String,
    active: bool,
    storage: CredentialStorage,
    created_at: String,
    last_login_at: String,
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned())
}

fn auth_store_path(operation: Operation) -> Result<PathBuf, CommandFailure> {
    let base = env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".config"))
        })
        .ok_or_else(|| CommandFailure::storage(operation))?;
    if !base.is_absolute() {
        return Err(CommandFailure::storage(operation));
    }
    Ok(base.join("slipstream").join("auth.json"))
}

fn empty_auth_store() -> AuthStore {
    AuthStore::default()
}

fn read_auth_store(operation: Operation) -> Result<AuthStore, CommandFailure> {
    let path = auth_store_path(operation)?;
    let bytes = {
        #[cfg(unix)]
        {
            let mut options = OpenOptions::new();
            options
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
            let file = match options.open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(empty_auth_store());
                }
                Err(_) => return Err(CommandFailure::storage(operation)),
            };
            let metadata = file
                .metadata()
                .map_err(|_| CommandFailure::storage(operation))?;
            if !metadata.is_file()
                || metadata.uid() != unsafe { libc::geteuid() }
                || metadata.mode() & 0o777 != 0o600
                || metadata.len() > MAXIMUM_AUTH_STORE_BYTES as u64
            {
                return Err(CommandFailure::storage(operation));
            }
            let mut bytes = Vec::with_capacity(MAXIMUM_AUTH_STORE_BYTES + 1);
            file.take((MAXIMUM_AUTH_STORE_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|_| CommandFailure::storage(operation))?;
            if bytes.len() > MAXIMUM_AUTH_STORE_BYTES {
                return Err(CommandFailure::storage(operation));
            }
            bytes
        }
        #[cfg(not(unix))]
        {
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(empty_auth_store());
                }
                Err(_) => return Err(CommandFailure::storage(operation)),
            };
            if !metadata.file_type().is_file() || metadata.len() > MAXIMUM_AUTH_STORE_BYTES as u64 {
                return Err(CommandFailure::storage(operation));
            }
            let file =
                std::fs::File::open(&path).map_err(|_| CommandFailure::storage(operation))?;
            let mut bytes = Vec::with_capacity(MAXIMUM_AUTH_STORE_BYTES + 1);
            file.take((MAXIMUM_AUTH_STORE_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|_| CommandFailure::storage(operation))?;
            if bytes.len() > MAXIMUM_AUTH_STORE_BYTES {
                return Err(CommandFailure::storage(operation));
            }
            bytes
        }
    };
    let store: AuthStore =
        serde_json::from_slice(&bytes).map_err(|_| CommandFailure::storage(operation))?;
    if store.version != AUTH_STORE_VERSION
        || store
            .entries
            .keys()
            .any(|origin| !is_canonical_auth_origin(origin))
        || store.active_origin.as_deref().is_some_and(|origin| {
            !is_canonical_auth_origin(origin) || !store.entries.contains_key(origin)
        })
    {
        return Err(CommandFailure::storage(operation));
    }
    Ok(store)
}

fn write_auth_store(store: &AuthStore, operation: Operation) -> Result<(), CommandFailure> {
    let path = auth_store_path(operation)?;
    let directory = path
        .parent()
        .ok_or_else(|| CommandFailure::storage(operation))?;
    std::fs::create_dir_all(directory).map_err(|_| CommandFailure::storage(operation))?;
    #[cfg(unix)]
    {
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| CommandFailure::storage(operation))?;
    }
    let bytes = serde_json::to_vec_pretty(store).map_err(|_| CommandFailure::storage(operation))?;
    if bytes.len() > MAXIMUM_AUTH_STORE_BYTES {
        return Err(CommandFailure::storage(operation));
    }
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&temporary)
        .map_err(|_| CommandFailure::storage(operation))?;
    if file
        .write_all(&bytes)
        .and_then(|_| file.sync_all())
        .is_err()
    {
        drop(file);
        let _ = std::fs::remove_file(&temporary);
        return Err(CommandFailure::storage(operation));
    }
    drop(file);
    #[cfg(windows)]
    if path.exists() {
        std::fs::remove_file(&path).map_err(|_| CommandFailure::storage(operation))?;
    }
    std::fs::rename(&temporary, &path).map_err(|_| {
        let _ = std::fs::remove_file(&temporary);
        CommandFailure::storage(operation)
    })?;
    Ok(())
}

fn service_origin_from_value(value: &str) -> Result<Url, CommandFailure> {
    let origin = parse_service_origin(value)?;
    if origin.scheme() != "https" {
        return Err(CommandFailure::invalid(
            "server",
            "Saved credentials require an HTTPS service origin.",
        ));
    }
    Ok(origin)
}

fn is_canonical_auth_origin(value: &str) -> bool {
    service_origin_from_value(value)
        .map(|origin| origin.as_str() == value)
        .unwrap_or(false)
}

fn keyring_available(deadline: Option<tokio::time::Instant>) -> bool {
    if env::var_os("SLIPSTREAM_DISABLE_KEYRING").is_some() {
        return false;
    }
    keyring_command(&["--version"], None, deadline).is_ok()
}

fn keyring_command(
    args: &[&str],
    token: Option<&str>,
    deadline: Option<tokio::time::Instant>,
) -> Result<String, ()> {
    let budget = deadline
        .map(|deadline| deadline.saturating_duration_since(tokio::time::Instant::now()))
        .unwrap_or(KEYRING_TIMEOUT)
        .min(KEYRING_TIMEOUT);
    if budget.is_zero() {
        return Err(());
    }
    let started = StdInstant::now();
    let mut command = ProcessCommand::new("secret-tool");
    command
        .args(args)
        .stdin(if token.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().map_err(|_| ())?;
    if let Some(token) = token {
        let mut stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(());
            }
        };
        if stdin.write_all(token.as_bytes()).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(());
        }
    }
    if started.elapsed() >= budget {
        let _ = child.kill();
        let _ = child.wait();
        return Err(());
    }
    loop {
        let status = match child.try_wait() {
            Ok(status) => status,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(());
            }
        };
        if let Some(status) = status {
            let mut stdout = child.stdout.take().ok_or(())?;
            #[cfg(unix)]
            {
                let fd = stdout.as_raw_fd();
                let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
                if flags < 0
                    || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
                {
                    return Err(());
                }
            }
            let mut output = Vec::new();
            loop {
                let mut chunk = [0_u8; 1024];
                match stdout.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(length) => {
                        output.extend_from_slice(&chunk[..length]);
                        if output.len() > 4096 {
                            let _ = child.kill();
                            let _ = child.wait();
                            return Err(());
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if started.elapsed() >= budget {
                            return Err(());
                        }
                        thread::sleep(
                            Duration::from_millis(10).min(budget.saturating_sub(started.elapsed())),
                        );
                    }
                    Err(_) => return Err(()),
                }
            }
            if !status.success() {
                return Err(());
            }
            return String::from_utf8(output).map_err(|_| ());
        }
        if started.elapsed() >= budget {
            let _ = child.kill();
            let _ = child.wait();
            return Err(());
        }
        thread::sleep(Duration::from_millis(10).min(budget.saturating_sub(started.elapsed())));
    }
}

fn keyring_store_reconciled(
    credential_id: &str,
    token: &str,
    deadline: Option<tokio::time::Instant>,
) -> Result<bool, ()> {
    if keyring_store(credential_id, token, deadline).is_ok() {
        return Ok(true);
    }
    match keyring_load(credential_id, deadline) {
        Ok(stored) if stored == token => Ok(true),
        Ok(_) => Err(()),
        Err(_) if keyring_delete(credential_id, deadline).is_ok() => Ok(false),
        Err(_) => Err(()),
    }
}

fn keyring_replace_reconciled(
    credential_id: &str,
    token: &str,
    deadline: Option<tokio::time::Instant>,
) -> Result<bool, ()> {
    if keyring_store(credential_id, token, deadline).is_ok() {
        return Ok(true);
    }
    Ok(matches!(
        keyring_load(credential_id, deadline),
        Ok(stored) if stored == token
    ))
}

fn new_keyring_id(origin: &str) -> String {
    format!(
        "{}#{}#{}",
        origin,
        OffsetDateTime::now_utc().unix_timestamp_nanos(),
        NEXT_KEYRING_CREDENTIAL.fetch_add(1, Ordering::Relaxed)
    )
}

fn keyring_store(
    credential_id: &str,
    token: &str,
    deadline: Option<tokio::time::Instant>,
) -> Result<(), ()> {
    keyring_command(
        &[
            "store",
            "--label=Slipstream Access Token",
            "application",
            KEYRING_SERVICE,
            "credential",
            credential_id,
        ],
        Some(token),
        deadline,
    )
    .map(|_| ())
}

fn keyring_load(credential_id: &str, deadline: Option<tokio::time::Instant>) -> Result<String, ()> {
    keyring_command(
        &[
            "lookup",
            "application",
            KEYRING_SERVICE,
            "credential",
            credential_id,
        ],
        None,
        deadline,
    )
    .map(|token| token.trim_end_matches(['\r', '\n']).to_owned())
}

fn keyring_delete(credential_id: &str, deadline: Option<tokio::time::Instant>) -> Result<(), ()> {
    keyring_command(
        &[
            "clear",
            "application",
            KEYRING_SERVICE,
            "credential",
            credential_id,
        ],
        None,
        deadline,
    )
    .map(|_| ())
}

fn store_token(
    store: &mut AuthStore,
    origin: &str,
    token: String,
    replacing: bool,
    deadline: tokio::time::Instant,
) -> Result<CredentialStorage, CommandFailure> {
    let now = now_rfc3339();
    let created_at = store
        .entries
        .get(origin)
        .map(|entry| entry.created_at.clone())
        .unwrap_or_else(|| now.clone());
    let previous = store.clone();
    let old_entry = store.entries.get(origin).cloned();
    let mut old_keyring_token = None;
    let mut credential_id = None;
    let storage = if let Some(old_entry) = old_entry
        .as_ref()
        .filter(|entry| entry.storage == CredentialStorage::Keyring)
    {
        let old_id = old_entry
            .credential_id
            .clone()
            .unwrap_or_else(|| origin.to_owned());
        let old_token = load_stored_token(old_entry, origin, Some(deadline), Operation::AuthLogin)?;
        if !keyring_replace_reconciled(&old_id, &token, Some(deadline)).is_ok_and(|stored| stored) {
            return Err(CommandFailure::storage(Operation::AuthLogin));
        }
        old_keyring_token = Some(old_token);
        credential_id = Some(old_id);
        CredentialStorage::Keyring
    } else if keyring_available(Some(deadline)) {
        let candidate = new_keyring_id(origin);
        match keyring_store_reconciled(&candidate, &token, Some(deadline)) {
            Ok(true) => {
                credential_id = Some(candidate);
                CredentialStorage::Keyring
            }
            Ok(false) => {
                eprintln!(
                    "Warning: system credential storage is unavailable; using a protected local file."
                );
                CredentialStorage::File
            }
            Err(()) => return Err(CommandFailure::storage(Operation::AuthLogin)),
        }
    } else {
        CredentialStorage::File
    };
    if tokio::time::Instant::now() >= deadline {
        if let Some(credential_id) = &credential_id {
            if let Some(old_token) = &old_keyring_token {
                if !keyring_store_reconciled(credential_id, old_token, Some(deadline))
                    .is_ok_and(|stored| stored)
                {
                    return Err(CommandFailure::storage(Operation::AuthLogin));
                }
            } else if keyring_delete(credential_id, Some(deadline)).is_err() {
                return Err(CommandFailure::storage(Operation::AuthLogin));
            }
        }
        return Err(CommandFailure::transport(Operation::AuthLogin));
    }
    let entry = AuthEntry {
        storage,
        credential_id: credential_id.clone(),
        token: (storage == CredentialStorage::File).then_some(token),
        created_at,
        last_login_at: now,
    };
    store.entries.insert(origin.to_owned(), entry);
    if !replacing || store.active_origin.is_none() {
        store.active_origin = Some(origin.to_owned());
    }
    if let Err(error) = write_auth_store(store, Operation::AuthLogin) {
        *store = previous;
        if let Some(credential_id) = credential_id {
            if let Some(old_token) = old_keyring_token {
                if !keyring_store_reconciled(&credential_id, &old_token, Some(deadline))
                    .is_ok_and(|stored| stored)
                {
                    return Err(CommandFailure::storage(Operation::AuthLogin));
                }
            } else if keyring_delete(&credential_id, Some(deadline)).is_err() {
                return Err(CommandFailure::storage(Operation::AuthLogin));
            }
        }
        return Err(error);
    }
    Ok(storage)
}

fn load_stored_token(
    entry: &AuthEntry,
    origin: &str,
    deadline: Option<tokio::time::Instant>,
    operation: Operation,
) -> Result<String, CommandFailure> {
    let token = match entry.storage {
        CredentialStorage::File => entry.token.clone(),
        CredentialStorage::Keyring => {
            keyring_load(entry.credential_id.as_deref().unwrap_or(origin), deadline).ok()
        }
    }
    .ok_or_else(|| CommandFailure::storage(operation))?;
    if !canonical_access_token(token.as_bytes()) {
        return Err(CommandFailure::storage(operation));
    }
    Ok(token)
}

async fn load_stored_token_async(
    entry: AuthEntry,
    origin: String,
    deadline: Option<tokio::time::Instant>,
    operation: Operation,
) -> Result<String, CommandFailure> {
    tokio::task::spawn_blocking(move || load_stored_token(&entry, &origin, deadline, operation))
        .await
        .map_err(|_| CommandFailure::storage(operation))?
}

fn logout_store_entry(
    store: &mut AuthStore,
    origin: &str,
    deadline: tokio::time::Instant,
) -> Result<bool, CommandFailure> {
    let previous = store.clone();
    let existing = store.entries.get(origin).cloned();
    let active_removed = store.active_origin.as_deref() == Some(origin);
    if existing.is_none() && !active_removed {
        return Ok(false);
    }
    let (old_keyring_id, old_keyring_token) = if let Some(entry) = existing
        .as_ref()
        .filter(|entry| entry.storage == CredentialStorage::Keyring)
    {
        let credential_id = entry
            .credential_id
            .clone()
            .unwrap_or_else(|| origin.to_owned());
        let token = load_stored_token(entry, origin, Some(deadline), Operation::AuthLogout)?;
        (Some(credential_id), Some(token))
    } else {
        (None, None)
    };
    if let Some(credential_id) = &old_keyring_id
        && keyring_delete(credential_id, Some(deadline)).is_err()
    {
        if let Some(token) = &old_keyring_token {
            let _ = keyring_store_reconciled(credential_id, token, Some(deadline));
        }
        return Err(CommandFailure::storage(Operation::AuthLogout));
    }
    let removed = store.entries.remove(origin);
    if active_removed {
        store.active_origin = None;
    }
    if let Err(error) = write_auth_store(store, Operation::AuthLogout) {
        *store = previous;
        if let (Some(credential_id), Some(token)) = (old_keyring_id, old_keyring_token)
            && !keyring_store_reconciled(&credential_id, &token, Some(deadline))
                .is_ok_and(|stored| stored)
        {
            return Err(CommandFailure::storage(Operation::AuthLogout));
        }
        return Err(error);
    }
    Ok(removed.is_some())
}

fn auth_entry_view(origin: &str, entry: &AuthEntry, active_origin: Option<&str>) -> AuthEntryView {
    AuthEntryView {
        origin: origin.to_owned(),
        active: active_origin == Some(origin),
        storage: entry.storage,
        created_at: entry.created_at.clone(),
        last_login_at: entry.last_login_at.clone(),
    }
}

fn auth_check_name<T>(result: &Result<T, CommandFailure>) -> &'static str {
    match result {
        Ok(_) => "valid",
        Err(failure)
            if failure.payload.code == "authentication_required"
                || failure.payload.code == "access_denied" =>
        {
            "invalid"
        }
        Err(failure) if failure.payload.code == "incompatible_server" => "incompatible",
        Err(_) => "unreachable",
    }
}

async fn auth_status(
    cli: &Cli,
    environment: Option<&str>,
    store: &AuthStore,
    deadline: tokio::time::Instant,
) -> Result<Value, CommandFailure> {
    let entries = store
        .entries
        .iter()
        .map(|(origin, entry)| auth_entry_view(origin, entry, store.active_origin.as_deref()))
        .collect::<Vec<_>>();
    let selected = cli
        .server
        .as_deref()
        .or(environment)
        .or(store.active_origin.as_deref());
    let selected = if let Some(value) = selected {
        let origin = service_origin_from_value(value)?;
        let key = origin.as_str().to_owned();
        let stored = store.entries.get(&key);
        let override_path = access_token_override(cli)?;
        let (source, token) = if let Some(path) = override_path {
            let token = read_access_token(path).await?;
            (
                Some(if cli.token_file.is_some() {
                    "explicit-file"
                } else {
                    "environment-file"
                }),
                Some(token),
            )
        } else if let Some(entry) = stored {
            let token = load_stored_token_async(
                entry.clone(),
                key.clone(),
                Some(deadline),
                Operation::AuthStatus,
            )
            .await?;
            (Some("stored"), Some(token))
        } else {
            (None, None)
        };
        let check = if let Some(token) = token {
            match ServiceClient::new(origin.clone(), token, Duration::from_secs(cli.timeout)) {
                Ok(client) => {
                    let result = client.capabilities(Operation::AuthStatus).await;
                    auth_check_name(&result)
                }
                Err(_) => "unreachable",
            }
        } else {
            "not-configured"
        };
        Some(json!({
            "origin": key,
            "active": store.active_origin.as_deref() == Some(key.as_str()),
            "stored": stored.is_some(),
            "storage": stored.map(|entry| entry.storage),
            "source": source,
            "check": check,
            "createdAt": stored.map(|entry| entry.created_at.clone()),
            "lastLoginAt": stored.map(|entry| entry.last_login_at.clone()),
        }))
    } else {
        None
    };
    Ok(json!({
        "activeOrigin": store.active_origin,
        "entries": entries,
        "selected": selected,
    }))
}

async fn execute_auth_command(
    cli: &Cli,
    environment: Option<&str>,
    store: &mut AuthStore,
    deadline: tokio::time::Instant,
) -> Result<Value, CommandFailure> {
    match &cli.command {
        Command::Auth {
            command: AuthCommand::Login(args),
        } => {
            if cli.token_file.is_some() {
                return Err(CommandFailure::invalid(
                    "token-file",
                    "Use auth login's --token-file, not the global --token-file.",
                ));
            }
            let origin = login_origin(cli, environment)?;
            let origin_key = origin.as_str().to_owned();
            let token = auth_input::read_login_token(args).await?;
            let replacing = store.entries.contains_key(&origin_key);
            if replacing && !args.force && !auth_input::confirm_replacement().await? {
                return Err(CommandFailure::invalid(
                    "force",
                    "An existing credential was kept. Use --force to replace it non-interactively.",
                ));
            }
            let client = ServiceClient::new(
                origin.clone(),
                token.clone(),
                Duration::from_secs(cli.timeout),
            )
            .map_err(|_| CommandFailure::transport(Operation::AuthLogin))?;
            client.capabilities(Operation::AuthLogin).await?;
            let storage = store_token(store, &origin_key, token, replacing, deadline)?;
            Ok(json!({
                "origin": origin_key,
                "storage": storage,
                "active": store.active_origin.as_deref() == Some(origin.as_str()),
            }))
        }
        Command::Auth {
            command: AuthCommand::Status,
        } => auth_status(cli, environment, store, deadline).await,
        Command::Auth {
            command: AuthCommand::Use,
        } => {
            let origin = login_origin(cli, environment)?;
            let key = origin.as_str().to_owned();
            if !store.entries.contains_key(&key) {
                return Err(CommandFailure::invalid(
                    "server",
                    "No saved credential exists for this server origin.",
                ));
            }
            store.active_origin = Some(key.clone());
            write_auth_store(store, Operation::AuthUse)?;
            Ok(json!({ "origin": key, "active": true }))
        }
        Command::Auth {
            command: AuthCommand::Logout,
        } => {
            let origin = cli
                .server
                .as_deref()
                .or(environment)
                .or(store.active_origin.as_deref())
                .ok_or_else(|| {
                    CommandFailure::invalid(
                        "server",
                        "Set --server or select an active saved origin first.",
                    )
                })?;
            let origin = service_origin_from_value(origin)?;
            let key = origin.as_str().to_owned();
            let removed = logout_store_entry(store, &key, deadline)?;
            Ok(json!({
                "origin": key,
                "removed": removed,
                "remoteRevoked": false,
                "environmentOverride": access_token_override(cli)?.is_some(),
            }))
        }
        _ => Err(CommandFailure::invalid("command", "Not an auth command.")),
    }
}

fn login_origin(cli: &Cli, environment: Option<&str>) -> Result<Url, CommandFailure> {
    let value = cli.server.as_deref().or(environment).ok_or_else(|| {
        CommandFailure::invalid(
            "server",
            "Set --server or SLIPSTREAM_SERVER_URL to an HTTPS origin.",
        )
    })?;
    service_origin_from_value(value)
}

fn access_token_override(cli: &Cli) -> Result<Option<PathBuf>, CommandFailure> {
    let path = cli
        .token_file
        .clone()
        .or_else(|| env::var_os("SLIPSTREAM_ACCESS_TOKEN_FILE").map(PathBuf::from));
    if path
        .as_ref()
        .is_some_and(|path| path.as_os_str().is_empty())
    {
        return Err(CommandFailure::invalid(
            "token-file",
            "The credential file path must not be empty.",
        ));
    }
    Ok(path)
}

pub(crate) async fn ordinary_credentials(
    cli: &Cli,
    environment: Option<&str>,
    operation: Operation,
    deadline: tokio::time::Instant,
) -> Result<(Url, String), CommandFailure> {
    let override_path = access_token_override(cli)?;
    let store = if override_path.is_some() && (cli.server.is_some() || environment.is_some()) {
        AuthStore::default()
    } else {
        read_auth_store(operation)?
    };
    let value = cli
        .server
        .as_deref()
        .or(environment)
        .or(store.active_origin.as_deref())
        .ok_or_else(|| {
            CommandFailure::invalid(
                "server",
                "Set --server, SLIPSTREAM_SERVER_URL, or run auth use.",
            )
        })?;
    // Explicit file credentials retain the operational HTTP and HTTPS contract.
    let origin = parse_service_origin(value)?;
    let token = if let Some(path) = override_path {
        read_access_token(path).await?
    } else {
        let entry = store.entries.get(origin.as_str()).ok_or_else(|| {
            CommandFailure::invalid(
                "token-file",
                "Set --token-file, SLIPSTREAM_ACCESS_TOKEN_FILE, or run auth login.",
            )
        })?;
        load_stored_token_async(entry.clone(), origin.to_string(), Some(deadline), operation)
            .await?
    };
    Ok((origin, token))
}

pub(crate) async fn execute_auth(
    cli: &Cli,
    environment: Option<&str>,
    deadline: tokio::time::Instant,
) -> Result<Value, CommandFailure> {
    if matches!(
        &cli.command,
        Command::Auth {
            command: AuthCommand::Login(_)
        }
    ) && cli.token_file.is_some()
    {
        return Err(CommandFailure::invalid(
            "token-file",
            "Use auth login's --token-file, not the global --token-file.",
        ));
    }
    let mut store = read_auth_store(command_operation(&cli.command))?;
    execute_auth_command(cli, environment, &mut store, deadline).await
}
