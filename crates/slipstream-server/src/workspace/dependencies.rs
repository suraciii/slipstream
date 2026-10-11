use super::*;
use photo_files::{Directory, EntryKind, LeaseMode, Observation};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{io::Read, path::Path, time::Duration};

/// Full retained inventory, including tool-private bytes. A limit is a refusal,
/// never an apparently complete first page of a larger loss inventory.
pub(super) fn inventory(manager: &WorkspaceManager, session: &WorkspaceSession) -> Result<Value> {
    let mut result =
        json!({"edits":[],"dependencies":[],"activeOperations":[],"unknown":[],"stateFacts":[]});
    let mut count = 0;
    let mut cached = std::collections::BTreeMap::new();
    for item in files::all_files(session)? {
        if item["state"] == "ready"
            && let (Some(path), Some(evidence)) =
                (item["relativePath"].as_str(), item.get("evidence"))
            && let Ok(evidence) = serde_json::from_value::<photo_files::Evidence>(evidence.clone())
        {
            cached.insert(path.to_owned(), evidence);
        }
        if let Some(provenance) = item.get("provenance").filter(|v| !v.is_null()) {
            for key in ["input", "dependencies", "capture", "source"] {
                if let Some(binding) = provenance.get(key) {
                    references(
                        binding,
                        "output",
                        &ReferenceOwner {
                            id: item["artifactId"].as_str().unwrap_or(""),
                            name: item["filename"].as_str().unwrap_or("Kept output"),
                            external: false,
                            workspace: &session.workspace.id,
                        },
                        &mut result,
                        0,
                    )?;
                }
            }
        }
    }
    manifest(
        &session.directory,
        Path::new(""),
        &mut count,
        &mut result,
        &cached,
    )?;
    discover(
        &session.directory,
        &session.workspace.id,
        false,
        &mut result,
    )?;
    let mut query = session.db.prepare(
        "SELECT owner,kind,artifact_id,revision FROM dependencies ORDER BY owner,kind,artifact_id",
    )?;
    let retained=query.query_map([],|row|Ok(json!({"id":row.get::<_,String>(0)?,"kind":row.get::<_,String>(1)?,"artifactId":row.get::<_,String>(2)?,"revision":row.get::<_,String>(3)?,"name":row.get::<_,String>(0)?,"action":"Settle or explicitly remove the retained job/output dependency","external":false})))?.collect::<std::result::Result<Vec<_>,_>>()?;
    result["dependencies"]
        .as_array_mut()
        .unwrap()
        .extend(retained);
    for id in manager.workspace_ids()? {
        if id == session.workspace.id {
            continue;
        }
        match manager.directory(&id) {
            Ok(directory) => {
                let _workspace_guard=match directory.lease(LeaseMode::Shared,Duration::ZERO) {
                    Ok(guard)=>guard,
                    Err(error) if error.kind()==std::io::ErrorKind::WouldBlock=>{
                        result["activeOperations"].as_array_mut().unwrap().push(json!({"kind":"active-use","id":id,"name":format!("{id}/workspace"),"action":"Wait for the workspace operation to settle","external":true,"reason":error.to_string()}));
                        continue;
                    },
                    Err(error)=>{unknown(&mut result,&id,"workspace",true,error.to_string());continue;},
                };
                directory.verify_path()?;
                discover(&directory, &id, true, &mut result)?;
            },
            Err(error) => result["unknown"].as_array_mut().unwrap().push(json!({"kind":"unverified-use","id":id,"name":"Unavailable workspace dependency inventory","action":"Recover or explicitly remove the unavailable workspace","reason":error.message,"external":true})),
        }
    }
    for key in [
        "edits",
        "dependencies",
        "activeOperations",
        "unknown",
        "stateFacts",
    ] {
        result[key]
            .as_array_mut()
            .unwrap()
            .sort_by_cached_key(|v| v.to_string());
        if result[key].as_array().unwrap().len() > ENTRY_MAX {
            return Err(limit());
        }
    }
    Ok(result)
}

