use crate::{
    journal::{ManagerPhase, ParentIdentity, Record},
    protocol::*,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Read,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{
            fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub(crate) type Result<T> = std::result::Result<T, ErrorCode>;
const CGROUP: &str = "/sys/fs/cgroup";
const WORKER: &str = "/usr/local/bin/slipstream-processing-worker";

pub(crate) fn create_workspace_directory(path: &Path) -> Result<()> {
    fs::create_dir(path).map_err(|_| ErrorCode::Uncertain)?;
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

fn storage_matches(stat: &libc::statfs, limits: &Limits) -> bool {
    stat.f_type == 0x01021994
        && stat.f_bsize > 0
        && stat.f_blocks.checked_mul(stat.f_bsize as u64) == Some(limits.storage_bytes)
        && stat.f_files == limits.storage_inodes
}

pub(crate) fn delegate_workload_controllers(scope: &Path, leaf: &Path) -> Result<()> {
    write(
        &scope.join("cgroup.subtree_control"),
        "+memory +cpu +pids +io",
    )?;
    // An empty io.stat is valid before any I/O. Missing accounting must keep
    // the blocked workload from being released, just like missing limits.
    read(&leaf.join("io.stat"))?;
    Ok(())
}

impl Backend {
    fn docker(&self, args: &[String]) -> Result<String> {
        let mut fixed = vec![
            "--host".into(),
            "unix:///var/run/docker.sock".into(),
            "--config".into(),
            format!("{}/docker-client", self.config.root),
        ];
        fixed.extend_from_slice(args);
        command("/usr/bin/docker", &fixed)
    }

    fn systemctl(&self, args: &[String]) -> Result<String> {
        let mut fixed = vec!["--system".into()];
        fixed.extend_from_slice(args);
        command("/usr/bin/systemctl", &fixed)
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

    pub fn setup(
        &self,
        record: &mut Record,
        mut persist: impl FnMut(&Record) -> Result<()>,
    ) -> Result<Gate> {
        record.manager_pending = Some(ManagerPhase::Slice);
        persist(record)?;
        let expected = self.parent_path().join(record.unit());
        let (invocation, inode) =
            crate::slice::create(record.unit(), &expected, &record.receipt.limits)?;
        record.unit_invocation = Some(invocation);
        record.cgroup_inode = Some(inode);
        let limits = self.limits_match(&expected, record.receipt.limits.memory_bytes);
        checked_setup_observation(record, &mut persist, limits)?;
        record.manager_pending = None;
        record.receipt.evidence = Some(Evidence {
            peak_bytes: 0,
            exit_code: None,
            docker_oom_killed: None,
            attempt_before: Some(events(&expected)?),
            attempt_after: None,
            parent_before: Some(events(&self.parent_path())?),
            parent_after: None,
            populated: Some(false),
            terminal_snapshot: None,
        });
        persist(record)?;
        crate::faults::at(&self.config, record, crate::faults::Phase::Slice)?;
        let workspace = self.workspace(record);
        create_workspace_directory(&workspace)?;
        let control = workspace.join("control");
        let work = workspace.join("work");
        create_control_directory(&control)?;
        fs::create_dir(&work).map_err(|_| ErrorCode::Unavailable)?;
        record.manager_pending = Some(ManagerPhase::Mount);
        persist(record)?;
        command(
            "/usr/bin/mount",
            &strings(&[
                "-t",
                "tmpfs",
                "-o",
                &format!(
                    "size={},nr_inodes={},noswap,nodev,nosuid,noexec,uid={},gid={},mode=0700",
                    record.receipt.limits.storage_bytes,
                    record.receipt.limits.storage_inodes,
                    if record.film.is_some() { 0 } else { 1000 },
                    if record.film.is_some() { 0 } else { 1000 }
                ),
                &format!("slipstream-{}", record.launch_id),
                work.to_str().ok_or(ErrorCode::Unavailable)?,
            ]),
        )?;
        record.mount_id = Some(mount_identity(&work)?.ok_or(ErrorCode::Uncertain)?);
        record.manager_pending = None;
        persist(record)?;
        let gate = if record.film.is_some() {
            let session =
                crate::staging::Session::prepare(Path::new(&self.config.root), &workspace, record)?;
            persist(record)?;
            Gate::Film(Box::new(session))
        } else {
            Gate::Native(create_native_gate(&control.join("gate"))?)
        };
        let mut args = strings(&[
            "create",
            "--pull",
            "never",
            "--name",
            &record.container_name(),
            "--label",
            &format!("slipstream.processing.instance={}", self.config.instance),
            "--label",
            &format!("slipstream.processing.launch={}", record.launch_id),
            "--label",
            &format!(
                "slipstream.processing.incarnation={}",
                record.receipt.incarnation
            ),
            "--cgroup-parent",
            record.unit(),
            "--cgroupns",
            "private",
            "--network",
            "none",
            "--user",
            "1000:1000",
            "--read-only",
            "--cap-drop",
            "ALL",
            "--security-opt",
            "no-new-privileges:true",
            "--log-driver",
            "none",
            "--memory",
            &record.receipt.limits.memory_bytes.to_string(),
            "--memory-swap",
            &record.receipt.limits.memory_bytes.to_string(),
            "--pids-limit",
            &record.receipt.limits.tasks.to_string(),
            "--cpus",
            &(record.receipt.limits.cpu_quota_us / 100000).to_string(),
            "--mount",
            &format!(
                "type=bind,source={},target=/control,readonly",
                control.display()
            ),
        ]);
        for (destination, source, writable) in self
            .mounts(record)
            .into_iter()
            .filter(|(target, _, _)| *target != "/control")
        {
            args.extend(strings(&[
                "--mount",
                &format!(
                    "type=bind,source={},target={destination}{}",
                    source.display(),
                    if writable { "" } else { ",readonly" }
                ),
            ]));
        }
        if self.config.version == 3 {
            args.extend(strings(&["--runtime", "runc"]));
        }
        args.extend(strings(&[
            &record.image_id,
            record.receipt.workload.name(),
            &record.launch_id,
            &record.receipt.deadline_unix_ms.to_string(),
        ]));
        record.manager_pending = Some(ManagerPhase::Create);
        persist(record)?;
        let id = self.docker(&args)?;
        if !hex(&id, 64) {
            return Err(ErrorCode::Uncertain);
        }
        record.manager_pending = Some(ManagerPhase::CreateReturned);
        persist(record)?;
        crate::faults::at(&self.config, record, crate::faults::Phase::CreateResponse)?;
        record
            .receipt
            .runtime
            .as_mut()
            .ok_or(ErrorCode::Uncertain)?
            .container_id = Some(id);
        record.manager_pending = None;
        persist(record)?;
        crate::faults::at(&self.config, record, crate::faults::Phase::ContainerBound)?;
        record.manager_pending = Some(ManagerPhase::Start);
        persist(record)?;
        self.docker(&strings(&["start", record.container_id()?]))?;
        record.manager_pending = None;
        persist(record)?;
        record.manager_pending = Some(ManagerPhase::Pause);
        persist(record)?;
        self.docker(&strings(&["pause", record.container_id()?]))?;
        record.manager_pending = None;
        persist(record)?;
        let live = self.live(record)?;
        if !live.running || live.pid == 0 {
            return Err(ErrorCode::Unavailable);
        }
        let scope = process_cgroup(live.pid)?;
        let placed = scope.parent() == Some(expected.as_path())
            && scope.file_name().and_then(|value| value.to_str())
                == Some(&format!("docker-{}.scope", record.container_id()?));
        checked_setup_observation(record, &mut persist, Ok(placed))?;
        if !read(&scope.join("cgroup.events"))?
            .lines()
            .any(|line| line == "frozen 1")
            || self.owned_container(record, record.container_id()?)?["State"]["Paused"] != true
        {
            return Err(ErrorCode::Uncertain);
        }
        // Freezing prevents the bootstrap timer and ordinary exit while placing it.
        // pidfd and the opened proc directory detect disappearance without retargeting
        // readback to a reused PID. External privileged interference is not qualified.
        let process =
            File::open(format!("/proc/{}", live.pid)).map_err(|_| ErrorCode::Uncertain)?;
        // SAFETY: pidfd_open accepts this checked positive PID and flags zero.
        let descriptor = unsafe { libc::syscall(libc::SYS_pidfd_open, live.pid, 0) };
        if descriptor < 0 {
            return Err(ErrorCode::Uncertain);
        }
        // SAFETY: successful pidfd_open returns a fresh descriptor owned by this call.
        let pidfd = unsafe { OwnedFd::from_raw_fd(descriptor as i32) };
        let proc_path = PathBuf::from(format!("/proc/self/fd/{}", process.as_raw_fd()));
        let placed = read(&proc_path.join("cgroup"))?
            == format!(
                "0::{}",
                scope
                    .strip_prefix(CGROUP)
                    .map_err(|_| ErrorCode::Uncertain)?
                    .display()
            )
            .replace("0::", "0::/");
        checked_setup_observation(record, &mut persist, Ok(placed))?;
        pidfd_alive(&pidfd)?;
        let limits = self.limits_match(&scope, record.receipt.limits.memory_bytes);
        checked_setup_observation(record, &mut persist, limits)?;
        let leaf = scope.join("workload");
        fs::create_dir(&leaf).map_err(|_| ErrorCode::Unavailable)?;
        write(&leaf.join("cgroup.procs"), &live.pid.to_string())?;
        pidfd_alive(&pidfd)?;
        delegate_workload_controllers(&scope, &leaf)?;
        for (key, value) in [
            ("memory.max", record.receipt.limits.memory_bytes.to_string()),
            ("memory.swap.max", "0".into()),
            ("memory.oom.group", "1".into()),
            (
                "cpu.max",
                format!(
                    "{} {}",
                    record.receipt.limits.cpu_quota_us, record.receipt.limits.cpu_period_us
                ),
            ),
            ("pids.max", record.receipt.limits.tasks.to_string()),
        ] {
            write(&leaf.join(key), &value)?;
        }
        let limits = self.limits_match(&leaf, record.receipt.limits.memory_bytes);
        checked_setup_observation(record, &mut persist, limits)?;
        let placed = read(&leaf.join("memory.oom.group"))? == "1"
            && read(&leaf.join("cgroup.procs"))? == live.pid.to_string()
            && process_cgroup(live.pid)? == leaf;
        checked_setup_observation(record, &mut persist, Ok(placed))?;
        self.owned_container(record, record.container_id()?)?;
        Ok(gate)
    }

    pub fn release(
        &self,
        record: &mut Record,
        mut persist: impl FnMut(&Record) -> Result<()>,
    ) -> Result<()> {
        self.owned_container(record, record.container_id()?)?;
        record.manager_pending = Some(ManagerPhase::Unpause);
        persist(record)?;
        self.docker(&strings(&["unpause", record.container_id()?]))?;
        record.manager_pending = None;
        persist(record)
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
        if self.limits_match(path, memory)? {
            Ok(())
        } else {
            Err(ErrorCode::Unavailable)
        }
    }

    fn limits_match(&self, path: &Path, memory: u64) -> Result<bool> {
        Ok(read(&path.join("memory.max"))? == memory.to_string()
            && read(&path.join("memory.swap.max"))? == "0"
            && read(&path.join("cpu.max"))?
                == format!(
                    "{} {}",
                    self.config.limits().cpu_quota_us,
                    self.config.limits().cpu_period_us
                )
            && read(&path.join("pids.max"))? == self.config.limits().tasks.to_string())
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

    pub fn discover(&self, record: &mut Record) -> Result<()> {
        if record
            .manager_pending
            .is_some_and(|phase| phase != ManagerPhase::CreateReturned)
        {
            return Err(ErrorCode::Uncertain);
        }
        if record.manager_pending == Some(ManagerPhase::CreateReturned)
            && record.container_id().is_err()
        {
            let ids = self.docker(&strings(&[
                "ps",
                "--all",
                "--no-trunc",
                "--quiet",
                "--filter",
                &format!("label=slipstream.processing.launch={}", record.launch_id),
            ]))?;
            let ids: Vec<_> = ids.lines().collect();
            match ids.as_slice() {
                [id] => {
                    self.owned_container(record, id)?;
                    record
                        .receipt
                        .runtime
                        .as_mut()
                        .ok_or(ErrorCode::Uncertain)?
                        .container_id = Some((*id).to_owned());
                }
                _ => return Err(ErrorCode::Uncertain),
            }
        }
        let path = self.parent_path().join(record.unit());
        if path.exists() {
            self.verify_unit(record)?;
        }
        let mount = mount_identity(&self.workspace(record).join("work"))?;
        if mount.is_some() && mount != record.mount_id {
            return Err(ErrorCode::Uncertain);
        }
        record.manager_pending = None;
        Ok(())
    }

    fn verify_unit(&self, record: &Record) -> Result<PathBuf> {
        let path = self.parent_path().join(record.unit());
        verify_slice_phase(
            record.manager_pending,
            record.stop_confirmed,
            || {
                crate::slice::verify(
                    record.unit(),
                    &path,
                    record
                        .unit_invocation
                        .as_deref()
                        .ok_or(ErrorCode::Uncertain)?,
                    record.cgroup_inode.ok_or(ErrorCode::Uncertain)?,
                )
            },
            || {
                crate::slice::wait_absent(
                    record.unit(),
                    &path,
                    record
                        .unit_invocation
                        .as_deref()
                        .ok_or(ErrorCode::Uncertain)?,
                    record.cgroup_inode.ok_or(ErrorCode::Uncertain)?,
                )
            },
        )?;
        Ok(path)
    }

    pub fn stop(&self, record: &Record) -> Result<()> {
        let Some(id) = record
            .receipt
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.container_id.as_deref())
        else {
            return Ok(());
        };
        let live = self.live(record)?;
        if live.running {
            self.verify_unit(record)?;
            self.docker(&strings(&["kill", "--signal", "KILL", id]))?;
        }
        Ok(())
    }

    pub fn evidence(&self, record: &Record) -> Result<Evidence> {
        let path = self.verify_unit(record)?;
        let has_container = record
            .receipt
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.container_id.as_ref())
            .is_some();
        let live = if has_container {
            Some(self.live(record)?)
        } else {
            None
        };
        if live.as_ref().is_some_and(|live| live.running) {
            return Err(ErrorCode::Uncertain);
        }
        if !cgroup_unpopulated(&path)? {
            return Err(ErrorCode::Uncertain);
        }
        let (terminal_snapshot, peak_bytes, attempt_after) = if has_container {
            let limit = attempt_memory_limit(record)?;
            let identity = self.terminal_identity(record, &path)?;
            let (snapshot, peak, attempt_events) =
                terminal_snapshot(&path, identity.clone(), limit)?;
            if !cgroup_unpopulated(&path)? {
                return Err(ErrorCode::Uncertain);
            }
            let after_path = self.verify_unit(record)?;
            let after_identity = self.terminal_identity(record, &after_path)?;
            if after_path != path || !same_terminal_identity(&identity, &after_identity) {
                return Err(ErrorCode::Uncertain);
            }
            (Some(snapshot), peak, attempt_events)
        } else {
            (
                None,
                read(&path.join("memory.peak"))?
                    .parse()
                    .map_err(|_| ErrorCode::Uncertain)?,
                events(&path)?,
            )
        };
        Ok(Evidence {
            peak_bytes,
            exit_code: live.as_ref().and_then(|live| live.exit_code),
            docker_oom_killed: live.and_then(|live| live.exit_code.map(|_| live.oom)),
            attempt_before: record
                .receipt
                .evidence
                .as_ref()
                .and_then(|e| e.attempt_before.clone()),
            attempt_after: Some(attempt_after),
            parent_before: record
                .receipt
                .evidence
                .as_ref()
                .and_then(|e| e.parent_before.clone()),
            parent_after: Some(events(&self.parent_path())?),
            populated: Some(false),
            terminal_snapshot,
        })
    }

    fn terminal_identity(&self, record: &Record, path: &Path) -> Result<TerminalSnapshot> {
        let identity = expected_terminal_identity(record, path)?;
        if fs::metadata(path).map_err(|_| ErrorCode::Uncertain)?.ino() != identity.cgroup_inode {
            return Err(ErrorCode::Uncertain);
        }
        Ok(identity)
    }

    pub fn validate_terminal_evidence(&self, record: &Record) -> Result<()> {
        let evidence = record
            .receipt
            .evidence
            .as_ref()
            .ok_or(ErrorCode::Uncertain)?;
        let container_id = record
            .receipt
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.container_id.as_ref());
        if container_id.is_none() {
            return if evidence.terminal_snapshot.is_none() {
                Ok(())
            } else {
                Err(ErrorCode::Uncertain)
            };
        }
        let path = self.parent_path().join(record.unit());
        let expected = expected_terminal_identity(record, &path)?;
        let snapshot = evidence
            .terminal_snapshot
            .as_ref()
            .ok_or(ErrorCode::Uncertain)?;
        if !same_terminal_identity(&expected, snapshot) {
            return Err(ErrorCode::Uncertain);
        }
        let (peak, events) = parse_terminal_values(snapshot, attempt_memory_limit(record)?)?;
        if evidence.peak_bytes != peak || evidence.attempt_after.as_ref() != Some(&events) {
            return Err(ErrorCode::Uncertain);
        }
        Ok(())
    }

    pub fn worker_outcome(&self, record: &Record) -> Result<Option<Outcome>> {
        let path = self.workspace(record).join("work/result");
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
        if !metadata.is_file() || metadata.len() != 4096 {
            return Err(ErrorCode::Uncertain);
        }
        let mut bytes = Vec::new();
        file.take(4097)
            .read_to_end(&mut bytes)
            .map_err(|_| ErrorCode::Uncertain)?;
        if bytes.len() != 4096 {
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
        if value.launch_id != record.launch_id {
            return Err(ErrorCode::Uncertain);
        }
        Ok(Some(value.outcome))
    }

    pub fn cleanup(
        &self,
        record: &mut Record,
        mut persist: impl FnMut(&Record) -> Result<()>,
    ) -> Result<()> {
        if record.manager_pending.is_some() {
            return Err(ErrorCode::Uncertain);
        }
        if let Some(id) = record
            .receipt
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.container_id.as_deref())
        {
            // Removal is idempotent only after an immutable terminal receipt was persisted.
            let ids = self.docker(&strings(&[
                "ps",
                "--all",
                "--no-trunc",
                "--quiet",
                "--filter",
                &format!("id={id}"),
            ]))?;
            if !ids.is_empty() {
                if self.live(record)?.running {
                    return Err(ErrorCode::Uncertain);
                }
                self.docker(&strings(&["rm", id]))?;
            }
        }
        crate::faults::at(&self.config, record, crate::faults::Phase::ContainerRemoval)?;
        let workspace = self.workspace(record);
        let work = workspace.join("work");
        if let Some(current) = mount_identity(&work)? {
            if Some(current) != record.mount_id {
                return Err(ErrorCode::Uncertain);
            }
            command(
                "/usr/bin/umount",
                &strings(&[work.to_str().ok_or(ErrorCode::Uncertain)?]),
            )?;
            if mount_identity(&work)?.is_some() {
                return Err(ErrorCode::Uncertain);
            }
        }
        crate::faults::at(&self.config, record, crate::faults::Phase::StorageUnmount)?;
        if workspace.exists() {
            fs::remove_dir_all(&workspace).map_err(|_| ErrorCode::Uncertain)?;
        }
        if record.unit_invocation.is_some() && !record.stop_confirmed {
            self.verify_unit(record)?;
            record.manager_pending = Some(ManagerPhase::SliceStop);
            persist(record)?;
            crate::faults::at(&self.config, record, crate::faults::Phase::SliceStopIntent)?;
            self.systemctl(&strings(&["stop", record.unit()]))?;
            record.stop_confirmed = true;
            record.manager_pending = None;
            persist(record)?;
        }
        crate::faults::at(&self.config, record, crate::faults::Phase::SliceStop)?;
        let path = self.parent_path().join(record.unit());
        if let Some(invocation) = &record.unit_invocation {
            if !record.stop_confirmed {
                return Err(ErrorCode::Uncertain);
            }
            crate::slice::wait_absent(
                record.unit(),
                &path,
                invocation,
                record.cgroup_inode.ok_or(ErrorCode::Uncertain)?,
            )?;
        } else {
            crate::slice::never_created_absent(record.unit(), &path)?;
        }
        Ok(())
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

pub(crate) fn require_unlimited_ancestor(path: &Path, allow_absent: bool) -> Result<()> {
    match File::open(path.join("memory.max")) {
        Ok(file) => {
            let mut value = String::new();
            file.take(64)
                .read_to_string(&mut value)
                .map_err(|_| ErrorCode::Unavailable)?;
            if value.trim() != "max" {
                return Err(ErrorCode::Unavailable);
            }
            Ok(())
        }
        Err(error) if allow_absent && error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(ErrorCode::Unavailable),
    }
}

pub(crate) fn process_cgroup(pid: u32) -> Result<PathBuf> {
    let text = read(&PathBuf::from(format!("/proc/{pid}/cgroup")))?;
    let path = text.strip_prefix("0::/").ok_or(ErrorCode::Unavailable)?;
    if path.contains('\n') || path.split('/').any(|component| component == "..") {
        return Err(ErrorCode::Unavailable);
    }
    Ok(Path::new(CGROUP).join(path))
}

pub(crate) fn secure_directory(path: &Path, owner: u32) -> Result<()> {
    if !path.is_absolute() || fs::canonicalize(path).map_err(|_| ErrorCode::Unavailable)? != path {
        return Err(ErrorCode::Unavailable);
    }
    for ancestor in path.ancestors() {
        let metadata = fs::symlink_metadata(ancestor).map_err(|_| ErrorCode::Unavailable)?;
        if !metadata.is_dir() || metadata.uid() != owner || metadata.mode() & 0o022 != 0 {
            return Err(ErrorCode::Unavailable);
        }
    }
    Ok(())
}

pub(crate) fn read(path: &Path) -> Result<String> {
    let file = File::open(path).map_err(|_| ErrorCode::Unavailable)?;
    let mut bytes = Vec::new();
    file.take(RESPONSE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ErrorCode::Unavailable)?;
    if bytes.len() > RESPONSE_BYTES {
        return Err(ErrorCode::Unavailable);
    }
    String::from_utf8(bytes)
        .map(|text| text.trim().to_owned())
        .map_err(|_| ErrorCode::Unavailable)
}

fn write(path: &Path, value: &str) -> Result<()> {
    fs::write(path, value).map_err(|_| ErrorCode::Unavailable)
}

fn counter_map(text: &str) -> Result<BTreeMap<&str, u64>> {
    let text = text.strip_suffix('\n').unwrap_or(text);
    if text.is_empty() {
        return Err(ErrorCode::Uncertain);
    }
    let mut counters = BTreeMap::new();
    for line in text.split('\n') {
        let (name, value) = line.split_once(' ').ok_or(ErrorCode::Uncertain)?;
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
            || value.is_empty()
            || !value.bytes().all(|byte| byte.is_ascii_digit())
            || counters
                .insert(name, value.parse().map_err(|_| ErrorCode::Uncertain)?)
                .is_some()
        {
            return Err(ErrorCode::Uncertain);
        }
    }
    Ok(counters)
}

fn events_from_raw(hierarchical: &str, local: &str) -> Result<Events> {
    let all = counter_map(hierarchical)?;
    let local = counter_map(local)?;
    let get = |map: &BTreeMap<&str, u64>, key| map.get(key).copied().ok_or(ErrorCode::Uncertain);
    Ok(Events {
        oom: get(&all, "oom")?,
        oom_kill: get(&all, "oom_kill")?,
        oom_group_kill: get(&all, "oom_group_kill")?,
        local_oom: get(&local, "oom")?,
        local_oom_kill: get(&local, "oom_kill")?,
        local_oom_group_kill: get(&local, "oom_group_kill")?,
    })
}

pub(crate) fn events(path: &Path) -> Result<Events> {
    let all = read(&path.join("memory.events"))?;
    let local = read(&path.join("memory.events.local"))?;
    events_from_raw(&all, &local)
}

pub(crate) fn cgroup_unpopulated(path: &Path) -> Result<bool> {
    let events = read(&path.join("cgroup.events")).map_err(|_| ErrorCode::Uncertain)?;
    Ok(counter_map(&events)?.get("populated") == Some(&0))
}

fn raw_terminal_source(path: &Path, total: &mut usize) -> Result<String> {
    let mut bytes = Vec::new();
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| ErrorCode::Uncertain)?
        .take(crate::protocol::TERMINAL_SNAPSHOT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| ErrorCode::Uncertain)?;
    let raw = String::from_utf8(bytes).map_err(|_| ErrorCode::Uncertain)?;
    count_terminal_source(&raw, total)?;
    Ok(raw)
}

fn count_terminal_source(raw: &str, total: &mut usize) -> Result<()> {
    if raw.len() > crate::protocol::TERMINAL_SNAPSHOT_BYTES
        || !raw.is_ascii()
        || !raw.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'_' | b' ' | b'\n')
        })
    {
        return Err(ErrorCode::Uncertain);
    }
    *total = total
        .checked_add(raw.len())
        .filter(|length| *length <= crate::protocol::TERMINAL_SNAPSHOT_BYTES)
        .ok_or(ErrorCode::Uncertain)?;
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TerminalIoOmission {
    Open(Option<i32>),
    Read(Option<i32>),
    TooLarge,
    InvalidText,
    AggregateLimit,
}

impl TerminalIoOmission {
    fn diagnostic(self) -> String {
        match self {
            Self::Open(Some(errno)) => format!("phase=open errno={errno}"),
            Self::Open(None) => "phase=open errno=unknown".into(),
            Self::Read(Some(errno)) => format!("phase=read errno={errno}"),
            Self::Read(None) => "phase=read errno=unknown".into(),
            Self::TooLarge => "phase=read reason=byte-limit".into(),
            Self::InvalidText => "phase=validate reason=invalid-text".into(),
            Self::AggregateLimit => "phase=validate reason=aggregate-limit".into(),
        }
    }
}

enum TerminalIoCapture {
    Captured(String),
    Omitted(TerminalIoOmission),
}

fn terminal_io_total(raw: &str, total: usize) -> std::result::Result<usize, TerminalIoOmission> {
    if raw.len() > crate::protocol::TERMINAL_SNAPSHOT_BYTES {
        return Err(TerminalIoOmission::TooLarge);
    }
    if !raw.is_ascii()
        || !raw
            .bytes()
            .all(|byte| byte == b'\n' || (b' '..=b'~').contains(&byte))
    {
        return Err(TerminalIoOmission::InvalidText);
    }
    total
        .checked_add(raw.len())
        .filter(|length| *length <= crate::protocol::TERMINAL_SNAPSHOT_BYTES)
        .ok_or(TerminalIoOmission::AggregateLimit)
}

fn optional_terminal_io_source(path: &Path, total: &mut usize) -> TerminalIoCapture {
    let Some(remaining) = crate::protocol::TERMINAL_SNAPSHOT_BYTES.checked_sub(*total) else {
        return TerminalIoCapture::Omitted(TerminalIoOmission::AggregateLimit);
    };
    let mut bytes = Vec::new();
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => file,
        Err(error) => {
            return TerminalIoCapture::Omitted(TerminalIoOmission::Open(error.raw_os_error()));
        }
    };
    if let Err(error) = file.take(remaining as u64 + 1).read_to_end(&mut bytes) {
        return TerminalIoCapture::Omitted(TerminalIoOmission::Read(error.raw_os_error()));
    }
    if bytes.len() > remaining {
        return TerminalIoCapture::Omitted(TerminalIoOmission::TooLarge);
    }
    let raw = match String::from_utf8(bytes) {
        Ok(raw) => raw,
        Err(_) => return TerminalIoCapture::Omitted(TerminalIoOmission::InvalidText),
    };
    let candidate_total = match terminal_io_total(&raw, *total) {
        Ok(total) => total,
        Err(reason) => return TerminalIoCapture::Omitted(reason),
    };
    *total = candidate_total;
    TerminalIoCapture::Captured(raw)
}

