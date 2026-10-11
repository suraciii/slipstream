use super::*;
fn legacy_payload(legacy: &Value) -> Result<Value, ToolError> {
    if legacy["schemaVersion"] != "darktable-params-1" || legacy["module"] != "darktable" {
        return Err(err(
            "schema_unsupported",
            "Only verified darktable-params-1 semantic documents are admitted",
        ));
    }
    let tree = legacy["tree"]
        .as_object()
        .ok_or_else(|| err("invalid_input", "Legacy tree must be an object"))?;
    if tree
        .keys()
        .any(|key| !matches!(key.as_str(), "stack" | "output" | "baseline" | "masks"))
    {
        return Err(err(
            "migration_unavailable",
            "Legacy tree contains undisclosed semantics",
        ));
    }
    let stack = tree
        .get("stack")
        .and_then(Value::as_array)
        .ok_or_else(|| err("invalid_input", "Legacy stack must be an array"))?;
    for entry in stack {
        let object = entry
            .as_object()
            .ok_or_else(|| err("invalid_input", "Legacy stack entry must be object"))?;
        if object.keys().any(|key| {
            !matches!(
                key.as_str(),
                "operation" | "version" | "params" | "enabled" | "multiPriority" | "blend"
            )
        }) {
            return Err(err(
                "migration_unavailable",
                "Legacy entry contains undisclosed semantics",
            ));
        }
        if !entry["params"].is_object() {
            return Err(err(
                "migration_unavailable",
                "Native binary parameter blobs are not admitted",
            ));
        }
    }
    Ok(legacy.clone())
}
pub fn review_legacy_import(input: &Path, legacy: &Value) -> Result<Value, ToolError> {
    let (input, _lease) = evidence(input)?;
    let engine = Engine::open()?;
    let mut converted =
        engine.import(Path::new(string(&input, "path")?), &legacy_payload(legacy)?)?;
    let mapping = converted
        .as_object_mut()
        .and_then(|o| o.remove("identityMappings"))
        .ok_or_else(|| {
            err(
                "migration_unavailable",
                "Native converter omitted exact identity mapping",
            )
        })?;
    let checked = engine.validate(Path::new(string(&input, "path")?), &converted)?;
    let mut checked = checked;
    let mut mapping = mapping;
    let mut identities = std::collections::HashMap::new();
    for (collection, key, prefix) in [
        ("instances", "instanceId", "instance"),
        ("masks", "maskId", "mask"),
    ] {
        for item in checked[collection].as_array().into_iter().flatten() {
            let original = string(item, key)?;
            identities.insert(original.to_owned(),format!("{prefix}-{}",digest(&json!({"source":legacy,"input":input["boundRevision"],"nativeIdentity":original}))?));
        }
    }
    fn remap(value: &mut Value, identities: &std::collections::HashMap<String, String>) {
        match value {
            Value::String(text) => {
                if let Some(identity) = identities.get(text) {
                    *text = identity.clone();
                }
            }
            Value::Array(items) => {
                for item in items {
                    remap(item, identities)
                }
            }
            Value::Object(object) => {
                for item in object.values_mut() {
                    remap(item, identities)
                }
            }
            _ => (),
        }
    }
    remap(&mut checked, &identities);
    remap(&mut mapping, &identities);
    let reviewed = json!({"converterId":"darktable-params-1-native-semantic","sourceDigest":digest(legacy)?,"input":input,"bundleId":engine.bundle_id(),"schemaVersion":engine.schema_version(),"snapshot":checked,"changes":[{"type":"canonical-semantic-import","source":legacy,"result":checked}],"identityMappings":mapping});
    let mut result = reviewed.clone();
    result["migrationToken"] = json!(digest(&reviewed)?);
    result["availability"] =
        json!({"read":true,"write":true,"execute":true,"reason":null,"recovery":null});
    Ok(result)
}
pub fn import_legacy(
    invocation: &Invocation,
    legacy: &Value,
    reviewed: &Value,
) -> Result<Value, ToolError> {
    let path = invocation
        .state_directory
        .as_ref()
        .ok_or_else(|| err("invalid_input", "State directory required"))?;
    let request = invocation.arguments["requestId"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or(id("request")?);
    identifier(&request)?;
    let payload = digest(
        &json!({"operation":"__migration-import","arguments":invocation.arguments,"legacy":legacy,"reviewed":reviewed}),
    )?;
    if let Ok(store) = Store::read(path)
        && let Some(result) = store.replay(&request, &payload)?
    {
        return Ok(result);
    }
    if invocation.arguments["confirm"] != true {
        return Err(err(
            "confirmation_required",
            "Explicit confirmation of reviewed legacy conversion is required",
        ));
    }
    let input = Path::new(string(&invocation.arguments, "inputPath")?);
    let current_review = review_legacy_import(input, legacy)?;
    if current_review["migrationToken"] != reviewed["migrationToken"] {
        return Err(err(
            "state_conflict",
            "Reviewed conversion, input, or target assets changed",
        ));
    }
    let store = match Store::read(path) {
        Ok(store) => store,
        Err(error) if error.code == "state_not_found" => {
            let mut opening = invocation.clone();
            opening.operation = "open".into();
            open(&opening)?;
            Store::read(path)?
        }
        Err(error) => return Err(error),
    };
    let _lock = store.lock()?;
    if let Some(result) = store.replay(&request, &payload)? {
        return Ok(result);
    }
    let revision = store.revision(None)?;
    store.reject_capture_request(&request)?;
    if revision["input"]["fingerprint"] != current_review["input"]["fingerprint"] {
        return Err(err(
            "input_binding_conflict",
            "Legacy import input differs from state binding",
        ));
    }
    if store.meta("current")? != store.meta("baseline")? {
        store.guard(&invocation.arguments, &revision, true)?;
    }
    let snapshot = current_review["snapshot"].clone();
    let dependencies = catalog::pin_dependencies(store.dir.path(), &snapshot, "local")?;
    let next = json!({"revision":id("revision")?,"parent":revision["revision"],"createdAt":timestamp(),"requestId":request,"snapshot":snapshot,"input":revision["input"],"bundleId":current_review["bundleId"],"schemaVersion":current_review["schemaVersion"],"dependencies":dependencies,"changedTargets":[{"kind":"legacy-import"}]});
    let token = store.token(&next)?;
    let result = json!({"stateDirectory":store.dir.path(),"executionHost":host(),"revision":next["revision"],"stateToken":token,"requestId":request,"retainUntil":deadline(),"outcome":"saved","changedTargets":next["changedTargets"],"effects":{"snapshot":snapshot},"generatedIds":{},"identityMappings":current_review["identityMappings"],"current":{"revision":next["revision"],"stateToken":token}});
    store.conn.execute_batch("BEGIN IMMEDIATE").map_err(db)?;
    let commit = (|| {
        store.insert_revision(&next)?;
        store.set_meta("current", &next["revision"])?;
        store.set_meta("legacyOriginal", legacy)?;
        store.set_meta("legacyReview", reviewed)?;
        store.receipt(&request, &payload, &result)?;
        Ok::<_, ToolError>(())
    })();
    if let Err(e) = commit {
        let _ = store.conn.execute_batch("ROLLBACK");
        return Err(e);
    }
    store.conn.execute_batch("COMMIT").map_err(db)?;
    Ok(result)
}
