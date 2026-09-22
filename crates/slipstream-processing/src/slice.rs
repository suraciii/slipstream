//! Fixed transient attempt units. Aggregate provisioning stays in the backend.
use crate::{
    backend::{Result, command_until},
    protocol::{ErrorCode, Limits, hex},
};
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

const DESTINATION: &str = "org.freedesktop.systemd1";
const MANAGER_PATH: &str = "/org/freedesktop/systemd1";
const MANAGER: &str = "org.freedesktop.systemd1.Manager";
const UNIT: &str = "org.freedesktop.systemd1.Unit";
const TRANSIENT: &str = "/run/systemd/transient";

fn bus(
    path: &str,
    interface: &str,
    method: &str,
    args: &[String],
    deadline: Instant,
) -> Result<Value> {
    let mut argv: Vec<String> = [
        "--system",
        "--allow-interactive-authorization=no",
        "--timeout=5",
        "--json=short",
        "call",
        DESTINATION,
        path,
        interface,
        method,
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    argv.extend_from_slice(args);
    serde_json::from_str(&command_until("/usr/bin/busctl", &argv, deadline)?)
        .map_err(|_| ErrorCode::Uncertain)
}

fn data(value: &Value, signature: &str) -> Result<Value> {
    if value["type"] != signature
        || value["data"]
            .as_array()
            .is_none_or(|items| items.len() != 1)
    {
        return Err(ErrorCode::Uncertain);
    }
    Ok(value["data"][0].clone())
}

#[derive(Debug, PartialEq)]
struct LoadedUnit {
    path: String,
    active: String,
}

fn loaded(name: &str, deadline: Instant) -> Result<Option<LoadedUnit>> {
    let reply = bus(
        MANAGER_PATH,
        MANAGER,
        "ListUnitsByPatterns",
        &["asas".into(), "0".into(), "1".into(), name.into()],
        deadline,
    )?;
    inventory(&reply, name)
}

fn inventory(reply: &Value, name: &str) -> Result<Option<LoadedUnit>> {
    let value = data(reply, "a(ssssssouso)")?;
    let rows = value.as_array().ok_or(ErrorCode::Uncertain)?;
    if rows.is_empty() {
        return Ok(None);
    }
    if rows.len() != 1 {
        return Err(ErrorCode::Uncertain);
    }
    let row = rows[0].as_array().ok_or(ErrorCode::Uncertain)?;
    if row.len() != 10
        || row[0] != name
        || row[..7].iter().any(|v| !v.is_string())
        || row[7].as_u64().is_none()
        || !row[8].is_string()
        || !row[9].is_string()
    {
        return Err(ErrorCode::Uncertain);
    }
    let path = row[6].as_str().ok_or(ErrorCode::Uncertain)?;
    if !path.starts_with("/org/freedesktop/systemd1/unit/") {
        return Err(ErrorCode::Uncertain);
    }
    Ok(Some(LoadedUnit {
        path: path.into(),
        active: row[3].as_str().ok_or(ErrorCode::Uncertain)?.into(),
    }))
}

fn property(value: &Value, name: &str, signature: &str) -> Result<Value> {
    let item = value.get(name).ok_or(ErrorCode::Uncertain)?;
    if item["type"] != signature {
        return Err(ErrorCode::Uncertain);
    }
    item.get("data").cloned().ok_or(ErrorCode::Uncertain)
}

#[derive(Debug)]
struct UnitState {
    id: String,
    invocation: String,
    active: String,
    transient: bool,
    fragment: String,
    drop_ins: Vec<String>,
    stop_when_unneeded: bool,
}

impl UnitState {
    fn parse(reply: &Value) -> Result<Self> {
        let values = data(reply, "a{sv}")?;
        let string = |name| {
            property(&values, name, "s")?
                .as_str()
                .map(str::to_owned)
                .ok_or(ErrorCode::Uncertain)
        };
        let boolean = |name| {
            property(&values, name, "b")?
                .as_bool()
                .ok_or(ErrorCode::Uncertain)
        };
        let bytes = property(&values, "InvocationID", "ay")?;
        let bytes = bytes.as_array().ok_or(ErrorCode::Uncertain)?;
        if bytes.len() != 16 {
            return Err(ErrorCode::Uncertain);
        }
        let mut invocation = String::new();
        for byte in bytes {
            let byte = byte
                .as_u64()
                .and_then(|b| u8::try_from(b).ok())
                .ok_or(ErrorCode::Uncertain)?;
            invocation.push_str(&format!("{byte:02x}"));
        }
        let drops = property(&values, "DropInPaths", "as")?;
        let drop_ins = drops
            .as_array()
            .ok_or(ErrorCode::Uncertain)?
            .iter()
            .map(|v| v.as_str().map(str::to_owned).ok_or(ErrorCode::Uncertain))
            .collect::<Result<_>>()?;
        Ok(Self {
            id: string("Id")?,
            invocation,
            active: string("ActiveState")?,
            transient: boolean("Transient")?,
            fragment: string("FragmentPath")?,
            drop_ins,
            stop_when_unneeded: boolean("StopWhenUnneeded")?,
        })
    }

    fn matches(&self, name: &str, invocation: &str) -> bool {
        self.id == name
            && self.invocation == invocation
            && self.invocation != "0".repeat(32)
            && self.transient
            && self.fragment == format!("{TRANSIENT}/{name}")
            && self.drop_ins.is_empty()
            && !self.stop_when_unneeded
    }
}

fn state(path: &str, deadline: Instant) -> Result<UnitState> {
    UnitState::parse(&bus(
        path,
        "org.freedesktop.DBus.Properties",
        "GetAll",
        &["s".into(), UNIT.into()],
        deadline,
    )?)
}

fn invocation_path(invocation: &str, deadline: Instant) -> Result<String> {
    if !hex(invocation, 32) {
        return Err(ErrorCode::Uncertain);
    }
    let mut args = vec!["ay".into(), "16".into()];
    for offset in (0..32).step_by(2) {
        args.push(
            u8::from_str_radix(&invocation[offset..offset + 2], 16)
                .map_err(|_| ErrorCode::Uncertain)?
                .to_string(),
        );
    }
    data(
        &bus(
            MANAGER_PATH,
            MANAGER,
            "GetUnitByInvocationID",
            &args,
            deadline,
        )?,
        "o",
    )?
    .as_str()
    .filter(|path| path.starts_with("/org/freedesktop/systemd1/unit/"))
    .map(str::to_owned)
    .ok_or(ErrorCode::Uncertain)
}

fn present(path: &Path) -> Result<bool> {
    Ok(metadata(path)?.is_some())
}

fn metadata(path: &Path) -> Result<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(ErrorCode::Uncertain),
    }
}

