use std::{
    io::Read,
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use super::Result;
use crate::protocol::{ErrorCode, RESPONSE_BYTES};

/// Execute a bounded, isolated host command and return trimmed UTF-8 stdout.
///
/// The process group, cleared environment, nonblocking pipes, output cap, and
/// deadline are part of the processing backend's security and liveness
/// contract. Keeping them together prevents callers from accidentally creating
/// an unbounded or environment-dependent subprocess.
pub(crate) fn command(program: &str, args: &[String]) -> Result<String> {
    command_until(program, args, Instant::now() + Duration::from_secs(5))
}

pub(crate) fn command_until(program: &str, args: &[String], deadline: Instant) -> Result<String> {
    if Instant::now() >= deadline {
        return Err(ErrorCode::Uncertain);
    }
    let mut child = Command::new(program)
        .args(args)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|_| ErrorCode::Unavailable)?;
    let mut stdout = child.stdout.take().ok_or(ErrorCode::Unavailable)?;
    let mut stderr = child.stderr.take().ok_or(ErrorCode::Unavailable)?;
    for descriptor in [stdout.as_raw_fd(), stderr.as_raw_fd()] {
        // SAFETY: both pipe descriptors are live. Nonblocking reads bound the complete command lifetime.
        if unsafe { libc::fcntl(descriptor, libc::F_SETFL, libc::O_NONBLOCK) } < 0 {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ErrorCode::Unavailable);
        }
    }
    let mut output = Vec::new();
    let mut errors = Vec::new();
    let mut output_eof = false;
    let mut errors_eof = false;
    let mut status = None;
    loop {
        let capture = |reader: &mut dyn Read, bytes: &mut Vec<u8>| -> Result<bool> {
            let mut buffer = [0u8; 4096];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => return Ok(true),
                    Ok(count) => {
                        if bytes.len() + count > RESPONSE_BYTES {
                            return Err(ErrorCode::Uncertain);
                        }
                        bytes.extend_from_slice(&buffer[..count]);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        return Ok(false);
                    }
                    Err(_) => return Err(ErrorCode::Uncertain),
                }
            }
        };
        let captured: Result<()> = (|| {
            if !output_eof {
                output_eof = capture(&mut stdout, &mut output)?;
            }
            if !errors_eof {
                errors_eof = capture(&mut stderr, &mut errors)?;
            }
            Ok(())
        })();
        if captured.is_err() || Instant::now() >= deadline {
            // This is our unreaped direct child, never a discovered/recycled engine PID.
            if status.is_none() {
                let _ = child.kill();
                let _ = child.wait();
            }
            return Err(ErrorCode::Uncertain);
        }
        if status.is_none() {
            status = child.try_wait().map_err(|_| ErrorCode::Uncertain)?;
        }
        if output_eof
            && errors_eof
            && let Some(status) = status
        {
            if !status.success() {
                return Err(ErrorCode::Unavailable);
            }
            return String::from_utf8(output)
                .map(|text| text.trim().to_owned())
                .map_err(|_| ErrorCode::Unavailable);
        }
        thread::sleep(Duration::from_millis(5));
    }
}

pub(crate) fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}
