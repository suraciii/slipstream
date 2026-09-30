//! Minimal deterministic MCP stdio client for the Photo worker.
//!
//! Speaks the MCP 2025-06-18 stdio transport the pinned darktable-mcp fork
//! implements: one JSON-RPC 2.0 object per line, server stdout only. The
//! worker owns this client; no LLM, no HTTP, and no ambient tool discovery.
//! Every response is decoded as a bounded JSON value before use, and a
//! truncated, oversized, or non-JSON line is a hard failure: the engine
//! either answers the pinned protocol or the attempt fails closed.

use std::{
    io::{self, BufRead, BufReader, Read, Write},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// One newline-delimited protocol message may not exceed this bound; the
/// worker reads engine metadata, never image bytes, over this channel.
const LINE_BYTES_MAX: usize = 16 * 1024 * 1024;

pub struct McpClient {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl McpClient {
    /// Start the engine with an explicit argument vector and environment.
    /// No shell, no inherited environment: the pinned bundle owns the
    /// process identity.
    pub fn spawn(program: &str, args: &[String], env: &[(String, String)]) -> io::Result<Self> {
        let mut command = Command::new(program);
        command
            .args(args)
            .env_clear()
            .envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("engine stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("engine stdout unavailable"))?;
        Ok(Self {
            child,
            stdin: Some(stdin),
            stdout: BufReader::new(stdout),
            next_id: 1,
        })
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
        let value: serde_json::Value = serde_json::from_slice(&line)
            .map_err(|_| io::Error::other("engine response is not JSON"))?;
        let object = value
            .as_object()
            .ok_or_else(|| io::Error::other("engine response is not a JSON-RPC object"))?;
        if object.get("jsonrpc").and_then(|v| v.as_str()) != Some("2.0")
            || object.get("id").and_then(|v| v.as_i64()) != Some(id)
            || object.contains_key("result") == object.contains_key("error")
        {
            return Err(io::Error::other(
                "engine response does not match the request",
            ));
        }
        Ok(value)
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
        // Kill and reap on every failure path; the attempt timer also tears
        // down descendants if the worker itself cannot finish cleanup.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
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
}
