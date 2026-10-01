//! Attempt lifecycle for the processing backend.
//!
//! This module owns one attempt's manager-phase transitions: provisioning the
//! slice, workspace, private storage mount, gate, and container (`setup`),
//! resuming a released attempt (`release`), rediscovering durable intent
//! after a lost create response (`discover`), stopping a running attempt,
//! sealing and validating terminal evidence (`evidence`,
//! `validate_terminal_evidence`), and the fail-closed teardown (`cleanup`).
//! Every step persists intent before acting and treats each unexpected
//! observation as tamper evidence; the parent module keeps host and image
//! qualification, parent-slice management, and container inspection.

use super::cgroup::{
    CGROUP, attempt_memory_limit, expected_terminal_identity, parse_terminal_values,
    same_terminal_identity, terminal_snapshot, write,
};
use super::{
    Backend, Gate, Result, cgroup_unpopulated, checked_setup_observation, command,
    create_control_directory, create_native_gate, create_private_directory,
    delegate_workload_controllers, events, limits_match, mount_identity, pidfd_alive,
    process_cgroup, read, strings, verify_slice_phase, worker_result,
};
use crate::journal::{ManagerPhase, Record};
use crate::protocol::{ErrorCode, Evidence, Outcome, TerminalSnapshot, hex};
use std::fs::{self, File};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

impl Backend {
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
        let limits = limits_match(
            &expected,
            &self.config.limits(),
            record.receipt.limits.memory_bytes,
        );
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
        create_private_directory(&workspace)?;
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
        let limits = limits_match(
            &scope,
            &self.config.limits(),
            record.receipt.limits.memory_bytes,
        );
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
        let limits = limits_match(
            &leaf,
            &self.config.limits(),
            record.receipt.limits.memory_bytes,
        );
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

    pub(super) fn verify_unit(&self, record: &Record) -> Result<PathBuf> {
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
        worker_result(
            &self.workspace(record).join("work/result"),
            &record.launch_id,
        )
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
