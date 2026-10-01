mod cgroup;
mod lifecycle;
mod process;

pub(crate) use cgroup::{
    cgroup_unpopulated, delegate_workload_controllers, events, limits_match, mount_identity,
    process_cgroup, read, require_unlimited_ancestor, secure_directory,
};
pub(crate) use process::{command, command_until, strings};

use crate::{
    journal::{ManagerPhase, ParentIdentity, Record},
    protocol::*,
};
use cgroup::{CGROUP, storage_matches};
use serde_json::Value;
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    os::{
        fd::{AsRawFd, OwnedFd},
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};

pub(crate) type Result<T> = std::result::Result<T, ErrorCode>;
const WORKER: &str = "/usr/local/bin/slipstream-processing-worker";

/// Creates one launcher-private directory at mode `0700`. The attempt
/// workspace is created once and then derived again: admission seals the source
/// copy into the workspace before provisioning reaches the same path, and a
/// recovered attempt may reach it a third time. A repeated creation is
/// therefore admitted, but only for a real directory that is not a link, and
/// the mode is enforced on every call.
pub(crate) fn create_private_directory(path: &Path) -> Result<()> {
    match fs::create_dir(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(path).map_err(|_| ErrorCode::Uncertain)?;
            if !metadata.is_dir() {
                return Err(ErrorCode::Uncertain);
            }
        }
        Err(_) => return Err(ErrorCode::Uncertain),
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|_| ErrorCode::Unavailable)
}

pub(crate) fn create_control_directory(path: &Path) -> Result<()> {
    fs::create_dir(path).map_err(|_| ErrorCode::Unavailable)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).map_err(|_| ErrorCode::Unavailable)
}

pub(crate) fn create_native_gate(path: &Path) -> Result<File> {
    let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| ErrorCode::Unavailable)?;
    // SAFETY: the path is a new child of the private, owned control directory.
    if unsafe { libc::mkfifo(c_path.as_ptr(), 0o644) } != 0 {
        return Err(ErrorCode::Unavailable);
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o644))
        .map_err(|_| ErrorCode::Unavailable)?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| ErrorCode::Unavailable)
}

pub(crate) enum Gate {
    Native(File),
    Film(Box<crate::staging::Session>),
}

#[derive(Clone)]
pub(crate) struct Backend {
    pub config: Config,
}

pub(crate) struct Live {
    pub running: bool,
    pub pid: u32,
    pub exit_code: Option<u8>,
    pub oom: bool,
}

pub(crate) fn observed_exit(state: &serde_json::Value) -> Result<Option<u8>> {
    let status = state["Status"].as_str().ok_or(ErrorCode::Uncertain)?;
    let started = state["StartedAt"].as_str().ok_or(ErrorCode::Uncertain)?;
    if !matches!(status, "exited" | "dead") || started.starts_with("0001-01-01T") {
        return Ok(None);
    }
    state["ExitCode"]
        .as_u64()
        .and_then(|code| u8::try_from(code).ok())
        .map(Some)
        .ok_or(ErrorCode::Uncertain)
}

fn checked_setup_observation(
    record: &mut Record,
    persist: &mut impl FnMut(&Record) -> Result<()>,
    observation: Result<bool>,
) -> Result<()> {
    // An unavailable read or a bootstrap exit is not positive evidence of
    // tampering. Only an observed unequal placement/limit invalidates context.
    if observation? {
        return Ok(());
    }
    if let Some(captured) = record.film.as_mut()
        && captured.grant.plan.qualified().is_some()
    {
        captured.qualification_observation_valid = Some(false);
        persist(record)?;
    }
    Err(ErrorCode::Unavailable)
}

pub(crate) fn docker(root: &str, args: &[String]) -> Result<String> {
    let mut fixed = vec![
        "--host".into(),
        "unix:///var/run/docker.sock".into(),
        "--config".into(),
        format!("{root}/docker-client"),
    ];
    fixed.extend_from_slice(args);
    command("/usr/bin/docker", &fixed)
}

pub(crate) fn systemctl(args: &[String]) -> Result<String> {
    let mut fixed = vec!["--system".into()];
    fixed.extend_from_slice(args);
    command("/usr/bin/systemctl", &fixed)
}

impl Backend {
    fn docker(&self, args: &[String]) -> Result<String> {
        docker(&self.config.root, args)
    }

    fn systemctl(&self, args: &[String]) -> Result<String> {
        systemctl(args)
    }