fn unit_paths(deadline: Instant) -> Result<Vec<PathBuf>> {
    let reply = bus(
        MANAGER_PATH,
        "org.freedesktop.DBus.Properties",
        "Get",
        &["ss".into(), MANAGER.into(), "UnitPath".into()],
        deadline,
    )?;
    parse_unit_paths(&reply)
}

fn parse_unit_paths(reply: &Value) -> Result<Vec<PathBuf>> {
    let value = data(reply, "v")?;
    if value["type"] != "as" {
        return Err(ErrorCode::Uncertain);
    }
    let values = value["data"].as_array().ok_or(ErrorCode::Uncertain)?;
    if values.is_empty() || values.len() > 64 {
        return Err(ErrorCode::Uncertain);
    }
    let mut paths = Vec::new();
    for value in values {
        let text = value.as_str().ok_or(ErrorCode::Uncertain)?;
        let path = PathBuf::from(text);
        if text.len() > 4096
            || !path.is_absolute()
            || path.components().any(|c| {
                matches!(
                    c,
                    std::path::Component::ParentDir | std::path::Component::CurDir
                )
            })
            || paths.contains(&path)
        {
            return Err(ErrorCode::Uncertain);
        }
        paths.push(path);
    }
    if !paths.iter().any(|p| p == Path::new(TRANSIENT)) {
        return Err(ErrorCode::Uncertain);
    }
    Ok(paths)
}

