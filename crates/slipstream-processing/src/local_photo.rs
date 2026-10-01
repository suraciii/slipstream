//! Local single-container Photo development.
//!
//! Public synchronous execution of the `development-tiff` workload directly
//! in the caller's container: the pinned native engine runs under the
//! application-owned process-group supervisor, with no shell, sidecar service,
//! worker PID 1, transport socket, or Library/HTTP dependency. The caller
//! owns a clean private work directory and serializes admission; every
//! engine-private path is derived below that directory, so concurrent callers
//! never share ambient state. The fixed Film stage remains unavailable locally.
//!
//! One run is bounded by the caller's cancellation flag and timeout: the
//! supervisor and engine run in their own process group, a per-call watchdog
//! kills the whole group on expiry, and parent death signals the supervisor,
//! which kills the group.

use crate::{native_development, photo_tiff};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{self, Read},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// Validated identity of one locally executed Development TIFF.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutputIdentity {
    pub size: u64,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
}

/// The bundle-pinned ICC asset bytes shipped with the application image.
/// The staged profile must hash to these bytes, and the server's installed
/// bundle check pins the same identity. The output contract pins these
/// asset bytes and the exact embedded profile bytes separately
/// (`design/development-color.md`).
pub const ICC_ASSET_SHA256: &str =
    "df7b2c677645f1ca5364b52e62f8db04ca61f80163792942f3e409a84a6b12ed";
/// Matches the Export artifact limit of the application seams; the
/// validated output can never declare more bytes than they accept.
const MAX_OUTPUT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// The bundle's ICC output profile asset bound, checked before spawn.
const PROFILE_BYTES_MAX: u64 = 16 * 1024 * 1024;
/// The staged name of the output profile inside the work directory.
const STAGED_PROFILE: &str = "config/color/out/linear-prophoto.icc";
/// The engine client identity of local execution.
const CLIENT: &str = "slipstream-local-photo";

fn cancelled() -> io::Error {
    io::Error::other("local development was cancelled")
}

/// Replace an engine error with its termination cause when the run ended
/// by authority instead of by protocol failure.
fn terminated(error: io::Error, cancellation: &AtomicBool, deadline: Instant) -> io::Error {
    if cancellation.load(Ordering::Relaxed) {
        return cancelled();
    }
    if Instant::now() >= deadline {
        return io::Error::other("local development timed out");
    }
    error
}

/// Derive the engine-private tree below the caller's work directory and
/// stage the validated output profile into it. Only the fixed skeleton is
/// created; nothing outside `work` is written.
fn prepare(work: &Path, profile: &Path) -> io::Result<PathBuf> {
    for directory in ["config/color/out", "cache", "tmp", "xdg"] {
        let path = work.join(directory);
        fs::create_dir_all(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    }
    let mut asset = File::open(profile)?;
    let metadata = asset.metadata()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > PROFILE_BYTES_MAX {
        return Err(io::Error::other(
            "output profile asset is missing or exceeds the bundle bound",
        ));
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = asset.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    if format!("{:x}", hasher.finalize()) != ICC_ASSET_SHA256 {
        return Err(io::Error::other("output profile identity mismatch"));
    }
    let staged = work.join(STAGED_PROFILE);
    fs::copy(profile, &staged)?;
    let mut permissions = fs::metadata(&staged)?.permissions();
    permissions.set_mode(0o444);
    fs::set_permissions(&staged, permissions)?;
    Ok(staged)
}

/// Execute one local `development-tiff` run over the caller-owned paths and
/// return the validated identity of the written output.
///
/// `engine` is the pinned `darktable-mcp` binary, `metadata` its bundle
/// engine-metadata contract, `profile` the bundle ICC output profile asset,
/// and `work` a clean private directory this call may own for the run; the
/// output is written to `output` and validated as a Development TIFF before
/// its identity is returned. The run is bounded by `cancellation` and
/// `timeout`: either kills the whole engine process group and returns an
/// error naming the cause.
#[allow(clippy::too_many_arguments)]
pub fn develop(
    engine: &Path,
    metadata: &Path,
    profile: &Path,
    work: &Path,
    input: &Path,
    output: &Path,
    exposure_milli_ev: i64,
    cancellation: Arc<AtomicBool>,
    timeout: Duration,
) -> io::Result<OutputIdentity> {
    if timeout.is_zero() {
        return Err(io::Error::other("local development requires a timeout"));
    }
    if cancellation.load(Ordering::Relaxed) {
        return Err(cancelled());
    }
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| io::Error::other("local development deadline overflow"))?;
    let staged = prepare(work, profile)?;
    if cancellation.load(Ordering::Relaxed) {
        return Err(cancelled());
    }
    native_development::develop_at(
        engine,
        metadata,
        &staged,
        work,
        input,
        output,
        exposure_milli_ev,
        CLIENT,
        Some(native_development::Guard {
            cancellation: cancellation.clone(),
            deadline,
        }),
    )
    .map_err(|error| terminated(error, &cancellation, deadline))?;
    let identity = photo_tiff::validate(output, MAX_OUTPUT_BYTES)
        .map_err(|code| io::Error::other(format!("development artifact rejected: {code:?}")))?;
    Ok(OutputIdentity {
        size: identity.size,
        sha256: identity.sha256,
        width: identity.width,
        height: identity.height,
    })
}
