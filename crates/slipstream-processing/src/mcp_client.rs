//! Minimal deterministic MCP stdio client for the local Photo executor.
//!
//! Speaks the MCP 2025-06-18 stdio transport the pinned darktable-mcp fork
//! implements: one JSON-RPC 2.0 object per line, server stdout only. The
//! executor owns this client; no LLM, no HTTP, and no ambient tool
//! discovery. Every response is decoded as a bounded JSON value before
//! use, and a truncated, oversized, or non-JSON line is a hard failure:
//! the engine either answers the pinned protocol or the attempt fails
//! closed.
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

/// One newline-delimited protocol message may not exceed this bound; the
/// client reads engine metadata, never image bytes, over this channel.
const LINE_BYTES_MAX: usize = 16 * 1024 * 1024;
/// How often one guarded run's watchdog rechecks cancellation and deadline.
const WATCHDOG_POLL: Duration = Duration::from_millis(100);
const SUPERVISOR_BINARY: &str = "slipstream-mcp-supervisor";

extern "C" fn terminate_supervisor(_: libc::c_int) {
    let pgid = unsafe { libc::getpgrp() };
    if pgid > 1 {
        // SAFETY: the supervisor owns its process group.
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }
}

/// Entry point for the small process-group supervisor shipped beside the
/// application. It is not a service: it exists only for one engine child and
/// dies with that child. The engine arguments remain direct argv values.
pub fn run_supervisor(arguments: &[String]) -> io::Result<i32> {
    let expected_parent = arguments
        .first()
        .ok_or_else(|| io::Error::other("supervisor parent identity is missing"))?
        .parse::<libc::pid_t>()
        .map_err(|_| io::Error::other("supervisor parent identity is invalid"))?;
    let program = arguments
        .get(1)
        .ok_or_else(|| io::Error::other("supervisor engine path is missing"))?;
    let engine_args = arguments.get(2..).unwrap_or_default();
    // SAFETY: installing one async-signal-safe handler before enabling the
    // parent-death signal closes the setup race.
    unsafe {
        if libc::signal(libc::SIGTERM, terminate_supervisor as *const () as usize) == libc::SIG_ERR
        {
            return Err(io::Error::last_os_error());
        }
        if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0) != 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::getppid() != expected_parent {
            terminate_supervisor(libc::SIGTERM);
            return Err(io::Error::other("supervisor parent vanished before setup"));
        }
    }
    let status = Command::new(program)
        .args(engine_args)
        .env_clear()
        .envs(std::env::vars_os())
        .status()?;
    Ok(status.code().unwrap_or(1))
}

pub(crate) fn supervisor_path() -> Option<PathBuf> {
    let installed = PathBuf::from("/usr/local/bin").join(SUPERVISOR_BINARY);
    if installed.is_file() {
        return Some(installed);
    }
    let current = std::env::current_exe().ok()?;
    let directory = current.parent()?;
    let directory = if directory.file_name().and_then(|name| name.to_str()) == Some("deps") {
        directory.parent()?
    } else {
        directory
    };
    let candidate = directory.join(SUPERVISOR_BINARY);
    candidate.is_file().then_some(candidate)
}

/// The joined-on-drop watchdog of one guarded engine run. It exists only
/// for the lifetime of its [`McpClient`], so a call never leaves a thread
/// behind.
pub(crate) struct Watchdog {
    handle: thread::JoinHandle<()>,
    done: Arc<AtomicBool>,
}

impl Watchdog {
    /// Arm the group-kill authority of one guarded run. Once the caller
    /// cancels or the deadline passes, the whole engine process group is
    /// killed, which unblocks any protocol read the owner thread is stuck
    /// in; the owner then observes the broken channel as an error.
    pub(crate) fn start(
        cancellation: Arc<AtomicBool>,
        deadline: Instant,
        pgid: libc::pid_t,
    ) -> io::Result<Self> {
        let done = Arc::new(AtomicBool::new(false));
        let armed = done.clone();
        let handle = thread::Builder::new()
            .name("mcp-engine-watchdog".into())
            .spawn(move || {
                while !armed.load(Ordering::Relaxed) {
                    if cancellation.load(Ordering::Relaxed) || Instant::now() >= deadline {
                        // SAFETY: one group signal to the client-owned group.
                        unsafe { libc::kill(-pgid, libc::SIGKILL) };
                        return;
                    }
                    thread::sleep(WATCHDOG_POLL);
                }
            })?;
        Ok(Self { handle, done })
    }

