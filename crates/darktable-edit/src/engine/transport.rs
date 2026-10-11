use serde_json::{Value, json};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::process::CommandExt,
    path::Path,
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

const FRAME_MAX: usize = 16 * 1024 * 1024;
const STREAM_MAX: usize = 128 * 1024 * 1024;
pub(super) const DEADLINE_SECONDS: u64 = 120;
pub(super) const OUTPUT_MAX: u64 = 4 * 1024 * 1024 * 1024;

extern "C" fn terminate(_: libc::c_int) {
    unsafe {
        libc::kill(-libc::getpgrp(), libc::SIGKILL);
        libc::_exit(1);
    }
}

/// Private supervisor entry point; arguments are parent PID, executable, native argv.
pub fn run_supervisor(arguments: &[String]) -> io::Result<i32> {
    let parent = arguments
        .first()
        .ok_or_else(|| io::Error::other("missing parent PID"))?
        .parse::<libc::pid_t>()
        .map_err(io::Error::other)?;
    let program = arguments
        .get(1)
        .ok_or_else(|| io::Error::other("missing native executable"))?;
    unsafe {
        if libc::getpgrp() != libc::getpid() {
            return Err(io::Error::other(
                "supervisor does not own its process group",
            ));
        }
        if libc::signal(libc::SIGTERM, terminate as *const () as usize) == libc::SIG_ERR
            || libc::signal(libc::SIGINT, terminate as *const () as usize) == libc::SIG_ERR
        {
            return Err(io::Error::last_os_error());
        }
        if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM, 0, 0, 0) != 0 {
            return Err(io::Error::last_os_error());
        }
        if libc::getppid() != parent {
            terminate(libc::SIGTERM);
        }
    }
    let supervisor = unsafe { libc::getpid() };
    let mut command = Command::new(program);
    command.args(&arguments[2..]);
    unsafe {
        command.pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) != 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::getppid() != supervisor {
                return Err(io::Error::other("supervisor disappeared"));
            }
            for (resource, limit) in [
                (libc::RLIMIT_AS, 8 * 1024 * 1024 * 1024u64),
                (libc::RLIMIT_FSIZE, OUTPUT_MAX),
                (libc::RLIMIT_CPU, DEADLINE_SECONDS),
                (libc::RLIMIT_NOFILE, 256),
                (libc::RLIMIT_CORE, 0),
            ] {
                let value = libc::rlimit {
                    rlim_cur: limit as libc::rlim_t,
                    rlim_max: limit as libc::rlim_t,
                };
                if libc::setrlimit(resource, &value) != 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
    Ok(command.status()?.code().unwrap_or(1))
}