fn limit() -> WorkspaceError {
    WorkspaceError::new(
        "resource_limit",
        413,
        "Complete workspace inventory exceeds the finite scan limit",
    )
}

fn manifest(
    directory: &Directory,
    prefix: &Path,
    count: &mut usize,
    inventory: &mut Value,
    cached: &std::collections::BTreeMap<String, photo_files::Evidence>,
) -> Result<()> {
    if prefix.components().count() > 64 {
        return Err(limit());
    }
    for entry in directory.entries(ENTRY_MAX)? {
        *count += 1;
        if *count > ENTRY_MAX {
            return Err(limit());
        }
        // SQLite and its server lock are represented by the canonical index in
        // the parent's token, not their mutable database/journal byte layout.
        if prefix.as_os_str().is_empty() && entry.name.starts_with(".workspace") {
            continue;
        }
        let path = prefix.join(&entry.name);
        let display = path.to_string_lossy().into_owned();
        match entry.kind {
            EntryKind::Directory => {
                inventory["stateFacts"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"path":display,"kind":"directory"}));
                manifest(
                    &directory.child(&entry.name)?,
                    &path,
                    count,
                    inventory,
                    cached,
                )?;
            }
            EntryKind::File => {
                let mut file = directory.open_file(&entry.name)?;
                let observation = Observation::of(&file)?;
                if observation.byte_length > FILE_BYTES_MAX {
                    return Err(limit());
                }
                if let Some(evidence) = cached
                    .get(&display)
                    .filter(|e| e.observation == observation)
                {
                    observation.verify(&directory.open_file(&entry.name)?)?;
                    inventory["stateFacts"].as_array_mut().unwrap().push(json!({"path":display,"kind":"file","byteLength":evidence.byte_length,"sha256":evidence.sha256,"observation":observation}));
                    continue;
                }
                let mut hash = Sha256::new();
                let mut bytes = 0u64;
                let mut buffer = [0u8; 65536];
                loop {
                    let n = file.read(&mut buffer)?;
                    if n == 0 {
                        break;
                    }
                    bytes = bytes
                        .checked_add(n as u64)
                        .filter(|n| *n <= FILE_BYTES_MAX)
                        .ok_or_else(limit)?;
                    hash.update(&buffer[..n]);
                }
                observation.verify(&file)?;
                if bytes != observation.byte_length {
                    return Err(WorkspaceError::new(
                        "workspace_changed",
                        409,
                        "File changed while reviewing workspace inventory",
                    ));
                }
                observation.verify(&directory.open_file(&entry.name)?)?;
                inventory["stateFacts"].as_array_mut().unwrap().push(json!({"path":display,"kind":"file","byteLength":bytes,"sha256":format!("{:x}",hash.finalize()),"observation":observation}));
            }
            EntryKind::Symlink | EntryKind::Other => {
                inventory["stateFacts"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({"path":display,"kind":"unsafe"}));
                inventory["unknown"].as_array_mut().unwrap().push(json!({"kind":"unverified-use","id":null,"name":display,"action":"Remove the symbolic link or special file through its owning tool before deletion"}));
            }
        }
    }
    Ok(())
}

