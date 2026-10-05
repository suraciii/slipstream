//! Local standalone SpektraFilm execution (Issue #496).
//!
//! Public synchronous execution of an artifact-bound `spektrafilm` step
//! directly in the caller's container: the pinned `spektrafilm-rs` fork
//! runtime runs under the application-owned process-group supervisor, with no
//! shell, sidecar service, worker PID 1, transport socket, or Library/HTTP
//! dependency, and it never invokes darktable. The input is the retained
//! linear float32 ProPhoto TIFF handoff another peer exported; its closed
//! contract is re-validated here before the engine starts, and the module-owned
//! parameter tree is wrapped in the fork's versioned recipe contract.
//!
//! One run is bounded by the caller's cancellation flag and timeout: the
//! supervisor and engine run in their own process group, a per-call
//! watchdog kills the whole group on expiry, and parent death signals the
//! supervisor, which kills the group. The caller owns a clean private work
//! directory and serializes admission.

use crate::mcp_client::{Watchdog, supervisor_path};
use crate::modules::{self, Parameters};
use crate::{local_photo, local_preview, photo_jpeg, photo_tiff};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// The pinned runner's terminal refusal code: the attempt was rejected
/// before any engine work, exactly like the module boundary's structured
/// refusals.
const RUNNER_REFUSED: i32 = 71;
/// Matches the Export artifact limit of the application seams; the
/// validated output can never declare more bytes than they accept.
const MAX_OUTPUT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// The bounded size of one Preview rendition.
const PREVIEW_BYTES_MAX: usize = 16 * 1024 * 1024;
/// The bounded stderr tail one failure report carries.
const STDERR_TAIL_BYTES: usize = 4 * 1024;

fn cancelled() -> io::Error {
    io::Error::other("standalone film execution was cancelled")
}

fn confined(path: &Path) -> io::Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| io::Error::other("film attempt path is not valid UTF-8"))
}

/// Replace an engine error with its termination cause when the run ended
/// by authority instead of by protocol failure.
fn terminated(error: io::Error, cancellation: &AtomicBool, deadline: Instant) -> io::Error {
    if cancellation.load(Ordering::Relaxed) {
        return cancelled();
    }
    if Instant::now() >= deadline {
        return io::Error::other("standalone film execution timed out");
    }
    error
}

/// The film workspace geometry bounds, mirroring the pinned runner's own
/// `MAX_EDGE`, `MAX_DECODED_BYTES`, and `MAX_COORDINATE_SUM` admission.
pub fn geometry_bounded(width: u64, height: u64) -> bool {
    modules::film_geometry_bounded(width, height)
}

/// Build the fork runner's structured recipe below the caller's private
/// workspace. The fork receives the complete module-owned tree verbatim and
/// resolves its own profiles and image writer from the bundle data root.
fn prepare(
    work: &Path,
    parameters: &Parameters,
    film_profile: &str,
    print_profile: &str,
    output_format: &str,
    max_edge: Option<u32>,
) -> io::Result<PathBuf> {
    fs::create_dir_all(work)?;
    fs::set_permissions(work, fs::Permissions::from_mode(0o700))?;
    modules::validate_spektrafilm_parameters(parameters).map_err(|error| {
        io::Error::other(format!(
            "film parameters were refused: {:?} ({})",
            error.code, error.message
        ))
    })?;
    let output = serde_json::json!({
        "format": output_format,
        "precisionBits": 8,
        "colorSpace": "sRGB",
        "transferFunction": "srgb",
        "geometry": if output_format == "png" { "bounded" } else { "input-preserving" },
        "encoding": if output_format == "png" { "bounded-preview" } else { "quality-85-baseline" },
        "maxEdge": max_edge,
    });
    let recipe = serde_json::json!({
        "schemaVersion": "spektrafilm-rs-params-1",
        "filmProfile": film_profile,
        "printProfile": print_profile,
        "parameters": parameters.tree.clone(),
        "output": output,
        "seed": 0,
    });
    let recipe_path = work.join("recipe.json");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&recipe_path)?;
    let bytes = serde_json::to_vec(&recipe)
        .map_err(|error| io::Error::other(format!("recipe is not serializable: {error}")))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(recipe_path)
}