fn configuration(name: &str, deadline: Instant) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for directory in unit_paths(deadline)? {
        for filename in [name.to_owned(), format!("{name}.d")] {
            let path = directory.join(filename);
            if let Some(metadata) = metadata(&path)? {
                if metadata.file_type().is_symlink() {
                    return Err(ErrorCode::Uncertain);
                }
                found.push(path);
            }
        }
    }
    if Instant::now() >= deadline {
        return Err(ErrorCode::Uncertain);
    }
    Ok(found)
}

fn absent(name: &str, group: &Path, deadline: Instant) -> Result<bool> {
    let unit_absent = loaded(name, deadline)?.is_none();
    let group_absent = !present(group)?;
    let config_absent = configuration(name, deadline)?.is_empty();
    Ok(unit_absent && group_absent && config_absent && Instant::now() < deadline)
}

fn creation_args(name: &str, limits: &Limits) -> Result<Vec<String>> {
    let quota = limits
        .cpu_quota_us
        .checked_mul(1_000_000)
        .and_then(|v| v.checked_div(limits.cpu_period_us))
        .ok_or(ErrorCode::Uncertain)?;
    Ok(vec![
        "ssa(sv)a(sa(sv))".into(),
        name.into(),
        "fail".into(),
        "6".into(),
        "MemoryMax".into(),
        "t".into(),
        limits.memory_bytes.to_string(),
        "MemorySwapMax".into(),
        "t".into(),
        "0".into(),
        "TasksMax".into(),
        "t".into(),
        limits.tasks.to_string(),
        "CPUQuotaPerSecUSec".into(),
        "t".into(),
        quota.to_string(),
        "CPUQuotaPeriodUSec".into(),
        "t".into(),
        limits.cpu_period_us.to_string(),
        "StopWhenUnneeded".into(),
        "b".into(),
        "false".into(),
        "0".into(),
    ])
}

pub(crate) fn create(name: &str, group: &Path, limits: &Limits) -> Result<(String, u64)> {
    if !absent(name, group, Instant::now() + Duration::from_secs(5))? {
        return Err(ErrorCode::Uncertain);
    }
    let reply = bus(
        MANAGER_PATH,
        MANAGER,
        "StartTransientUnit",
        &creation_args(name, limits)?,
        Instant::now() + Duration::from_secs(5),
    )?;
    if !data(&reply, "o")?
        .as_str()
        .is_some_and(|s| s.starts_with("/org/freedesktop/systemd1/job/"))
    {
        return Err(ErrorCode::Uncertain);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(unit) = loaded(name, deadline)? {
            // Ordinary GC cannot remove an active explicitly retained slice.
            // Name lookup is used only once to capture its first InvocationID.
            if unit.active == "activating" {
                pause(deadline)?;
                continue;
            }
            if unit.active != "active" {
                return Err(ErrorCode::Uncertain);
            }
            let current = state(&unit.path, deadline)?;
            if !current.matches(name, &current.invocation) {
                return Err(ErrorCode::Uncertain);
            }
            if current.active == "active" {
                let inode = fs::metadata(group).map_err(|_| ErrorCode::Uncertain)?.ino();
                verify(name, group, &current.invocation, inode)?;
                return Ok((current.invocation, inode));
            }
            if current.active != "activating" {
                return Err(ErrorCode::Uncertain);
            }
        }
        pause(deadline)?;
    }
}

