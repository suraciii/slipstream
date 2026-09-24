//! Fixed production Photo PID 1. It runs the pinned `development-tiff`
//! adapter inside the isolated attempt boundary and reports one bounded
//! terminal outcome. It must never run as a host process.

use sha2::{Digest, Sha256};
use slipstream_processing::{photo::ICC_ASSET_SHA256, protocol::{Outcome, now}};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// The bundle-pinned ICC asset bytes shipped inside the worker image. The
/// output contract pins these asset bytes and the exact embedded profile
/// bytes separately (`design/development-color.md`).
const ICC_ASSET: &str = "/opt/slipstream-photo/icc/LargeRGB-elle-V2-g10.icc";
const ADAPTER: &str = "/opt/slipstream-photo/adapter.py";
const RESULT_BYTES: usize = 4096;

fn bad(message: &str) -> io::Error {
    io::Error::other(message)
}

fn deadline_timer(deadline_ms: u64) -> std::io::Result<()> {
    extern "C" fn expired(_: libc::c_int) {
        // SAFETY: _exit is async-signal-safe. PID namespace teardown settles all descendants.
        unsafe {
            libc::_exit(76);
        }
    }
    // SAFETY: these C POD structures are initialized before their respective syscalls.
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = expired as *const () as usize;
        libc::sigemptyset(&mut action.sa_mask);
        if libc::sigaction(libc::SIGALRM, &action, std::ptr::null_mut()) != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut event: libc::sigevent = std::mem::zeroed();
        event.sigev_notify = libc::SIGEV_SIGNAL;
        event.sigev_signo = libc::SIGALRM;
        let mut timer: libc::timer_t = std::mem::zeroed();
        if libc::timer_create(libc::CLOCK_REALTIME, &mut event, &mut timer) != 0 {
            return Err(io::Error::last_os_error());
        }
        let setting = libc::itimerspec {
            it_interval: libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            },
            it_value: libc::timespec {
                tv_sec: (deadline_ms / 1000) as _,
                tv_nsec: ((deadline_ms % 1000) * 1_000_000) as _,
            },
        };
        if libc::timer_settime(timer, 0, &setting, std::ptr::null_mut()) != 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// The armed absolute deadline is the one attempt termination authority.
fn ensure_before_deadline(deadline: u64) -> io::Result<()> {
    if now().map_err(|_| bad("clock"))? >= deadline {
        // As PID 1, exiting also terminates descendants even if timer
        // delivery has not occurred yet.
        std::process::exit(76);
    }
    Ok(())
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// Read and verify the release gate. No engine work happens before the
/// launcher released the verified attempt boundary.
fn gate(token: &str) -> io::Result<()> {
    let mut bytes = Vec::new();
    File::open("/control/gate")?
        .take(256)
        .read_to_end(&mut bytes)?;
    let text = String::from_utf8(bytes).map_err(|_| bad("gate token is not UTF-8"))?;
    if text.trim_end_matches('\n') != token {
        return Err(bad("gate token does not match the launch identity"));
    }
    Ok(())
}

/// The launcher-written engine grant. It is the only carrier of the guarded
/// recipe payload; the service request never reaches the worker directly.
fn grant(launch: &str) -> io::Result<i64> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct DevelopmentGrant {
        version: u8,
        kind: String,
        launch_id: String,
        workload: String,
        exposure_milli_ev: i64,
        white_balance_mode: String,
        profile_id: String,
        icc_asset_sha256: String,
    }
    let path = Path::new("/control/grant.json");
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != 0
        || metadata.nlink() != 1
        || metadata.mode() & 0o777 != 0o444
        || metadata.len() > 16 * 1024
    {
        return Err(bad("engine grant identity is invalid"));
    }
    let mut bytes = Vec::new();
    file.take(16 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 16 * 1024 {
        return Err(bad("engine grant exceeds the bounded frame"));
    }
    let value: DevelopmentGrant =
        serde_json::from_slice(&bytes).map_err(|_| bad("engine grant is not parseable"))?;
    if value.version != 1
        || value.kind != "photo-development-grant"
        || value.launch_id != launch
        || value.workload != "development-tiff"
        || value.white_balance_mode != "as-shot"
        || !(0..=1000).contains(&value.exposure_milli_ev)
        || value.profile_id.is_empty()
        || value.profile_id.len() > 64
        || value.icc_asset_sha256 != ICC_ASSET_SHA256
    {
        return Err(bad("engine grant does not match the pinned bundle"));
    }
    Ok(value.exposure_milli_ev)
}

fn prepare_work() -> io::Result<()> {
    for name in [
        "config",
        "config/color",
        "config/color/out",
        "cache",
        "tmp",
        "output",
        "baseline",
        "xdg",
    ] {
        let path = PathBuf::from("/work").join(name);
        fs::create_dir_all(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    }
    let metadata = fs::metadata(ICC_ASSET)?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(bad("pinned ICC asset is missing"));
    }
    if sha256_file(Path::new(ICC_ASSET))? != ICC_ASSET_SHA256 {
        return Err(bad("pinned ICC asset identity mismatch"));
    }
    fs::copy(ICC_ASSET, "/work/config/color/out/linear-prophoto.icc")?;
    let mut permissions = fs::metadata("/work/config/color/out/linear-prophoto.icc")?
        .permissions();
    permissions.set_mode(0o444);
    fs::set_permissions("/work/config/color/out/linear-prophoto.icc", permissions)?;
    Ok(())
}

/// Exactly one regular, read-only source file may be mounted at /input.
fn input_name() -> io::Result<String> {
    let mut found = None;
    for entry in fs::read_dir("/input")? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if !metadata.is_file() {
            return Err(bad("unexpected non-regular input entry"));
        }
        if metadata.mode() & 0o222 != 0 {
            return Err(bad("input is not sealed read-only"));
        }
        if found.replace(entry.file_name()).is_some() {
            return Err(bad("more than one staged input"));
        }
    }
    found
        .and_then(|name| name.into_string().ok())
        .ok_or_else(|| bad("no staged input"))
}

