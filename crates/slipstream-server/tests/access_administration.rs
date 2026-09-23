use slipstream_server::{Config, start_server};
use std::{
    fs,
    io::Read,
    os::fd::FromRawFd,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!(
            "slipstream-access-admin-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&base).unwrap();
        fs::create_dir(base.join("originals")).unwrap();
        fs::create_dir(base.join("web")).unwrap();
        fs::write(base.join("web/index.html"), "shell").unwrap();
        Self(base)
    }
    fn command(&self, operation: &str) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_slipstream-server"));
        command
            .arg(operation)
            .env("SLIPSTREAM_LIBRARY_ROOT", self.0.join("originals"))
            .env("SLIPSTREAM_STATE_DIRECTORY", self.0.join("state"))
            .env("SLIPSTREAM_CACHE_DIRECTORY", self.0.join("cache"));
        command
    }
    fn terminal(&self, operation: &str) -> (bool, String) {
        let mut master = -1;
        let mut slave = -1;
        // SAFETY: openpty initializes both descriptors; each is owned once below.
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        let mut master = unsafe { fs::File::from_raw_fd(master) };
        let slave = unsafe { fs::File::from_raw_fd(slave) };
        let mut command = self.command(operation);
        command
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::null());
        let status = command.status().unwrap();
        drop(command);
        drop(slave);
        let mut output = String::new();
        // Linux PTYs return EIO after the slave closes, after delivering its bytes.
        let _ = master.read_to_string(&mut output);
        (status.success(), output.trim().to_owned())
    }
    fn config(&self) -> Config {
        Config {
            library_root: self.0.join("originals"),
            state_directory: self.0.join("state"),
            cache_directory: self.0.join("cache"),
            database_basename: "library.sqlite".into(),
            host: "127.0.0.1".into(),
            port: 0,
            public_origin: "https://photos.example.test".into(),
            web_root: Some(self.0.join("web")),
            processing: None,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn read_records(base: &Path) -> serde_json::Value {
    let database = rusqlite::Connection::open(base.join("state/access.sqlite")).unwrap();
    let value: String = database
        .query_row("SELECT records FROM access WHERE id=1", [], |r| r.get(0))
        .unwrap();
    serde_json::from_str(&value).unwrap()
}
#[tokio::test]
async fn offline_commands_require_terminal_lock_and_preserve_library() {
    let fixture = Fixture::new();
    fs::write(
        fixture.0.join("originals/precious.ARW"),
        "camera-owned-original",
    )
    .unwrap();
    let piped = fixture.command("access-create").output().unwrap();
    assert!(!piped.status.success());
    assert!(piped.stdout.is_empty());
    assert!(!fixture.0.join("state").exists());
    let (ok, first) = fixture.terminal("access-create");
    assert!(ok);
    assert_eq!(first.len(), 43);
    let records = read_records(&fixture.0);
    assert!(records["credential"].is_object());
    assert!(!serde_json::to_string(&records).unwrap().contains(&first));
    assert!(!fixture.terminal("access-create").0);
    let server = start_server(fixture.config()).await.unwrap();
    let before = fs::read(fixture.0.join("state/access.sqlite")).unwrap();
    let blocked = fixture.command("access-revoke").output().unwrap();
    assert!(!blocked.status.success());
    assert!(blocked.stdout.is_empty());
    assert_eq!(
        fs::read(fixture.0.join("state/access.sqlite")).unwrap(),
        before
    );
    server.close().await.unwrap();
    let library_before = fs::read(fixture.0.join("state/library.sqlite")).unwrap();
    let (ok, rotated) = fixture.terminal("access-rotate");
    assert!(ok);
    assert_eq!(rotated.len(), 43);
    assert_ne!(first, rotated);
    assert_ne!(
        records["credential"]["generation"],
        read_records(&fixture.0)["credential"]["generation"]
    );
    assert!(
        fixture
            .command("access-revoke")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(read_records(&fixture.0)["credential"].is_null());
    assert!(
        fixture
            .command("access-revoke")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(
        fs::read(fixture.0.join("state/library.sqlite")).unwrap(),
        library_before
    );
    assert_eq!(
        fs::read_to_string(fixture.0.join("originals/precious.ARW")).unwrap(),
        "camera-owned-original"
    );
    assert!(fixture.terminal("access-rotate").0);
}
