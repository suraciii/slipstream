use super::{Result, ServiceError};
use photo_files::{Directory, atomic_json, read_json};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
pub(crate) fn rebase_restored_host(state_directory: &Path, roots: &Value) -> Result<()> {
    let list = roots
        .as_array()
        .ok_or_else(|| ServiceError::new("invalid_input", "Restore roots must be an array", 400))?;
    let mut pairs = Vec::new();
    for root in list {
        let old = PathBuf::from(
            root["previousRoot"]
                .as_str()
                .ok_or_else(|| ServiceError::new("invalid_input", "Previous root missing", 400))?,
        );
        let new = PathBuf::from(
            root["restoredRoot"]
                .as_str()
                .ok_or_else(|| ServiceError::new("invalid_input", "Restored root missing", 400))?,
        );
        if !old.is_absolute() || !new.is_absolute() {
            return Err(ServiceError::new(
                "invalid_input",
                "Restore roots must be absolute",
                400,
            ));
        }
        pairs.push((old, new));
    }
    pairs.sort_by_key(|(old, _)| std::cmp::Reverse(old.components().count()));
    let path = state_directory.join("darktable-host");
    if !path.try_exists().map_err(ServiceError::io)? {
        return Ok(());
    }
    let host = Directory::open(path).map_err(ServiceError::io)?;
    let mut changes = Vec::new();
    collect(&host, "", &pairs, &mut changes)?;
    for (directory, name, value) in changes {
        atomic_json(&directory, name, &value).map_err(ServiceError::io)?;
    }
    Ok(())
}
fn relocate(value: &mut Value, pairs: &[(PathBuf, PathBuf)]) {
    if let Some(text) = value.as_str() {
        for (old, new) in pairs {
            if let Ok(relative) = Path::new(text).strip_prefix(old) {
                *value = json!(new.join(relative));
                break;
            }
        }
    }
}
fn facts(value: &mut Value, pairs: &[(PathBuf, PathBuf)]) {
    if let Some(object) = value.as_object_mut() {
        for key in ["path", "stateDirectory", "directory"] {
            if let Some(path) = object.get_mut(key) {
                relocate(path, pairs);
            }
        }
        for key in ["input", "comparison", "current", "result"] {
            if let Some(nested) = object.get_mut(key) {
                facts(nested, pairs);
            }
        }
        if let Some(dependencies) = object.get_mut("dependencies").and_then(Value::as_array_mut) {
            for dependency in dependencies {
                if let Some(path) = dependency.get_mut("path") {
                    relocate(path, pairs);
                }
            }
        }
        if object.contains_key("captureId")
            && let Some(arguments) = object.get_mut("arguments")
        {
            for key in ["directory", "file"] {
                if let Some(path) = arguments.get_mut(key) {
                    relocate(path, pairs);
                }
            }
            if let Some(path) = arguments.pointer_mut("/input/comparison/stateDirectory") {
                relocate(path, pairs);
            }
        }
    }
}
fn collect(
    directory: &Directory,
    relative: &str,
    pairs: &[(PathBuf, PathBuf)],
    changes: &mut Vec<(Directory, String, Value)>,
) -> Result<()> {
    for entry in directory.entries(10000).map_err(ServiceError::io)? {
        let path = directory.path().join(&entry.name);
        if path.is_dir() {
            let child = directory.child(&entry.name).map_err(ServiceError::io)?;
            let next = if relative.is_empty() {
                entry.name.clone()
            } else {
                format!("{relative}/{}", entry.name)
            };
            collect(&child, &next, pairs, changes)?;
            continue;
        }
        if !entry.name.ends_with(".json") {
            continue;
        }
        let job = relative == "jobs";
        let transfer = relative.starts_with("transfers/") && entry.name == "receipt.json";
        let review = relative == "migration-reviews";
        if !job && !transfer && !review {
            continue;
        }
        let mut value = read_json(
            directory.open_file(&entry.name).map_err(ServiceError::io)?,
            64 * 1024 * 1024,
        )
        .map_err(ServiceError::io)?;
        if job {
            if let Some(path) = value.get_mut("stateDirectory") {
                relocate(path, pairs);
            }
            for key in ["capture", "result", "transfer"] {
                if let Some(f) = value.get_mut(key) {
                    facts(f, pairs);
                }
            }
        }
        if transfer {
            rebase_transfer(directory, &mut value, pairs)?;
        }
        if review {
            facts(&mut value["reviewed"], pairs);
        }
        changes.push((
            directory.try_clone().map_err(ServiceError::io)?,
            entry.name,
            value,
        ));
    }
    Ok(())
}
fn rebase_transfer(
    directory: &Directory,
    value: &mut Value,
    pairs: &[(PathBuf, PathBuf)],
) -> Result<()> {
    let transfer: super::transfer::Transfer =
        serde_json::from_value(value["transfer"].clone()).map_err(ServiceError::io)?;
    let mut expected: photo_files::Evidence =
        serde_json::from_value(value["evidence"].clone()).map_err(ServiceError::io)?;
    let mut file = directory
        .open_file(&transfer.filename)
        .map_err(ServiceError::io)?;
    let actual = photo_files::fingerprint(&mut file, super::transfer::MAX_BYTES)
        .map_err(ServiceError::io)?;
    if actual.sha256 != expected.sha256 || actual.byte_length != expected.byte_length {
        return Err(ServiceError::new(
            "state_conflict",
            "Restored transfer bytes differ from retained evidence",
            409,
        ));
    }
    expected.observation = actual.observation;
    value["evidence"] = serde_json::to_value(expected).map_err(ServiceError::io)?;
    relocate(&mut value["transfer"]["path"], pairs);
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_rebase_only_changes_owned_paths() {
        let pairs = vec![(PathBuf::from("/old"), PathBuf::from("/new"))];
        let mut captured = json!({"captureId":"capture","input":{"path":"/old/input.raw"},"arguments":{"file":"/old/output.png","input":{"label":"/old/user-text","comparison":{"stateDirectory":"/old/edit"}}},"snapshot":{"parameters":{"text":"/old/user-text"}}});
        facts(&mut captured, &pairs);
        assert_eq!(captured["input"]["path"], "/new/input.raw");
        assert_eq!(captured["arguments"]["file"], "/new/output.png");
        assert_eq!(captured["arguments"]["input"]["label"], "/old/user-text");
        assert_eq!(captured["snapshot"]["parameters"]["text"], "/old/user-text");
    }
    #[test]
    fn restored_transfer_rebases_observation_and_remains_downloadable_and_importable() {
        use std::{
            fs,
            io::{Read, Write},
            sync::Arc,
        };
        let root =
            std::env::temp_dir().join(photo_files::identity("restore-transfer-test").unwrap());
        let old_state = root.join("old");
        let state = root.join("restored");
        fs::create_dir_all(&old_state).unwrap();
        fs::create_dir_all(&state).unwrap();
        let host = Directory::create(state.join("darktable-host")).unwrap();
        let container = host
            .create_directory("transfers")
            .unwrap()
            .create_directory("global")
            .unwrap()
            .create_directory("transfer-test")
            .unwrap();
        let old_path = old_state.join("payload.bin");
        fs::write(&old_path, b"retained transfer bytes").unwrap();
        let expected = photo_files::fingerprint(
            &mut fs::File::open(&old_path).unwrap(),
            super::super::transfer::MAX_BYTES,
        )
        .unwrap();
        container
            .create_file("payload.bin", 0o600)
            .unwrap()
            .write_all(b"retained transfer bytes")
            .unwrap();
        let transfer = super::super::transfer::Transfer {
            id: "transfer-test".into(),
            scope: None,
            filename: "payload.bin".into(),
            path: old_state.join("darktable-host/transfers/global/transfer-test/payload.bin"),
        };
        atomic_json(
            &container,
            "receipt.json",
            &json!({"transfer":transfer,"evidence":expected}),
        )
        .unwrap();
        let roots = json!([{"previousRoot":old_state,"restoredRoot":state}]);
        rebase_restored_host(&state, &roots).unwrap();
        let receipt = read_json(container.open_file("receipt.json").unwrap(), 16384).unwrap();
        assert_ne!(
            receipt["evidence"]["observation"],
            serde_json::to_value(&expected.observation).unwrap()
        );
        assert_eq!(receipt["evidence"]["sha256"], expected.sha256);
        assert_eq!(
            receipt["transfer"]["path"],
            json!(container.path().join("payload.bin"))
        );
        let workspaces = Arc::new(
            crate::workspace::WorkspaceManager::open(
                &state,
                root.join("library"),
                Arc::new(|_| Ok(None)),
                1024 * 1024,
            )
            .unwrap(),
        );
        let service = super::super::ToolService {
            jobs: super::super::jobs::Jobs::open(&host).unwrap(),
            root: host,
            workspaces,
        };
        let mut download = service.transfer_download(None, "transfer-test").unwrap();
        let mut bytes = Vec::new();
        download.file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"retained transfer bytes");
        drop(download);
        let mut arguments = json!({"file":{"transferId":"transfer-test"}});
        assert!(
            service
                .prepare_transfer(None, "style import", &mut arguments)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            fs::read(arguments["file"].as_str().unwrap()).unwrap(),
            b"retained transfer bytes"
        );
        fs::write(
            container.path().join("payload.bin"),
            b"different restored bytes",
        )
        .unwrap();
        assert_eq!(
            rebase_restored_host(&state, &roots).unwrap_err().value["error"]["code"],
            "state_conflict"
        );
        assert!(service.transfer_download(None, "transfer-test").is_err());
        drop(service);
        fs::remove_dir_all(root).unwrap();
    }
}
