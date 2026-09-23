use rusqlite::Connection;
use rustls::{ServerConfig, ServerConnection};
use sha2::Digest;
use slipstream_server::{Config, RunningServer, start_server};
use std::{
    fs::{self, OpenOptions},
    io::{self, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::Command,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    thread,
    time::Duration,
};

pub const ACCESS_TOKEN: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const GENERATION: &str = "EEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEE";
const TEST_CA: &[u8] = include_bytes!("../../../../tools/test-tls/cert.pem");
const TEST_CERT: &[u8] = include_bytes!("../../../../tools/test-tls/server-cert.pem");
const TEST_KEY: &[u8] = include_bytes!("../../../../tools/test-tls/server-key.pem");

static NEXT_CREDENTIAL: AtomicU64 = AtomicU64::new(0);
static CREDENTIAL_FILE: OnceLock<PathBuf> = OnceLock::new();
static TLS_CONFIG: OnceLock<Arc<ServerConfig>> = OnceLock::new();

pub fn credential_file() -> &'static PathBuf {
    CREDENTIAL_FILE.get_or_init(|| {
        loop {
            let sequence = NEXT_CREDENTIAL.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "slipstream-cli-credential-{}-{sequence}",
                std::process::id()
            ));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(mut file) => {
                    file.write_all(ACCESS_TOKEN.as_bytes()).unwrap();
                    return path;
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create CLI test credential: {error}"),
            }
        }
    })
}

pub fn cli_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_slipstream"));
    command.env("SSL_CERT_FILE", test_ca_path());
    command
}

pub fn test_ca_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/test-tls/cert.pem")
}

pub fn test_certificate() -> reqwest::Certificate {
    reqwest::Certificate::from_pem(TEST_CA).expect("test CA certificate is valid PEM")
}

fn tls_config() -> Arc<ServerConfig> {
    Arc::clone(TLS_CONFIG.get_or_init(|| {
        let certificates = rustls_pemfile::certs(&mut BufReader::new(TEST_CERT))
            .collect::<Result<Vec<_>, _>>()
            .expect("test server certificate is valid PEM");
        let key = rustls_pemfile::private_key(&mut BufReader::new(TEST_KEY))
            .expect("test server key is valid PEM")
            .expect("test server key is present");
        Arc::new(
            ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(certificates, key)
                .expect("test certificate and key match"),
        )
    }))
}

pub fn accept_tls(
    listener: &TcpListener,
) -> (
    rustls::StreamOwned<ServerConnection, TcpStream>,
    std::net::SocketAddr,
) {
    let (stream, address) = listener.accept().unwrap();
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    (tls_stream(stream), address)
}

pub fn tls_stream(stream: TcpStream) -> rustls::StreamOwned<ServerConnection, TcpStream> {
    let connection = ServerConnection::new(tls_config()).expect("create TLS server connection");
    rustls::StreamOwned::new(connection, stream)
}

pub fn assert_bearer(request: &[u8]) {
    let request = String::from_utf8_lossy(request).to_ascii_lowercase();
    let expected = format!("authorization: bearer {ACCESS_TOKEN}").to_ascii_lowercase();
    assert!(
        request.contains(&expected),
        "CLI request did not carry the configured bearer credential"
    );
}

pub async fn start_authenticated_server(config: Config) -> RunningServer {
    start_authenticated_server_with_upstream(config).await.0
}

pub async fn start_authenticated_server_with_upstream(
    mut config: Config,
) -> (RunningServer, String) {
    config.public_origin = "https://localhost".to_owned();
    seed_access(&config);
    let mut server = start_server(config)
        .await
        .expect("start test Slipstream service");
    let upstream = server.url.clone();
    server.url = tls_proxy(&server.url);
    (server, upstream)
}

fn seed_access(config: &Config) {
    fs::create_dir_all(&config.state_directory).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&config.state_directory, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let connection = Connection::open(config.state_directory.join("access.sqlite")).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE access (id INTEGER PRIMARY KEY CHECK(id=1), records TEXT NOT NULL);",
        )
        .unwrap();
    let digest: [u8; 32] = sha2::Sha256::digest(ACCESS_TOKEN.as_bytes()).into();
    let records = serde_json::json!({
        "credential": { "digest": digest, "generation": GENERATION },
        "sessions": []
    });
    connection
        .execute(
            "INSERT INTO access (id, records) VALUES (1, ?1)",
            [records.to_string()],
        )
        .unwrap();
    drop(connection);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            config.state_directory.join("access.sqlite"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
}

pub fn tls_proxy(upstream: &str) -> String {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let upstream = upstream
        .strip_prefix("http://")
        .unwrap_or(upstream)
        .to_owned();
    let tls_config = tls_config();
    thread::spawn(move || {
        while let Ok((stream, _)) = listener.accept() {
            let config = Arc::clone(&tls_config);
            let upstream = upstream.clone();
            thread::spawn(move || proxy_one(stream, config, &upstream));
        }
    });
    format!("https://127.0.0.1:{}", address.port())
}

fn proxy_one(stream: TcpStream, config: Arc<ServerConfig>, upstream: &str) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let Ok(connection) = ServerConnection::new(config) else {
        return;
    };
    let mut client = rustls::StreamOwned::new(connection, stream);
    let Some((head, body)) = read_http_message(&mut client) else {
        return;
    };
    let Ok(mut service) = TcpStream::connect(upstream) else {
        return;
    };
    let _ = service.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = service.set_write_timeout(Some(Duration::from_secs(10)));
    let mut request = close_connection_request(&head);
    request.extend_from_slice(&body);
    if service.write_all(&request).is_err() {
        return;
    }
    let mut response = Vec::new();
    if service.read_to_end(&mut response).is_ok() {
        let _ = client.write_all(&response);
        let _ = client.flush();
    }
}

pub fn read_http_message(stream: &mut impl Read) -> Option<(String, Vec<u8>)> {
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    loop {
        stream.read_exact(&mut byte).ok()?;
        head.push(byte[0]);
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
        if head.len() > 16 * 1024 {
            return None;
        }
    }
    let head = String::from_utf8(head).ok()?;
    let length = head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0);
    if length > 1024 * 1024 {
        return None;
    }
    let mut body = vec![0_u8; length];
    stream.read_exact(&mut body).ok()?;
    Some((head, body))
}

fn close_connection_request(head: &str) -> Vec<u8> {
    let head = head.strip_suffix("\r\n\r\n").unwrap_or(head);
    let mut request = head
        .lines()
        .filter(|line| !line.to_ascii_lowercase().starts_with("connection:"))
        .collect::<Vec<_>>()
        .join("\r\n");
    request.push_str("\r\nConnection: close\r\n\r\n");
    request.into_bytes()
}

#[allow(dead_code)]
pub fn stalled_tls_service() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (mut stream, _) = accept_tls(&listener);
        let mut request = [0_u8; 8192];
        let _ = stream.read(&mut request);
        thread::sleep(Duration::from_secs(2));
    });
    (format!("https://127.0.0.1:{}", address.port()), handle)
}
