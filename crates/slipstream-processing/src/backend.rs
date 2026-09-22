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

fn observed_exit(state: &serde_json::Value) -> Result<Option<u8>> {
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

fn delegate_workload_controllers(scope: &Path, leaf: &Path) -> Result<()> {
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
        record.manager_pending = None;
        let limits = self.limits_match(&expected, record.receipt.limits.memory_bytes);
        checked_setup_observation(record, &mut persist, limits)?;
        record.receipt.evidence = Some(Evidence {
            peak_bytes: 0,
            exit_code: None,
            docker_oom_killed: None,
            attempt_before: Some(events(&expected)?),
            attempt_after: None,
            parent_before: Some(events(&self.parent_path())?),
            parent_after: None,
            populated: Some(false),
        });
        persist(record)?;
        crate::faults::at(&self.config, record, crate::faults::Phase::Slice)?;
        let workspace = self.workspace(record);
        fs::create_dir(&workspace).map_err(|_| ErrorCode::Uncertain)?;
        fs::set_permissions(&workspace, fs::Permissions::from_mode(0o700))
            .map_err(|_| ErrorCode::Unavailable)?;
        let control = workspace.join("control");
        let work = workspace.join("work");
        fs::create_dir(&control).map_err(|_| ErrorCode::Unavailable)?;
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
            let fifo = control.join("gate");
            let path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes())
                .map_err(|_| ErrorCode::Unavailable)?;
            // SAFETY: path is a valid NUL-terminated pathname in a sealed owned directory.
            if unsafe { libc::mkfifo(path.as_ptr(), 0o644) } != 0 {
                return Err(ErrorCode::Unavailable);
            }
            let gate = OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
                .open(&fifo)
                .map_err(|_| ErrorCode::Unavailable)?;
            Gate::Native(gate)
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
        let live = if record
            .receipt
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.container_id.as_ref())
            .is_some()
        {
            Some(self.live(record)?)
        } else {
            None
        };
        if live.as_ref().is_some_and(|live| live.running) {
            return Err(ErrorCode::Uncertain);
        }
        let populated = read(&path.join("cgroup.events"))?
            .lines()
            .any(|line| line == "populated 1");
        if populated {
            return Err(ErrorCode::Uncertain);
        }
        Ok(Evidence {
            peak_bytes: read(&path.join("memory.peak"))?
                .parse()
                .map_err(|_| ErrorCode::Uncertain)?,
            exit_code: live.as_ref().and_then(|live| live.exit_code),
            docker_oom_killed: live.and_then(|live| live.exit_code.map(|_| live.oom)),
            attempt_before: record
                .receipt
                .evidence
                .as_ref()
                .and_then(|e| e.attempt_before.clone()),
            attempt_after: Some(events(&path)?),
            parent_before: record
                .receipt
                .evidence
                .as_ref()
                .and_then(|e| e.parent_before.clone()),
            parent_after: Some(events(&self.parent_path())?),
            populated: Some(false),
        })
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

fn verify_slice_phase(
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

fn require_unlimited_ancestor(path: &Path, allow_absent: bool) -> Result<()> {
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

fn process_cgroup(pid: u32) -> Result<PathBuf> {
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

fn events(path: &Path) -> Result<Events> {
    fn counters(text: &str) -> Result<BTreeMap<&str, u64>> {
        text.lines()
            .map(|line| {
                let (name, value) = line.split_once(' ').ok_or(ErrorCode::Uncertain)?;
                Ok((name, value.parse().map_err(|_| ErrorCode::Uncertain)?))
            })
            .collect()
    }
    let all = read(&path.join("memory.events"))?;
    let local = read(&path.join("memory.events.local"))?;
    let all = counters(&all)?;
    let local = counters(&local)?;
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

fn mount_identity(path: &Path) -> Result<Option<u64>> {
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

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn command(program: &str, args: &[String]) -> Result<String> {
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