pub(crate) fn verify(name: &str, group: &Path, invocation: &str, inode: u64) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let path = invocation_path(invocation, deadline)?;
    let current = state(&path, deadline)?;
    let control = data(
        &bus(
            &path,
            "org.freedesktop.DBus.Properties",
            "Get",
            &[
                "ss".into(),
                "org.freedesktop.systemd1.Slice".into(),
                "ControlGroup".into(),
            ],
            deadline,
        )?,
        "v",
    )?;
    if !current.matches(name, invocation)
        || current.active != "active"
        || control["type"] != "s"
        || control["data"]
            != group
                .strip_prefix("/sys/fs/cgroup")
                .map_err(|_| ErrorCode::Uncertain)?
                .to_str()
                .map(|s| format!("/{s}"))
                .ok_or(ErrorCode::Uncertain)?
        || fs::metadata(group).map_err(|_| ErrorCode::Uncertain)?.ino() != inode
    {
        return Err(ErrorCode::Uncertain);
    }
    Ok(())
}

fn pause(deadline: Instant) -> Result<()> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or(ErrorCode::Uncertain)?;
    thread::sleep(remaining.min(Duration::from_millis(10)));
    if Instant::now() >= deadline {
        return Err(ErrorCode::Uncertain);
    }
    Ok(())
}

pub(crate) fn wait_absent(name: &str, group: &Path, invocation: &str, inode: u64) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let found = loaded(name, deadline)?;
        let config = configuration(name, deadline)?;
        // Only the exact transient fragment can still be undergoing normal unload.
        if config
            .iter()
            .any(|path| path != &Path::new(TRANSIENT).join(name))
        {
            return Err(ErrorCode::Uncertain);
        }
        let group_metadata = metadata(group)?;
        if group_metadata
            .as_ref()
            .is_some_and(|metadata| metadata.ino() != inode)
        {
            return Err(ErrorCode::Uncertain);
        }
        if found.is_none()
            && config.is_empty()
            && group_metadata.is_none()
            && Instant::now() < deadline
        {
            return Ok(());
        }
        if found.is_some() {
            let observed =
                invocation_path(invocation, deadline).and_then(|path| state(&path, deadline));
            match observed {
                Ok(current) => {
                    if !current.matches(name, invocation)
                        || !matches!(current.active.as_str(), "inactive" | "deactivating")
                    {
                        return Err(ErrorCode::Uncertain);
                    }
                }
                Err(_) => {
                    // The failed read proves nothing. An independently complete final
                    // observation may prove disappearance after the confirmed stop.
                    return if absent(name, group, deadline)? {
                        Ok(())
                    } else {
                        Err(ErrorCode::Uncertain)
                    };
                }
            }
        }
        pause(deadline)?;
    }
}