/// The deterministic environment for one fork attempt. The bundle owns the
/// executable and data tree; no host path or catalog is made visible.
fn environment(work: &Path, data_root: &Path, binary: &Path) -> io::Result<Vec<(String, String)>> {
    let value = |path: &Path| -> io::Result<String> {
        path.to_str()
            .map(str::to_owned)
            .ok_or_else(|| io::Error::other("film attempt path is not valid UTF-8"))
    };
    let library_root = binary
        .parent()
        .ok_or_else(|| io::Error::other("film binary has no bundle parent"))?
        .join("lib");
    Ok([
        ("HOME", value(&work.join("home"))?),
        ("TMPDIR", value(&work.join("tmp"))?),
        ("LD_LIBRARY_PATH", value(&library_root)?),
        ("SPEKTRAFILM_BACKEND", "cpu".to_owned()),
        ("SPEKTRAFILM_DATA_DIR", value(data_root)?),
        ("OMP_NUM_THREADS", "4".to_owned()),
        ("OPENBLAS_NUM_THREADS", "4".to_owned()),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_owned(), value))
    .collect())
}

/// The fork CLI argv for one structured render.
fn runner_arguments(
    input: &Path,
    output: &Path,
    recipe: &Path,
    data_root: &Path,
) -> io::Result<Vec<String>> {
    Ok(vec![
        "render".to_owned(),
        "--input".to_owned(),
        confined(input)?,
        "--recipe".to_owned(),
        confined(recipe)?,
        "--output".to_owned(),
        confined(output)?,
        "--data-dir".to_owned(),
        confined(data_root)?,
    ])
}

/// One supervised standalone fork attempt: the `spektrafilm-rs` binary runs
/// in its own process group behind the application supervisor, stdout is
/// discarded (the CLI writes files, not protocol), and a bounded stderr log
/// survives for the failure report. The watchdog kills the whole group on
/// cancellation or deadline; dropping the attempt reaps the group.
struct Attempt {
    child: std::process::Child,
    pgid: libc::pid_t,
    watchdog: Option<Watchdog>,
    stderr_path: PathBuf,
}

