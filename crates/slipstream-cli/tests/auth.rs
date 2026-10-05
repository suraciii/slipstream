#[allow(dead_code)]
mod common;
use std::{
    fs,
    io::{Read, Write},
    net::TcpListener,
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    thread,
};

use clap::Parser;
use serde_json::{Value, json};
use slipstream_cli::{AuthCommand, Cli, Command};

static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

fn capabilities() -> Value {
    common::capabilities_body()
}

fn status() -> Value {
    json!({
        "serverVersion": "0.0.0",
        "cliContractVersion": 1,
        "published": true,
        "publication": "publication",
        "photoCount": 0,
        "scan": {
            "state": "idle",
            "publication": "publication",
            "completed": 0,
            "total": 0,
            "lastRecovery": null,
            "fingerprints": null
        }
    })
}

fn write_json(stream: &mut impl Write, body: &Value) {
    let bytes = serde_json::to_vec(body).unwrap();
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        bytes.len()
    )
    .unwrap();
    stream.write_all(&bytes).unwrap();
}

fn fake_service() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        for request_index in 0..4 {
            let (mut stream, _) = common::accept_tls(&listener);
            let mut request = [0_u8; 8192];
            let count = stream.read(&mut request).unwrap();
            common::assert_bearer(&request[..count]);
            let body = if request_index == 3 {
                status()
            } else {
                capabilities()
            };
            write_json(&mut stream, &body);
        }
    });
    (format!("https://127.0.0.1:{}", address.port()), handle)
}
fn fake_status_service() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        for request_index in 0..2 {
            let (mut stream, _) = common::accept_tls(&listener);
            let mut request = [0_u8; 8192];
            let count = stream.read(&mut request).unwrap();
            common::assert_bearer(&request[..count]);
            let body = if request_index == 1 {
                status()
            } else {
                capabilities()
            };
            write_json(&mut stream, &body);
        }
    });
    (format!("https://127.0.0.1:{}", address.port()), handle)
}

#[test]
fn auth_parser_keeps_login_token_source_local() {
    let login = Cli::try_parse_from([
        "slipstream",
        "--server",
        "https://example.test:443",
        "auth",
        "login",
        "--token-stdin",
    ])
    .unwrap();
    assert!(matches!(
        login.command,
        Command::Auth {
            command: AuthCommand::Login(_)
        }
    ));
    assert!(
        Cli::try_parse_from([
            "slipstream",
            "--token-file",
            "token",
            "auth",
            "login",
            "--token-stdin",
        ])
        .is_ok()
    );
}