fn count_terminal_io_source(raw: &str, total: &mut usize) -> Result<()> {
    *total = terminal_io_total(raw, *total).map_err(|_| ErrorCode::Uncertain)?;
    Ok(())
}

fn raw_terminal_u64(raw: &str) -> Result<u64> {
    let digits = raw.strip_suffix('\n').ok_or(ErrorCode::Uncertain)?;
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ErrorCode::Uncertain);
    }
    digits.parse().map_err(|_| ErrorCode::Uncertain)
}

fn terminal_snapshot(
    path: &Path,
    mut snapshot: TerminalSnapshot,
    expected_memory_limit: u64,
) -> Result<(TerminalSnapshot, u64, Events)> {
    let mut total = 0;
    snapshot.memory_peak_raw = raw_terminal_source(&path.join("memory.peak"), &mut total)?;
    snapshot.memory_max_raw = raw_terminal_source(&path.join("memory.max"), &mut total)?;
    snapshot.memory_swap_current_raw =
        raw_terminal_source(&path.join("memory.swap.current"), &mut total)?;
    snapshot.memory_swap_max_raw = raw_terminal_source(&path.join("memory.swap.max"), &mut total)?;
    snapshot.memory_events_raw = raw_terminal_source(&path.join("memory.events"), &mut total)?;
    snapshot.memory_events_local_raw =
        raw_terminal_source(&path.join("memory.events.local"), &mut total)?;
    snapshot.io_stat_raw = match optional_terminal_io_source(&path.join("io.stat"), &mut total) {
        TerminalIoCapture::Captured(raw) => Some(raw),
        TerminalIoCapture::Omitted(reason) => {
            eprintln!(
                "processing terminal io.stat omitted {}",
                reason.diagnostic()
            );
            None
        }
    };

    let (peak, events) = parse_terminal_values(&snapshot, expected_memory_limit)?;
    Ok((snapshot, peak, events))
}