impl Attempt {
    fn start(
        engine: &Path,
        arguments: &[String],
        env: &[(String, String)],
        work: &Path,
        cancellation: Arc<AtomicBool>,
        deadline: Instant,
    ) -> io::Result<Self> {
        let supervisor = supervisor_path()
            .ok_or_else(|| io::Error::other("Photo process supervisor is unavailable"))?;
        let stderr_path = work.join("stderr.log");
        let stderr = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&stderr_path)?;
        let parent = unsafe { libc::getpid() };
        let mut command_args = Vec::with_capacity(arguments.len() + 2);
        command_args.push(parent.to_string());
        command_args.push(confined(engine)?);
        command_args.extend(arguments.iter().cloned());
        let mut command = Command::new(supervisor);
        command
            .args(&command_args)
            .env_clear()
            .envs(env.iter().map(|(key, value)| (key, value)))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::from(stderr));
        // SAFETY: the closure runs between fork and exec and only performs
        // single async-signal-safe POSIX calls.
        unsafe {
            command.pre_exec(move || {
                // SAFETY: see the surrounding unsafe block.
                if libc::setpgid(0, 0) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn()?;
        let pgid = child.id() as libc::pid_t;
        let mut attempt = Self {
            child,
            pgid,
            watchdog: None,
            stderr_path,
        };
        attempt.watchdog = match Watchdog::start(cancellation, deadline, pgid) {
            Ok(watchdog) => Some(watchdog),
            Err(error) => {
                attempt.kill_group();
                let _ = attempt.child.kill();
                let _ = attempt.child.wait();
                return Err(error);
            }
        };
        Ok(attempt)
    }

    /// Signal the whole private engine process group; descendants that
    /// outlived their leader remain in the group, so this reaches them.
    fn kill_group(&self) {
        // SAFETY: one group signal; the result is deliberately ignored.
        unsafe {
            libc::kill(-self.pgid, libc::SIGKILL);
        }
    }

    /// Wait for the attempt's terminal status and classify it. The attempt
    /// stays alive so its evidence log can be read; dropping it reaps the
    /// (already finished) group without leaving a zombie behind.
    fn finish(&mut self) -> io::Result<i32> {
        let status = self.child.wait()?;
        if let Some(watchdog) = self.watchdog.take() {
            watchdog.stop();
        }
        if let Some(code) = status.code() {
            return Ok(code);
        }
        // A signal termination without an exit code is either the group
        // authority or a crash; either way the attempt produced no result.
        self.kill_group();
        Err(io::Error::other(
            "standalone film runtime was terminated by a signal",
        ))
    }

    /// The bounded tail of the attempt's stderr log, for failure reports.
    fn stderr_tail(&self) -> String {
        let Ok(mut file) = File::open(&self.stderr_path) else {
            return String::new();
        };
        let mut bytes = Vec::new();
        let _ = Read::read_to_end(
            &mut (&mut file).take(STDERR_TAIL_BYTES as u64 + 1),
            &mut bytes,
        );
        let tail = if bytes.len() > STDERR_TAIL_BYTES {
            bytes.split_off(bytes.len() - STDERR_TAIL_BYTES)
        } else {
            bytes
        };
        String::from_utf8_lossy(&tail).trim().to_owned()
    }
}

impl Drop for Attempt {
    fn drop(&mut self) {
        if let Some(watchdog) = self.watchdog.take() {
            watchdog.stop();
        }
        self.kill_group();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Map one finished attempt's exit code onto the caller's result. The
/// pinned refusal, an engine failure, and an unexpected status are all
/// distinct errors carrying the bounded stderr evidence.
fn attempt_outcome(code: i32, attempt: &Attempt) -> io::Result<()> {
    match code {
        0 => Ok(()),
        RUNNER_REFUSED => Err(io::Error::other(format!(
            "the pinned film runtime refused the attempt: {}",
            attempt.stderr_tail()
        ))),
        code => Err(io::Error::other(format!(
            "the pinned film runtime failed (status {code}): {}",
            attempt.stderr_tail()
        ))),
    }
}

/// Validate the retained Development TIFF handoff the film stage consumes:
/// the closed float32 ProPhoto contract plus the film workspace geometry
/// bounds. This is the module boundary's own input admission; the pinned
/// runner re-verifies the same identity inside the attempt.
pub fn validate_input(path: &Path) -> io::Result<local_photo::OutputIdentity> {
    let identity = photo_tiff::validate(path, MAX_OUTPUT_BYTES)
        .map_err(|code| io::Error::other(format!("film input rejected: {code:?}")))?;
    if !geometry_bounded(u64::from(identity.width), u64::from(identity.height)) {
        return Err(io::Error::other(
            "film input geometry is outside the pinned workspace bounds",
        ));
    }
    Ok(local_photo::OutputIdentity {
        size: identity.size,
        sha256: identity.sha256,
        width: identity.width,
        height: identity.height,
    })
}

/// The shared guarded-run sequence of both render modes.
#[allow(clippy::too_many_arguments)]
fn run(
    binary: &Path,
    data_root: &Path,
    film_profile: &str,
    print_profile: &str,
    work: &Path,
    input: &Path,
    output: &Path,
    output_format: &str,
    max_edge: Option<u32>,
    parameters: &Parameters,
    cancellation: Arc<AtomicBool>,
    timeout: Duration,
) -> io::Result<()> {
    if timeout.is_zero() {
        return Err(io::Error::other(
            "standalone film execution requires a timeout",
        ));
    }
    if cancellation.load(Ordering::Relaxed) {
        return Err(cancelled());
    }
    let recipe_path = prepare(
        work,
        parameters,
        film_profile,
        print_profile,
        output_format,
        max_edge,
    )?;
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| io::Error::other("standalone film deadline overflow"))?;
    if cancellation.load(Ordering::Relaxed) {
        return Err(cancelled());
    }
    let arguments = runner_arguments(input, output, &recipe_path, data_root)?;
    let env = environment(work, data_root, binary)?;
    let mut attempt = Attempt::start(
        binary,
        &arguments,
        &env,
        work,
        cancellation.clone(),
        deadline,
    )
    .map_err(|error| terminated(error, &cancellation, deadline))?;
    let code = attempt
        .finish()
        .map_err(|error| terminated(error, &cancellation, deadline))?;
    attempt_outcome(code, &attempt).map_err(|error| terminated(error, &cancellation, deadline))
}

/// Execute one standalone Film Export of an artifact-bound step's complete
/// module-owned parameter snapshot and return the validated identity of
/// the written Finished JPEG.
/// `binary` is the pinned fork CLI, `data_root` its immutable data tree,
/// `work` a clean private directory this call may own for the run, `input`
/// the retained Development TIFF handoff, and `output` the path the validated
/// Finished JPEG is written to.
#[allow(clippy::too_many_arguments)]
pub fn develop_selected_step(
    binary: &Path,
    data_root: &Path,
    film_profile: &str,
    print_profile: &str,
    work: &Path,
    input: &Path,
    output: &Path,
    parameters: &Parameters,
    cancellation: Arc<AtomicBool>,
    timeout: Duration,
) -> io::Result<local_photo::OutputIdentity> {
    let input_identity = validate_input(input)?;
    run(
        binary,
        data_root,
        film_profile,
        print_profile,
        work,
        input,
        output,
        "jpeg",
        None,
        parameters,
        cancellation,
        timeout,
    )?;
    let identity = photo_jpeg::validate(output, MAX_OUTPUT_BYTES)
        .map_err(|code| io::Error::other(format!("finished film artifact rejected: {code:?}")))?;
    if identity.width != input_identity.width || identity.height != input_identity.height {
        return Err(io::Error::other(
            "finished film artifact does not preserve the input geometry",
        ));
    }
    Ok(local_photo::OutputIdentity {
        size: identity.size,
        sha256: identity.sha256,
        width: identity.width,
        height: identity.height,
    })
}

/// Execute one bounded selected-step standalone Film Preview over the
/// caller-owned paths and return the validated identity of the written
/// PNG rendition.
#[allow(clippy::too_many_arguments)]
pub fn render_selected_step(
    binary: &Path,
    data_root: &Path,
    film_profile: &str,
    print_profile: &str,
    work: &Path,
    input: &Path,
    output: &Path,
    parameters: &Parameters,
    max_edge: u32,
    cancellation: Arc<AtomicBool>,
    timeout: Duration,
) -> io::Result<local_preview::PreviewIdentity> {
    if max_edge == 0 || max_edge > local_preview::PREVIEW_LONG_EDGE {
        return Err(io::Error::other(
            "the film preview geometry exceeds the disclosed Preview bound",
        ));
    }
    validate_input(input)?;
    run(
        binary,
        data_root,
        film_profile,
        print_profile,
        work,
        input,
        output,
        "png",
        Some(max_edge),
        parameters,
        cancellation,
        timeout,
    )?;
    let bytes = fs::read(output)?;
    if bytes.len() > PREVIEW_BYTES_MAX {
        return Err(io::Error::other(
            "bounded film preview exceeds the rendition byte bound",
        ));
    }
    let (width, height) = local_preview::png_dimensions(&bytes)?;
    if width.max(height) > max_edge {
        return Err(io::Error::other(
            "bounded film preview exceeds the disclosed geometry",
        ));
    }
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(local_preview::PreviewIdentity {
        size: bytes.len() as u64,
        sha256: format!("{:x}", hasher.finalize()),
        width,
        height,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parameters(tree: serde_json::Value) -> Parameters {
        Parameters {
            module: modules::SPEKTRAFILM_MODULE.to_owned(),
            version: modules::SPEKTRAFILM_PARAMETER_VERSION.to_owned(),
            tree,
        }
    }

    fn groups_tree() -> serde_json::Value {
        modules::spektrafilm_default_tree()
    }

    #[test]
    fn admitted_envelopes_carry_only_the_runtime_groups_verbatim() {
        // A tree of real owned groups plus the pinned output admits.
        assert!(modules::validate_spektrafilm_parameters(&parameters(groups_tree())).is_ok());
        // Groups stay verbatim objects: an unknown group never flattens
        // into an admitted tree.
        let mut unknown = groups_tree();
        unknown["stack"] = json!([]);
        assert!(modules::validate_spektrafilm_parameters(&parameters(unknown)).is_err());
        // A non-object group is malformed, not merged.
        let mut scalar = groups_tree();
        scalar["camera"] = json!(1);
        assert!(modules::validate_spektrafilm_parameters(&parameters(scalar)).is_err());
        // A wrong module or version is refused before the tree is read.
        let mut owned_elsewhere = parameters(groups_tree());
        owned_elsewhere.module = modules::DARKTABLE_MODULE.to_owned();
        assert!(modules::validate_spektrafilm_parameters(&owned_elsewhere).is_err());
        let mut wrong_version = parameters(groups_tree());
        wrong_version.version = "spektrafilm-params-2".to_owned();
        assert!(modules::validate_spektrafilm_parameters(&wrong_version).is_err());
    }

    #[test]
    fn the_only_admitted_output_is_the_pinned_finished_jpeg() {
        let mut raw_quality = groups_tree();
        raw_quality["output"]["encoding"] = json!("quality-92-baseline");
        assert!(modules::validate_spektrafilm_parameters(&parameters(raw_quality)).is_err());
        let mut tiff_output = groups_tree();
        tiff_output["output"] = json!({
            "format": "tiff",
            "precisionBits": 32,
            "colorSpace": "prophoto-rgb",
            "transferFunction": "linear",
        });
        assert!(modules::validate_spektrafilm_parameters(&parameters(tiff_output)).is_err());
        // An absent output stays admitted: the module produces exactly one
        // output, so the pin needs no defaulting to be complete.
        let mut without_output = groups_tree();
        without_output.as_object_mut().unwrap().remove("output");
        assert!(modules::validate_spektrafilm_parameters(&parameters(without_output)).is_ok());
    }

    #[test]
    fn film_geometry_bounds_mirror_the_pinned_workspace_admission() {
        // The qualified full-resolution handoff and the bounded Preview
        // geometry both admit.
        assert!(geometry_bounded(9504, 6336));
        assert!(geometry_bounded(1, 1));
        // Zero and beyond-edge frames are refused before any allocation.
        assert!(!geometry_bounded(0, 1));
        assert!(!geometry_bounded(1, 0));
        assert!(!geometry_bounded(9569, 1));
        assert!(!geometry_bounded(30_000, 1));
        // The pinned edge bound already keeps every admitted frame inside
        // the decoded-byte and coordinate-sum allowances, exactly like the
        // pinned adapter's own redundant guards.
        assert!(geometry_bounded(9504, 9504));
    }

    #[test]
    fn a_cancelled_attempt_never_spawns_the_engine() {
        let work =
            std::env::temp_dir().join(format!("slipstream-film-cancel-{}", std::process::id()));
        fs::create_dir_all(&work).unwrap();
        let cancellation = Arc::new(AtomicBool::new(true));
        let error = run(
            Path::new("/nonexistent/spektrafilm"),
            Path::new("/nonexistent/data"),
            "kodak_portra_400",
            "kodak_portra_endura",
            &work,
            Path::new("/nonexistent/in.tif"),
            Path::new("/nonexistent/out.jpg"),
            "jpeg",
            None,
            &parameters(groups_tree()),
            cancellation,
            Duration::from_secs(1),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "standalone film execution was cancelled");
        let _ = fs::remove_dir_all(&work);
    }

    #[test]
    fn a_zero_timeout_is_refused_before_any_work_directory_is_written() {
        let work =
            std::env::temp_dir().join(format!("slipstream-film-timeout-{}", std::process::id()));
        fs::create_dir_all(&work).unwrap();
        let error = run(
            Path::new("/nonexistent/spektrafilm"),
            Path::new("/nonexistent/data"),
            "kodak_portra_400",
            "kodak_portra_endura",
            &work,
            Path::new("/nonexistent/in.tif"),
            Path::new("/nonexistent/out.jpg"),
            "jpeg",
            None,
            &parameters(groups_tree()),
            Arc::new(AtomicBool::new(false)),
            Duration::ZERO,
        )
        .unwrap_err();
        assert!(error.to_string().contains("requires a timeout"));
        assert!(!work.join("numba").exists());
        let _ = fs::remove_dir_all(&work);
    }

    /// The guarded attempt kills the whole process group at its deadline,
    /// so a hanging runtime cannot outlive its authority.
    #[test]
    fn a_hanging_film_runtime_is_killed_at_its_deadline() {
        let work = std::env::temp_dir().join(format!(
            "slipstream-film-deadline-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        fs::create_dir_all(&work).unwrap();
        prepare(
            &work,
            &parameters(groups_tree()),
            "kodak_portra_400",
            "kodak_portra_endura",
            "jpeg",
            None,
        )
        .unwrap();
        let cancellation = Arc::new(AtomicBool::new(false));
        let deadline = Instant::now() + Duration::from_millis(150);
        let arguments = vec!["300".to_owned()];
        let started = Instant::now();
        let mut attempt = Attempt::start(
            Path::new("/bin/sleep"),
            &arguments,
            &[],
            &work,
            cancellation,
            deadline,
        )
        .unwrap();
        let outcome = attempt.finish();
        assert!(outcome.is_err());
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the deadline watchdog did not unblock the film attempt promptly"
        );
        let _ = fs::remove_dir_all(&work);
    }

    /// The same group-kill authority answers the cancellation flag long
    /// before the deadline.
    #[test]
    fn a_hanging_film_runtime_is_killed_once_cancelled() {
        let work = std::env::temp_dir().join(format!(
            "slipstream-film-cancelled-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        fs::create_dir_all(&work).unwrap();
        prepare(
            &work,
            &parameters(groups_tree()),
            "kodak_portra_400",
            "kodak_portra_endura",
            "jpeg",
            None,
        )
        .unwrap();
        let cancellation = Arc::new(AtomicBool::new(false));
        let mut attempt = Attempt::start(
            Path::new("/bin/sleep"),
            &["300".to_owned()],
            &[],
            &work,
            cancellation.clone(),
            Instant::now() + Duration::from_secs(600),
        )
        .unwrap();
        cancellation.store(true, Ordering::Relaxed);
        let started = Instant::now();
        assert!(attempt.finish().is_err());
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the cancellation watchdog did not unblock the film attempt promptly"
        );
        let _ = fs::remove_dir_all(&work);
    }

    /// A pinned refusal (exit 71) is a distinct, structured failure that
    /// carries the bounded stderr evidence; an engine failure is another.
    #[test]
    fn runner_exit_codes_classify_refusal_and_failure() {
        let work = std::env::temp_dir().join(format!(
            "slipstream-film-codes-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        fs::create_dir_all(&work).unwrap();
        prepare(
            &work,
            &parameters(groups_tree()),
            "kodak_portra_400",
            "kodak_portra_endura",
            "jpeg",
            None,
        )
        .unwrap();

        let mut refusal = Attempt::start(
            Path::new("/bin/sh"),
            &[
                "-c".to_owned(),
                "echo 'refusing the pinned film contract: probe' >&2; exit 71".to_owned(),
            ],
            &[],
            &work,
            Arc::new(AtomicBool::new(false)),
            Instant::now() + Duration::from_secs(30),
        )
        .unwrap();
        assert_eq!(refusal.finish().unwrap(), RUNNER_REFUSED);
        let refusal_error = attempt_outcome(RUNNER_REFUSED, &refusal).unwrap_err();
        assert!(refusal_error.to_string().contains("refused the attempt"));
        assert!(
            refusal
                .stderr_tail()
                .contains("refusing the pinned film contract")
        );

        let second = work.join("second");
        fs::create_dir_all(&second).unwrap();
        let mut failure = Attempt::start(
            Path::new("/bin/sh"),
            &["-c".to_owned(), "echo boom >&2; exit 1".to_owned()],
            &[],
            &second,
            Arc::new(AtomicBool::new(false)),
            Instant::now() + Duration::from_secs(30),
        )
        .unwrap();
        assert_eq!(failure.finish().unwrap(), 1);
        let failure_error = attempt_outcome(1, &failure).unwrap_err();
        assert!(failure_error.to_string().contains("failed (status 1)"));
        assert!(failure.stderr_tail().contains("boom"));
        let _ = fs::remove_dir_all(&work);
    }

    /// The frozen parameter file carries the tree verbatim — no field is
    /// dropped, merged, or renamed on its way to the pinned runtime — and
    /// a second prepare over the same work directory never overwrites it.
    #[test]
    fn the_frozen_parameter_file_carries_the_tree_verbatim() {
        let work = std::env::temp_dir().join(format!(
            "slipstream-film-parameters-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .subsec_nanos()
        ));
        fs::create_dir_all(&work).unwrap();
        let tree = groups_tree();
        let path = prepare(
            &work,
            &parameters(tree.clone()),
            "kodak_portra_400",
            "kodak_portra_endura",
            "jpeg",
            None,
        )
        .unwrap();
        let written: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["parameters"], tree);
        assert_eq!(written["schemaVersion"], "spektrafilm-rs-params-1");
        assert!(
            prepare(
                &work,
                &parameters(tree),
                "kodak_portra_400",
                "kodak_portra_endura",
                "jpeg",
                None,
            )
            .is_err()
        );
        let _ = fs::remove_dir_all(&work);
    }
}
