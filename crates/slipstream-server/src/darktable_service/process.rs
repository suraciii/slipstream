use super::{Result, ServiceError};
use serde_json::{Value, json};
use std::os::unix::process::CommandExt;
use std::{
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

pub(crate) fn invoke(
    state: Option<&Path>,
    operation: &str,
    arguments: Value,
    cancel: &AtomicBool,
    deadline: Duration,
) -> Result<Value> {
    let binary = std::env::var_os("DARKTABLE_EDIT_BINARY")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|p| p.join("darktable-edit")))
                .filter(|p| p.is_file())
                .unwrap_or_else(|| "darktable-edit".into())
        });
    let input = serde_json::to_vec(
        &json!({"operation":operation,"stateDirectory":state,"arguments":arguments}),
    )
    .map_err(ServiceError::io)?;
    let mut command = Command::new(binary);
    command
        .arg("__invoke")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let parent = unsafe { libc::getpid() };
    unsafe {
        command.pre_exec(move || {
            if libc::setpgid(0, 0) != 0 || libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::getppid() != parent {
                return Err(std::io::Error::other("execution owner exited"));
            }
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .map_err(|e| ServiceError::new("engine_unavailable", e.to_string(), 503))?;
    let pid = child.id() as i32;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let out = std::thread::spawn(move || bounded_read(stdout, 16 * 1024 * 1024));
    let err = std::thread::spawn(move || bounded_read(stderr, 64 * 1024));
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || {
        let result = stdin.write_all(&input);
        drop(stdin);
        result
    });
    let start = Instant::now();
    let mut interrupted = None;
    let status = loop {
        let mut information: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let observed = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as u32,
                &mut information,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if observed != 0 {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
            break child.wait();
        }
        if unsafe { information.si_pid() } == pid {
            // WNOWAIT retains child identity until the entire group is killed.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
            break child.wait();
        }
        if cancel.load(Ordering::Acquire) || start.elapsed() >= deadline {
            interrupted = Some(if cancel.load(Ordering::Acquire) {
                "cancelled"
            } else {
                "deadline_exceeded"
            });
            // Child is still our unreaped child; its process-group identity cannot be reused.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
            break child.wait();
        }
        std::thread::park_timeout(Duration::from_millis(20));
    };
    // Every termination above kills descendants before reaping the child.
    let _ = writer.join();
    let bytes = out
        .join()
        .map_err(|_| ServiceError::unknown("Tool output reader failed"))?
        .map_err(ServiceError::io)?;
    let _ = err.join();
    if let Some(code) = interrupted {
        return Err(ServiceError::new(
            code,
            "Tool execution interrupted; reconcile the request receipt",
            409,
        )
        .effect("unknown"));
    }
    let status = status.map_err(ServiceError::io)?;
    let envelope: Value = serde_json::from_slice(&bytes)
        .map_err(|_| ServiceError::unknown("Tool returned no complete command envelope"))?;
    if envelope["status"] == "ok" && status.success() {
        return Ok(envelope["data"].clone());
    }
    if let Some(error) = envelope.get("error") {
        return Err(ServiceError::tool(error.clone()));
    }
    Err(ServiceError::unknown(
        "Tool exited without a command receipt",
    ))
}
fn bounded_read(mut reader: impl Read, maximum: usize) -> std::io::Result<Vec<u8>> {
    let mut result = Vec::new();
    let mut chunk = [0u8; 8192];
    let mut exceeded = false;
    loop {
        let count = reader.read(&mut chunk)?;
        if count == 0 {
            break;
        }
        if result.len() + count <= maximum {
            result.extend_from_slice(&chunk[..count]);
        } else {
            exceeded = true;
        }
    }
    if exceeded {
        Err(std::io::Error::other(
            "Tool response exceeds transport limit",
        ))
    } else {
        Ok(result)
    }
}
pub(crate) fn inspect_state(path: &Path) -> Result<Value> {
    invoke(
        Some(path),
        "__inspect",
        json!({}),
        &Arc::new(AtomicBool::new(false)),
        Duration::from_secs(30),
    )
}
pub(crate) fn inspect_state_retained(
    state: &photo_files::Directory,
    state_lease: &photo_files::FileLease,
) -> Result<Value> {
    state.verify_path().map_err(ServiceError::io)?;
    darktable_edit::inspect_retained_directory(state, state_lease)
        .map_err(|error| ServiceError::tool(json!(error)))
}
