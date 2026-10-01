//! cgroup v2 and attempt-mount observation for the processing backend.
//!
//! This module owns every read of host control files that proves one attempt
//! stayed inside its finite boundary: cgroup limit readback, memory event
//! accounting, terminal evidence snapshots, workload controller delegation,
//! ancestor limit checks, attempt mount identity, and the sealed storage
//! capacity check. Every parse here fails closed: a missing, malformed, or
//! unexpectedly shaped control file is `Uncertain` tamper evidence, never a
//! best-effort observation. Callers keep ownership of manager phase
//! transitions; this module only observes and compares.

use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Read,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

use super::Result;
use crate::{
    journal::Record,
    protocol::{ErrorCode, Events, Limits, RESPONSE_BYTES, TerminalSnapshot},
};

pub(super) const CGROUP: &str = "/sys/fs/cgroup";

pub(super) fn storage_matches(stat: &libc::statfs, limits: &Limits) -> bool {
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

/// Read the four workload limit control files of one cgroup and compare
/// them with the expected values. The expected `memory_bytes` is supplied
/// by the caller: qualification verifies each record's own receipt memory
/// while the remaining limits always come from the configured profile.
pub(crate) fn limits_match(path: &Path, limits: &Limits, memory_bytes: u64) -> Result<bool> {
    Ok(read(&path.join("memory.max"))? == memory_bytes.to_string()
        && read(&path.join("memory.swap.max"))? == "0"
        && read(&path.join("cpu.max"))?
            == format!("{} {}", limits.cpu_quota_us, limits.cpu_period_us)
        && read(&path.join("pids.max"))? == limits.tasks.to_string())
}

pub(super) fn write(path: &Path, value: &str) -> Result<()> {
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
fn terminal_source_path(directory: &File, name: &str) -> PathBuf {
    PathBuf::from(format!("/proc/self/fd/{}/{}", directory.as_raw_fd(), name))
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

pub(super) fn terminal_snapshot(
    path: &Path,
    mut snapshot: TerminalSnapshot,
    expected_memory_limit: u64,
) -> Result<(TerminalSnapshot, u64, Events)> {
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| ErrorCode::Uncertain)?;
    let source = |name: &str| terminal_source_path(&directory, name);
    let mut total = 0;
    snapshot.memory_peak_raw = raw_terminal_source(&source("memory.peak"), &mut total)?;
    snapshot.memory_max_raw = raw_terminal_source(&source("memory.max"), &mut total)?;
    snapshot.memory_swap_current_raw =
        raw_terminal_source(&source("memory.swap.current"), &mut total)?;
    snapshot.memory_swap_max_raw = raw_terminal_source(&source("memory.swap.max"), &mut total)?;
    snapshot.memory_events_raw = raw_terminal_source(&source("memory.events"), &mut total)?;
    snapshot.memory_events_local_raw =
        raw_terminal_source(&source("memory.events.local"), &mut total)?;
    snapshot.io_stat_raw = match optional_terminal_io_source(&source("io.stat"), &mut total) {
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

pub(super) fn parse_terminal_values(
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

pub(super) fn expected_terminal_identity(record: &Record, path: &Path) -> Result<TerminalSnapshot> {
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

pub(super) fn same_terminal_identity(left: &TerminalSnapshot, right: &TerminalSnapshot) -> bool {
    left.cgroup_path == right.cgroup_path
        && left.cgroup_inode == right.cgroup_inode
        && left.unit_invocation == right.unit_invocation
        && left.launch_id == right.launch_id
        && left.container_id == right.container_id
        && left.attempt_unit == right.attempt_unit
        && left.incarnation == right.incarnation
        && left.sequence == right.sequence
}

pub(super) fn attempt_memory_limit(record: &Record) -> Result<u64> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::TERMINAL_SNAPSHOT_BYTES;

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
    fn terminal_source_path_survives_cgroup_directory_cleanup() {
        let (root, _) = snapshot_fixture();
        let directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&root)
            .unwrap();
        let detached = root.with_extension("detached");
        fs::rename(&root, &detached).unwrap();
        assert_eq!(
            fs::read_to_string(terminal_source_path(&directory, "io.stat")).unwrap(),
            "8:0 rbytes=1 wbytes=2 rios=3 wios=4 cost.usage=9\n"
        );
        drop(directory);
        fs::remove_dir_all(detached).unwrap();
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
        let TerminalIoCapture::Omitted(missing_reason) = missing else {
            unreachable!("missing io.stat must be diagnosed");
        };

        fs::create_dir(&path).unwrap();
        let unreadable = optional_terminal_io_source(&path, &mut total);
        assert!(matches!(
            &unreadable,
            TerminalIoCapture::Omitted(TerminalIoOmission::Read(Some(libc::EISDIR)))
        ));
        let TerminalIoCapture::Omitted(read_reason) = unreadable else {
            unreachable!("reading an io.stat directory must be diagnosed");
        };

        fs::remove_dir(&path).unwrap();
        fs::write(&path, b"ab").unwrap();
        total = TERMINAL_SNAPSHOT_BYTES - 1;
        let oversized = optional_terminal_io_source(&path, &mut total);
        let TerminalIoCapture::Omitted(limit_reason) = oversized else {
            unreachable!("over-limit io.stat must remain omitted");
        };
        assert_eq!(limit_reason, TerminalIoOmission::TooLarge);

        for diagnostic in [
            missing_reason.diagnostic(),
            read_reason.diagnostic(),
            limit_reason.diagnostic(),
        ] {
            assert!(!diagnostic.contains(&root.to_string_lossy().to_string()));
        }
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
}