    fn worker(&self) -> &'static str {
        if matches!(self.config.version, 2 | 3) {
            crate::film::WORKER
        } else {
            WORKER
        }
    }

    pub fn environment(&self) -> Result<crate::qualified::Environment> {
        let version = self.docker(&strings(&["version", "--format", "{{json .Server}}"]))?;
        let runtimes = self.docker(&strings(&["info", "--format", "{{json .Runtimes}}"]))?;
        crate::environment::observe(&version, &runtimes)
    }

    pub fn verify_film_image(&self, image: &str, catalogue: &crate::film::Catalogue) -> Result<()> {
        let value: Value = serde_json::from_str(&self.docker(&strings(&[
            "image",
            "inspect",
            "--format",
            "{{json .Config.Labels}}",
            image,
        ]))?)
        .map_err(|_| ErrorCode::Unavailable)?;
        for (key, expected) in [
            ("numerical_bundle", &catalogue.numerical_bundle),
            ("recipe", &catalogue.recipe),
            ("input_icc_sha256", &catalogue.input_icc_sha256),
            ("output_icc_sha256", &catalogue.output_icc_sha256),
            ("procedure", &catalogue.procedure),
        ] {
            if value[format!("slipstream.processing.film.{key}")] != *expected {
                return Err(ErrorCode::IncompatibleBundle);
            }
        }
        Ok(())
    }

    pub fn image(&self) -> Result<String> {
        let info: Value =
            serde_json::from_str(&self.docker(&strings(&["info", "--format", "{{json .}}"]))?)
                .map_err(|_| ErrorCode::Unavailable)?;
        if info["CgroupDriver"] != "systemd"
            || info["CgroupVersion"] != "2"
            || info["SecurityOptions"].as_array().is_none_or(|options| {
                options.iter().any(|option| {
                    option
                        .as_str()
                        .is_some_and(|text| text.contains("rootless") || text.contains("userns"))
                })
            })
        {
            return Err(ErrorCode::Unavailable);
        }
        let text = self.docker(&strings(&[
            "image",
            "inspect",
            "--format",
            "{{json .}}",
            &self.config.image,
        ]))?;
        let image: Value = serde_json::from_str(&text).map_err(|_| ErrorCode::Unavailable)?;
        let id = image["Id"].as_str().ok_or(ErrorCode::Unavailable)?;
        if !id
            .strip_prefix("sha256:")
            .is_some_and(|value| hex(value, 64))
            || image["Config"]["Entrypoint"] != serde_json::json!([self.worker()])
        {
            return Err(ErrorCode::Unavailable);
        }
        Ok(id.to_owned())
    }

    pub fn check_caller(&self, pid: u32) -> Result<()> {
        let parent = self.parent_path();
        for pid in [pid, std::process::id()] {
            let path = process_cgroup(pid)?;
            if path.starts_with(&parent) {
                return Err(ErrorCode::Unavailable);
            }
            for ancestor in parent.ancestors() {
                if path.starts_with(ancestor) {
                    require_unlimited_ancestor(ancestor, ancestor == Path::new(CGROUP))?;
                }
                if ancestor == Path::new(CGROUP) {
                    break;
                }
            }
        }
        Ok(())
    }

    pub fn parent_path(&self) -> PathBuf {
        Path::new(CGROUP).join(self.config.parent_unit())
    }

    pub fn verify_parent(&self, identity: Option<&ParentIdentity>) -> Result<()> {
        let path = self.parent_path();
        if let Some(identity) = identity {
            if fs::metadata(&path).map_err(|_| ErrorCode::Uncertain)?.ino() != identity.inode
                || self.property(&self.config.parent_unit(), "InvocationID")? != identity.invocation
                || !read(&path.join("cgroup.procs"))?.is_empty()
            {
                return Err(ErrorCode::Uncertain);
            }
            return Ok(());
        }
        // Inventory queries do not synthesize/load a unit and cannot grant ownership.
        if path.exists()
            || !self
                .systemctl(&strings(&[
                    "list-units",
                    "--all",
                    "--plain",
                    "--no-legend",
                    "--no-pager",
                    &self.config.parent_unit(),
                ]))?
                .is_empty()
        {
            return Err(ErrorCode::Uncertain);
        }
        let unit_paths = self.systemctl(&strings(&["show", "--property=UnitPath", "--value"]))?;
        if unit_paths.is_empty() {
            return Err(ErrorCode::Unavailable);
        }
        for directory in unit_paths.split_whitespace() {
            if !Path::new(directory).is_absolute() {
                return Err(ErrorCode::Unavailable);
            }
            for name in [
                self.config.parent_unit(),
                format!("{}.d", self.config.parent_unit()),
            ] {
                match fs::symlink_metadata(Path::new(directory).join(name)) {
                    Ok(_) => return Err(ErrorCode::Uncertain),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => return Err(ErrorCode::Uncertain),
                }
            }
        }
        Ok(())
    }

    pub fn prepare_parent(&self, previous: Option<&ParentIdentity>) -> Result<ParentIdentity> {
        self.check_caller(std::process::id())?;
        self.verify_parent(previous)?;
        self.configure_slice(&self.config.parent_unit(), self.config.memory_bytes)?;
        self.read_limits(&self.parent_path(), self.config.memory_bytes)?;
        let identity = ParentIdentity {
            invocation: self.property(&self.config.parent_unit(), "InvocationID")?,
            inode: fs::metadata(self.parent_path())
                .map_err(|_| ErrorCode::Uncertain)?
                .ino(),
        };
        if !hex(&identity.invocation, 32) || previous.is_some_and(|previous| previous != &identity)
        {
            return Err(ErrorCode::Uncertain);
        }
        self.verify_parent(Some(&identity))?;
        Ok(identity)
    }

    fn configure_slice(&self, name: &str, memory: u64) -> Result<()> {
        self.systemctl(&strings(&[
            "set-property",
            "--runtime",
            name,
            &format!("MemoryMax={memory}"),
            "MemorySwapMax=0",
            &format!("TasksMax={}", self.config.limits().tasks),
            &format!("CPUQuota={}%", self.config.limits().cpu_quota_us / 1000),
        ]))?;
        self.systemctl(&strings(&["start", name]))?;
        if self.property(name, "StopWhenUnneeded")? != "no" {
            return Err(ErrorCode::Unavailable);
        }
        Ok(())
    }

    pub fn property(&self, unit: &str, property: &str) -> Result<String> {
        self.systemctl(&strings(&["show", unit, "--property", property, "--value"]))
    }

    pub fn scan_unowned(&self, records: &[Record]) -> Result<()> {
        if self.parent_path().exists() {
            for entry in fs::read_dir(self.parent_path()).map_err(|_| ErrorCode::Uncertain)? {
                let entry = entry.map_err(|_| ErrorCode::Uncertain)?;
                if entry
                    .file_type()
                    .map_err(|_| ErrorCode::Uncertain)?
                    .is_dir()
                    && !records.iter().any(|record| {
                        record.receipt.state != State::Settled
                            && entry.file_name() == record.unit()
                            && self.verify_unit(record).is_ok()
                    })
                {
                    return Err(ErrorCode::Uncertain);
                }
            }
        }
        let names = self.systemctl(&strings(&[
            "list-units",
            "--all",
            "--plain",
            "--no-legend",
            "--no-pager",
            &format!("slipstreamprocessing{}-*.slice", self.config.instance),
        ]))?;
        for line in names.lines() {
            let name = line.split_whitespace().next().ok_or(ErrorCode::Uncertain)?;
            if !records
                .iter()
                .any(|record| record.receipt.state != State::Settled && record.unit() == name)
            {
                return Err(ErrorCode::Uncertain);
            }
        }
        let ids = self.docker(&strings(&[
            "ps",
            "--all",
            "--no-trunc",
            "--quiet",
            "--filter",
            &format!(
                "label=slipstream.processing.instance={}",
                self.config.instance
            ),
        ]))?;
        for id in ids.lines() {
            if !records.iter().any(|record| {
                record.receipt.state != State::Settled
                    && record
                        .receipt
                        .runtime
                        .as_ref()
                        .and_then(|runtime| runtime.container_id.as_deref())
                        == Some(id)
            }) {
                // A durable pre-create intent can identify a lost create response.
                if !records.iter().any(|record| {
                    record.manager_pending == Some(ManagerPhase::CreateReturned)
                        && record.container_id().is_err()
                        && self.owned_container(record, id).is_ok()
                }) {
                    return Err(ErrorCode::Uncertain);
                }
            }
        }
        Ok(())
    }

    pub fn workspace(&self, record: &Record) -> PathBuf {
        Path::new(&self.config.root)
            .join("attempts")
            .join(&record.launch_id)
    }

    pub fn admission_ready(&self, identity: Option<&ParentIdentity>) -> Result<()> {
        let identity = identity.ok_or(ErrorCode::Unavailable)?;
        if fs::metadata(self.parent_path())
            .map_err(|_| ErrorCode::Unavailable)?
            .ino()
            != identity.inode
            || !read(&self.parent_path().join("cgroup.procs"))?.is_empty()
        {
            return Err(ErrorCode::Unavailable);
        }
        self.read_limits(&self.parent_path(), self.config.memory_bytes)
    }

    fn read_limits(&self, path: &Path, memory: u64) -> Result<()> {
        if limits_match(path, &self.config.limits(), memory)? {
            Ok(())
        } else {
            Err(ErrorCode::Unavailable)
        }
    }

    fn owned_container(&self, record: &Record, id: &str) -> Result<Value> {
        if !hex(id, 64) || record.container_id().is_ok_and(|expected| expected != id) {
            return Err(ErrorCode::Uncertain);
        }
        let value: Value = serde_json::from_str(&self.docker(&strings(&[
            "inspect",
            "--format",
            "{{json .}}",
            id,
        ]))?)
        .map_err(|_| ErrorCode::Uncertain)?;
        if value["Id"] != id
            || value["Image"] != record.image_id
            || value["Name"] != format!("/{}", record.container_name())
            || value["Config"]["Labels"]["slipstream.processing.instance"] != self.config.instance
            || value["Config"]["Labels"]["slipstream.processing.launch"] != record.launch_id
            || value["Config"]["Labels"]["slipstream.processing.incarnation"]
                != record.receipt.incarnation
            || value["HostConfig"]["CgroupParent"] != record.unit()
            || value["Config"]["User"] != "1000:1000"
            || value["Config"]["Entrypoint"] != serde_json::json!([self.worker()])
            || value["Config"]["Cmd"]
                != serde_json::json!([
                    record.receipt.workload.name(),
                    record.launch_id,
                    record.receipt.deadline_unix_ms.to_string()
                ])
            || value["HostConfig"]["LogConfig"]["Type"] != "none"
            || value["HostConfig"]["Privileged"] != false
            || value["HostConfig"]["ReadonlyRootfs"] != true
            || value["HostConfig"]["NetworkMode"] != "none"
            || value["HostConfig"]["PidMode"] != ""
            || value["HostConfig"]["CgroupnsMode"] != "private"
            || (self.config.version == 3 && value["HostConfig"]["Runtime"] != "runc")
            || value["HostConfig"]["CapDrop"] != serde_json::json!(["ALL"])
            || !value["HostConfig"]["CapAdd"].is_null()
            || value["HostConfig"]["SecurityOpt"] != serde_json::json!(["no-new-privileges:true"])
            || value["HostConfig"]["Memory"] != record.receipt.limits.memory_bytes
            || value["HostConfig"]["MemorySwap"] != record.receipt.limits.memory_bytes
            || value["HostConfig"]["PidsLimit"] != record.receipt.limits.tasks
            || value["HostConfig"]["NanoCpus"] != record.receipt.limits.cpu_quota_us * 10000
            || value["Config"]["Tty"] != false
            || value["Config"]["OpenStdin"] != false
        {
            return Err(ErrorCode::Uncertain);
        }
        let mounts = value["Mounts"].as_array().ok_or(ErrorCode::Uncertain)?;
        let expected_mounts = self.mounts(record);
        if mounts.len() != expected_mounts.len() {
            return Err(ErrorCode::Uncertain);
        }
        for (destination, source, writable) in expected_mounts {
            if mounts
                .iter()
                .filter(|mount| {
                    mount["Type"] == "bind"
                        && mount["Destination"] == destination
                        && mount["Source"]
                            .as_str()
                            .is_some_and(|value| Path::new(value) == source)
                        && mount["RW"] == writable
                        && mount["Propagation"] == "rprivate"
                })
                .count()
                != 1
            {
                return Err(ErrorCode::Uncertain);
            }
        }
        Ok(value)
    }

    fn mounts(&self, record: &Record) -> Vec<(&'static str, PathBuf, bool)> {
        let base = self.workspace(record);
        let storage = base.join("work");
        let mut mounts = vec![("/control", base.join("control"), false)];
        if record.film.is_some() {
            mounts.extend([
                ("/input", storage.join("input"), false),
                ("/work", storage.join("work"), true),
                ("/output", storage.join("output"), true),
                ("/tmp", storage.join("work"), true),
                ("/dev/shm", storage.join("work"), true),
            ]);
        } else {
            mounts.extend([
                ("/work", storage.clone(), true),
                ("/tmp", storage.clone(), true),
                ("/dev/shm", storage, true),
            ]);
        }
        mounts
    }
    pub fn verify_film_bootstrap(&self, record: &Record, pid: u32) -> Result<()> {
        let live = self.live(record)?;
        let leaf = self
            .parent_path()
            .join(record.unit())
            .join(format!("docker-{}.scope/workload", record.container_id()?));
        if !live.running
            || live.pid != pid
            || process_cgroup(pid)? != leaf
            || read(&leaf.join("cgroup.procs"))? != pid.to_string()
            || read(&leaf.join("memory.oom.group"))? != "1"
        {
            return Err(ErrorCode::Uncertain);
        }
        self.read_limits(&leaf, record.receipt.limits.memory_bytes)
    }

    pub fn verify_film_limits(&self, record: &Record) -> Result<()> {
        let attempt = self.verify_unit(record)?;
        let scope = attempt.join(format!("docker-{}.scope", record.container_id()?));
        let leaf = scope.join("workload");
        for path in [&attempt, &scope, &leaf] {
            self.read_limits(path, record.receipt.limits.memory_bytes)?;
        }
        let storage = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(self.workspace(record).join("work"))
            .map_err(|_| ErrorCode::Unavailable)?;
        let info = read(Path::new(&format!(
            "/proc/self/fdinfo/{}",
            storage.as_raw_fd()
        )))?;
        let observed: Vec<_> = info
            .lines()
            .filter_map(|line| line.strip_prefix("mnt_id:").map(str::trim))
            .collect();
        if observed.len() != 1 || observed[0].parse::<u64>().ok() != record.mount_id {
            return Err(ErrorCode::Uncertain);
        }
        // SAFETY: storage owns an open directory on the exact captured mount;
        // fstatfs writes a correctly sized C structure and does not mutate it.
        let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstatfs(storage.as_raw_fd(), &mut stat) } != 0 {
            return Err(ErrorCode::Unavailable);
        }
        if !storage_matches(&stat, &record.receipt.limits) {
            return Err(ErrorCode::Unavailable);
        }
        self.audit_film_storage(record)
    }
    pub fn pause_film(
        &self,
        record: &mut Record,
        mut persist: impl FnMut(&Record) -> Result<()>,
    ) -> Result<PathBuf> {
        record.manager_pending = Some(ManagerPhase::Pause);
        persist(record)?;
        self.docker(&strings(&["pause", record.container_id()?]))?;
        record.manager_pending = None;
        persist(record)?;
        let live = self.live(record)?;
        let leaf = process_cgroup(live.pid)?;
        if !live.running
            || self.owned_container(record, record.container_id()?)?["State"]["Paused"] != true
            || leaf
                != self
                    .parent_path()
                    .join(record.unit())
                    .join(format!("docker-{}.scope/workload", record.container_id()?))
            || !read(
                &leaf
                    .parent()
                    .ok_or(ErrorCode::Uncertain)?
                    .join("cgroup.events"),
            )?
            .lines()
            .any(|s| s == "frozen 1")
        {
            return Err(ErrorCode::Uncertain);
        }
        self.audit_film_storage(record)?;
        Ok(leaf)
    }
    pub fn audit_film_storage(&self, record: &Record) -> Result<()> {
        let captured = record.film.as_ref().ok_or(ErrorCode::Uncertain)?;
        let storage = self.workspace(record).join("work");
        if mount_identity(&storage)? != record.mount_id {
            return Err(ErrorCode::Uncertain);
        }
        for (name, expected, length) in [
            (
                "input/grant.json",
                captured.grant_file.as_ref(),
                crate::film::canonical(&captured.grant)?.len() as u64,
            ),
            (
                "native/result",
                captured.result_file.as_ref(),
                crate::film::FRAME as u64,
            ),
            (
                "input/input.tif",
                captured.snapshot.as_ref(),
                if captured.phase == crate::film::Phase::Preparing {
                    0
                } else {
                    captured.grant.fixture.source_bytes()
                },
            ),
        ] {
            if let Some(expected) = expected {
                let dir = File::open(&storage).map_err(|_| ErrorCode::Uncertain)?;
                let file = crate::staging::safe_open(&dir, name, libc::O_RDONLY)
                    .map_err(|_| ErrorCode::Uncertain)?;
                let meta = file.metadata().map_err(|_| ErrorCode::Uncertain)?;
                if crate::staging::identity(&file).map_err(|_| ErrorCode::Uncertain)? != *expected
                    || !meta.is_file()
                    || meta.uid() != 0
                    || meta.nlink() != 1
                    || meta.mode() & 0o777 != 0o444
                    || meta.len() != length
                {
                    return Err(ErrorCode::Uncertain);
                }
            }
        }
        Ok(())
    }
    pub fn film_result(
        &self,
        record: &Record,
        held: Option<&File>,
    ) -> Result<Option<crate::film::WorkerResult>> {
        let captured = record.film.as_ref().ok_or(ErrorCode::Uncertain)?;
        let Some(expected) = &captured.result_file else {
            return Ok(None);
        };
        let storage = self.workspace(record).join("work");
        if mount_identity(&storage)? != record.mount_id {
            return Err(ErrorCode::Uncertain);
        }
        let reopened;
        let file = if let Some(file) = held {
            file
        } else {
            let dir = File::open(&storage).map_err(|_| ErrorCode::Uncertain)?;
            reopened = crate::staging::safe_open(&dir, "native/result", libc::O_RDONLY)
                .map_err(|_| ErrorCode::Uncertain)?;
            &reopened
        };
        let meta = file.metadata().map_err(|_| ErrorCode::Uncertain)?;
        if crate::staging::identity(file).map_err(|_| ErrorCode::Uncertain)? != *expected
            || meta.uid() != 0
            || !meta.is_file()
            || meta.nlink() != 1
            || meta.mode() & 0o777 != 0o444
            || meta.len() != crate::film::FRAME as u64
        {
            return Err(ErrorCode::Uncertain);
        }
        crate::staging::read_result(file, &captured.grant)
    }

    pub fn live(&self, record: &Record) -> Result<Live> {
        let value = self.owned_container(record, record.container_id()?)?;
        Ok(Live {
            running: value["State"]["Running"]
                .as_bool()
                .ok_or(ErrorCode::Uncertain)?,
            pid: value["State"]["Pid"]
                .as_u64()
                .and_then(|pid| u32::try_from(pid).ok())
                .ok_or(ErrorCode::Uncertain)?,
            exit_code: observed_exit(&value["State"])?,
            oom: value["State"]["OOMKilled"]
                .as_bool()
                .ok_or(ErrorCode::Uncertain)?,
        })
    }
}

