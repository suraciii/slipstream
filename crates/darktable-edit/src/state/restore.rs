use super::*;
fn relocate(value: &mut Value, roots: &[(PathBuf, PathBuf)]) {
    fn path(value: &mut Value, roots: &[(PathBuf, PathBuf)]) {
        if let Some(text) = value.as_str() {
            for (old, new) in roots {
                if let Ok(relative) = Path::new(text).strip_prefix(old) {
                    *value = json!(new.join(relative));
                    break;
                }
            }
        }
    }
    if value.is_string() {
        path(value, roots);
        return;
    }
    if !value.is_object() {
        return;
    }
    if value.get("path").is_some() {
        path(&mut value["path"], roots);
    }
    for key in ["stateDirectory", "directory"] {
        if value.get(key).is_some() {
            path(&mut value[key], roots);
        }
    }
    if let Some(input) = value.get_mut("input")
        && input.is_object()
        && input.get("fingerprint").is_some()
        && let Some(input_path) = input.get_mut("path")
    {
        path(input_path, roots);
    }
    if let Some(dependencies) = value.get_mut("dependencies").and_then(Value::as_array_mut) {
        for dependency in dependencies {
            if let Some(dependency_path) = dependency.get_mut("path") {
                path(dependency_path, roots);
            }
        }
    }
    for key in ["comparison", "current", "result"] {
        if let Some(nested) = value.get_mut(key) {
            relocate(nested, roots);
        }
    }
    if value.get("captureId").is_some()
        && let Some(arguments) = value.get_mut("arguments").and_then(Value::as_object_mut)
    {
        for key in ["directory", "file"] {
            if let Some(argument) = arguments.get_mut(key) {
                path(argument, roots);
            }
        }
        if let Some(comparison) = arguments
            .get_mut("input")
            .and_then(|input| input.get_mut("comparison"))
            .and_then(Value::as_object_mut)
            && let Some(directory) = comparison.get_mut("stateDirectory")
        {
            path(directory, roots);
        }
    }
}
fn verify(value: &mut Value) -> Result<(), ToolError> {
    fn facts(value: &mut Value) -> Result<(), ToolError> {
        if value["availability"]["read"] == false {
            return Ok(());
        }
        if value["path"].is_string() {
            let expected = value
                .get("fingerprint")
                .or_else(|| value.get("byteEvidence"))
                .cloned()
                .unwrap_or_else(|| value.clone());
            if expected["sha256"].is_string() {
                let (observed, _lease) = evidence(Path::new(string(value, "path")?))?;
                if observed["fingerprint"]["sha256"] != expected["sha256"]
                    || observed["fingerprint"]["byteLength"] != expected["byteLength"]
                {
                    return Err(err(
                        "restore_content_conflict",
                        "Restored content does not match retained digest and byte length",
                    ));
                }
                if value.get("fingerprint").is_some() {
                    value["fingerprint"] = observed["fingerprint"].clone();
                    value["observedRevision"] = observed["observedRevision"].clone();
                }
                if value.get("byteEvidence").is_some() {
                    value["byteEvidence"] = observed["fingerprint"].clone();
                }
            }
        }
        Ok(())
    }
    if !value.is_object() {
        return Ok(());
    }
    facts(value)?;
    if let Some(input) = value.get_mut("input")
        && input.get("fingerprint").is_some()
    {
        facts(input)?;
    }
    if let Some(dependencies) = value.get_mut("dependencies").and_then(Value::as_array_mut) {
        for dependency in dependencies {
            facts(dependency)?;
        }
    }
    for key in ["comparison", "result"] {
        if let Some(nested) = value.get_mut(key) {
            verify(nested)?;
        }
    }
    Ok(())
}
pub(super) fn rebase(store: &Store, args: &Value) -> Result<Value, ToolError> {
    let _lock = store.lock()?;
    let roots = if let Some(roots) = args["roots"].as_array() {
        roots.clone()
    } else {
        vec![json!({"previousRoot":args["previousRoot"],"restoredRoot":args["restoredRoot"]})]
    };
    let mut pairs = Vec::new();
    for root in roots {
        let old = PathBuf::from(string(&root, "previousRoot")?);
        let new = PathBuf::from(string(&root, "restoredRoot")?);
        if !old.is_absolute() || !new.is_absolute() {
            return Err(err("invalid_input", "Restore roots must be absolute"));
        }
        pairs.push((old, new));
    }
    pairs.sort_by_key(|(old, _)| std::cmp::Reverse(old.components().count()));
    let mut revisions = Vec::new();
    let mut query = store.conn.prepare("SELECT id FROM revisions").map_err(db)?;
    for row in query.query_map([], |r| r.get::<_, String>(0)).map_err(db)? {
        let mut revision = store.revision(Some(&row.map_err(db)?))?;
        relocate(&mut revision, &pairs);
        verify(&mut revision)?;
        revisions.push(revision);
    }
    let mut captures = Vec::new();
    let mut query = store
        .conn
        .prepare("SELECT id,snapshot,result FROM captures")
        .map_err(db)?;
    for row in query
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })
        .map_err(db)?
    {
        let (id, snapshot, result) = row.map_err(db)?;
        let mut snapshot: Value = serde_json::from_str(&snapshot).map_err(db)?;
        relocate(&mut snapshot, &pairs);
        verify(&mut snapshot)?;
        let mut result = result
            .map(|v| serde_json::from_str::<Value>(&v).map_err(db))
            .transpose()?;
        if let Some(result) = result.as_mut() {
            relocate(result, &pairs);
            verify(result)?;
        }
        captures.push((id, snapshot, result));
    }
    let mut receipts = Vec::new();
    let mut query = store
        .conn
        .prepare("SELECT id,result FROM receipts")
        .map_err(db)?;
    for row in query
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(db)?
    {
        let (id, text) = row.map_err(db)?;
        let mut result: Value = serde_json::from_str(&text).map_err(db)?;
        relocate(&mut result, &pairs);
        verify(&mut result)?;
        receipts.push((id, result));
    }
    store.conn.execute_batch("BEGIN IMMEDIATE").map_err(db)?;
    let update = (|| {
        for revision in &revisions {
            store
                .conn
                .execute(
                    "UPDATE revisions SET snapshot=?,input=?,dependencies=? WHERE id=?",
                    params![
                        revision["snapshot"].to_string(),
                        revision["input"].to_string(),
                        revision["dependencies"].to_string(),
                        revision["revision"].as_str()
                    ],
                )
                .map_err(db)?;
        }
        for (id, snapshot, result) in &captures {
            store
                .conn
                .execute(
                    "UPDATE captures SET snapshot=?,input=?,dependencies=?,result=? WHERE id=?",
                    params![
                        snapshot.to_string(),
                        snapshot["input"].to_string(),
                        snapshot["dependencies"].to_string(),
                        result.as_ref().map(Value::to_string),
                        id
                    ],
                )
                .map_err(db)?;
        }
        for (id, result) in &receipts {
            store
                .conn
                .execute(
                    "UPDATE receipts SET result=? WHERE id=?",
                    params![result.to_string(), id],
                )
                .map_err(db)?;
        }
        for key in ["previewDirectory", "outputDirectory"] {
            let mut value = store.meta(key)?;
            relocate(&mut value, &pairs);
            store.set_meta(key, &value)?;
        }
        Ok::<_, ToolError>(())
    })();
    if let Err(error) = update {
        let _ = store.conn.execute_batch("ROLLBACK");
        return Err(error);
    }
    store.conn.execute_batch("COMMIT").map_err(db)?;
    Ok(
        json!({"stateDirectory":store.dir.path(),"outcome":"restored","retainedRevisions":revisions.len(),"captures":captures.len(),"receipts":receipts.len()}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restore_relocates_infrastructure_without_editing_semantic_text() {
        let mut value = json!({"input":{"path":"/old/raw","fingerprint":{"sha256":"saved","byteLength":4}},"dependencies":[{"path":"/old/resources/digest"}],"snapshot":{"instances":[{"parameters":{"caption":"/old/keep-exact"}}]},"effects":{"snapshot":{"output":{"settings":{"filename":"/old/keep-output-intent"}}}},"comparison":{"path":"/old/preview.png"},"path":"/old/render.tiff"});
        relocate(
            &mut value,
            &[(PathBuf::from("/old"), PathBuf::from("/new"))],
        );
        assert_eq!(value["input"]["path"], "/new/raw");
        assert_eq!(value["dependencies"][0]["path"], "/new/resources/digest");
        assert_eq!(
            value["snapshot"]["instances"][0]["parameters"]["caption"],
            "/old/keep-exact"
        );
        assert_eq!(
            value["effects"]["snapshot"]["output"]["settings"]["filename"],
            "/old/keep-output-intent"
        );
        assert_eq!(value["comparison"]["path"], "/new/preview.png");
    }
}
