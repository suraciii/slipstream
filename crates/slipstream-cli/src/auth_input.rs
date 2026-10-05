use super::*;
use std::io::{IsTerminal, Read, Write};
#[cfg(unix)]
use std::os::unix::io::AsRawFd;
pub(super) async fn read_login_token(args: &AuthLoginArgs) -> Result<String, CommandFailure> {
    if let Some(path) = &args.token_file {
        if path.as_os_str().is_empty() {
            return Err(CommandFailure::invalid(
                "token-file",
                "The credential file path must not be empty.",
            ));
        }
        return read_access_token(path.clone()).await;
    }
    if args.token_stdin {
        return tokio::task::spawn_blocking(read_access_token_stdin)
            .await
            .map_err(|_| CommandFailure::local_credential(None))?;
    }
    read_access_token_prompt().await
}

fn parse_access_token_bytes(bytes: &[u8]) -> Result<String, CommandFailure> {
    let token_bytes = bytes
        .strip_suffix(b"\r\n")
        .or_else(|| bytes.strip_suffix(b"\n"))
        .unwrap_or(bytes);
    if !canonical_access_token(token_bytes) {
        return Err(CommandFailure::invalid(
            "token",
            "The Access Token must be one canonical token with no extra content.",
        ));
    }
    Ok(String::from_utf8(token_bytes.to_vec()).expect("validated token is ASCII"))
}

fn read_access_token_stdin() -> Result<String, CommandFailure> {
    let stdin = std::io::stdin().lock();
    let mut bytes = Vec::with_capacity(MAXIMUM_CREDENTIAL_BYTES);
    stdin
        .take((MAXIMUM_CREDENTIAL_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| CommandFailure::local_credential(None))?;
    if bytes.len() > MAXIMUM_CREDENTIAL_BYTES {
        return Err(CommandFailure::invalid(
            "token-stdin",
            "The Access Token input is too large.",
        ));
    }
    parse_access_token_bytes(&bytes)
}

#[cfg(unix)]
struct TerminalEchoGuard {
    fd: std::os::unix::io::RawFd,
    original: libc::termios,
    active: bool,
}

#[cfg(unix)]
impl TerminalEchoGuard {
    fn restore(&mut self) {
        if self.active {
            let _ = unsafe { libc::tcsetattr(self.fd, libc::TCSANOW, &self.original) };
            self.active = false;
        }
    }
}

#[cfg(unix)]
impl Drop for TerminalEchoGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

#[cfg(unix)]
async fn read_access_token_prompt() -> Result<String, CommandFailure> {
    if !std::io::stdin().is_terminal() {
        return Err(CommandFailure::invalid(
            "token",
            "Interactive login requires a terminal; use --token-stdin instead.",
        ));
    }
    let tty = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open("/dev/tty")
        .map_err(|_| CommandFailure::local_credential(None))?;
    let tty =
        tokio::io::unix::AsyncFd::new(tty).map_err(|_| CommandFailure::local_credential(None))?;
    let fd = tty.get_ref().as_raw_fd();
    let mut original: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(fd, &mut original) } != 0 {
        return Err(CommandFailure::local_credential(None));
    }
    let mut hidden = original;
    hidden.c_lflag &= !libc::ECHO;
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &hidden) } != 0 {
        return Err(CommandFailure::local_credential(None));
    }
    let mut echo_guard = TerminalEchoGuard {
        fd,
        original,
        active: true,
    };
    let _ = tty.get_ref().write_all(b"Access Token: ");
    let mut bytes = Vec::with_capacity(MAXIMUM_CREDENTIAL_BYTES);
    let read_result = loop {
        let mut readiness = tty
            .readable()
            .await
            .map_err(|_| CommandFailure::local_credential(None))?;
        let mut byte = [0_u8; 1];
        match readiness.try_io(|_| tty.get_ref().read(&mut byte)) {
            Ok(Ok(0)) => break Err(CommandFailure::local_credential(None)),
            Ok(Ok(_)) if byte[0] == b'\n' || byte[0] == b'\r' => break Ok(()),
            Ok(Ok(_)) => {
                if bytes.len() >= MAXIMUM_CREDENTIAL_BYTES {
                    break Err(CommandFailure::invalid(
                        "token",
                        "The Access Token input is too large.",
                    ));
                }
                bytes.push(byte[0]);
            }
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::WouldBlock => continue,
            Ok(Err(_)) => break Err(CommandFailure::local_credential(None)),
            Err(_) => continue,
        }
    };
    echo_guard.restore();
    let _ = tty.get_ref().write_all(b"\n");
    read_result?;
    parse_access_token_bytes(&bytes)
}

#[cfg(not(unix))]
async fn read_access_token_prompt() -> Result<String, CommandFailure> {
    Err(CommandFailure::invalid(
        "token",
        "Interactive login is unavailable; use --token-stdin instead.",
    ))
}

#[cfg(unix)]
pub(super) async fn confirm_replacement() -> Result<bool, CommandFailure> {
    tokio::task::spawn_blocking(|| {
        if !std::io::stdin().is_terminal() {
            return Ok(false);
        }
        let mut tty = OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .map_err(|_| CommandFailure::local_credential(None))?;
        tty.write_all(b"A saved credential exists. Replace it? [y/N] ")
            .map_err(|_| CommandFailure::local_credential(None))?;
        let mut answer = [0_u8; 1];
        let result = tty
            .read_exact(&mut answer)
            .map(|_| matches!(answer[0], b'y' | b'Y'))
            .map_err(|_| CommandFailure::local_credential(None));
        let _ = tty.write_all(b"\n");
        result
    })
    .await
    .map_err(|_| CommandFailure::local_credential(None))?
}

#[cfg(not(unix))]
pub(super) async fn confirm_replacement() -> Result<bool, CommandFailure> {
    if !std::io::stdin().is_terminal() {
        return Ok(false);
    }
    Err(CommandFailure::invalid(
        "force",
        "Interactive replacement confirmation is unavailable; use --force instead.",
    ))
}