pub(crate) fn verify_slice_phase(
    pending: Option<ManagerPhase>,
    stop_confirmed: bool,
    active: impl FnOnce() -> Result<()>,
    stopped: impl FnOnce() -> Result<()>,
) -> Result<()> {
    if pending == Some(ManagerPhase::SliceStop) || (stop_confirmed && pending.is_some()) {
        return Err(ErrorCode::Uncertain);
    }
    if stop_confirmed { stopped() } else { active() }
}

/// The launcher-owned worker result: one fixed-size, NUL-padded JSON record
/// binding the attempt's launch id to its terminal outcome. A missing file
/// is one unfinished attempt, and an unparseable body is treated the same;
/// every violation of the sealed shape — wrong size, trailing non-padding,
/// a foreign launch id — is tamper evidence.
pub(crate) fn worker_result(path: &Path, launch_id: &str) -> Result<Option<Outcome>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(ErrorCode::Uncertain),
    };
    let metadata = file.metadata().map_err(|_| ErrorCode::Uncertain)?;
    if !metadata.is_file() || metadata.len() != RESULT_BYTES as u64 {
        return Err(ErrorCode::Uncertain);
    }
    let mut bytes = Vec::new();
    file.take(RESULT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ErrorCode::Uncertain)?;
    if bytes.len() != RESULT_BYTES {
        return Err(ErrorCode::Uncertain);
    }
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    if bytes[end..].iter().any(|byte| *byte != 0) {
        return Err(ErrorCode::Uncertain);
    }
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ResultFile {
        launch_id: String,
        outcome: Outcome,
    }
    let value: ResultFile = match serde_json::from_slice(&bytes[..end]) {
        Ok(value) => value,
        Err(_) => return Ok(None),
    };
    if value.launch_id != launch_id {
        return Err(ErrorCode::Uncertain);
    }
    Ok(Some(value.outcome))
}

fn pidfd_alive(pidfd: &OwnedFd) -> Result<()> {
    let mut descriptor = libc::pollfd {
        fd: pidfd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: descriptor is a live pidfd and the single pollfd buffer is valid.
    if unsafe { libc::poll(&mut descriptor, 1, 0) } != 0 || descriptor.revents != 0 {
        return Err(ErrorCode::Uncertain);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