pub(super) struct Client {
    child: Child,
    pgid: libc::pid_t,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
    done: Arc<AtomicBool>,
    watchdog: Option<thread::JoinHandle<()>>,
    bytes: usize,
}
impl Client {
    pub(super) fn spawn(
        supervisor: &Path,
        binary: &Path,
        arguments: &[String],
        environment: &[(String, String)],
        work: &Path,
        input_fds: Vec<libc::c_int>,
    ) -> io::Result<Self> {
        let parent = unsafe { libc::getpid() };
        let mut command = Command::new(supervisor);
        command
            .arg(parent.to_string())
            .arg(binary)
            .args(arguments)
            .env_clear()
            .envs(environment.iter().map(|(k, v)| (k, v)))
            .current_dir(work)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        unsafe {
            command.pre_exec(move || {
                if libc::setpgid(0, 0) != 0 {
                    return Err(io::Error::last_os_error());
                }
                // Parent death is armed before exec too, closing the supervisor startup window.
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL, 0, 0, 0) != 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    return Err(io::Error::other("caller disappeared"));
                }
                for fd in &input_fds {
                    let flags = libc::fcntl(*fd, libc::F_GETFD);
                    if flags < 0 || libc::fcntl(*fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        let mut child = command.spawn()?;
        let pgid = child.id() as libc::pid_t;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        let done = Arc::new(AtomicBool::new(false));
        let armed = done.clone();
        let deadline = Instant::now() + Duration::from_secs(DEADLINE_SECONDS);
        let watchdog = match thread::Builder::new()
            .name("darktable-edit-deadline".into())
            .spawn(move || {
                while !armed.load(Ordering::Acquire) {
                    if Instant::now() >= deadline {
                        unsafe {
                            libc::kill(-pgid, libc::SIGKILL);
                        }
                        break;
                    }
                    thread::sleep(Duration::from_millis(50));
                }
            }) {
            Ok(handle) => handle,
            Err(error) => {
                unsafe {
                    libc::kill(-pgid, libc::SIGKILL);
                }
                let _ = child.wait();
                return Err(error);
            }
        };
        Ok(Self {
            child,
            pgid,
            stdin,
            stdout,
            next: 1,
            done,
            watchdog: Some(watchdog),
            bytes: 0,
        })
    }
    fn send(&mut self, value: &Value) -> io::Result<()> {
        let mut bytes = serde_json::to_vec(value)?;
        bytes.push(b'\n');
        if bytes.len() > FRAME_MAX {
            return Err(io::Error::other("native request exceeds bound"));
        }
        self.stdin.write_all(&bytes)?;
        self.stdin.flush()
    }
    fn request(&mut self, method: &str, params: Value) -> io::Result<Value> {
        let id = self.next;
        self.next += 1;
        self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))?;
        loop {
            let mut line = Vec::new();
            let read = self
                .stdout
                .by_ref()
                .take((FRAME_MAX + 1) as u64)
                .read_until(b'\n', &mut line)?;
            self.bytes = self
                .bytes
                .checked_add(read)
                .ok_or_else(|| io::Error::other("native stream overflow"))?;
            if read == 0
                || line.len() > FRAME_MAX
                || !line.ends_with(b"\n")
                || self.bytes > STREAM_MAX
            {
                return Err(io::Error::other(
                    "native response exceeds bound or closed prematurely",
                ));
            }
            let value: Value = match serde_json::from_slice(&line) {
                Ok(value) => value,
                Err(_) => continue,
            };
            if value["jsonrpc"] != "2.0" || value["id"].as_u64() != Some(id) {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Err(io::Error::other(format!(
                    "native {method} refused: {error}"
                )));
            }
            return value
                .get("result")
                .cloned()
                .ok_or_else(|| io::Error::other("native result missing"));
        }
    }
    pub(super) fn initialize(&mut self, protocol: &str) -> io::Result<()> {
        let result = self.request("initialize", json!({"protocolVersion":protocol,"capabilities":{},"clientInfo":{"name":"darktable-edit","version":"1"}}))?;
        if result["protocolVersion"] != protocol {
            return Err(io::Error::other("native protocol mismatch"));
        }
        self.send(&json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}))
    }
    pub(super) fn tools(&mut self) -> io::Result<Value> {
        self.request("tools/list", json!({}))?
            .get("tools")
            .cloned()
            .ok_or_else(|| io::Error::other("native tools missing"))
    }
    pub(super) fn call(&mut self, name: &str, arguments: Value) -> io::Result<Value> {
        let result = self.request("tools/call", json!({"name":name,"arguments":arguments}))?;
        let content = result["content"]
            .as_array()
            .filter(|v| v.len() == 1)
            .ok_or_else(|| io::Error::other("native result is not one content block"))?;
        let block = content[0]
            .as_object()
            .filter(|v| v.len() == 2)
            .ok_or_else(|| io::Error::other("native content block differs"))?;
        let text = block
            .get("text")
            .and_then(Value::as_str)
            .filter(|_| block.get("type").and_then(Value::as_str) == Some("text"))
            .ok_or_else(|| io::Error::other("native result is not text"))?;
        if result["isError"] != false {
            return Err(io::Error::other(format!("native {name} refused: {text}")));
        }
        serde_json::from_str(text).map_err(io::Error::other)
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-self.pgid, libc::SIGKILL);
        }
        let _ = self.child.wait();
        self.done.store(true, Ordering::Release);
        if let Some(handle) = self.watchdog.take() {
            let _ = handle.join();
        }
    }
}