fn parse_terminal_values(
    snapshot: &TerminalSnapshot,
    expected_memory_limit: u64,
) -> Result<(u64, Events)> {
    let mut total = 0;
    for raw in [
        &snapshot.memory_peak_raw,
        &snapshot.memory_max_raw,
        &snapshot.memory_swap_current_raw,
        &snapshot.memory_swap_max_raw,
        &snapshot.memory_events_raw,
        &snapshot.memory_events_local_raw,
    ] {
        count_terminal_source(raw, &mut total)?;
    }
    if let Some(raw) = &snapshot.io_stat_raw {
        count_terminal_io_source(raw, &mut total)?;
    }
    let peak = raw_terminal_u64(&snapshot.memory_peak_raw)?;
    if raw_terminal_u64(&snapshot.memory_max_raw)? != expected_memory_limit
        || raw_terminal_u64(&snapshot.memory_swap_current_raw)? != 0
        || raw_terminal_u64(&snapshot.memory_swap_max_raw)? != 0
    {
        return Err(ErrorCode::Uncertain);
    }
    let events = events_from_raw(
        &snapshot.memory_events_raw,
        &snapshot.memory_events_local_raw,
    )?;
    Ok((peak, events))
}

fn expected_terminal_identity(record: &Record, path: &Path) -> Result<TerminalSnapshot> {
    let runtime = record
        .receipt
        .runtime
        .as_ref()
        .ok_or(ErrorCode::Uncertain)?;
    let container_id = runtime
        .container_id
        .as_deref()
        .filter(|id| crate::protocol::hex(id, 64))
        .ok_or(ErrorCode::Uncertain)?;
    Ok(TerminalSnapshot {
        cgroup_path: path
            .to_str()
            .map(str::to_owned)
            .ok_or(ErrorCode::Uncertain)?,
        cgroup_inode: record.cgroup_inode.ok_or(ErrorCode::Uncertain)?,
        unit_invocation: record.unit_invocation.clone().ok_or(ErrorCode::Uncertain)?,
        launch_id: record.launch_id.clone(),
        container_id: container_id.to_owned(),
        attempt_unit: runtime.attempt_unit.clone(),
        incarnation: record.receipt.incarnation.clone(),
        sequence: record.receipt.sequence,
        memory_peak_raw: String::new(),
        memory_max_raw: String::new(),
        memory_swap_current_raw: String::new(),
        memory_swap_max_raw: String::new(),
        memory_events_raw: String::new(),
        memory_events_local_raw: String::new(),
        io_stat_raw: None,
    })
}

