//! Host boundary of the production Photo executor: the systemd slice,
//! cgroup, mount and container identities of one attempt, their
//! provisioning, verification, teardown, terminal evidence, and the
//! measured control-reserve and shared-ancestor headroom admission
//! re-checks. Everything here fails closed on observed host state.

use super::*;

pub(super) fn stop_container(
    record: &mut PhotoRecord,
    paused: bool,
    mut update: impl FnMut(&PhotoRecord) -> Result<(), ErrorCode>,
    mut command: impl FnMut(&[String]) -> Result<String, ErrorCode>,
) -> Result<(), ErrorCode> {
    let id = record.container_id.clone().ok_or(ErrorCode::Uncertain)?;
    if paused {
        record.manager_pending = Some(ManagerPhase::Unpause);
        update(record)?;
        command(&backend::strings(&["unpause", &id]))?;
        record.manager_pending = None;
        update(record)?;
    }
    command(&backend::strings(&["kill", "--signal", "KILL", &id]))?;
    Ok(())
}

fn docker(config: &Config, args: &[String]) -> Result<String, ErrorCode> {
    backend::docker(&config.root, args)
}

/// Resolve and verify the pinned worker image. The image entrypoint and the
/// configured bundle label bind the execution identity to the configuration.
pub(super) fn inspect_image(config: &Config) -> Result<String, ErrorCode> {
    let info: Value = serde_json::from_str(&docker(
        config,
        &backend::strings(&["info", "--format", "{{json .}}"]),
    )?)
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
    let image: Value = serde_json::from_str(&docker(
        config,
        &backend::strings(&["image", "inspect", "--format", "{{json .}}", &config.image]),
    )?)
    .map_err(|_| ErrorCode::Unavailable)?;
    let id = image["Id"].as_str().ok_or(ErrorCode::Unavailable)?;
    if !id.strip_prefix("sha256:").is_some_and(|id| hex(id, 64))
        || image["Config"]["Entrypoint"] != serde_json::json!([WORKER])
        || image["Config"]["Labels"]["slipstream.processing.photo.bundle"] != config.bundle
    {
        return Err(ErrorCode::Unavailable);
    }
    Ok(id.to_owned())
}

/// Measured control-path storage and shared-ancestor memory headroom. Both
/// must cover their configured reserve before an intent is persisted or a
/// worker is released (`design/processing-photo-protocol.md` admission
/// ordering step 3).
#[derive(Clone, Copy, Debug)]
pub(super) struct Headroom {
    pub(super) control_free_bytes: u64,
    pub(super) ancestor_headroom_bytes: u64,
}

impl Headroom {
    /// Measure the actual filesystem supply at the control root and the
    /// memory headroom of the shared ancestor above the processing subtree.
    pub(super) fn measure(config: &Config) -> Result<Self, ErrorCode> {
        Ok(Self {
            control_free_bytes: control_free_bytes(Path::new(&config.root))?,
            ancestor_headroom_bytes: shared_ancestor_headroom()?,
        })
    }

    /// An unmet or unmeasurable reserve leaves the capability unavailable:
    /// admission is refused and no start intent is persisted, so no worker
    /// is ever released onto an unqualified boundary.
    pub(super) fn satisfied(&self, config: &Config) -> Result<(), ErrorCode> {
        if self.control_free_bytes < config.control_reserve_bytes
            || self.ancestor_headroom_bytes < config.shared_ancestor_headroom_bytes
        {
            return Err(ErrorCode::Unavailable);
        }
        Ok(())
    }
}

/// Bytes currently free on the filesystem that carries the control root.
fn control_free_bytes(root: &Path) -> Result<u64, ErrorCode> {
    let file = File::open(root).map_err(|_| ErrorCode::Unavailable)?;
    // SAFETY: fstatvfs receives the live descriptor and a correctly sized output struct.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstatvfs(file.as_raw_fd(), &mut stat) } != 0 {
        return Err(ErrorCode::Unavailable);
    }
    let blocks = stat.f_bavail;
    let frsize = stat.f_frsize;
    blocks.checked_mul(frsize).ok_or(ErrorCode::Unavailable)
}