#[test]
fn login_persists_origin_credential_and_reuses_it_for_status() {
    let base = std::env::temp_dir().join(format!(
        "slipstream-cli-auth-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&base).unwrap();
    let (server, service) = fake_service();
    let token = common::ACCESS_TOKEN;

    let mut login = common::cli_command();
    login
        .env_remove("SLIPSTREAM_SERVER_URL")
        .env_remove("SLIPSTREAM_ACCESS_TOKEN_FILE")
        .env("XDG_CONFIG_HOME", &base)
        .env("SLIPSTREAM_DISABLE_KEYRING", "1")
        .arg("--server")
        .arg(&server)
        .args(["auth", "login", "--token-stdin"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let mut child = login.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{token}\n").as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "login failed: {:?}", output);
    assert!(!String::from_utf8_lossy(&output.stdout).contains(token));

    let status = common::cli_command()
        .env_remove("SLIPSTREAM_SERVER_URL")
        .env_remove("SLIPSTREAM_ACCESS_TOKEN_FILE")
        .env("XDG_CONFIG_HOME", &base)
        .env("SLIPSTREAM_DISABLE_KEYRING", "1")
        .args(["auth", "status"])
        .output()
        .unwrap();
    assert!(status.status.success(), "auth status failed: {:?}", status);
    let status_json: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(
        status_json["data"]["entries"][0]["origin"],
        format!("{server}/")
    );
    assert_eq!(status_json["data"]["entries"][0]["storage"], "file");
    assert!(!String::from_utf8_lossy(&status.stdout).contains(token));

    let selected = common::cli_command()
        .env_remove("SLIPSTREAM_SERVER_URL")
        .env_remove("SLIPSTREAM_ACCESS_TOKEN_FILE")
        .env("XDG_CONFIG_HOME", &base)
        .args(["--server", &server, "auth", "use"])
        .output()
        .unwrap();
    assert!(selected.status.success(), "auth use failed: {:?}", selected);
    let selected_json: Value = serde_json::from_slice(&selected.stdout).unwrap();
    assert_eq!(selected_json["data"]["origin"], format!("{server}/"));
    assert_eq!(selected_json["data"]["active"], true);

    let ordinary = common::cli_command()
        .env_remove("SLIPSTREAM_SERVER_URL")
        .env_remove("SLIPSTREAM_ACCESS_TOKEN_FILE")
        .env("XDG_CONFIG_HOME", &base)
        .env("SLIPSTREAM_DISABLE_KEYRING", "1")
        .arg("status")
        .output()
        .unwrap();
    assert!(
        ordinary.status.success(),
        "ordinary status failed: {:?}",
        ordinary
    );
    assert!(!String::from_utf8_lossy(&ordinary.stdout).contains(token));

    let logout = common::cli_command()
        .env_remove("SLIPSTREAM_SERVER_URL")
        .env_remove("SLIPSTREAM_ACCESS_TOKEN_FILE")
        .env("XDG_CONFIG_HOME", &base)
        .env("SLIPSTREAM_DISABLE_KEYRING", "1")
        .args(["--server", &server, "auth", "logout"])
        .output()
        .unwrap();
    assert!(logout.status.success(), "auth logout failed: {:?}", logout);
    let logout_json: Value = serde_json::from_slice(&logout.stdout).unwrap();
    assert_eq!(logout_json["data"]["removed"], true);
    assert_eq!(logout_json["data"]["remoteRevoked"], false);

    service.join().unwrap();
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn explicit_token_file_works_when_auth_store_is_unreadable() {
    let base = std::env::temp_dir().join(format!(
        "slipstream-cli-auth-store-failure-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(base.join("slipstream")).unwrap();
    fs::write(base.join("slipstream/auth.json"), b"not-json").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            base.join("slipstream/auth.json"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    let (server, service) = fake_status_service();
    let output = common::cli_command()
        .env_remove("SLIPSTREAM_SERVER_URL")
        .env_remove("SLIPSTREAM_ACCESS_TOKEN_FILE")
        .env("XDG_CONFIG_HOME", &base)
        .env("SLIPSTREAM_DISABLE_KEYRING", "1")
        .args([
            "--server",
            &server,
            "--token-file",
            common::credential_file().to_str().unwrap(),
            "status",
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "status failed: {:?}", output);
    assert!(!String::from_utf8_lossy(&output.stdout).contains(common::ACCESS_TOKEN));
    service.join().unwrap();
    let (environment_server, environment_service) = fake_status_service();
    let environment_output = common::cli_command()
        .env("SLIPSTREAM_SERVER_URL", &environment_server)
        .env("SLIPSTREAM_ACCESS_TOKEN_FILE", common::credential_file())
        .env("XDG_CONFIG_HOME", &base)
        .arg("status")
        .output()
        .unwrap();
    assert!(
        environment_output.status.success(),
        "environment status failed: {:?}",
        environment_output
    );
    environment_service.join().unwrap();
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn auth_login_rejects_the_global_token_file_override() {
    let base = std::env::temp_dir().join(format!(
        "slipstream-cli-auth-global-token-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&base).unwrap();
    let output = common::cli_command()
        .env_remove("SLIPSTREAM_SERVER_URL")
        .env_remove("SLIPSTREAM_ACCESS_TOKEN_FILE")
        .env("XDG_CONFIG_HOME", &base)
        .env("SLIPSTREAM_DISABLE_KEYRING", "1")
        .args([
            "--server",
            "https://example.test",
            "--token-file",
            common::credential_file().to_str().unwrap(),
            "auth",
            "login",
            "--token-stdin",
        ])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "unexpected result: {:?}",
        output
    );
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["code"], "invalid_input");
    assert_eq!(error["error"]["details"]["argument"], "token-file");
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn auth_status_reports_stored_credential_read_failures() {
    let base = std::env::temp_dir().join(format!(
        "slipstream-cli-auth-status-failure-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
    let store_directory = base.join("slipstream");
    fs::create_dir_all(&store_directory).unwrap();
    fs::write(
        store_directory.join("auth.json"),
        serde_json::to_vec_pretty(&json!({
            "version": 1,
            "activeOrigin": "https://example.test/",
            "entries": {
                "https://example.test/": {
                    "storage": "file",
                    "token": "invalid",
                    "createdAt": "2026-01-01T00:00:00Z",
                    "lastLoginAt": "2026-01-01T00:00:00Z"
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            store_directory.join("auth.json"),
            fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }

    let output = common::cli_command()
        .env_remove("SLIPSTREAM_SERVER_URL")
        .env_remove("SLIPSTREAM_ACCESS_TOKEN_FILE")
        .env("XDG_CONFIG_HOME", &base)
        .env("SLIPSTREAM_DISABLE_KEYRING", "1")
        .args(["auth", "status"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(6),
        "unexpected result: {:?}",
        output
    );
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["details"]["operation"], "auth-status");
    fs::remove_dir_all(base).unwrap();
}

#[cfg(unix)]
#[test]
fn failed_keyring_logout_restores_credential_and_preserves_metadata() {
    use std::os::unix::fs::PermissionsExt;

    let base = std::env::temp_dir().join(format!(
        "slipstream-cli-auth-keyring-{}-{}",
        std::process::id(),
        NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(base.join("slipstream")).unwrap();
    fs::create_dir_all(base.join("bin")).unwrap();
    let credential = base.join("credential");
    fs::write(&credential, common::ACCESS_TOKEN).unwrap();
    let helper = base.join("bin/secret-tool");
    fs::write(
        &helper,
        concat!(
            "#!/bin/sh\n",
            "case \"$1\" in\n",
            "lookup) cat \"$AUTH_TEST_CREDENTIAL\" ;;\n",
            "store) cat > \"$AUTH_TEST_CREDENTIAL\" ;;\n",
            "clear) rm -f \"$AUTH_TEST_CREDENTIAL\"; exit 1 ;;\n",
            "--version) exit 0 ;;\n",
            "*) exit 1 ;;\n",
            "esac\n",
        ),
    )
    .unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
    let store_path = base.join("slipstream/auth.json");
    let original = serde_json::to_vec(&json!({
        "version": 1,
        "activeOrigin": "https://example.test/",
        "entries": {
            "https://example.test/": {
                "storage": "keyring",
                "credentialId": "test-credential",
                "token": null,
                "createdAt": "2026-01-01T00:00:00Z",
                "lastLoginAt": "2026-01-01T00:00:00Z"
            }
        }
    }))
    .unwrap();
    fs::write(&store_path, &original).unwrap();
    fs::set_permissions(&store_path, fs::Permissions::from_mode(0o600)).unwrap();
    let helper_path = format!("{}:/usr/bin:/bin", base.join("bin").display());
    let output = common::cli_command()
        .env_remove("SLIPSTREAM_DISABLE_KEYRING")
        .env_remove("SLIPSTREAM_SERVER_URL")
        .env_remove("SLIPSTREAM_ACCESS_TOKEN_FILE")
        .env("PATH", helper_path)
        .env("AUTH_TEST_CREDENTIAL", &credential)
        .env("XDG_CONFIG_HOME", &base)
        .args(["auth", "logout"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(6));
    let error: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(error["error"]["details"]["operation"], "auth-logout");
    assert_eq!(fs::read(&store_path).unwrap(), original);
    assert_eq!(
        fs::read_to_string(&credential).unwrap(),
        common::ACCESS_TOKEN
    );
    assert!(!String::from_utf8_lossy(&output.stdout).contains(common::ACCESS_TOKEN));
    fs::remove_dir_all(base).unwrap();
}