    /// Stop and join the watchdog so no thread outlives the run.
    pub(crate) fn stop(self) {
        self.done.store(true, Ordering::Relaxed);
        let _ = self.handle.join();
    }
}

pub struct McpClient {
    child: Child,
    /// The private process group led by the supervisor child. The engine and
    /// every descendant inherit it, so one group signal reaches the whole
    /// attempt even after the engine itself has exited.
    pgid: libc::pid_t,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
    watchdog: Option<Watchdog>,
}

impl McpClient {
    /// Start the engine with an explicit argument vector and environment.
    /// No shell, no inherited environment: the pinned bundle owns the
    /// process identity. A sibling supervisor owns the group lifetime and
    /// is killed by the kernel if this parent dies.
    pub fn spawn(program: &str, args: &[String], env: &[(String, String)]) -> io::Result<Self> {
        Self::start(program, args, env, None)
    }

    /// Start the engine under an external termination authority: once the
    /// caller cancels or the deadline passes, the watchdog kills the whole
    /// engine process group, unblocking any protocol read this owner thread
    /// is stuck in. The watchdog is joined when the client drops, so no
    /// thread outlives the call.
    pub fn spawn_guarded(
        program: &str,
        args: &[String],
        env: &[(String, String)],
        cancellation: Arc<AtomicBool>,
        deadline: Instant,
    ) -> io::Result<Self> {
        Self::start(program, args, env, Some((cancellation, deadline)))
    }