fn same_terminal_identity(left: &TerminalSnapshot, right: &TerminalSnapshot) -> bool {
    left.cgroup_path == right.cgroup_path
        && left.cgroup_inode == right.cgroup_inode
        && left.unit_invocation == right.unit_invocation
        && left.launch_id == right.launch_id
        && left.container_id == right.container_id
        && left.attempt_unit == right.attempt_unit
        && left.incarnation == right.incarnation
        && left.sequence == right.sequence
}

fn attempt_memory_limit(record: &Record) -> Result<u64> {
    let receipt_limit = record.receipt.limits.memory_bytes;
    let accepted_limit = record
        .film
        .as_ref()
        .and_then(|film| film.grant.plan.qualified())
        .map_or(receipt_limit, |plan| plan.attempt_limit_bytes);
    if receipt_limit != accepted_limit {
        return Err(ErrorCode::Uncertain);
    }
    Ok(accepted_limit)
}

pub(crate) fn mount_identity(path: &Path) -> Result<Option<u64>> {
    let text = read(Path::new("/proc/self/mountinfo"))?;
    for line in text.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields
            .get(4)
            .is_some_and(|target| Path::new(target) == path)
        {
            let separator = fields
                .iter()
                .position(|field| *field == "-")
                .ok_or(ErrorCode::Uncertain)?;
            if fields.get(separator + 1) != Some(&"tmpfs") {
                return Err(ErrorCode::Uncertain);
            }
            return fields[0]
                .parse()
                .map(Some)
                .map_err(|_| ErrorCode::Uncertain);
        }
    }
    Ok(None)
}