fn finish(mut file: &File, launch_id: &str, outcome: Outcome) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(&serde_json::json!({
        "launch_id": launch_id,
        "outcome": outcome,
    }))?;
    if bytes.len() > RESULT_BYTES {
        return Err(bad("result exceeds the bounded frame"));
    }
    bytes.resize(RESULT_BYTES, 0);
    file.write_all(&bytes)?;
    file.sync_all()
}

fn run(launch: &str, deadline: u64) -> io::Result<()> {
    gate(launch)?;
    let exposure_milli_ev = grant(launch)?;
    prepare_work()?;
    let input = input_name()?;
    ensure_before_deadline(deadline)?;
    // One fixed adapter invocation. No shell, no caller-controlled argv,
    // no engine choice: the pinned bundle owns the exact engine command.
    let status = Command::new("python3")
        .arg(ADAPTER)
        .arg("--input")
        .arg(format!("/input/{input}"))
        .arg("--exposure-milli-ev")
        .arg(exposure_milli_ev.to_string())
        .arg("--work")
        .arg("/work")
        .arg("--output")
        .arg("/work/output/development.tif")
        .arg("--icc-asset")
        .arg(ICC_ASSET)
        .env_clear()
        .env("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")
        .env("TMPDIR", "/work/tmp")
        .env("XDG_CONFIG_HOME", "/work/xdg")
        .env("XDG_CACHE_HOME", "/work/cache")
        .env("HOME", "/work/xdg")
        .stdin(Stdio::null())
        .status()?;
    if !status.success() {
        return Err(bad("pinned engine adapter failed"));
    }
    let output = fs::metadata("/work/output/development.tif")?;
    if !output.is_file() || output.len() == 0 {
        return Err(bad("adapter produced no output"));
    }
    ensure_before_deadline(deadline)?;
    Ok(())
}

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    // SAFETY: getpid reads the current namespace identity. This worker must never run as a host process.
    if unsafe { libc::getpid() } != 1 {
        std::process::exit(70);
    }
    if args.len() != 3
        || args[0] != "development-tiff"
        || args[1].len() != 32
        || !args[1]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        std::process::exit(70);
    }
    let deadline = args[2]
        .parse::<u64>()
        .unwrap_or_else(|_| std::process::exit(70));
    if deadline_timer(deadline).is_err() {
        std::process::exit(70);
    }
    // SAFETY: PR_SET_DUMPABLE removes same-UID process-interface access.
    if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } != 0 {
        std::process::exit(70);
    }
    let launch = args[1].clone();
    let result = (|| {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open("/work/result")?;
        match run(&launch, deadline) {
            Ok(()) => finish(&file, &launch, Outcome::Completed),
            Err(_) => finish(&file, &launch, Outcome::EngineFailed),
        }
    })();
    if result.is_err() {
        std::process::exit(75);
    }
}