/// The memory headroom of the shared ancestor above the capped processing
/// subtree. A missing or unlimited ancestor limit defers to the host supply
/// reported by the kernel; a finite limit leaves `max - current` bytes. An
/// overcommitted ancestor has no measurable headroom and refuses admission.
fn shared_ancestor_headroom() -> Result<u64, ErrorCode> {
    let ancestor = match backend::read(&Path::new(CGROUP).join("memory.max")) {
        Ok(limit) if limit.trim() != "max" => {
            let limit: u64 = limit.trim().parse().map_err(|_| ErrorCode::Unavailable)?;
            let current: u64 = backend::read(&Path::new(CGROUP).join("memory.current"))?
                .trim()
                .parse()
                .map_err(|_| ErrorCode::Unavailable)?;
            limit.checked_sub(current).ok_or(ErrorCode::Unavailable)?
        }
        Ok(_) => u64::MAX,
        Err(_) => u64::MAX,
    };
    Ok(ancestor.min(meminfo_available_bytes()?))
}

/// The kernel's estimate of memory available for new work without swapping.
fn meminfo_available_bytes() -> Result<u64, ErrorCode> {
    let text = std::fs::read_to_string("/proc/meminfo").map_err(|_| ErrorCode::Unavailable)?;
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("MemAvailable:") {
            let kilobytes: u64 = value
                .trim()
                .strip_suffix("kB")
                .ok_or(ErrorCode::Unavailable)?
                .trim()
                .parse()
                .map_err(|_| ErrorCode::Unavailable)?;
            return kilobytes.checked_mul(1024).ok_or(ErrorCode::Unavailable);
        }
    }
    Err(ErrorCode::Unavailable)
}

impl PhotoExecutor {
    fn limits(&self) -> Limits {
        Limits {
            memory_bytes: self.config.memory_bytes,
            swap_bytes: 0,
            cpu_quota_us: self.config.cpu_quota_us,
            cpu_period_us: 100_000,
            tasks: u64::from(self.config.tasks),
            storage_bytes: self.config.staged_storage_bytes_max,
            storage_inodes: self.config.staged_storage_inodes_max,
        }
    }

    fn systemctl(&self, args: &[String]) -> Result<String, ErrorCode> {
        backend::systemctl(args)
    }

    fn property(&self, unit: &str, property: &str) -> Result<String, ErrorCode> {
        self.systemctl(&backend::strings(&[
            "show",
            unit,
            "--property",
            property,
            "--value",
        ]))
    }

    fn parent_path(&self) -> PathBuf {
        Path::new(CGROUP).join(parent_unit(&self.config.instance))
    }

    fn container_name(&self, record: &PhotoRecord) -> String {
        format!("slipstream-processing-{}", record.launch_id)
    }

    fn mounts(&self, record: &PhotoRecord) -> Vec<(&'static str, PathBuf, bool)> {
        let base = record.workspace(Path::new(&self.config.root));
        vec![
            ("/control", base.join("control"), false),
            ("/input", base.join("source"), false),
            ("/work", base.join("work"), true),
        ]
    }

    pub(super) fn check_caller(&self, pid: u32) -> Result<(), ErrorCode> {
        let parent = self.parent_path();
        for pid in [pid, std::process::id()] {
            let path = backend::process_cgroup(pid)?;
            if path.starts_with(&parent) {
                return Err(ErrorCode::Unavailable);
            }
            for ancestor in parent.ancestors() {
                if path.starts_with(ancestor) {
                    backend::require_unlimited_ancestor(ancestor, ancestor == Path::new(CGROUP))?;
                }
                if ancestor == Path::new(CGROUP) {
                    break;
                }
            }
        }
        Ok(())
    }

