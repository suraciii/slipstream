//! Instance credentials belong to the HTTP adapter, never to Photo models.
use crate::*;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use slipstream_core::{
    LibraryRoot,
    persistence::{DatabaseName, StateDirectory},
};
use std::{
    collections::{HashMap, VecDeque},
    io::{IsTerminal, Write},
    net::IpAddr,
};
use subtle::ConstantTimeEq;

const COOKIE: &str = "__Host-slipstream";
const LIFETIME: u64 = 7 * 24 * 60 * 60;
const SESSION_PATH: &str = "/api/access/session";

pub(crate) fn canonical_origin(value: &str) -> Option<String> {
    if value.chars().any(char::is_whitespace) {
        return None;
    }
    let url = url::Url::parse(value).ok()?;
    (url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && url.path() == "/")
        .then(|| url.origin().ascii_serialization())
}
fn secret() -> Result<String, ()> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|_| ())?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
fn valid_secret(value: &str) -> bool {
    value.len() == 43 && URL_SAFE_NO_PAD.decode(value).is_ok_and(|v| v.len() == 32)
}
fn digest(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}
fn equal(a: &str, b: &str) -> bool {
    bool::from(digest(a).ct_eq(&digest(b)))
}
fn now() -> Result<u64, ()> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|v| v.as_secs())
        .map_err(|_| ())
}