fn discover(
    directory: &Directory,
    workspace: &str,
    external: bool,
    inventory: &mut Value,
) -> Result<()> {
    let edits = match directory.child(".edits") {
        Ok(edits) => edits,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            unknown(inventory, workspace, ".edits", external, error.to_string());
            return Ok(());
        }
    };
    let mut count = 0;
    for tool in edits.entries(ENTRY_MAX)? {
        count += 1;
        if count > ENTRY_MAX {
            return Err(limit());
        }
        if tool.kind != EntryKind::Directory
            || !matches!(tool.name.as_str(), "darktable" | "spektrafilm")
        {
            unknown(
                inventory,
                workspace,
                &format!(".edits/{}", tool.name),
                external,
                "Unsupported private tool state".into(),
            );
            continue;
        }
        let tool_directory = edits.child(&tool.name)?;
        for state in tool_directory.entries(ENTRY_MAX)? {
            count += 1;
            if count > ENTRY_MAX {
                return Err(limit());
            }
            let name = format!(".edits/{}/{}", tool.name, state.name);
            if state.kind != EntryKind::Directory || !valid_id(&state.name) {
                unknown(
                    inventory,
                    workspace,
                    &name,
                    external,
                    "Uninspectable private state entry".into(),
                );
                continue;
            }
            let state_directory = tool_directory.child(&state.name)?;
            let _guard = match state_directory.lease(LeaseMode::Shared, Duration::ZERO) {
                Ok(guard) => guard,
                Err(error) => {
                    inventory["activeOperations"].as_array_mut().unwrap().push(json!({"kind":"active-use","id":state.name,"name":format!("{workspace}/{name}"),"action":"Wait for the independent tool using this edit to settle","external":external,"reason":error.to_string()}));
                    continue;
                }
            };
            state_directory.verify_path()?;
            let inspected = if tool.name == "darktable" {
                crate::darktable_service::inspect_state_retained(&state_directory, &_guard)
                    .map_err(|error| error.value.to_string())
            } else {
                crate::processing_recipe::inspect_film_state(state_directory.path())
            };
            match inspected {
                Ok(mut state_value) => {
                    state_value["editId"] = json!(state.name);
                    state_value["tool"] = json!(tool.name);
                    state_value["name"] = json!(state.name);
                    state_value["workspaceId"] = json!(workspace);
                    state_value["external"] = json!(external);
                    references(
                        &state_value,
                        "state",
                        &ReferenceOwner {
                            id: &state.name,
                            name: &format!("{workspace}/{name}"),
                            external,
                            workspace,
                        },
                        inventory,
                        0,
                    )?;
                    if !external {
                        inventory["edits"].as_array_mut().unwrap().push(state_value);
                    }
                }
                Err(error) => unknown(inventory, workspace, &name, external, error),
            }
        }
    }
    Ok(())
}

fn unknown(inventory: &mut Value, workspace: &str, name: &str, external: bool, reason: String) {
    inventory["unknown"].as_array_mut().unwrap().push(json!({"kind":"unverified-use","id":null,"name":format!("{workspace}/{name}"),"action":"Recover the owning tool's read-only state inspection or explicitly resolve this state","reason":reason,"external":external}));
}

struct ReferenceOwner<'a> {
    id: &'a str,
    name: &'a str,
    external: bool,
    workspace: &'a str,
}

fn references(
    value: &Value,
    kind: &str,
    owner: &ReferenceOwner<'_>,
    inventory: &mut Value,
    depth: usize,
) -> Result<()> {
    if depth > 64 {
        return Err(limit());
    }
    match value {
        Value::Object(object) => {
            if object.get("path").is_some_and(Value::is_string)
                || object.get("artifactId").is_some_and(Value::is_string)
            {
                let entry = json!({"artifactId":value["artifactId"],"path":value["path"],"revision":value["revision"],"sha256":value["sha256"],"kind":kind,"id":owner.id,"name":owner.name,"action":if kind=="output" {"Remove the retained output through its owning tool"} else {"Explicitly rebind or remove the retained edit/capture through its owning tool"},"external":owner.external,"ownerWorkspaceId":owner.workspace,"workspaceId":value.get("workspaceId").cloned().unwrap_or_else(||json!(owner.workspace))});
                let dependencies = inventory["dependencies"].as_array_mut().unwrap();
                if dependencies.len() >= ENTRY_MAX {
                    return Err(limit());
                }
                if !dependencies.contains(&entry) {
                    dependencies.push(entry);
                }
            }
            for (key, child) in object {
                if matches!(
                    key.as_str(),
                    "instances" | "masks" | "availability" | "executionHost"
                ) {
                    continue;
                }
                let child_kind = match key.as_str() {
                    "completedRenders" | "result" => "output",
                    "captures" => "job",
                    "dependencies" => "dependency",
                    _ => kind,
                };
                references(child, child_kind, owner, inventory, depth + 1)?;
            }
        }
        Value::Array(items) => {
            for child in items {
                references(child, kind, owner, inventory, depth + 1)?;
            }
        }
        _ => (),
    }
    Ok(())
}