    fn start(
        program: &str,
        args: &[String],
        env: &[(String, String)],
        guard: Option<(Arc<AtomicBool>, Instant)>,
    ) -> io::Result<Self> {
        let parent = unsafe { libc::getpid() };
        let supervisor = supervisor_path()
            .ok_or_else(|| io::Error::other("Photo process supervisor is unavailable"))?;
        let mut command_args = Vec::with_capacity(args.len() + 2);
        command_args.push(parent.to_string());
        command_args.push(program.to_owned());
        command_args.extend(args.iter().cloned());
        let mut command = Command::new(supervisor);
        command
            .args(&command_args)
            .env_clear()
            .envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
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
        let mut child = command.spawn()?;
        let pgid = child.id() as libc::pid_t;
        let stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                unsafe { libc::kill(-pgid, libc::SIGKILL) };
                let _ = child.wait();
                return Err(io::Error::other("engine stdin unavailable"));
            }
        };
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                unsafe { libc::kill(-pgid, libc::SIGKILL) };
                let _ = child.wait();
                return Err(io::Error::other("engine stdout unavailable"));
            }
        };
        let mut client = Self {
            child,
            pgid,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
            next_id: 1,
            watchdog: None,
        };
        if let Some((cancellation, deadline)) = guard {
            client.watchdog = match Watchdog::start(cancellation, deadline, client.pgid) {
                Ok(watchdog) => Some(watchdog),
                Err(error) => {
                    client.kill_group();
                    let _ = client.child.kill();
                    let _ = client.child.wait();
                    return Err(error);
                }
            };
        }
        Ok(client)
    }

    /// Signal the whole private engine process group. Descendants that
    /// outlived their leader remain in the group, so this reaches them
    /// even after the engine itself has exited; an already-empty group is
    /// not an error condition.
    fn kill_group(&self) {
        // SAFETY: one group signal; the result is deliberately ignored.
        unsafe {
            libc::kill(-self.pgid, libc::SIGKILL);
        }
    }

    fn request(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> io::Result<serde_json::Value> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| io::Error::other("request id overflow"))?;
        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut line = serde_json::to_string(&frame)?;
        line.push('\n');
        if line.len() > LINE_BYTES_MAX {
            return Err(io::Error::other("request exceeds the protocol bound"));
        }
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::other("engine stdin unavailable"))?;
        stdin.write_all(line.as_bytes())?;
        stdin.flush()?;
        self.read_response(id)
    }

    fn read_response(&mut self, id: i64) -> io::Result<serde_json::Value> {
        let mut line = Vec::new();
        loop {
            line.clear();
            let read = self
                .stdout
                .by_ref()
                .take((LINE_BYTES_MAX + 1) as u64)
                .read_until(b'\n', &mut line)?;
            if read == 0 {
                return Err(io::Error::other("engine closed the protocol channel"));
            }
            if line.len() > LINE_BYTES_MAX || !line.ends_with(b"\n") {
                return Err(io::Error::other(
                    "engine response is oversized or incomplete",
                ));
            }
            // The pinned engine can prefix its channel with human-readable
            // log lines. Only a JSON-RPC object answering this request is
            // a response; every other line is skipped, and the caller's
            // watchdog bounds a channel that never answers.
            let Ok(value) = serde_json::from_slice::<serde_json::Value>(&line) else {
                continue;
            };
            let Some(object) = value.as_object() else {
                continue;
            };
            if object.get("jsonrpc").and_then(|v| v.as_str()) != Some("2.0")
                || object.get("id").and_then(|v| v.as_i64()) != Some(id)
            {
                continue;
            }
            if object.contains_key("result") == object.contains_key("error") {
                return Err(io::Error::other(
                    "engine response does not match the request",
                ));
            }
            return Ok(value);
        }
    }

    /// Complete the MCP handshake and require the pinned protocol version.
    pub fn initialize(&mut self, client: &str) -> io::Result<String> {
        let params = serde_json::json!({
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": client, "version": "1" },
        });
        let value = self.request("initialize", params)?;
        if value.get("error").is_some() {
            return Err(io::Error::other("engine rejected initialization"));
        }
        let server_version = value
            .pointer("/result/protocolVersion")
            .and_then(|v| v.as_str())
            .ok_or_else(|| io::Error::other("engine did not answer the protocol version"))?
            .to_string();
        if server_version != "2025-06-18" {
            return Err(io::Error::other("engine protocol version mismatch"));
        }
        let notification = serde_json::json!({
            "jsonrpc": "2.0", "method": "notifications/initialized"
        });
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::other("engine stdin unavailable"))?;
        serde_json::to_writer(&mut *stdin, &notification)?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        Ok(server_version)
    }

    /// Read the complete tool list once. The caller compares it against the
    /// bundle contract; discovery here grants nothing.
    pub fn tools_list(&mut self) -> io::Result<serde_json::Value> {
        let value = self.request("tools/list", serde_json::json!({}))?;
        if value.get("error").is_some() {
            return Err(io::Error::other("engine did not list tools"));
        }
        value
            .pointer("/result/tools")
            .cloned()
            .ok_or_else(|| io::Error::other("engine tool list is missing"))
    }

    /// Decode one tool's JSON text result or fail with the engine refusal.
    pub fn call(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> io::Result<serde_json::Value> {
        let value = self.request(
            "tools/call",
            serde_json::json!({ "name": name, "arguments": arguments }),
        )?;
        if let Some(error) = value.get("error") {
            return Err(io::Error::other(format!("engine rejected {name}: {error}")));
        }
        let result = value
            .get("result")
            .ok_or_else(|| io::Error::other("engine tool result is missing"))?;
        let content = result
            .get("content")
            .and_then(serde_json::Value::as_array)
            .filter(|content| content.len() == 1)
            .ok_or_else(|| io::Error::other("engine tool result is not one text block"))?;
        let text = content[0]
            .get("text")
            .and_then(serde_json::Value::as_str)
            .filter(|_| content[0].get("type").and_then(serde_json::Value::as_str) == Some("text"))
            .ok_or_else(|| io::Error::other("engine tool result is not text"))?;
        if result.get("isError").and_then(serde_json::Value::as_bool) != Some(false) {
            return Err(io::Error::other(format!("engine rejected {name}: {text}")));
        }
        serde_json::from_str(text).map_err(|_| io::Error::other("engine tool result is not JSON"))
    }

    /// Decode one tool's PNG image result or fail with the engine refusal.
    /// The `render` tool answers with exactly one `image/png` content
    /// block whose base64 `data` is the rendition itself; image bytes are
    /// never parsed as JSON text.
    pub fn call_image_png(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> io::Result<Vec<u8>> {
        let value = self.request(
            "tools/call",
            serde_json::json!({ "name": name, "arguments": arguments }),
        )?;
        decode_image_png(&value, name)
    }
    /// Terminate the engine child with the attempt. EOF before a complete
    /// shutdown is a failure the caller reports, not an empty success.
    pub fn shutdown(mut self) -> io::Result<()> {
        self.request("shutdown", serde_json::json!({}))?;
        // Closing stdin is the stdio transport's lifecycle signal. The
        // pinned engine also accepts the shutdown extension, but EOF keeps
        // this client correct if that extension is unavailable.
        drop(self.stdin.take());
        let deadline = Instant::now() + Duration::from_secs(2);
        let status = loop {
            if let Some(status) = self.child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                self.kill_group();
                self.child.kill()?;
                break self.child.wait()?;
            }
            thread::sleep(Duration::from_millis(10));
        };
        if !status.success() {
            return Err(io::Error::other("engine child exited unsuccessfully"));
        }
        Ok(())
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        // Stop and join the watchdog first so no thread outlives the call.
        if let Some(watchdog) = self.watchdog.take() {
            watchdog.stop();
        }
        // Kill and reap on every path. The group signal reaches descendants
        // even after the engine leader itself has exited, and the wait
        // leaves no zombie behind.
        self.kill_group();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Decode one `tools/call` response value's PNG image content block: the
/// engine's one `image/png` block with strict padded base64 `data`. A
/// JSON-RPC error, an `isError` text refusal, any other block shape or
/// mime type, and any non-canonical base64 payload fail closed.
fn decode_image_png(value: &serde_json::Value, name: &str) -> io::Result<Vec<u8>> {
    if let Some(error) = value.get("error") {
        return Err(io::Error::other(format!("engine rejected {name}: {error}")));
    }
    let result = value
        .get("result")
        .ok_or_else(|| io::Error::other("engine tool result is missing"))?;
    let content = result
        .get("content")
        .and_then(serde_json::Value::as_array)
        .filter(|content| content.len() == 1)
        .ok_or_else(|| io::Error::other("engine tool result is not one content block"))?;
    let block = &content[0];
    if result.get("isError").and_then(serde_json::Value::as_bool) != Some(false) {
        let refusal = block
            .get("text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("engine gave no refusal text");
        return Err(io::Error::other(format!(
            "engine rejected {name}: {refusal}"
        )));
    }
    if block.get("type").and_then(serde_json::Value::as_str) != Some("image")
        || block.get("mimeType").and_then(serde_json::Value::as_str) != Some("image/png")
    {
        return Err(io::Error::other(
            "engine tool result is not a PNG image block",
        ));
    }
    let data = block
        .get("data")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| io::Error::other("engine image block carries no data"))?;
    decode_base64(data)
}

/// The base64 alphabet the MCP image content block specifies. Padding is
/// `=`; whitespace and every other byte are refused.
fn base64_value(byte: u8) -> Option<u32> {
    match byte {
        b'A'..=b'Z' => Some(u32::from(byte - b'A')),
        b'a'..=b'z' => Some(u32::from(byte - b'a') + 26),
        b'0'..=b'9' => Some(u32::from(byte - b'0') + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// Decode one strictly padded base64 payload. Only canonical quad-aligned
/// input with `=` padding in the final quad decodes; the engine's
/// `g_base64_encode` output always has this shape, so anything else is a
/// corrupted or foreign channel and fails closed.
fn decode_base64(text: &str) -> io::Result<Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return Err(io::Error::other(
            "base64 payload is not a padded quad sequence",
        ));
    }
    let mut decoded = Vec::with_capacity(bytes.len() / 4 * 3);
    let quads = bytes.len() / 4;
    for (index, quad) in bytes.chunks_exact(4).enumerate() {
        let padding = match (quad[2], quad[3]) {
            (_, b'=') if index + 1 == quads => {
                if quad[2] == b'=' {
                    2
                } else {
                    1
                }
            }
            (b'=', _) | (_, b'=') => {
                return Err(io::Error::other("base64 payload pads a non-final quad"));
            }
            _ => 0,
        };
        let mut values = [0u32; 4];
        for (position, byte) in quad.iter().enumerate().take(4 - padding) {
            values[position] = base64_value(*byte)
                .ok_or_else(|| io::Error::other("base64 payload has a non-alphabet byte"))?;
        }
        let triple = (values[0] << 18) | (values[1] << 12) | (values[2] << 6) | values[3];
        decoded.push((triple >> 16) as u8);
        if padding < 2 {
            decoded.push((triple >> 8) as u8);
        }
        if padding < 1 {
            decoded.push(triple as u8);
        }
    }
    Ok(decoded)
}
#[cfg(test)]
mod tests {
    use super::*;

    /// Engine schemas carry engine-emitted floats with up to 17 significant
    /// digits. The pinned inventory was recorded by a correctly rounding
    /// parser, so this client must decode the exact same double; the default
    /// fast float parse is one ulp off for values like the grain default and
    /// would refuse an unmodified engine as `engine schema differs`.
    #[test]
    #[allow(clippy::excessive_precision)]
    fn engine_floats_decode_to_the_exact_recorded_double() {
        let value: serde_json::Value =
            serde_json::from_str(r#"{"default":7.5046906471252441}"#).unwrap();
        assert_eq!(
            value["default"].as_f64().unwrap().to_bits(),
            7.5046906471252441_f64.to_bits()
        );
        let shortest: serde_json::Value =
            serde_json::from_str(r#"{"default":7.504690647125244}"#).unwrap();
        assert_eq!(value, shortest);
    }

    /// A guarded run never outlives its deadline: the watchdog kills the
    /// whole engine process group, which unblocks the protocol read the
    /// owner thread is stuck in, and the attempt fails instead of hanging.
    #[test]
    fn a_hanging_engine_is_killed_at_its_deadline() {
        let mut engine = McpClient::spawn_guarded(
            "/bin/sleep",
            &["300".to_string()],
            &[],
            Arc::new(AtomicBool::new(false)),
            Instant::now() + Duration::from_millis(100),
        )
        .unwrap();
        let started = Instant::now();
        assert!(engine.initialize("watchdog-probe").is_err());
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the deadline watchdog did not unblock the run promptly"
        );
    }

    /// The same group-kill authority answers the cancellation flag even
    /// long before the deadline.
    #[test]
    fn a_hanging_engine_is_killed_once_cancelled() {
        let cancellation = Arc::new(AtomicBool::new(false));
        let mut engine = McpClient::spawn_guarded(
            "/bin/sleep",
            &["300".to_string()],
            &[],
            cancellation.clone(),
            Instant::now() + Duration::from_secs(600),
        )
        .unwrap();
        cancellation.store(true, Ordering::Relaxed);
        let started = Instant::now();
        assert!(engine.initialize("watchdog-probe").is_err());
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the cancellation watchdog did not unblock the run promptly"
        );
    }

    /// The image content block decodes exactly the engine's base64 PNG
    /// payload and refuses every other block shape or refusal channel.
    #[test]
    fn image_content_blocks_decode_only_one_canonical_png_block() {
        let png = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        let data = "iVBORw0KGgo=";
        let rendered = serde_json::json!({
            "result": {
                "content": [{
                    "type": "image",
                    "mimeType": "image/png",
                    "data": data,
                }],
                "isError": false,
            }
        });
        assert_eq!(decode_image_png(&rendered, "render").unwrap(), png);

        let refused = |value: serde_json::Value| decode_image_png(&value, "render").is_err();
        // A JSON-RPC level error and an isError text refusal.
        assert!(refused(serde_json::json!({"error": {"code": -1}})));
        assert!(refused(serde_json::json!({
            "result": {
                "content": [{"type": "text", "text": "render failed"}],
                "isError": true,
            }
        })));
        // Wrong block shape, mime type, missing data, and extra blocks.
        assert!(refused(serde_json::json!({"result": {"content": []}})));
        assert!(refused(serde_json::json!({
            "result": {
                "content": [
                    {"type": "image", "mimeType": "image/png", "data": data},
                    {"type": "text", "text": "extra"},
                ],
                "isError": false,
            }
        })));
        assert!(refused(serde_json::json!({
            "result": {
                "content": [{"type": "image", "mimeType": "image/jpeg", "data": data}],
                "isError": false,
            }
        })));
        assert!(refused(serde_json::json!({
            "result": {
                "content": [{"type": "image", "mimeType": "image/png"}],
                "isError": false,
            }
        })));
        assert!(refused(serde_json::json!({"result": {"isError": false}})));
        assert!(refused(serde_json::json!({
            "result": {
                "content": [{"type": "image", "mimeType": "image/png", "data": "not base64!"}],
                "isError": false,
            }
        })));
    }

    /// Only canonical padded base64 decodes: known vectors round-trip and
    /// every malformed payload fails closed.
    #[test]
    fn base64_decodes_canonical_padding_and_refuses_malformed_payloads() {
        assert_eq!(decode_base64("").unwrap(), Vec::<u8>::new());
        assert_eq!(decode_base64("TWFu").unwrap(), b"Man".to_vec());
        assert_eq!(decode_base64("TWE=").unwrap(), b"Ma".to_vec());
        assert_eq!(decode_base64("TQ==").unwrap(), b"M".to_vec());
        assert_eq!(decode_base64("AAAA").unwrap(), vec![0, 0, 0]);
        assert_eq!(decode_base64("/+8=").unwrap(), vec![0xff, 0xef]);

        for malformed in [
            "T", "TWE", "TWE=TWFu", "TW E=", "T*==", "T=W=", "=TWF", "TQ=",
        ] {
            assert!(decode_base64(malformed).is_err(), "{malformed} decoded");
        }
    }
}