#[derive(Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Records {
    credential: Option<Credential>,
    sessions: Vec<Session>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Credential {
    digest: [u8; 32],
    generation: String,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Session {
    digest: [u8; 32],
    generation: String,
    created: u64,
    expires: u64,
    csrf: String,
}

struct Store {
    connection: Connection,
    _directory: StateDirectory,
    identity: slipstream_core::persistence::StateFileIdentity,
}
impl Store {
    fn open(library: &Path, state: &Path) -> Result<Self, ()> {
        let root = LibraryRoot::open(library).map_err(|_| ())?;
        let directory = StateDirectory::open_or_create(&root, state).map_err(|_| ())?;
        let name = DatabaseName::parse("access.sqlite").map_err(|_| ())?;
        let path = directory.sqlite_path(&name);
        let exists = match fs::symlink_metadata(&path) {
            Ok(_) => true,
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(_) => return Err(()),
        };
        let identity = directory.prepare_database(&name).map_err(|_| ())?;
        let connection = Connection::open(&path).map_err(|_| ())?;
        directory.verify_database(&name, identity).map_err(|_| ())?;
        connection
            .execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;")
            .map_err(|_| ())?;
        if !exists {
            connection.execute_batch("BEGIN IMMEDIATE; CREATE TABLE access (id INTEGER PRIMARY KEY CHECK(id=1), records TEXT NOT NULL); INSERT INTO access VALUES(1, '{\"credential\":null,\"sessions\":[]}'); COMMIT;").map_err(|_| ())?;
        }
        let store = Self {
            connection,
            _directory: directory,
            identity,
        };
        store.read()?;
        Ok(store)
    }
    fn read(&self) -> Result<Records, ()> {
        self._directory
            .verify_database(
                &DatabaseName::parse("access.sqlite").map_err(|_| ())?,
                self.identity,
            )
            .map_err(|_| ())?;
        self._directory
            .admit_sidecars(&DatabaseName::parse("access.sqlite").map_err(|_| ())?)
            .map_err(|_| ())?;
        let value: String = self
            .connection
            .query_row("SELECT records FROM access WHERE id=1", [], |r| r.get(0))
            .map_err(|_| ())?;
        let records: Records = serde_json::from_str(&value).map_err(|_| ())?;
        if records.sessions.len() > 32
            || records
                .credential
                .as_ref()
                .is_some_and(|c| !valid_secret(&c.generation))
            || records.sessions.iter().any(|s| {
                !valid_secret(&s.csrf)
                    || !valid_secret(&s.generation)
                    || s.created.checked_add(LIFETIME) != Some(s.expires)
                    || !records
                        .credential
                        .as_ref()
                        .is_some_and(|c| equal(&c.generation, &s.generation))
            })
        {
            return Err(());
        }
        Ok(records)
    }
    fn write(&mut self, records: &Records) -> Result<(), ()> {
        let value = serde_json::to_string(records).map_err(|_| ())?;
        let transaction = self.connection.transaction().map_err(|_| ())?;
        if transaction
            .execute("UPDATE access SET records=? WHERE id=1", [value])
            .map_err(|_| ())?
            != 1
        {
            return Err(());
        }
        transaction.commit().map_err(|_| ())
    }
}

/// Offline administration acquires the exact Library database lock without scanning.
pub fn administer_access(config: ExpansionConfig, command: &str) -> Result<(), ServerError> {
    let fail = || {
        ServerError::Join(
            "Access administration failed; check stopped service and protected state".into(),
        )
    };
    validate_expansion_storage_layout(&config)?;
    let disclose = matches!(command, "access-create" | "access-rotate");
    if !disclose && command != "access-revoke" {
        return Err(fail());
    }
    // Never disclose a credential into a redirected output or container log.
    if disclose && (!io::stdin().is_terminal() || !io::stdout().is_terminal()) {
        return Err(ServerError::Join(
            "Access creation and rotation require a private interactive terminal".into(),
        ));
    }
    let root = LibraryRoot::open(&config.library_root).map_err(|_| fail())?;
    let directory =
        StateDirectory::open_or_create(&root, &config.state_directory).map_err(|_| fail())?;
    let name = DatabaseName::parse(&config.database_basename).map_err(|_| fail())?;
    let identity = directory.prepare_database(&name).map_err(|_| fail())?;
    let _lock = directory.lock_database(&name).map_err(|_| fail())?;
    directory
        .verify_database(&name, identity)
        .map_err(|_| fail())?;
    let existed = fs::symlink_metadata(config.state_directory.join("access.sqlite")).is_ok();
    if command != "access-create" && !existed {
        return Err(fail());
    }
    let mut store =
        Store::open(&config.library_root, &config.state_directory).map_err(|_| fail())?;
    let mut records = store.read().map_err(|_| fail())?;
    if command == "access-create" && records.credential.is_some() {
        return Err(fail());
    }
    let token = disclose.then(secret).transpose().map_err(|_| fail())?;
    records.credential = token
        .as_ref()
        .map(|token| {
            Ok(Credential {
                digest: digest(token),
                generation: secret()?,
            })
        })
        .transpose()
        .map_err(|_: ()| fail())?;
    records.sessions.clear();
    store.write(&records).map_err(|_| fail())?;
    let mut output = io::stdout().lock();
    let delivery = if let Some(token) = token {
        writeln!(output, "{token}")
    } else {
        writeln!(output, "Access revoked")
    };
    delivery.and_then(|_| output.flush()).map_err(|_| {
        ServerError::Join(
            "Access changed, but terminal delivery failed; rotate the token before use".into(),
        )
    })
}

#[derive(Default)]
struct Rate {
    all: VecDeque<Instant>,
    peers: HashMap<IpAddr, VecDeque<Instant>>,
}
impl Rate {
    fn admit(&mut self, peer: IpAddr, time: Instant) -> Result<(), u64> {
        let trim = |v: &mut VecDeque<Instant>| {
            while v
                .front()
                .is_some_and(|t| time.duration_since(*t) >= Duration::from_secs(60))
            {
                v.pop_front();
            }
        };
        trim(&mut self.all);
        self.peers.retain(|_, v| {
            trim(v);
            !v.is_empty()
        });
        if !self.peers.contains_key(&peer) && self.peers.len() >= 1024 {
            return Err(60);
        }
        let bucket = self.peers.entry(peer).or_default();
        let mut retry = 0;
        for (v, limit) in [(&self.all, 20), (&*bucket, 5)] {
            if v.len() >= limit {
                retry = retry.max(
                    (Duration::from_secs(60) - time.duration_since(v[0]))
                        .as_secs_f64()
                        .ceil() as u64,
                );
            }
        }
        if retry > 0 {
            return Err(retry);
        }
        self.all.push_back(time);
        bucket.push_back(time);
        Ok(())
    }
}

pub(crate) struct Access {
    store: Mutex<Store>,
    origin: String,
    rate: Mutex<Rate>,
    exchanges: tokio::sync::Semaphore,
}
impl Access {
    pub(crate) fn open(config: &Config) -> Result<Self, ServerError> {
        let origin = canonical_origin(&config.public_origin)
            .ok_or(ConfigError::Invalid("SLIPSTREAM_PUBLIC_ORIGIN"))?;
        let store = Store::open(&config.library_root, &config.state_directory).map_err(|_| {
            ServerError::Join("Authentication storage is invalid or unavailable".into())
        })?;
        Ok(Self {
            store: Mutex::new(store),
            origin,
            rate: Mutex::new(Rate::default()),
            exchanges: tokio::sync::Semaphore::new(4),
        })
    }
    fn origin_matches(&self, request: &Request<Body>) -> bool {
        let mut values = request.headers().get_all(header::ORIGIN).iter();
        values
            .next()
            .and_then(|v| v.to_str().ok())
            .and_then(canonical_origin)
            .as_deref()
            == Some(&self.origin)
            && values.next().is_none()
    }
    fn session<'a>(records: &'a Records, cookie: Option<&str>, time: u64) -> Option<&'a Session> {
        let value = cookie.filter(|v| valid_secret(v))?;
        records.sessions.iter().find(|s| {
            s.expires > time
                && bool::from(s.digest.ct_eq(&digest(value)))
                && records
                    .credential
                    .as_ref()
                    .is_some_and(|c| equal(&c.generation, &s.generation))
        })
    }
    #[cfg(test)]
    pub(crate) fn seed_test_token(&self) {
        let mut store = self.store.lock().unwrap();
        let mut records = store.read().unwrap();
        records.credential = Some(Credential {
            digest: digest(TEST_TOKEN),
            generation: secret().unwrap(),
        });
        store.write(&records).unwrap();
    }
    pub(crate) fn admit(&self, request: &Request<Body>) -> Result<(), Box<Response<Body>>> {
        let headers = request.headers();
        let bearer = headers.get(header::AUTHORIZATION);
        let cookie = cookie(request)?;
        if bearer.is_some() && cookie.is_some() {
            return Err(error(400, "invalid_request"));
        }
        if headers.get_all(header::AUTHORIZATION).iter().count() > 1 {
            return Err(error(400, "invalid_request"));
        }
        let mut store = self
            .store
            .lock()
            .map_err(|_| error(503, "access_unavailable"))?;
        let mut records = store.read().map_err(|_| error(503, "access_unavailable"))?;
        let time = now().map_err(|_| error(503, "access_unavailable"))?;
        let old = records.sessions.len();
        records.sessions.retain(|s| s.expires > time);
        if old != records.sessions.len() {
            store
                .write(&records)
                .map_err(|_| error(503, "access_unavailable"))?;
        }
        if let Some(bearer) = bearer {
            let token = bearer
                .to_str()
                .ok()
                .and_then(|v| v.split_once(' '))
                .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("Bearer"))
                .map(|(_, token)| token.trim_start_matches(' '))
                .filter(|v| valid_secret(v));
            if !token.is_some_and(|t| {
                records
                    .credential
                    .as_ref()
                    .is_some_and(|c| bool::from(c.digest.ct_eq(&digest(t))))
            }) {
                return Err(unauthorized(true));
            }
            if headers.contains_key(header::ORIGIN) && !self.origin_matches(request) {
                return Err(error(403, "access_denied"));
            }
        } else {
            let session = Self::session(&records, cookie.as_deref(), time)
                .ok_or_else(|| unauthorized(false))?;
            if !matches!(request.method().as_str(), "GET" | "HEAD")
                && (!self.origin_matches(request) || !csrf(request, session))
            {
                return Err(error(403, "access_denied"));
            }
        }
        Ok(())
    }
    pub(crate) async fn endpoint(&self, request: Request<Body>) -> Response<Body> {
        match self.endpoint_inner(request).await {
            Ok(response) => response,
            Err(response) => *response,
        }
    }
    async fn endpoint_inner(
        &self,
        request: Request<Body>,
    ) -> Result<Response<Body>, Box<Response<Body>>> {
        if !matches!(request.method().as_str(), "GET" | "POST" | "DELETE") {
            return Err(error(405, "invalid_request"));
        }
        if request.headers().contains_key(header::AUTHORIZATION) {
            return Err(error(400, "invalid_request"));
        }
        let cookie = cookie(&request)?;
        if request.method() != "GET" && !self.origin_matches(&request) {
            return Err(error(403, "access_denied"));
        }
        if request.method() == "POST" {
            let _permit = self
                .exchanges
                .try_acquire()
                .map_err(|_| limited("rate_limited", 1))?;
            let peer = request
                .extensions()
                .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                .map(|v| v.0.ip())
                .unwrap_or(IpAddr::from([127, 0, 0, 1]));
            self.rate
                .lock()
                .map_err(|_| error(503, "access_unavailable"))?
                .admit(peer, Instant::now())
                .map_err(|retry| limited("rate_limited", retry))?;
            if request
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.split(';').next())
                != Some("application/json")
            {
                return Err(error(400, "invalid_request"));
            }
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Exchange {
                token: String,
            }
            let bytes = to_bytes(request.into_body(), 256)
                .await
                .map_err(|_| error(400, "invalid_request"))?;
            let input: Exchange =
                serde_json::from_slice(&bytes).map_err(|_| error(400, "invalid_request"))?;
            if !valid_secret(&input.token) {
                return Err(error(400, "invalid_request"));
            }
            let mut store = self
                .store
                .lock()
                .map_err(|_| error(503, "access_unavailable"))?;
            let mut records = store.read().map_err(|_| error(503, "access_unavailable"))?;
            let credential = records
                .credential
                .as_ref()
                .ok_or_else(|| error(503, "access_unconfigured"))?;
            if !bool::from(credential.digest.ct_eq(&digest(&input.token))) {
                return Err(error(401, "invalid_token"));
            }
            let generation = credential.generation.clone();
            let time = now().map_err(|_| error(503, "access_unavailable"))?;
            records.sessions.retain(|s| s.expires > time);
            if records.sessions.len() >= 32 {
                return Err(limited(
                    "session_capacity",
                    records
                        .sessions
                        .iter()
                        .map(|s| s.expires - time)
                        .min()
                        .unwrap_or(1),
                ));
            }
            let value = secret().map_err(|_| error(503, "access_unavailable"))?;
            let session = Session {
                digest: digest(&value),
                generation,
                created: time,
                expires: time + LIFETIME,
                csrf: secret().map_err(|_| error(503, "access_unavailable"))?,
            };
            records.sessions.push(session);
            store
                .write(&records)
                .map_err(|_| error(503, "access_unavailable"))?;
            return Ok(with_cookie(empty(), &value, LIFETIME));
        }
        let mut store = self
            .store
            .lock()
            .map_err(|_| error(503, "access_unavailable"))?;
        let mut records = store.read().map_err(|_| error(503, "access_unavailable"))?;
        let session = Self::session(
            &records,
            cookie.as_deref(),
            now().map_err(|_| error(503, "access_unavailable"))?,
        )
        .cloned();
        if request.method() == "DELETE" {
            if let Some(session) = session {
                if !csrf(&request, &session) {
                    return Err(error(403, "access_denied"));
                }
                records.sessions.retain(|s| s.digest != session.digest);
                store
                    .write(&records)
                    .map_err(|_| error(503, "access_unavailable"))?;
            }
            return Ok(with_cookie(empty(), "", 0));
        }
        if let Some(session) = session {
            let expiry = time::OffsetDateTime::from_unix_timestamp(session.expires as i64)
                .map_err(|_| error(503, "access_unavailable"))?
                .format(&time::format_description::well_known::Rfc3339)
                .map_err(|_| error(503, "access_unavailable"))?;
            Ok(Json(serde_json::json!({"authenticated": true, "expiresAt": expiry, "csrfToken": session.csrf})).into_response())
        } else {
            let response = Json(serde_json::json!({"authenticated": false, "configured": records.credential.is_some()})).into_response();
            Ok(if cookie.is_some() {
                with_cookie(response, "", 0)
            } else {
                response
            })
        }
    }
}
fn csrf(request: &Request<Body>, session: &Session) -> bool {
    let mut values = request.headers().get_all("x-csrf-token").iter();
    values
        .next()
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| equal(v, &session.csrf))
        && values.next().is_none()
}
fn cookie(request: &Request<Body>) -> Result<Option<String>, Box<Response<Body>>> {
    let mut result = None;
    for header in request.headers().get_all(header::COOKIE) {
        for part in header
            .to_str()
            .map_err(|_| error(400, "invalid_request"))?
            .split(';')
        {
            if let Some((name, value)) = part.trim().split_once('=')
                && name == COOKIE
            {
                if result.is_some() {
                    return Err(error(400, "invalid_request"));
                }
                result = Some(value.to_owned());
            }
        }
    }
    Ok(result)
}
fn empty() -> Response<Body> {
    StatusCode::NO_CONTENT.into_response()
}
fn with_cookie(mut response: Response<Body>, value: &str, age: u64) -> Response<Body> {
    response.headers_mut().insert(
        header::SET_COOKIE,
        format!("{COOKIE}={value}; Secure; HttpOnly; SameSite=Lax; Path=/; Max-Age={age}")
            .parse()
            .unwrap(),
    );
    response
}
pub(crate) fn error(status: u16, code: &str) -> Box<Response<Body>> {
    let mut response = (
        StatusCode::from_u16(status).unwrap(),
        Json(serde_json::json!({"error": code})),
    )
        .into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    Box::new(response)
}
fn limited(code: &str, retry: u64) -> Box<Response<Body>> {
    let mut r = error(429, code);
    r.headers_mut().insert(
        header::RETRY_AFTER,
        retry.max(1).to_string().parse().unwrap(),
    );
    r
}
fn unauthorized(bearer: bool) -> Box<Response<Body>> {
    let mut r = error(401, "authentication_required");
    r.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        if bearer {
            "Bearer realm=\"slipstream\", error=\"invalid_token\""
        } else {
            "Bearer realm=\"slipstream\""
        }
        .parse()
        .unwrap(),
    );
    r
}