pub(crate) fn never_created_absent(name: &str, group: &Path) -> Result<()> {
    if absent(name, group, Instant::now() + Duration::from_secs(5))? {
        Ok(())
    } else {
        Err(ErrorCode::Uncertain)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn properties() -> Value {
        json!({"type":"a{sv}","data":[{
            "Id":{"type":"s","data":"attempt.slice"},
            "InvocationID":{"type":"ay","data":[1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1]},
            "ActiveState":{"type":"s","data":"active"},
            "Transient":{"type":"b","data":true},
            "FragmentPath":{"type":"s","data":"/run/systemd/transient/attempt.slice"},
            "DropInPaths":{"type":"as","data":[]},
            "StopWhenUnneeded":{"type":"b","data":false}
        }]})
    }

    #[test]
    fn configuration_provenance_rejects_legacy_overrides_and_other_generations() {
        let good = properties();
        assert!(
            UnitState::parse(&good)
                .unwrap()
                .matches("attempt.slice", &"01".repeat(16))
        );
        for (key, value) in [
            ("Id", json!("foreign.slice")),
            ("Transient", json!(false)),
            ("FragmentPath", json!("/etc/systemd/system/attempt.slice")),
            (
                "DropInPaths",
                json!(["/etc/systemd/system/slice.d/override.conf"]),
            ),
            (
                "DropInPaths",
                json!(["/run/systemd/system.control/attempt.slice.d/50-MemoryMax.conf"]),
            ),
            ("StopWhenUnneeded", json!(true)),
            ("InvocationID", json!(vec![2; 16])),
        ] {
            let mut changed = good.clone();
            changed["data"][0][key]["data"] = value;
            assert!(
                !UnitState::parse(&changed)
                    .unwrap()
                    .matches("attempt.slice", &"01".repeat(16)),
                "{key}"
            );
        }
        let mut malformed = good;
        malformed["data"][0]["InvocationID"]["data"] = json!(vec![256; 16]);
        assert!(UnitState::parse(&malformed).is_err());
    }

    #[test]
    fn inventory_requires_complete_exact_typed_rows() {
        assert_eq!(
            inventory(
                &json!({"type":"a(ssssssouso)","data":[[]]}),
                "attempt.slice"
            )
            .unwrap(),
            None
        );
        for bad in [
            json!({}),
            json!({"type":"a(ssssssouso)","data":[]}),
            json!({"type":"a(ssssssouso)","data":[[[]]]}),
            json!({"type":"a(ssssssouso)","data":[[["foreign.slice","","loaded","inactive","dead","","/org/freedesktop/systemd1/unit/foreign",0,"","/"]]]}),
        ] {
            assert!(inventory(&bad, "attempt.slice").is_err());
        }
    }

    #[test]
    fn manager_paths_are_bounded_absolute_and_include_transient_inventory() {
        let reply = |paths: Value| json!({"type":"v","data":[{"type":"as","data":paths}]});
        assert_eq!(
            parse_unit_paths(&reply(json!([TRANSIENT, "/etc/systemd/system"])))
                .unwrap()
                .len(),
            2
        );
        for paths in [
            json!([]),
            json!(["relative"]),
            json!(["/etc/systemd/system"]),
            json!([TRANSIENT, TRANSIENT]),
            json!([TRANSIENT, "/run/../etc/systemd"]),
            json!([TRANSIENT, 1]),
            json!(vec![TRANSIENT; 65]),
        ] {
            assert!(parse_unit_paths(&reply(paths)).is_err());
        }
    }

    #[test]
    fn single_metadata_observation_preserves_symlinks_and_normal_disappearance() {
        let path =
            std::env::temp_dir().join(format!("slipstream-slice-link-{}", std::process::id()));
        std::os::unix::fs::symlink("unavailable-target", &path).unwrap();
        assert!(metadata(&path).unwrap().unwrap().file_type().is_symlink());
        assert_eq!(present(&path), Ok(true));
        fs::remove_file(&path).unwrap();
        assert!(metadata(&path).unwrap().is_none());
        fs::write(&path, b"owned transient fragment").unwrap();
        let observed = metadata(&path).unwrap().unwrap();
        fs::remove_file(&path).unwrap();
        assert!(observed.is_file());
        assert!(observed.ino() > 0);
        assert!(metadata(&path).unwrap().is_none());
    }

    #[test]
    fn creation_binds_finite_captured_resources_without_auxiliary_units() {
        let mut limits = Limits::new(134217728);
        limits.cpu_quota_us = 400000;
        let args = creation_args("attempt.slice", &limits).unwrap();
        assert_eq!(
            &args[..4],
            &["ssa(sv)a(sa(sv))", "attempt.slice", "fail", "6"]
        );
        let values: Vec<_> = args[4..args.len() - 1].chunks_exact(3).collect();
        assert_eq!(values[0], &["MemoryMax", "t", "134217728"]);
        assert_eq!(values[1], &["MemorySwapMax", "t", "0"]);
        assert_eq!(values[3], &["CPUQuotaPerSecUSec", "t", "4000000"]);
        assert_eq!(values[4], &["CPUQuotaPeriodUSec", "t", "100000"]);
        assert_eq!(values[5], &["StopWhenUnneeded", "b", "false"]);
        assert_eq!(args.last().unwrap(), "0");
        limits.cpu_period_us = 0;
        assert!(creation_args("attempt.slice", &limits).is_err());
        limits.cpu_period_us = 100000;
        limits.cpu_quota_us = u64::MAX;
        assert!(creation_args("attempt.slice", &limits).is_err());
    }
}
