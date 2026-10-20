use super::*;
use photo_files::{EntryKind, WalkLimits, fingerprint, read_json, walk};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path},
    sync::atomic::AtomicBool,
};

const MANIFEST_BYTES: usize = 64 * 1024 * 1024;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    format: u32,
    roots: BTreeMap<String, Root>,
    files: Vec<ManifestFile>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Root {
    previous_root: PathBuf,
    restored_root: PathBuf,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ManifestFile {
    relative_path: String,
    sha256: String,
    byte_length: u64,
}
fn refuse(message: impl Into<String>) -> WorkspaceError {
    WorkspaceError::new("restore_mismatch", 409, message)
}

/// Restore is invoked only against newly extracted isolated bytes. It never
/// opens the Library Original and never repairs data with today's defaults.
pub(crate) fn restore(state: &Path, manifest_path: &Path) -> Result<Value> {
    let manifest_parent = Directory::open(
        manifest_path
            .parent()
            .ok_or_else(|| refuse("Restore manifest has no directory."))?,
    )?;
    let manifest_file = manifest_parent.open_file(
        manifest_path
            .file_name()
            .ok_or_else(|| refuse("Restore manifest has no filename."))?,
    )?;
    let manifest: Manifest = serde_json::from_value(read_json(manifest_file, MANIFEST_BYTES)?)?;
    if manifest.format != 1
        || manifest.roots.is_empty()
        || manifest.roots.len() > 128
        || manifest.files.len() > ENTRY_MAX
    {
        return Err(refuse("Unsupported or oversized restore manifest."));
    }
    if !state.is_absolute()
        || state
            .components()
            .any(|c| matches!(c, Component::ParentDir))
    {
        return Err(refuse("Restored state must be an absolute isolated path."));
    }
    let state_key = manifest
        .roots
        .iter()
        .find(|(_, root)| root.restored_root == state)
        .map(|(key, _)| key.clone())
        .ok_or_else(|| refuse("State directory is not a restored manifest root."))?;
    let mut roots = BTreeMap::new();
    let mut leases = Vec::new();
    for (key, root) in &manifest.roots {
        if !key
            .strip_prefix("root-")
            .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()))
            || !root.previous_root.is_absolute()
            || !root.restored_root.is_absolute()
            || root
                .previous_root
                .components()
                .chain(root.restored_root.components())
                .any(|c| matches!(c, Component::ParentDir))
        {
            return Err(refuse("Invalid restore root mapping."));
        }
        for other in manifest.roots.values() {
            if root.restored_root.starts_with(&other.previous_root)
                || other.previous_root.starts_with(&root.restored_root)
            {
                return Err(refuse(
                    "Restored roots must be isolated from all live roots.",
                ));
            }
            if root.restored_root != other.restored_root
                && (root.restored_root.starts_with(&other.restored_root)
                    || other.restored_root.starts_with(&root.restored_root))
            {
                return Err(refuse("Restored roots overlap."));
            }
        }
        let directory = Directory::open(&root.restored_root)?;
        leases.push(directory.lease(LeaseMode::Shared, LOCK_WAIT)?);
        roots.insert(key.clone(), directory);
    }
    let mut expected = BTreeMap::new();
    for file in &manifest.files {
        let path = Path::new(&file.relative_path);
        if path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
            || file.sha256.len() != 64
            || !file
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
            || file.byte_length > FILE_BYTES_MAX
        {
            return Err(refuse("Invalid restore file evidence."));
        }
        let (root_key, relative) = file
            .relative_path
            .split_once('/')
            .ok_or_else(|| refuse("Manifest file is outside a root."))?;
        let root = roots
            .get(root_key)
            .ok_or_else(|| refuse("Manifest file has an unknown root."))?;
        if expected.insert(file.relative_path.clone(), file).is_some() {
            return Err(refuse("Duplicate manifest file."));
        }
        let mut opened = root.open_file(relative)?;
        let metadata = photo_files::Observation::of(&opened)?;
        let digest = if file.byte_length == 0 {
            use sha2::{Digest, Sha256};
            if metadata.byte_length != 0 {
                return Err(refuse(format!(
                    "Restored file changed: {}",
                    file.relative_path
                )));
            }
            format!("{:x}", Sha256::digest([]))
        } else {
            let evidence = fingerprint(&mut opened, FILE_BYTES_MAX)?;
            if evidence.byte_length != file.byte_length {
                return Err(refuse(format!(
                    "Restored length differs: {}",
                    file.relative_path
                )));
            }
            evidence.sha256
        };
        if digest != file.sha256 {
            return Err(refuse(format!(
                "Restored digest differs: {}",
                file.relative_path
            )));
        }
        metadata.verify(&root.open_file(relative)?)?;
    }
    let mut actual = BTreeSet::new();
    for (key, root) in &roots {
        enumerate(root, Path::new(key), &mut actual)?;
    }
    if actual != expected.keys().cloned().collect() {
        return Err(refuse(
            "Restored file inventory differs from the complete manifest.",
        ));
    }
    let state_root = &roots[&state_key];
    // SQLite integrity is checked before any observation metadata is updated.
    for path in actual
        .iter()
        .filter(|path| path.starts_with(&format!("{state_key}/")) && path.ends_with(".sqlite"))
    {
        let relative = path.strip_prefix(&format!("{state_key}/")).unwrap();
        let database = state_root.path().join(relative);
        let _descriptor = state_root.open_file(relative)?;
        let connection = rusqlite::Connection::open_with_flags(
            database,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let integrity: String = connection.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        if integrity != "ok" {
            return Err(refuse(format!(
                "Restored SQLite integrity failed: {relative}"
            )));
        }
    }
    let mut workspace_count = 0;
    if actual.contains(&format!("{state_key}/workspaces.sqlite")) {
        let manager = WorkspaceManager {
            root: state_root.child("workspaces")?,
            registry: Mutex::new(store::connection(state_root, "workspaces.sqlite")?),
            library_root: PathBuf::from("/"),
            resolve: Arc::new(|_| Ok(None)),
            allowance: DEFAULT_STORAGE_BYTES,
            queries: Mutex::new(query::Queries::new()),
            supervised_tools: Mutex::new(Vec::new()),
        };
        for workspace in manager.workspace_ids()? {
            let session = manager.locked(&workspace, LeaseMode::Exclusive)?;
            let prefix = format!("{state_key}/workspaces/{workspace}/");
            let records=manifest.files.iter().filter_map(|file|file.relative_path.strip_prefix(&prefix).map(|relative|json!({"relativePath":relative,"sha256":file.sha256,"byteLength":file.byte_length}))).collect::<Vec<_>>();
            files::rebase_restored_observations(&session, &json!(records))?;
            workspace_count += 1;
        }
    }
    let mappings = json!(
        manifest
            .roots
            .values()
            .map(
                |root| json!({"previousRoot":root.previous_root,"restoredRoot":root.restored_root})
            )
            .collect::<Vec<_>>()
    );
    let mut states = Vec::new();
    for path in &actual {
        if !path.ends_with("/state.sqlite3") {
            continue;
        }
        let (key, relative) = path.split_once('/').unwrap();
        let relative = Path::new(relative)
            .parent()
            .ok_or_else(|| refuse("Invalid tool state location."))?;
        let directory = if relative.as_os_str().is_empty() {
            roots[key].try_clone()?
        } else {
            roots[key].child(relative)?
        };
        directory.open_file("state.lock")?;
        let result = crate::darktable_service::restore_state(
            directory.path(),
            &mappings,
            &AtomicBool::new(false),
        )
        .map_err(|error| {
            refuse(format!(
                "Restored tool state refused {}: {}",
                directory.path().display(),
                error.value
            ))
        })?;
        states.push(result);
    }
    crate::darktable_service::rebase_restored_host(state, &mappings)
        .map_err(|error| refuse(error.value.to_string()))?;
    Ok(
        json!({"stateDirectory":state,"outcome":"restored","verifiedFiles":actual.len(),"workspaces":workspace_count,"toolStates":states}),
    )
}
fn enumerate(directory: &Directory, prefix: &Path, files: &mut BTreeSet<String>) -> Result<()> {
    let bound = || refuse("Restore directory depth exceeds the supported bound.");
    let inventory = || refuse("Restore file inventory exceeds the supported bound.");
    walk(
        directory,
        prefix,
        WalkLimits::new(ENTRY_MAX),
        bound,
        |reported| {
            match reported.entry.kind {
                EntryKind::File => {
                    files.insert(
                        reported
                            .path
                            .to_str()
                            .ok_or_else(|| refuse("Restore path is not UTF-8."))?
                            .to_owned(),
                    );
                    if files.len() > ENTRY_MAX {
                        return Err(inventory());
                    }
                }
                EntryKind::Directory => {
                    return Ok(Some(reported.directory.child(&reported.entry.name)?));
                }
                _ => return Err(refuse("Restore contains a symbolic link or special file.")),
            }
            Ok(None)
        },
        |_| Ok(()),
    )
}