pub(crate) async fn boundary(
    State(state): State<crate::http::HttpState>,
    request: Request<Body>,
    next: Next,
) -> Response<Body> {
    let path = request.uri().path();
    // Every registered route is protected by default. Only health and the
    // compiled nonprivate Web fallback are public; unknown APIs stay protected.
    let private = path == "/api"
        || path.starts_with("/api/")
        || request
            .extensions()
            .get::<axum::extract::MatchedPath>()
            .is_some_and(|matched| matched.as_str() != crate::HEALTH_PATH);
    let access = &state.application.access;
    let mut response = if request
        .headers()
        .iter()
        .map(|(k, v)| k.as_str().len() + v.len())
        .sum::<usize>()
        > MAXIMUM_HEADER_BYTES
    {
        *error(431, "invalid_request")
    } else if path == SESSION_PATH {
        access.endpoint(request).await
    } else if private {
        match access.admit(&request) {
            Ok(()) => next.run(request).await,
            Err(response) => *response,
        }
    } else {
        next.run(request).await
    };
    if private {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, "no-store".parse().unwrap());
    }
    response
}

#[cfg(test)]
pub(crate) const TEST_TOKEN: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    struct Fixture {
        base: PathBuf,
        config: Config,
    }
    impl Fixture {
        fn new() -> Self {
            let base = env::temp_dir().join(format!(
                "slipstream-access-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&base).unwrap();
            fs::create_dir(base.join("originals")).unwrap();
            fs::create_dir(base.join("web")).unwrap();
            fs::write(base.join("web/index.html"), "public shell").unwrap();
            let config = Config {
                library_root: base.join("originals"),
                state_directory: base.join("state"),
                cache_directory: base.join("cache"),
                database_basename: "library.sqlite".into(),
                host: "127.0.0.1".into(),
                port: 0,
                public_origin: "https://camera.local".into(),
                web_root: Some(base.join("web")),
                processing: None,
                export_retained_output_bytes: None,
            };
            Self { base, config }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }
    fn request(method: &str, path: &str) -> ::http::request::Builder {
        Request::builder().method(method).uri(path)
    }
    async fn exchange(access: &Access) -> Response<Body> {
        access
            .endpoint(
                request("POST", SESSION_PATH)
                    .header("origin", "https://camera.local")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"token":TEST_TOKEN}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
    }
    fn cookie_value(response: &Response<Body>) -> String {
        response.headers()[header::SET_COOKIE]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned()
    }
    async fn json(response: Response<Body>) -> Value {
        serde_json::from_slice(&to_bytes(response.into_body(), 10000).await.unwrap()).unwrap()
    }

    #[test]
    fn configured_origin_is_exact_https_origin() {
        for input in [
            "http://camera.local",
            "https://camera.local/extra",
            "https://a:b@camera.local",
            "https://camera.local?x",
            "https://camera.local#x",
            "https://",
        ] {
            assert!(canonical_origin(input).is_none(), "{input}");
        }
        assert_eq!(
            canonical_origin("https://CAMERA.local:443/"),
            Some("https://camera.local".into())
        );
    }
    #[test]
    fn rate_windows_and_peer_storage_are_bounded() {
        let mut rate = Rate::default();
        let start = Instant::now();
        let peer = IpAddr::from([1, 2, 3, 4]);
        for _ in 0..5 {
            assert!(rate.admit(peer, start).is_ok());
        }
        assert_eq!(rate.admit(peer, start), Err(60));
        assert!(rate.admit(peer, start + Duration::from_secs(60)).is_ok());
        for index in 1..=19 {
            assert!(
                rate.admit(
                    IpAddr::from([1, 2, 4, index]),
                    start + Duration::from_secs(60)
                )
                .is_ok()
            );
        }
        assert!(
            rate.admit(IpAddr::from([1, 2, 4, 40]), start + Duration::from_secs(60))
                .is_err()
        );
        let mut rate = Rate::default();
        for index in 0..1024 {
            rate.peers.insert(
                IpAddr::V4(std::net::Ipv4Addr::from(index)),
                VecDeque::from([start]),
            );
        }
        assert_eq!(rate.admit(peer, start), Err(60));
        assert!(rate.admit(peer, start + Duration::from_secs(60)).is_ok());
    }
    #[tokio::test]
    async fn session_persists_and_logout_revokes_only_presented_session() {
        let fixture = Fixture::new();
        let access = Access::open(&fixture.config).unwrap();
        access.seed_test_token();
        let first = exchange(&access).await;
        assert_eq!(first.status(), 204);
        let cookie = cookie_value(&first);
        let set = first.headers()[header::SET_COOKIE].to_str().unwrap();
        for attribute in [
            "Secure",
            "HttpOnly",
            "SameSite=Lax",
            "Path=/",
            "Max-Age=604800",
        ] {
            assert!(set.contains(attribute));
        }
        let second = cookie_value(&exchange(&access).await);
        drop(access);
        let access = Access::open(&fixture.config).unwrap();
        let status = json(
            access
                .endpoint(
                    request("GET", SESSION_PATH)
                        .header("cookie", &cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await,
        )
        .await;
        assert_eq!(status["authenticated"], true);
        assert!(
            !fs::read(fixture.config.state_directory.join("access.sqlite"))
                .unwrap()
                .windows(TEST_TOKEN.len())
                .any(|v| v == TEST_TOKEN.as_bytes())
        );
        let csrf = status["csrfToken"].as_str().unwrap();
        let mutation = || {
            request("POST", "/api/albums")
                .header("cookie", &cookie)
                .header("origin", "https://camera.local")
        };
        assert_eq!(
            access
                .admit(&mutation().body(Body::empty()).unwrap())
                .unwrap_err()
                .status(),
            403
        );
        assert!(
            access
                .admit(
                    &mutation()
                        .header("x-csrf-token", csrf)
                        .body(Body::empty())
                        .unwrap()
                )
                .is_ok()
        );
        let logout = access
            .endpoint(
                request("DELETE", SESSION_PATH)
                    .header("cookie", &cookie)
                    .header("origin", "https://camera.local")
                    .header("x-csrf-token", csrf)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert_eq!(logout.status(), 204);
        assert_eq!(
            access
                .admit(
                    &request("GET", "/api/status")
                        .header("cookie", &cookie)
                        .body(Body::empty())
                        .unwrap()
                )
                .unwrap_err()
                .status(),
            401
        );
        assert!(
            access
                .admit(
                    &request("GET", "/api/status")
                        .header("cookie", second)
                        .body(Body::empty())
                        .unwrap()
                )
                .is_ok()
        );
        assert_eq!(
            access
                .endpoint(
                    request("DELETE", SESSION_PATH)
                        .header("cookie", cookie)
                        .header("origin", "https://camera.local")
                        .body(Body::empty())
                        .unwrap()
                )
                .await
                .status(),
            204
        );
    }
    #[tokio::test]
    async fn expiry_capacity_and_revocation_deny_without_replaying() {
        let fixture = Fixture::new();
        let access = Access::open(&fixture.config).unwrap();
        access.seed_test_token();
        let first = exchange(&access).await;
        let cookie = cookie_value(&first);
        {
            let mut store = access.store.lock().unwrap();
            let mut records = store.read().unwrap();
            records.sessions[0].created = 1;
            records.sessions[0].expires = 1 + LIFETIME;
            store.write(&records).unwrap();
        }
        assert_eq!(
            access
                .admit(
                    &request("GET", "/api/status")
                        .header("cookie", &cookie)
                        .body(Body::empty())
                        .unwrap()
                )
                .unwrap_err()
                .status(),
            401
        );
        assert!(
            access
                .store
                .lock()
                .unwrap()
                .read()
                .unwrap()
                .sessions
                .is_empty()
        );
        exchange(&access).await;
        {
            let mut store = access.store.lock().unwrap();
            let mut records = store.read().unwrap();
            let session = records.sessions[0].clone();
            records.sessions.resize(32, session);
            store.write(&records).unwrap();
        }
        let response = exchange(&access).await;
        assert_eq!(response.status(), 429);
        assert_eq!(json(response).await["error"], "session_capacity");
        {
            let mut store = access.store.lock().unwrap();
            store.write(&Records::default()).unwrap();
        }
        assert_eq!(exchange(&access).await.status(), 503);
        assert_eq!(
            access
                .admit(
                    &request("GET", "/api/status")
                        .header("authorization", format!("Bearer {TEST_TOKEN}"))
                        .body(Body::empty())
                        .unwrap()
                )
                .unwrap_err()
                .status(),
            401
        );
    }
    #[tokio::test]
    async fn exchange_rejects_malformed_origin_credentials_and_ambiguous_input() {
        let fixture = Fixture::new();
        let access = Access::open(&fixture.config).unwrap();
        access.seed_test_token();
        for body in [
            format!("{{\"token\":\"{TEST_TOKEN}\",\"token\":\"{TEST_TOKEN}\"}}"),
            format!("{{\"token\":\"{TEST_TOKEN}\",\"extra\":1}}"),
            format!("{{\"token\":\"{TEST_TOKEN}\"}} trailing"),
            "x".repeat(257),
        ] {
            let response = access
                .endpoint(
                    request("POST", SESSION_PATH)
                        .header("origin", "https://camera.local")
                        .header("content-type", "application/json")
                        .body(Body::from(body))
                        .unwrap(),
                )
                .await;
            assert_eq!(response.status(), 400);
        }
        assert_eq!(
            access
                .endpoint(request("POST", SESSION_PATH).body(Body::empty()).unwrap())
                .await
                .status(),
            403
        );
        assert_eq!(
            access
                .endpoint(
                    request("POST", SESSION_PATH)
                        .header("origin", "https://camera.local.evil")
                        .body(Body::empty())
                        .unwrap()
                )
                .await
                .status(),
            403
        );
        let response = exchange(&access).await;
        assert_eq!(response.status(), 204);
        let cookie = cookie_value(&response);
        let mixed = request("GET", "/api/status")
            .header("cookie", &cookie)
            .header("authorization", format!("Bearer {TEST_TOKEN}"))
            .body(Body::empty())
            .unwrap();
        assert_eq!(access.admit(&mixed).unwrap_err().status(), 400);
        let bearer = request("POST", "/api/scan")
            .header("authorization", format!("Bearer {TEST_TOKEN}"))
            .body(Body::empty())
            .unwrap();
        assert!(access.admit(&bearer).is_ok());
        let bad_origin = request("POST", "/api/scan")
            .header("authorization", format!("Bearer {TEST_TOKEN}"))
            .header("origin", "https://evil.local")
            .body(Body::empty())
            .unwrap();
        assert_eq!(access.admit(&bad_origin).unwrap_err().status(), 403);
        assert_eq!(exchange(&access).await.status(), 429);
    }
    #[tokio::test]
    async fn routing_protects_all_api_methods_before_contract_or_resource_processing() {
        let fixture = Fixture::new();
        let application = Application::open(&fixture.config).await.unwrap();
        application.access.seed_test_token();
        let router = create_router(Arc::clone(&application), fixture.config.web_root());
        let paths = [
            "/api/status",
            "/api/capabilities",
            "/api/processing/capability",
            "/api/overview",
            "/api/albums",
            "/api/albums/missing",
            "/api/album-summaries",
            "/api/photo-queries",
            "/api/photos/missing",
            "/api/photos/missing/preview",
            "/api/photos/missing/thumbnail",
            "/api/photos/missing/metadata",
            "/api/photos/missing/albums",
            "/api/photos/state",
            "/api/photos/remove",
            "/api/photos/restore",
            "/api/photos/removed",
            "/api/trash",
            "/api/trash/review",
            "/api/trash/delete",
            "/api/trash/operations/missing",
            "/api/browse",
            "/api/browse/missing",
            "/api/scan",
            "/api/recovery/unavailable",
            "/api/recovery/propose",
            "/api/recovery/apply",
            "/api/derivatives/missing/review/image.jpg",
            "/api/private/derivatives/missing/review/image.jpg",
            "/api/future-route",
            "/api/exports/missing",
        ];
        for path in paths {
            for method in ["GET", "HEAD", "POST", "DELETE", "PUT"] {
                let response = router
                    .clone()
                    .oneshot(
                        request(method, path)
                            .header("range", "bytes=0-1")
                            .header("if-none-match", "\"cached\"")
                            .header("slipstream-cli-contract", "999")
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.status(), 401, "{method} {path}");
                assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
                assert!(response.headers().contains_key(header::WWW_AUTHENTICATE));
            }
        }
        for path in ["/healthz", "/", "/api/access/session"] {
            let response = router
                .clone()
                .oneshot(request("GET", path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), 200, "{path}");
        }
        let response = router
            .oneshot(
                request("GET", "/api/derivatives/missing/review/image.jpg")
                    .header("authorization", format!("Bearer {TEST_TOKEN}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), 404);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        application.shutdown().await.unwrap();
    }
    #[test]
    fn corrupt_or_unsafe_authentication_storage_fails_closed() {
        let fixture = Fixture::new();
        drop(Access::open(&fixture.config).unwrap());
        fs::write(
            fixture.config.state_directory.join("access.sqlite"),
            "corrupt",
        )
        .unwrap();
        assert!(Access::open(&fixture.config).is_err());
        fs::remove_file(fixture.config.state_directory.join("access.sqlite")).unwrap();
        std::os::unix::fs::symlink(
            fixture.base.join("outside"),
            fixture.config.state_directory.join("access.sqlite"),
        )
        .unwrap();
        assert!(Access::open(&fixture.config).is_err());
        assert!(!fixture.base.join("outside").exists());
    }
}