pub(super) fn blockers(
    inventory: &Value,
    artifact: Option<&Value>,
    directory: &Path,
) -> Vec<Value> {
    let mut result: Vec<Value> = inventory["unknown"]
        .as_array()
        .into_iter()
        .flatten()
        .cloned()
        .collect();
    result.extend(
        inventory["activeOperations"]
            .as_array()
            .into_iter()
            .flatten()
            .cloned(),
    );
    for dependency in inventory["dependencies"].as_array().into_iter().flatten() {
        let selected = if let Some(artifact) = artifact {
            let path = directory.join(artifact["relativePath"].as_str().unwrap_or(""));
            dependency["artifactId"].is_string()
                && dependency["artifactId"] == artifact["artifactId"]
                || dependency["path"].as_str().is_some_and(|p| {
                    Path::new(p) == path
                        || Path::new(p)
                            == Path::new(artifact["relativePath"].as_str().unwrap_or(""))
                })
        } else {
            dependency["external"] == true
                && (dependency["path"]
                    .as_str()
                    .is_some_and(|p| Path::new(p).starts_with(directory))
                    || dependency["workspaceId"].as_str().is_some_and(|id| {
                        Some(id) == directory.file_name().and_then(|n| n.to_str())
                    }))
        };
        if selected {
            result.push(dependency.clone());
        }
    }
    result.sort_by_cached_key(|v| v.to_string());
    result.dedup();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retained_capture_and_external_output_references_block_exact_file() {
        let mut inventory = json!({"dependencies":[],"activeOperations":[],"unknown":[]});
        let owner = ReferenceOwner {
            id: "edit",
            name: "main",
            external: false,
            workspace: "w",
        };
        references(&json!({"retainedRevisions":[{"input":{"path":"/workspace/input/a.raw"}}],"captures":[{"input":{"path":"/workspace/input/a.raw"},"result":{"path":"/workspace/outputs/b.tif"}}]}),"state",&owner,&mut inventory,0).unwrap();
        assert_eq!(
            blockers(
                &inventory,
                Some(&json!({"artifactId":"a","relativePath":"input/a.raw"})),
                Path::new("/workspace")
            )
            .len(),
            2
        );
        assert_eq!(
            blockers(
                &inventory,
                Some(&json!({"artifactId":"b","relativePath":"outputs/b.tif"})),
                Path::new("/workspace")
            )
            .len(),
            1
        );
        assert!(
            blockers(
                &inventory,
                Some(&json!({"artifactId":"c","relativePath":"outputs/c.tif"})),
                Path::new("/workspace")
            )
            .is_empty()
        );
    }
    #[test]
    fn unknown_state_is_never_disposable() {
        let inventory = json!({"dependencies":[],"activeOperations":[],"unknown":[{"name":"unsupported state","kind":"unverified-use"}]});
        assert_eq!(blockers(&inventory, None, Path::new("/workspace")).len(), 1);
    }
    #[test]
    fn external_exclusive_workspace_admission_returns_active_use_without_waiting() {
        let root = std::env::temp_dir().join(photo_files::identity("dependency-test").unwrap());
        Directory::create(&root).unwrap();
        let manager = WorkspaceManager::open(
            &root,
            root.clone(),
            std::sync::Arc::new(|_| None),
            1024 * 1024 * 1024,
        )
        .unwrap();
        let first = manager.create_workspace("first").unwrap();
        let second = manager.create_workspace("second").unwrap();
        let session = manager.locked(&first.id, LeaseMode::Exclusive).unwrap();
        let other = manager.directory(&second.id).unwrap();
        let held = other.lease(LeaseMode::Exclusive, Duration::ZERO).unwrap();
        let result = inventory(&manager, &session).unwrap();
        assert_eq!(result["activeOperations"][0]["kind"], "active-use");
        assert_eq!(result["activeOperations"][0]["id"], second.id);
        drop(held);
        drop(session);
        drop(manager);
        std::fs::remove_dir_all(root).unwrap();
    }
}