    pub(super) fn verify_parent(&self, identity: Option<&ParentIdentity>) -> Result<(), ErrorCode> {
        let path = self.parent_path();
        if let Some(identity) = identity {
            if fs::metadata(&path).map_err(|_| ErrorCode::Uncertain)?.ino() != identity.inode
                || self.property(&parent_unit(&self.config.instance), "InvocationID")?
                    != identity.invocation
                || !backend::read(&path.join("cgroup.procs"))?.is_empty()
            {
                return Err(ErrorCode::Uncertain);
            }
            return Ok(());
        }
        // Inventory queries do not synthesize a unit and cannot grant ownership.
        if path.exists()
            || !self
                .systemctl(&backend::strings(&[
                    "list-units",
                    "--all",
                    "--plain",
                    "--no-legend",
                    "--no-pager",
                    &parent_unit(&self.config.instance),
                ]))?
                .is_empty()
        {
            return Err(ErrorCode::Uncertain);
        }
        Ok(())
    }

    pub(super) fn prepare_parent(
        &self,
        previous: Option<&ParentIdentity>,
    ) -> Result<ParentIdentity, ErrorCode> {
        self.check_caller(std::process::id())?;
        self.verify_parent(previous)?;
        let unit = parent_unit(&self.config.instance);
        let limits = self.limits();
        self.systemctl(&backend::strings(&[
            "set-property",
            "--runtime",
            &unit,
            &format!("MemoryMax={}", self.config.memory_bytes),
            "MemorySwapMax=0",
            &format!("TasksMax={}", limits.tasks),
            &format!("CPUQuota={}%", limits.cpu_quota_us / 1000),
        ]))?;
        self.systemctl(&backend::strings(&["start", &unit]))?;
        if self.property(&unit, "StopWhenUnneeded")? != "no" {
            return Err(ErrorCode::Unavailable);
        }
        if !self.limits_match(&self.parent_path())? {
            return Err(ErrorCode::Unavailable);
        }
        let identity = ParentIdentity {
            invocation: self.property(&unit, "InvocationID")?,
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

    pub(super) fn admission_ready(
        &self,
        identity: Option<&ParentIdentity>,
    ) -> Result<(), ErrorCode> {
        // The configured control reserve and shared-ancestor headroom are
        // part of admission: an unmet or unmeasured boundary keeps the
        // reported capability blocked, never falsely available.
        Headroom::measure(&self.config)?.satisfied(&self.config)?;
        let identity = identity.ok_or(ErrorCode::Unavailable)?;
        if fs::metadata(self.parent_path())
            .map_err(|_| ErrorCode::Unavailable)?
            .ino()
            != identity.inode
            || !backend::read(&self.parent_path().join("cgroup.procs"))?.is_empty()
        {
            return Err(ErrorCode::Unavailable);
        }
        if self.limits_match(&self.parent_path())? {
            Ok(())
        } else {
            Err(ErrorCode::Unavailable)
        }
    }

    fn limits_match(&self, path: &Path) -> Result<bool, ErrorCode> {
        let limits = self.limits();
        backend::limits_match(path, &limits, limits.memory_bytes)
    }

    pub(super) fn scan_unowned(&self, records: &[PhotoRecord]) -> Result<(), ErrorCode> {
        let unit = |record: &PhotoRecord| attempt_unit(&self.config.instance, &record.launch_id);
        if self.parent_path().exists() {
            for entry in fs::read_dir(self.parent_path()).map_err(|_| ErrorCode::Uncertain)? {
                let entry = entry.map_err(|_| ErrorCode::Uncertain)?;
                if entry
                    .file_type()
                    .map_err(|_| ErrorCode::Uncertain)?
                    .is_dir()
                    && !records.iter().any(|record| {
                        record.state != State::Settled
                            && entry.file_name() == unit(record).as_str()
                            && self.verify_unit(record).is_ok()
                    })
                {
                    return Err(ErrorCode::Uncertain);
                }
            }
        }
        let names = self.systemctl(&backend::strings(&[
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
                .any(|record| record.state != State::Settled && unit(record) == name)
            {
                return Err(ErrorCode::Uncertain);
            }
        }
        let ids = docker(
            &self.config,
            &backend::strings(&[
                "ps",
                "--all",
                "--no-trunc",
                "--quiet",
                "--filter",
                &format!(
                    "label=slipstream.processing.instance={}",
                    self.config.instance
                ),
            ]),
        )?;
        for id in ids.lines() {
            if !records.iter().any(|record| {
                record.container_id.as_deref() == Some(id) && record.state != State::Settled
            }) {
                return Err(ErrorCode::Uncertain);
            }
        }
        Ok(())
    }

    pub(super) fn discover(&self, record: &mut PhotoRecord) -> Result<(), ErrorCode> {
        if record.manager_pending == Some(ManagerPhase::SliceStop) {
            // A pending slice stop decides whether the attempt boundary still
            // exists, and it is cleared only with its confirmed return. A
            // restart cannot assume its effect, so it stays ambiguous and
            // requires operator reconciliation.
            return Err(ErrorCode::Uncertain);
        }
        if matches!(
            record.manager_pending,
            Some(ManagerPhase::Create | ManagerPhase::CreateReturned)
        ) && record.container_id.is_none()
        {
            let ids = docker(
                &self.config,
                &backend::strings(&[
                    "ps",
                    "--all",
                    "--no-trunc",
                    "--quiet",
                    "--filter",
                    &format!("label=slipstream.processing.launch={}", record.launch_id),
                ]),
            )?;
            let ids: Vec<_> = ids.lines().collect();
            match ids.as_slice() {
                [id] => {
                    let candidate = record.container_id.clone();
                    record.container_id = Some((*id).to_owned());
                    if self.owned_container(record).is_err() {
                        record.container_id = candidate;
                        return Err(ErrorCode::Uncertain);
                    }
                }
                [] => {}
                _ => return Err(ErrorCode::Uncertain),
            }
        }
        let path = self
            .parent_path()
            .join(attempt_unit(&self.config.instance, &record.launch_id));
        if path.exists() {
            self.verify_unit(record)?;
        }
        let mount =
            backend::mount_identity(&record.workspace(Path::new(&self.config.root)).join("work"))?;
        if mount.is_some() && mount != record.mount_id {
            return Err(ErrorCode::Uncertain);
        }
        // The remaining recorded phases mark an effect whose outcome the
        // observed slice, mount and container identities settle here: the
        // attempt boundary is exactly what the launcher recorded, and every
        // later step fails closed on the actual container state instead of on
        // this marker. Leaving them set would block settlement and cleanup
        // forever after a worker that failed before its release gate, because
        // the release-gate pause cannot complete on a worker that already
        // exited.
        record.manager_pending = None;
        Ok(())
    }

    fn verify_unit(&self, record: &PhotoRecord) -> Result<PathBuf, ErrorCode> {
        let name = attempt_unit(&self.config.instance, &record.launch_id);
        let path = self.parent_path().join(&name);
        backend::verify_slice_phase(
            record.manager_pending,
            record.stop_confirmed,
            || {
                slice::verify(
                    &name,
                    &path,
                    record
                        .unit_invocation
                        .as_deref()
                        .ok_or(ErrorCode::Uncertain)?,
                    record.cgroup_inode.ok_or(ErrorCode::Uncertain)?,
                )
            },
            || {
                slice::wait_absent(
                    &name,
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

    fn owned_container(&self, record: &PhotoRecord) -> Result<Value, ErrorCode> {
        let id = record.container_id.as_deref().ok_or(ErrorCode::Uncertain)?;
        if !hex(id, 64) {
            return Err(ErrorCode::Uncertain);
        }
        let value: Value = serde_json::from_str(&docker(
            &self.config,
            &backend::strings(&["inspect", "--format", "{{json .}}", id]),
        )?)
        .map_err(|_| ErrorCode::Uncertain)?;
        if value["Id"] != id
            || value["Image"] != self.image_id
            || value["Name"] != format!("/{}", self.container_name(record))
            || value["Config"]["Labels"]["slipstream.processing.instance"] != self.config.instance
            || value["Config"]["Labels"]["slipstream.processing.launch"] != record.launch_id
            || value["Config"]["Labels"]["slipstream.processing.incarnation"] != record.incarnation
            || value["HostConfig"]["CgroupParent"]
                != attempt_unit(&self.config.instance, &record.launch_id)
            || value["Config"]["User"] != "1000:1000"
            || value["Config"]["Entrypoint"] != serde_json::json!([WORKER])
            || value["Config"]["Cmd"]
                != serde_json::json!([
                    record.workload,
                    record.launch_id,
                    record.deadline_unix_ms.to_string()
                ])
            || value["HostConfig"]["LogConfig"]["Type"] != "none"
            || value["HostConfig"]["Privileged"] != false
            || value["HostConfig"]["ReadonlyRootfs"] != true
            || value["HostConfig"]["NetworkMode"] != "none"
            || value["HostConfig"]["PidMode"] != ""
            || value["HostConfig"]["CgroupnsMode"] != "private"
            || value["HostConfig"]["CapDrop"] != serde_json::json!(["ALL"])
            || !value["HostConfig"]["CapAdd"].is_null()
            || value["HostConfig"]["SecurityOpt"] != serde_json::json!(["no-new-privileges:true"])
            || value["HostConfig"]["Memory"] != self.config.memory_bytes
            || value["HostConfig"]["MemorySwap"] != self.config.memory_bytes
            || value["HostConfig"]["PidsLimit"] != u64::from(self.config.tasks)
            || value["HostConfig"]["NanoCpus"] != self.config.cpu_quota_us * 10000
            || value["Config"]["Tty"] != false
            || value["Config"]["OpenStdin"] != false
        {
            return Err(ErrorCode::Uncertain);
        }
        let mounts = value["Mounts"].as_array().ok_or(ErrorCode::Uncertain)?;
        let expected = self.mounts(record);
        if mounts.len() != expected.len() {
            return Err(ErrorCode::Uncertain);
        }
        for (destination, source, writable) in expected {
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

    pub(super) fn live(&self, record: &PhotoRecord) -> Result<Live, ErrorCode> {
        let value = self.owned_container(record)?;
        Ok(Live {
            running: value["State"]["Running"]
                .as_bool()
                .ok_or(ErrorCode::Uncertain)?,
            paused: value["State"]["Paused"]
                .as_bool()
                .ok_or(ErrorCode::Uncertain)?,
            pid: value["State"]["Pid"]
                .as_u64()
                .and_then(|pid| u32::try_from(pid).ok())
                .ok_or(ErrorCode::Uncertain)?,
            exit_code: backend::observed_exit(&value["State"])?,
            oom: value["State"]["OOMKilled"]
                .as_bool()
                .ok_or(ErrorCode::Uncertain)?,
        })
    }

    /// Create the fresh retained attempt boundary before worker bootstrap:
    /// the attempt slice, the bounded private tmpfs, the release gate and the
    /// pinned paused worker with verified placement and limits.
    pub(super) fn provision(&self, record: &mut PhotoRecord) -> Result<File, ErrorCode> {
        let name = attempt_unit(&self.config.instance, &record.launch_id);
        let expected = self.parent_path().join(&name);
        let workspace = record.workspace(Path::new(&self.config.root));
        record.manager_pending = Some(ManagerPhase::Slice);
        self.update(record)?;
        let (invocation, inode) = slice::create(&name, &expected, &self.limits())?;
        record.unit_invocation = Some(invocation);
        record.cgroup_inode = Some(inode);
        if !self.limits_match(&expected)? {
            return Err(ErrorCode::Unavailable);
        }
        record.manager_pending = None;
        self.update(record)?;
        backend::create_private_directory(&workspace)?;
        let control = workspace.join("control");
        let work = workspace.join("work");
        backend::create_control_directory(&control)?;
        fs::create_dir(&work).map_err(|_| ErrorCode::Unavailable)?;
        record.manager_pending = Some(ManagerPhase::Mount);
        self.update(record)?;
        backend::command(
            "/usr/bin/mount",
            &backend::strings(&[
                "-t",
                "tmpfs",
                "-o",
                &format!(
                    "size={},nr_inodes={},noswap,nodev,nosuid,noexec,uid=1000,gid=1000,mode=0700",
                    self.limits().storage_bytes,
                    self.limits().storage_inodes,
                ),
                &format!("slipstream-{}", record.launch_id),
                work.to_str().ok_or(ErrorCode::Unavailable)?,
            ]),
        )?;
        record.mount_id = Some(backend::mount_identity(&work)?.ok_or(ErrorCode::Uncertain)?);
        record.manager_pending = None;
        self.update(record)?;
        let gate = backend::create_native_gate(&control.join("gate"))?;
        write_grant(record, &control)?;
        let mut args = backend::strings(&[
            "create",
            "--pull",
            "never",
            "--name",
            &self.container_name(record),
            "--label",
            &format!("slipstream.processing.instance={}", self.config.instance),
            "--label",
            &format!("slipstream.processing.launch={}", record.launch_id),
            "--label",
            &format!("slipstream.processing.incarnation={}", record.incarnation),
            "--cgroup-parent",
            &name,
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
            &self.config.memory_bytes.to_string(),
            "--memory-swap",
            &self.config.memory_bytes.to_string(),
            "--pids-limit",
            &self.config.tasks.to_string(),
            "--cpus",
            &(self.config.cpu_quota_us / 100_000).to_string(),
        ]);
        for (destination, source, writable) in self.mounts(record) {
            args.extend(backend::strings(&[
                "--mount",
                &format!(
                    "type=bind,source={},target={destination}{}",
                    source.display(),
                    if writable { "" } else { ",readonly" }
                ),
            ]));
        }
        args.extend(backend::strings(&[
            &self.image_id,
            &record.workload,
            &record.launch_id,
            &record.deadline_unix_ms.to_string(),
        ]));
        record.manager_pending = Some(ManagerPhase::Create);
        self.update(record)?;
        let id = docker(&self.config, &args)?;
        if !hex(&id, 64) {
            return Err(ErrorCode::Uncertain);
        }
        record.manager_pending = Some(ManagerPhase::CreateReturned);
        self.update(record)?;
        record.container_id = Some(id);
        record.manager_pending = None;
        self.update(record)?;
        record.manager_pending = Some(ManagerPhase::Start);
        self.update(record)?;
        docker(
            &self.config,
            &backend::strings(&[
                "start",
                record.container_id.as_deref().ok_or(ErrorCode::Uncertain)?,
            ]),
        )?;
        record.manager_pending = None;
        self.update(record)?;
        record.manager_pending = Some(ManagerPhase::Pause);
        self.update(record)?;
        docker(
            &self.config,
            &backend::strings(&[
                "pause",
                record.container_id.as_deref().ok_or(ErrorCode::Uncertain)?,
            ]),
        )?;
        record.manager_pending = None;
        self.update(record)?;
        self.place(record, &expected)?;
        record.image_id = Some(self.image_id.clone());
        record.phase = Phase::Provisioned;
        self.update(record)?;
        Ok(gate)
    }

    /// Verify the frozen worker placement: its cgroup scope, the paused
    /// frozen state, the delegated workload leaf and the enforced limits.
    fn place(&self, record: &PhotoRecord, expected: &Path) -> Result<(), ErrorCode> {
        let live = self.live(record)?;
        if !live.running || live.pid == 0 {
            return Err(ErrorCode::Unavailable);
        }
        let id = record.container_id.as_deref().ok_or(ErrorCode::Uncertain)?;
        let scope = backend::process_cgroup(live.pid)?;
        if scope.parent() != Some(expected)
            || scope.file_name().and_then(|value| value.to_str())
                != Some(&format!("docker-{id}.scope"))
        {
            return Err(ErrorCode::Unavailable);
        }
        // The release-gate pause freezes the worker's own cgroup scope; the
        // attempt slice above it stays unfrozen, so the frozen state is read
        // from the scope, exactly as the reference placement check does.
        if !backend::read(&scope.join("cgroup.events"))?
            .lines()
            .any(|line| line == "frozen 1")
            || self.owned_container(record)?["State"]["Paused"] != true
        {
            return Err(ErrorCode::Uncertain);
        }
        // pidfd and the opened proc directory detect disappearance without
        // retargeting readback to a reused PID.
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
        let placed = backend::read(&proc_path.join("cgroup"))?
            == format!(
                "0::{}",
                scope
                    .strip_prefix(CGROUP)
                    .map_err(|_| ErrorCode::Uncertain)?
                    .display()
            )
            .replace("0::", "0::/");
        if !placed {
            return Err(ErrorCode::Unavailable);
        }
        pidfd_alive(&pidfd)?;
        if !self.limits_match(&scope)? {
            return Err(ErrorCode::Unavailable);
        }
        let leaf = scope.join("workload");
        fs::create_dir(&leaf).map_err(|_| ErrorCode::Unavailable)?;
        write_cgroup(&leaf.join("cgroup.procs"), &live.pid.to_string())?;
        backend::delegate_workload_controllers(&scope, &leaf)?;
        let limits = self.limits();
        for (key, value) in [
            ("memory.max", limits.memory_bytes.to_string()),
            ("memory.swap.max", "0".into()),
            ("memory.oom.group", "1".into()),
            (
                "cpu.max",
                format!("{} {}", limits.cpu_quota_us, limits.cpu_period_us),
            ),
            ("pids.max", limits.tasks.to_string()),
        ] {
            write_cgroup(&leaf.join(key), &value)?;
        }
        if !self.limits_match(&leaf)? {
            return Err(ErrorCode::Unavailable);
        }
        let placed = backend::read(&leaf.join("memory.oom.group"))? == "1"
            && backend::read(&leaf.join("cgroup.procs"))? == live.pid.to_string()
            && backend::process_cgroup(live.pid)? == leaf;
        if !placed {
            return Err(ErrorCode::Unavailable);
        }
        self.owned_container(record)?;
        Ok(())
    }

    pub(super) fn release(&self, record: &mut PhotoRecord) -> Result<(), ErrorCode> {
        self.owned_container(record)?;
        let id = record.container_id.clone().ok_or(ErrorCode::Uncertain)?;
        record.manager_pending = Some(ManagerPhase::Unpause);
        self.update(record)?;
        docker(&self.config, &backend::strings(&["unpause", &id]))?;
        record.manager_pending = None;
        record.released = true;
        self.update(record)
    }
    pub(super) fn stop(&self, record: &mut PhotoRecord) -> Result<(), ErrorCode> {
        if record.container_id.is_none() {
            return Ok(());
        }
        let live = self.live(record)?;
        if live.running {
            self.verify_unit(record)?;
            stop_container(
                record,
                live.paused,
                |record| self.update(record),
                |args| docker(&self.config, args),
            )?;
        }
        Ok(())
    }

    pub(super) fn terminal_evidence(&self, record: &PhotoRecord) -> Result<Evidence, ErrorCode> {
        let path = self.verify_unit(record)?;
        let has_container = record.container_id.is_some();
        let live = if has_container {
            Some(self.live(record)?)
        } else {
            None
        };
        if live.as_ref().is_some_and(|live| live.running) {
            return Err(ErrorCode::Uncertain);
        }
        if !backend::cgroup_unpopulated(&path)? {
            return Err(ErrorCode::Uncertain);
        }
        let peak_bytes = backend::read(&path.join("memory.peak"))?
            .parse()
            .map_err(|_| ErrorCode::Uncertain)?;
        Ok(Evidence {
            peak_bytes,
            exit_code: live.as_ref().and_then(|live| live.exit_code),
            docker_oom_killed: live
                .as_ref()
                .and_then(|live| live.exit_code.map(|_| live.oom)),
            attempt_before: record
                .evidence
                .as_ref()
                .and_then(|evidence| evidence.attempt_before.clone()),
            attempt_after: Some(backend::events(&path)?),
            parent_before: record
                .evidence
                .as_ref()
                .and_then(|evidence| evidence.parent_before.clone()),
            parent_after: Some(backend::events(&self.parent_path())?),
            populated: Some(false),
            terminal_snapshot: None,
        })
    }

    pub(super) fn worker_outcome(
        &self,
        record: &PhotoRecord,
    ) -> Result<Option<Outcome>, ErrorCode> {
        backend::worker_result(
            &record
                .workspace(Path::new(&self.config.root))
                .join("work")
                .join(RESULT_NAME),
            &record.launch_id,
        )
    }

    pub(super) fn cleanup(&self, record: &mut PhotoRecord) -> Result<(), ErrorCode> {
        if record.manager_pending.is_some() {
            return Err(ErrorCode::Uncertain);
        }
        if let Some(id) = record.container_id.clone() {
            let ids = docker(
                &self.config,
                &backend::strings(&[
                    "ps",
                    "--all",
                    "--no-trunc",
                    "--quiet",
                    "--filter",
                    &format!("id={id}"),
                ]),
            )?;
            if !ids.is_empty() {
                if self.live(record)?.running {
                    return Err(ErrorCode::Uncertain);
                }
                docker(&self.config, &backend::strings(&["rm", &id]))?;
            }
        }
        let workspace = record.workspace(Path::new(&self.config.root));
        let work = workspace.join("work");
        if let Some(current) = backend::mount_identity(&work)? {
            if Some(current) != record.mount_id {
                return Err(ErrorCode::Uncertain);
            }
            backend::command(
                "/usr/bin/umount",
                &backend::strings(&[work.to_str().ok_or(ErrorCode::Uncertain)?]),
            )?;
            if backend::mount_identity(&work)?.is_some() {
                return Err(ErrorCode::Uncertain);
            }
        }
        if workspace.exists() {
            fs::remove_dir_all(&workspace).map_err(|_| ErrorCode::Uncertain)?;
        }
        if record.unit_invocation.is_some() && !record.stop_confirmed {
            self.verify_unit(record)?;
            record.manager_pending = Some(ManagerPhase::SliceStop);
            self.update(record)?;
            self.systemctl(&backend::strings(&[
                "stop",
                &attempt_unit(&self.config.instance, &record.launch_id),
            ]))?;
            record.stop_confirmed = true;
            record.manager_pending = None;
            self.update(record)?;
        }
        let path = self
            .parent_path()
            .join(attempt_unit(&self.config.instance, &record.launch_id));
        if let Some(invocation) = &record.unit_invocation {
            if !record.stop_confirmed {
                return Err(ErrorCode::Uncertain);
            }
            slice::wait_absent(
                &attempt_unit(&self.config.instance, &record.launch_id),
                &path,
                invocation,
                record.cgroup_inode.ok_or(ErrorCode::Uncertain)?,
            )?;
        } else {
            slice::never_created_absent(
                &attempt_unit(&self.config.instance, &record.launch_id),
                &path,
            )?;
        }
        Ok(())
    }
}

/// Write the bounded engine grant into the read-only control mount before
/// the container exists. The recipe payload reaches the worker only through
/// this launcher-owned file; the worker verifies its launch binding.
fn write_grant(record: &PhotoRecord, control: &Path) -> Result<(), ErrorCode> {
    let grant = serde_json::json!({
        "version": 1,
        "kind": "photo-development-grant",
        "launch_id": record.launch_id,
        "workload": record.workload,
        "exposure_milli_ev": record.recipe.exposure_milli_ev,
        "white_balance_mode": record.recipe.white_balance_mode,
        "profile_id": record.source.profile_id,
        "icc_asset_sha256": crate::photo::ICC_ASSET_SHA256,
    });
    let bytes = serde_json::to_vec(&grant).map_err(|_| ErrorCode::Uncertain)?;
    if bytes.len() > 16 * 1024 {
        return Err(ErrorCode::Uncertain);
    }
    let path = control.join("grant.json");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|_| ErrorCode::Unavailable)?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| ErrorCode::Unavailable)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444))
        .map_err(|_| ErrorCode::Unavailable)?;
    File::open(control)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| ErrorCode::Unavailable)
}

fn pidfd_alive(pidfd: &OwnedFd) -> Result<(), ErrorCode> {
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

fn write_cgroup(path: &Path, value: &str) -> Result<(), ErrorCode> {
    fs::write(path, value).map_err(|_| ErrorCode::Unavailable)
}