pub(crate) fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

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
mod tests {
    use super::*;

    fn snapshot_fixture() -> (PathBuf, TerminalSnapshot) {
        let root = std::env::temp_dir().join(format!(
            "slipstream-terminal-snapshot-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&root).unwrap();
        for (name, contents) in [
            ("memory.peak", "12345\n"),
            ("memory.max", "65536\n"),
            ("memory.swap.current", "0\n"),
            ("memory.swap.max", "0\n"),
            (
                "memory.events",
                "low 0\nhigh 0\nmax 0\noom 1\noom_kill 2\noom_group_kill 3\n",
            ),
            (
                "memory.events.local",
                "low 0\nhigh 0\nmax 0\noom 4\noom_kill 5\noom_group_kill 6\n",
            ),
            (
                "io.stat",
                "8:0 rbytes=1 wbytes=2 rios=3 wios=4 cost.usage=9\n",
            ),
        ] {
            fs::write(root.join(name), contents).unwrap();
        }
        let snapshot = TerminalSnapshot {
            cgroup_path: root.to_str().unwrap().to_owned(),
            cgroup_inode: 11,
            unit_invocation: "1".repeat(32),
            launch_id: "2".repeat(32),
            container_id: "3".repeat(64),
            attempt_unit: "slipstreamprocessing0-22222222222222222222222222222222.slice".into(),
            incarnation: "4".repeat(32),
            sequence: 5,
            memory_peak_raw: String::new(),
            memory_max_raw: String::new(),
            memory_swap_current_raw: String::new(),
            memory_swap_max_raw: String::new(),
            memory_events_raw: String::new(),
            memory_events_local_raw: String::new(),
            io_stat_raw: None,
        };
        (root, snapshot)
    }

    fn padded_event_file(target_bytes: usize) -> String {
        let mut contents = String::from("oom 0\noom_kill 0\noom_group_kill 0\n");
        let mut suffix = 0_u64;
        while contents.len() < target_bytes {
            let line = format!("future_{suffix} 0\n");
            if contents.len() + line.len() > target_bytes {
                break;
            }
            contents.push_str(&line);
            suffix += 1;
        }
        contents
    }

    #[test]
    fn terminal_snapshot_captures_exact_raw_values_and_derives_events() {
        let (root, identity) = snapshot_fixture();
        let (snapshot, peak, events) = terminal_snapshot(&root, identity.clone(), 65536).unwrap();
        assert!(same_terminal_identity(&identity, &snapshot));
        assert_eq!(snapshot.memory_peak_raw, "12345\n");
        assert_eq!(snapshot.memory_max_raw, "65536\n");
        assert_eq!(snapshot.memory_swap_current_raw, "0\n");
        assert_eq!(
            snapshot.memory_events_raw,
            "low 0\nhigh 0\nmax 0\noom 1\noom_kill 2\noom_group_kill 3\n"
        );
        assert_eq!(
            snapshot.io_stat_raw.as_deref(),
            Some("8:0 rbytes=1 wbytes=2 rios=3 wios=4 cost.usage=9\n")
        );
        assert_eq!(peak, 12345);
        assert_eq!(
            events,
            Events {
                oom: 1,
                oom_kill: 2,
                oom_group_kill: 3,
                local_oom: 4,
                local_oom_kill: 5,
                local_oom_group_kill: 6,
            }
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn terminal_snapshot_rejects_missing_malformed_and_inconsistent_sources() {
        for (name, contents) in [
            ("memory.peak", None),
            ("memory.peak", Some("12x\n")),
            ("memory.peak", Some("12345\r\n")),
            ("memory.max", Some("max\n")),
            ("memory.max", Some("65535\n")),
            ("memory.swap.current", Some("1\n")),
            ("memory.swap.max", Some("1\n")),
            ("memory.events.local", Some("oom 0\noom_kill 0\n")),
            (
                "memory.events",
                Some("oom 0\noom 0\noom_kill 0\noom_group_kill 0\n"),
            ),
            (
                "memory.events",
                Some("oom 0\noom_kill 0\t1\noom_group_kill 0\n"),
            ),
        ] {
            let (root, identity) = snapshot_fixture();
            let path = root.join(name);
            if let Some(contents) = contents {
                fs::write(&path, contents).unwrap();
            } else {
                fs::remove_file(&path).unwrap();
            }
            assert!(
                terminal_snapshot(&root, identity, 65536).is_err(),
                "{name} with {contents:?}"
            );
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn terminal_snapshot_enforces_per_source_and_aggregate_bounds() {
        let (root, identity) = snapshot_fixture();
        fs::write(root.join("memory.events"), "x".repeat(4097)).unwrap();
        assert_eq!(
            terminal_snapshot(&root, identity, 65536),
            Err(ErrorCode::Uncertain)
        );
        fs::remove_dir_all(root).unwrap();

        let (root, identity) = snapshot_fixture();
        fs::write(root.join("memory.events"), padded_event_file(2100)).unwrap();
        fs::write(root.join("memory.events.local"), padded_event_file(2100)).unwrap();
        assert_eq!(
            terminal_snapshot(&root, identity, 65536),
            Err(ErrorCode::Uncertain)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn optional_terminal_io_is_bounded_and_never_invalidates_memory_evidence() {
        for io_stat in [None, Some(vec![0xff])] {
            let (root, identity) = snapshot_fixture();
            match io_stat {
                Some(contents) => fs::write(root.join("io.stat"), contents).unwrap(),
                None => fs::remove_file(root.join("io.stat")).unwrap(),
            }
            let (snapshot, peak, _) =
                terminal_snapshot(&root, identity, 65536).expect("memory snapshot stays valid");
            assert_eq!(peak, 12345);
            assert_eq!(snapshot.io_stat_raw, None);
            fs::remove_dir_all(root).unwrap();
        }

        let (root, identity) = snapshot_fixture();
        let fixed_bytes: usize = [
            "memory.peak",
            "memory.max",
            "memory.swap.current",
            "memory.swap.max",
            "memory.events",
            "memory.events.local",
        ]
        .iter()
        .map(|name| fs::read(root.join(name)).unwrap().len())
        .sum();
        let remaining = TERMINAL_SNAPSHOT_BYTES - fixed_bytes;
        fs::write(root.join("io.stat"), format!("{}\n", "x".repeat(remaining))).unwrap();
        let (snapshot, _, _) = terminal_snapshot(&root, identity.clone(), 65536).unwrap();
        assert_eq!(snapshot.io_stat_raw, None);
        fs::write(
            root.join("io.stat"),
            format!("{}\n", "x".repeat(remaining - 1)),
        )
        .unwrap();
        let (snapshot, _, _) = terminal_snapshot(&root, identity.clone(), 65536).unwrap();
        assert_eq!(
            snapshot.io_stat_raw.as_ref().map(String::len),
            Some(remaining)
        );
        fs::remove_dir_all(root).unwrap();

        let (root, identity) = snapshot_fixture();
        fs::remove_file(root.join("io.stat")).unwrap();
        fs::write(root.join("io-stat-target"), "8:0 rbytes=1\n").unwrap();
        std::os::unix::fs::symlink(root.join("io-stat-target"), root.join("io.stat")).unwrap();
        let (snapshot, peak, _) = terminal_snapshot(&root, identity, 65536).unwrap();
        assert_eq!(peak, 12345);
        assert_eq!(snapshot.io_stat_raw, None);
        fs::remove_dir_all(root).unwrap();

        let (root, identity) = snapshot_fixture();
        fs::write(root.join("io.stat"), "8:0 rbytes=1 cost.usage=9\n").unwrap();
        let (snapshot, _, _) = terminal_snapshot(&root, identity, 65536).unwrap();
        assert_eq!(
            snapshot.io_stat_raw.as_deref(),
            Some("8:0 rbytes=1 cost.usage=9\n")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn terminal_io_omissions_report_bounded_phase_errno_or_limit_without_paths() {
        let (root, _) = snapshot_fixture();
        let path = root.join("io.stat");
        let mut total = 0;
        fs::remove_file(&path).unwrap();
        let missing = optional_terminal_io_source(&path, &mut total);
        assert!(matches!(
            &missing,
            TerminalIoCapture::Omitted(TerminalIoOmission::Open(Some(libc::ENOENT)))
        ));
        let TerminalIoCapture::Omitted(reason) = missing else {
            unreachable!("missing io.stat must be diagnosed");
        };
        assert_eq!(reason.diagnostic(), "phase=open errno=2");

        fs::create_dir(&path).unwrap();
        let unreadable = optional_terminal_io_source(&path, &mut total);
        assert!(matches!(
            &unreadable,
            TerminalIoCapture::Omitted(TerminalIoOmission::Read(Some(libc::EISDIR)))
        ));
        let TerminalIoCapture::Omitted(reason) = unreadable else {
            unreachable!("reading an io.stat directory must be diagnosed");
        };
        assert_eq!(reason.diagnostic(), "phase=read errno=21");

        fs::remove_dir(&path).unwrap();
        fs::write(&path, b"ab").unwrap();
        total = TERMINAL_SNAPSHOT_BYTES - 1;
        let oversized = optional_terminal_io_source(&path, &mut total);
        let TerminalIoCapture::Omitted(reason) = oversized else {
            unreachable!("over-limit io.stat must remain omitted");
        };
        assert_eq!(reason, TerminalIoOmission::TooLarge);
        assert_eq!(reason.diagnostic(), "phase=read reason=byte-limit");

        let diagnostics = [
            "phase=open errno=2",
            "phase=read errno=21",
            "phase=read reason=byte-limit",
        ];
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| !diagnostic.contains(&root.to_string_lossy().to_string()))
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn old_terminal_snapshot_receipts_default_missing_io_diagnostic() {
        let (_, snapshot) = snapshot_fixture();
        let mut value = serde_json::to_value(snapshot).unwrap();
        value.as_object_mut().unwrap().remove("io_stat_raw");
        let restored: TerminalSnapshot = serde_json::from_value(value).unwrap();
        assert_eq!(restored.io_stat_raw, None);
    }

    #[test]
    fn terminal_snapshot_identity_compares_all_bound_fields() {
        let (root, identity) = snapshot_fixture();
        let (snapshot, _, _) = terminal_snapshot(&root, identity.clone(), 65536).unwrap();
        for change in [
            "path",
            "inode",
            "invocation",
            "launch",
            "container",
            "unit",
            "incarnation",
            "sequence",
        ] {
            let mut changed = identity.clone();
            match change {
                "path" => changed.cgroup_path.push('x'),
                "inode" => changed.cgroup_inode += 1,
                "invocation" => changed.unit_invocation.push('x'),
                "launch" => changed.launch_id.push('x'),
                "container" => changed.container_id.push('x'),
                "unit" => changed.attempt_unit.push('x'),
                "incarnation" => changed.incarnation.push('x'),
                "sequence" => changed.sequence += 1,
                _ => unreachable!(),
            }
            assert!(!same_terminal_identity(&changed, &snapshot), "{change}");
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn persisted_terminal_snapshot_must_match_record_identity_and_receipt_facts() {
        let config = Config {
            version: 1,
            mode: "qualification".into(),
            instance: "0".repeat(32),
            root: "/tmp/slipstream-terminal-evidence".into(),
            socket: "/tmp/slipstream-terminal-evidence.sock".into(),
            peer_uid: 1000,
            image: format!("sha256:{}", "1".repeat(64)),
            memory_bytes: 128 * 1024 * 1024,
            receipt_retention_seconds: 86400,
        };
        let backend = Backend {
            config: config.clone(),
        };
        let launch_id = "2".repeat(32);
        let attempt_unit = format!("slipstreamprocessing{}-{launch_id}.slice", config.instance);
        let container_id = "3".repeat(64);
        let mut record = Record {
            receipt: Receipt {
                incarnation: "4".repeat(32),
                sequence: 5,
                workload: Workload::ProbeSuccess,
                policy: "5".repeat(64),
                bundle: "6".repeat(64),
                state: State::Settling,
                cancellation_requested: false,
                accepted_at_unix_ms: 0,
                deadline_unix_ms: 30000,
                outcome: Some(Outcome::Completed),
                runtime: Some(Runtime {
                    launch_id: launch_id.clone(),
                    container_id: Some(container_id.clone()),
                    attempt_unit: attempt_unit.clone(),
                }),
                limits: Limits::new(config.memory_bytes),
                evidence: None,
                cleanup: Cleanup::Pending,
            },
            launch_id: launch_id.clone(),
            image_id: format!("sha256:{}", "7".repeat(64)),
            unit_invocation: Some("8".repeat(32)),
            cgroup_inode: Some(9),
            mount_id: None,
            released: true,
            termination_reason: None,
            manager_pending: None,
            stop_confirmed: false,
            settled_at_unix_ms: None,
            film: None,
        };
        let cgroup_path = backend.parent_path().join(&attempt_unit);
        record.receipt.evidence = Some(Evidence {
            peak_bytes: 123,
            exit_code: Some(0),
            docker_oom_killed: Some(false),
            attempt_before: Some(Events::default()),
            attempt_after: Some(Events::default()),
            parent_before: Some(Events::default()),
            parent_after: Some(Events::default()),
            populated: Some(false),
            terminal_snapshot: Some(TerminalSnapshot {
                cgroup_path: cgroup_path.to_str().unwrap().to_owned(),
                cgroup_inode: 9,
                unit_invocation: "8".repeat(32),
                launch_id,
                container_id,
                attempt_unit,
                incarnation: record.receipt.incarnation.clone(),
                sequence: record.receipt.sequence,
                memory_peak_raw: "123\n".into(),
                memory_max_raw: format!("{}\n", config.memory_bytes),
                memory_swap_current_raw: "0\n".into(),
                memory_swap_max_raw: "0\n".into(),
                memory_events_raw: "oom 0\noom_kill 0\noom_group_kill 0\n".into(),
                memory_events_local_raw: "oom 0\noom_kill 0\noom_group_kill 0\n".into(),
                io_stat_raw: Some("8:0 rbytes=1 wbytes=2 rios=3 wios=4 cost.usage=9\n".into()),
            }),
        });
        assert_eq!(backend.validate_terminal_evidence(&record), Ok(()));
        let mut before_create = record.clone();
        before_create.receipt.runtime.as_mut().unwrap().container_id = None;
        before_create
            .receipt
            .evidence
            .as_mut()
            .unwrap()
            .terminal_snapshot = None;
        assert_eq!(backend.validate_terminal_evidence(&before_create), Ok(()));

        for change in ["missing", "peak", "events", "identity", "limit"] {
            let mut changed = record.clone();
            let evidence = changed.receipt.evidence.as_mut().unwrap();
            let snapshot = evidence.terminal_snapshot.as_mut().unwrap();
            match change {
                "missing" => evidence.terminal_snapshot = None,
                "peak" => snapshot.memory_peak_raw = "124\n".into(),
                "events" => {
                    snapshot.memory_events_local_raw =
                        "oom 1\noom_kill 0\noom_group_kill 0\n".into()
                }
                "identity" => snapshot.container_id.push('a'),
                "limit" => snapshot.memory_max_raw = "65536\n".into(),
                _ => unreachable!(),
            }
            assert_eq!(
                backend.validate_terminal_evidence(&changed),
                Err(ErrorCode::Uncertain),
                "{change}"
            );
        }
    }

    #[test]
    fn terminal_snapshot_limit_must_match_the_accepted_qualified_plan() {
        let record = crate::journal::tests::qualified_record(1);
        let plan_limit = record
            .film
            .as_ref()
            .unwrap()
            .grant
            .plan
            .qualified()
            .unwrap()
            .attempt_limit_bytes;
        assert_eq!(attempt_memory_limit(&record), Ok(plan_limit));
        let mut changed = record;
        changed.receipt.limits.memory_bytes += 4096;
        assert_eq!(attempt_memory_limit(&changed), Err(ErrorCode::Uncertain));
    }

    #[test]
    fn unpopulated_cgroup_requires_an_explicit_zero_counter() {
        let (root, _) = snapshot_fixture();
        for (contents, expected) in [
            ("populated 0\nfrozen 0\n", true),
            ("populated 1\nfrozen 0\n", false),
            ("frozen 0\n", false),
        ] {
            fs::write(root.join("cgroup.events"), contents).unwrap();
            assert_eq!(cgroup_unpopulated(&root).unwrap(), expected);
        }
        fs::write(root.join("cgroup.events"), "populated x\nfrozen 0\n").unwrap();
        assert!(cgroup_unpopulated(&root).is_err());
        fs::remove_file(root.join("cgroup.events")).unwrap();
        assert!(cgroup_unpopulated(&root).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_control_and_native_gate_modes_ignore_restrictive_umask() {
        const CHILD: &str = "SLIPSTREAM_CONTROL_MODE_TEST_CHILD_7C2A";
        if std::env::var_os(CHILD).is_some() {
            // SAFETY: this test branch runs in a dedicated subprocess, so the
            // process-wide umask cannot affect other parallel tests.
            unsafe { libc::umask(0o077) };
            let root = std::env::temp_dir().join(format!(
                "slipstream-control-mode-test-{}",
                std::process::id()
            ));
            fs::create_dir(&root).unwrap();
            let workspace = root.join("attempt");
            create_workspace_directory(&workspace).unwrap();
            let control = workspace.join("control");
            create_control_directory(&control).unwrap();
            let gate = create_native_gate(&control.join("gate")).unwrap();

            assert_eq!(fs::metadata(&workspace).unwrap().mode() & 0o777, 0o700);
            assert_eq!(fs::metadata(&control).unwrap().mode() & 0o777, 0o755);
            assert_eq!(
                fs::metadata(control.join("gate")).unwrap().mode() & 0o777,
                0o644
            );

            drop(gate);
            fs::remove_dir_all(root).unwrap();
            return;
        }

        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "backend::tests::workspace_control_and_native_gate_modes_ignore_restrictive_umask",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "umask subprocess failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn workload_delegation_requires_io_accounting_even_before_io_occurs() {
        let scope = std::env::temp_dir().join(format!(
            "slipstream-workload-delegation-{}",
            std::process::id()
        ));
        let leaf = scope.join("workload");
        fs::create_dir_all(&leaf).unwrap();
        let control = scope.join("cgroup.subtree_control");
        fs::write(&control, "").unwrap();
        // These files exercise failure propagation and the requested controller
        // set. Only the real executor qualification proves kernel delegation.
        assert_eq!(
            delegate_workload_controllers(&scope, &leaf),
            Err(ErrorCode::Unavailable)
        );
        assert_eq!(
            fs::read_to_string(&control).unwrap(),
            "+memory +cpu +pids +io"
        );
        let accounting = leaf.join("io.stat");
        for contents in ["", "8:0 rbytes=4096 wbytes=0 rios=1 wios=0\n"] {
            fs::write(&accounting, contents).unwrap();
            assert_eq!(delegate_workload_controllers(&scope, &leaf), Ok(()));
            assert_eq!(fs::read_to_string(&accounting).unwrap(), contents);
        }
        fs::remove_file(&accounting).unwrap();
        fs::create_dir(&accounting).unwrap();
        assert_eq!(
            delegate_workload_controllers(&scope, &leaf),
            Err(ErrorCode::Unavailable)
        );
        fs::remove_dir(&accounting).unwrap();
        fs::write(&accounting, "").unwrap();
        fs::remove_file(&control).unwrap();
        fs::create_dir(&control).unwrap();
        assert_eq!(
            delegate_workload_controllers(&scope, &leaf),
            Err(ErrorCode::Unavailable)
        );
        assert_eq!(fs::read_to_string(&accounting).unwrap(), "");
        assert!(!leaf.join("io.max").exists());
        assert!(!leaf.join("io.weight").exists());
        fs::remove_dir_all(scope).unwrap();
    }

    #[test]
    fn storage_capacity_checks_type_bytes_and_inodes_without_requiring_free_space() {
        // SAFETY: statfs is a C POD value; this synthetic observation is never
        // passed to the kernel and only its initialized count fields are read.
        let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
        let limits = crate::film::limits(8 << 30);
        stat.f_type = 0x01021994;
        stat.f_bsize = 4096;
        stat.f_blocks = limits.storage_bytes / 4096;
        stat.f_files = limits.storage_inodes;
        assert!(storage_matches(&stat, &limits));
        for (field, value) in [
            ("type", 0),
            ("bytes", 1),
            ("inodes", 1),
            ("block-size", 0),
            ("overflow", u64::MAX),
        ] {
            let mut changed = stat;
            match field {
                "type" => changed.f_type = value as _,
                "bytes" | "overflow" => changed.f_blocks = value,
                "inodes" => changed.f_files = value,
                "block-size" => changed.f_bsize = value as _,
                _ => unreachable!(),
            }
            assert!(!storage_matches(&changed, &limits), "{field}");
        }
    }
    #[test]
    fn created_or_never_started_containers_have_no_observed_execution_exit() {
        let mut state = serde_json::json!({"Status":"created", "StartedAt":"0001-01-01T00:00:00Z", "ExitCode":0});
        assert_eq!(observed_exit(&state).unwrap(), None);
        state["Status"] = "exited".into();
        assert_eq!(observed_exit(&state).unwrap(), None);
        state["StartedAt"] = "2026-09-22T00:00:00Z".into();
        assert_eq!(observed_exit(&state).unwrap(), Some(0));
        state["ExitCode"] = 137.into();
        assert_eq!(observed_exit(&state).unwrap(), Some(137));
        state["Status"] = "running".into();
        assert_eq!(observed_exit(&state).unwrap(), None);
    }

    #[test]
    fn mount_root_rejects_finite_limits_and_only_true_root_may_omit_memory_max() {
        let root =
            std::env::temp_dir().join(format!("slipstream-cgroup-root-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        assert_eq!(require_unlimited_ancestor(&root, true), Ok(()));
        assert_eq!(
            require_unlimited_ancestor(&root, false),
            Err(ErrorCode::Unavailable)
        );
        fs::write(root.join("memory.max"), "max\n").unwrap();
        assert_eq!(require_unlimited_ancestor(&root, true), Ok(()));
        assert_eq!(require_unlimited_ancestor(&root, false), Ok(()));
        fs::write(root.join("memory.max"), "67108864\n").unwrap();
        assert_eq!(
            require_unlimited_ancestor(&root, true),
            Err(ErrorCode::Unavailable)
        );
        assert_eq!(
            require_unlimited_ancestor(&root, false),
            Err(ErrorCode::Unavailable)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn observed_setup_drift_is_persisted_but_missing_limits_and_bootstrap_exits_are_not_tampering()
    {
        let root =
            std::env::temp_dir().join(format!("slipstream-setup-limits-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let backend = Backend {
            config: Config {
                version: 3,
                mode: "film-qualified-fixtures".into(),
                instance: "0".repeat(32),
                root: root.to_string_lossy().into_owned(),
                socket: "/test.sock".into(),
                peer_uid: 0,
                image: format!("sha256:{}", "1".repeat(64)),
                memory_bytes: 8 << 30,
                receipt_retention_seconds: 1,
            },
        };
        let mut record = crate::journal::tests::qualified_record(1);
        let mut persisted = Vec::new();
        let mut save = |record: &Record| {
            persisted.push(serde_json::to_vec(record).unwrap());
            Ok(())
        };
        let absent = backend.limits_match(&root, 8 << 30);
        assert!(absent.is_err());
        assert!(checked_setup_observation(&mut record, &mut save, absent).is_err());
        assert_eq!(
            record
                .film
                .as_ref()
                .unwrap()
                .qualification_observation_valid,
            Some(true)
        );
        for (key, value) in [
            ("memory.max", (8u64 << 30).to_string()),
            ("memory.swap.max", "0".into()),
            ("cpu.max", "400000 100000".into()),
            ("pids.max", crate::film::limits(8 << 30).tasks.to_string()),
        ] {
            fs::write(root.join(key), value).unwrap();
        }
        let unchanged = backend.limits_match(&root, 8 << 30);
        assert_eq!(unchanged, Ok(true));
        checked_setup_observation(&mut record, &mut save, unchanged).unwrap();
        assert_eq!(
            record
                .film
                .as_ref()
                .unwrap()
                .qualification_observation_valid,
            Some(true)
        );
        fs::write(root.join("memory.max"), (7u64 << 30).to_string()).unwrap();
        let drift = backend.limits_match(&root, 8 << 30);
        assert_eq!(drift, Ok(false));
        assert_eq!(
            checked_setup_observation(&mut record, &mut save, drift),
            Err(ErrorCode::Unavailable)
        );
        assert_eq!(persisted.len(), 1);
        let recovered: Record = serde_json::from_slice(&persisted[0]).unwrap();
        assert_eq!(
            recovered.film.unwrap().qualification_observation_valid,
            Some(false)
        );
        let mut record = crate::journal::tests::qualified_record(1);
        assert_eq!(
            checked_setup_observation(&mut record, &mut |_| Err(ErrorCode::Uncertain), Ok(false)),
            Err(ErrorCode::Uncertain)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unresolved_manager_effects_quarantine_before_any_runtime_lookup() {
        let config = Config {
            version: 1,
            mode: "qualification".into(),
            instance: "0".repeat(32),
            root: "/does-not-exist".into(),
            socket: "/does-not-exist.sock".into(),
            peer_uid: 0,
            image: format!("sha256:{}", "1".repeat(64)),
            memory_bytes: 64 * 1024 * 1024,
            receipt_retention_seconds: 1,
        };
        let backend = Backend { config };
        for phase in [
            ManagerPhase::Slice,
            ManagerPhase::SliceStop,
            ManagerPhase::Mount,
            ManagerPhase::Create,
            ManagerPhase::Start,
            ManagerPhase::Pause,
            ManagerPhase::Unpause,
        ] {
            let mut record:Record=serde_json::from_value(serde_json::json!({
                "receipt":{"incarnation":"11111111111111111111111111111111","sequence":1,"workload":"probe-success","policy":"2".repeat(64),"bundle":"3".repeat(64),"state":"accepted","cancellation_requested":false,"accepted_at_unix_ms":0,"deadline_unix_ms":30000,"outcome":null,"runtime":null,"limits":Limits::new(64*1024*1024),"evidence":null,"cleanup":"pending"},
                "launch_id":"4".repeat(32),"image_id":format!("sha256:{}","5".repeat(64)),"unit_invocation":null,"cgroup_inode":null,"mount_id":null,"released":false,"termination_reason":null,"manager_pending":phase,"settled_at_unix_ms":null
            })).unwrap();
            assert_eq!(backend.discover(&mut record), Err(ErrorCode::Uncertain));
            assert_eq!(record.manager_pending, Some(phase));
            assert!(record.receipt.runtime.is_none());
            record.receipt.outcome = Some(Outcome::Completed);
            record.stop_confirmed = true;
            assert_eq!(
                backend.cleanup(&mut record, |_| panic!(
                    "pending cleanup must not persist or execute"
                )),
                Err(ErrorCode::Uncertain)
            );
        }
    }

    #[test]
    fn expired_command_deadline_does_not_spawn_even_a_valid_program() {
        assert_eq!(
            command_until(
                "/usr/bin/true",
                &[],
                Instant::now() - Duration::from_millis(1)
            ),
            Err(ErrorCode::Uncertain)
        );
    }

    #[test]
    fn confirmed_stop_routes_only_to_read_only_convergence_and_pending_always_blocks() {
        let active = std::cell::Cell::new(0);
        let stopped = std::cell::Cell::new(0);
        for stop_confirmed in [false, true] {
            assert_eq!(
                verify_slice_phase(
                    None,
                    stop_confirmed,
                    || {
                        active.set(active.get() + 1);
                        Ok(())
                    },
                    || {
                        stopped.set(stopped.get() + 1);
                        Ok(())
                    }
                ),
                Ok(())
            );
            for pending in [ManagerPhase::SliceStop]
                .into_iter()
                .chain(stop_confirmed.then_some(ManagerPhase::Start))
            {
                assert_eq!(
                    verify_slice_phase(
                        Some(pending),
                        stop_confirmed,
                        || panic!("pending cannot validate an active replacement"),
                        || panic!("pending cannot infer stop from absence")
                    ),
                    Err(ErrorCode::Uncertain)
                );
            }
        }
        assert_eq!((active.get(), stopped.get()), (1, 1));
        assert_eq!(
            verify_slice_phase(
                Some(ManagerPhase::CreateReturned),
                false,
                || Ok(()),
                || panic!("lost create response still uses active ownership")
            ),
            Ok(())
        );
        assert_eq!(
            verify_slice_phase(
                None,
                true,
                || panic!("completed stop cannot require live properties"),
                || Err(ErrorCode::Uncertain)
            ),
            Err(ErrorCode::Uncertain)
        );
    }
}
