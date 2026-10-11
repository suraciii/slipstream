use super::*;
pub(super) fn window(
    items: Vec<Value>,
    args: &Value,
    operation: &str,
    state: Option<&Path>,
    response: Value,
) -> Result<Value, ToolError> {
    crate::continuations::window(items, args, operation, state, response)
}
pub(super) fn is_read(op: &str) -> bool {
    matches!(
        op,
        "edit show"
            | "edit output show"
            | "edit migration show"
            | "history list"
            | "history show"
            | "modules list"
            | "modules describe"
            | "edit instances list"
            | "edit instances show"
            | "edit masks list"
            | "edit masks show"
            | "edit actions list"
            | "edit actions describe"
    )
}
pub(super) fn view(store: &Store, revision: &Value) -> Result<Value, ToolError> {
    let snapshot = &revision["snapshot"];
    let observed = evidence(Path::new(string(&revision["input"], "path")?)).ok();
    let mut input = revision["input"].clone();
    if let Some((facts, _)) = observed {
        input["observedRevision"] = facts["observedRevision"].clone();
        if facts["fingerprint"] != input["fingerprint"] {
            input["availability"] = json!({"read":true,"write":false,"execute":false,"reason":"input_changed","recovery":"explicit rebind"});
        }
    } else {
        input["availability"] = json!({"read":false,"write":false,"execute":false,"reason":"input_unavailable","recovery":"restore the acknowledged input file"});
    }
    let engine = Engine::open().ok();
    let compatible = engine.as_ref().is_some_and(|e| {
        e.bundle_id() == revision["bundleId"].as_str().unwrap_or("")
            && e.schema_version() == revision["schemaVersion"].as_str().unwrap_or("")
    });
    let capability = json!({"read":true,"write":compatible,"execute":compatible,"reason":if compatible {Value::Null}else{json!("engine_incompatible")},"recovery":if compatible{Value::Null}else{json!("install the retained qualified engine or review migration")}});
    let latest:Option<String>=store.conn.query_row("SELECT result FROM captures WHERE result IS NOT NULL AND json_extract(result,'$.kind')='preview' AND json_extract(result,'$.fileCommitted')=1 ORDER BY (revision = ?) DESC,rowid DESC LIMIT 1",[revision["revision"].as_str()],|r|r.get(0)).optional().map_err(db)?;
    let latest = latest
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .filter(|v| v["kind"] == "preview")
        .map(|mut v| {
            v["stale"] = json!(v["revision"] != revision["revision"]);
            v
        });
    let discovery = store.meta("discovery")?;
    Ok(
        json!({"stateDirectory":store.dir.path(),"inputPath":input["path"],"executionHost":host(),"input":input,"revision":revision["revision"],"baselineRevision":store.meta("baseline")?,"stateToken":store.token(revision)?,"engineId":"darktable","bundleId":revision["bundleId"],"schemaVersion":revision["schemaVersion"],"availability":capability,"initialization":{"state":"ready","reason":null},"instances":window(snapshot["instances"].as_array().cloned().unwrap_or_default(),&json!({"limit":60}),"edit instances list",Some(store.dir.path()),json!({}))?,"masks":window(snapshot["masks"].as_array().cloned().unwrap_or_default(),&json!({"limit":60}),"edit masks list",Some(store.dir.path()),json!({}))?,"actions":window(snapshot["actions"].as_array().cloned().unwrap_or_default(),&json!({"limit":60}),"edit actions list",Some(store.dir.path()),json!({}))?,"output":snapshot["output"],"frames":snapshot["frames"],"previewCapabilities":{"region":null,"statistics":discovery["previewStatistics"],"fileBytes":discovery["limits"]["previewBytes"]},"latestPreview":latest,"limits":discovery["limits"],"catalog":catalog::describe(Some(store.dir.path()), "local")?,"globalCatalog":discovery["globalCatalog"]}),
    )
}
pub(super) fn discovery(
    invocation: &Invocation,
    context: Option<(&Store, &Value)>,
) -> Result<Value, ToolError> {
    let engine = Engine::open().ok();
    let mut discovery = match &engine {
        Some(engine) => engine.discover()?,
        None => context
            .map(|(store, _)| store.meta("discovery"))
            .transpose()?
            .ok_or_else(|| err("engine_unavailable", "No qualified discovery is retained"))?,
    };
    let bundle = engine
        .as_ref()
        .map(|e| e.bundle_id().to_owned())
        .unwrap_or_else(|| {
            context.unwrap().1["bundleId"]
                .as_str()
                .unwrap_or("")
                .to_owned()
        });
    let schema = engine
        .as_ref()
        .map(|e| e.schema_version().to_owned())
        .unwrap_or_else(|| {
            context.unwrap().1["schemaVersion"]
                .as_str()
                .unwrap_or("")
                .to_owned()
        });
    if engine.is_none()
        && let Some(modules) = discovery["modules"].as_array_mut()
    {
        for module in modules {
            module["availability"] = json!({"read":true,"write":false,"execute":false,"reason":"engine_unavailable","recovery":"install the retained qualified engine"});
        }
    }
    if let Some((store, revision)) = context {
        store.guard(&invocation.arguments, revision, false)?;
        discovery["instances"] = revision["snapshot"]["instances"].clone();
        discovery["frames"] = revision["snapshot"]["frames"].clone();
        discovery["output"] = revision["snapshot"]["output"].clone();
    }
    let modules = discovery["modules"].as_array().cloned().unwrap_or_default();
    if invocation.operation == "modules describe" {
        let module = string(&invocation.arguments, "moduleId")?;
        return modules
            .into_iter()
            .find(|m| m["moduleId"] == module)
            .ok_or_else(|| err("module_not_found", "Module is not advertised"));
    }
    let search = invocation.arguments["search"]
        .as_str()
        .unwrap_or("")
        .to_lowercase();
    let selected = modules
        .into_iter()
        .filter(|m| search.is_empty() || m.to_string().to_lowercase().contains(&search))
        .collect();
    window(
        selected,
        &invocation.arguments,
        "modules list",
        invocation.state_directory.as_deref(),
        json!({"bundleId":bundle,"schemaVersion":schema}),
    )
}
pub(super) fn dispatch(store: &Store, inv: &Invocation) -> Result<Value, ToolError> {
    let rev = store.revision(None)?;
    store.guard(&inv.arguments, &rev, false)?;
    if inv.arguments["ifState"].is_string() {
        verify_input(&rev).map_err(|error| err("state_conflict", error.message))?;
    }
    if inv.operation.starts_with("modules ") {
        return discovery(inv, Some((store, &rev)));
    }
    if inv.operation == "edit show" {
        return view(store, &rev);
    }
    if inv.operation == "edit output show" {
        return Ok(rev["snapshot"]["output"].clone());
    }
    if inv.operation == "history show" {
        return store.revision(Some(string(&inv.arguments, "revision")?));
    }
    if inv.operation == "history list" {
        let mut stmt = store
            .conn
            .prepare("SELECT id FROM revisions ORDER BY rowid DESC")
            .map_err(db)?;
        let ids = stmt.query_map([], |r| r.get::<_, String>(0)).map_err(db)?;
        let mut items = Vec::new();
        for id in ids {
            let mut record = store.revision(Some(&id.map_err(db)?))?;
            record.as_object_mut().unwrap().remove("snapshot");
            record["availability"] =
                json!({"read":true,"write":false,"execute":true,"reason":null,"recovery":null});
            record["pinned"] = json!(true);
            items.push(record);
        }
        return window(
            items,
            &inv.arguments,
            "history list",
            Some(store.dir.path()),
            json!({}),
        );
    }
    if inv.operation == "edit migration show" {
        let target_bundle = string(&inv.arguments, "bundleId")?;
        let target_schema = string(&inv.arguments, "schemaVersion")?;
        return Ok(
            json!({"sourceRevision":rev["revision"],"sourceBundleId":rev["bundleId"],"sourceSchemaVersion":rev["schemaVersion"],"targetBundleId":target_bundle,"targetSchemaVersion":target_schema,"availability":{"read":true,"write":false,"execute":false,"reason":"No complete qualified converter is available for this source and target","recovery":"Install a qualified lossless semantic converter"},"changes":[],"identityMappings":[],"stateToken":store.token(&rev)?,"migrationToken":null}),
        );
    }
    let (collection, key) = if inv.operation.starts_with("edit instances ") {
        ("instances", "instanceId")
    } else if inv.operation.starts_with("edit masks ") {
        ("masks", "maskId")
    } else {
        ("actions", "actionId")
    };
    let items = rev["snapshot"][collection]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if inv.operation.ends_with(" list") {
        let filtered = items
            .into_iter()
            .filter(|v| {
                inv.arguments["instanceId"].is_null()
                    || v["instanceId"] == inv.arguments["instanceId"]
            })
            .collect();
        return window(
            filtered,
            &inv.arguments,
            &inv.operation,
            Some(store.dir.path()),
            json!({}),
        );
    }
    let target = string(&inv.arguments, key)?;
    let mut item = items
        .into_iter()
        .find(|v| {
            v[key] == target
                && (inv.arguments["instanceId"].is_null()
                    || v["instanceId"] == inv.arguments["instanceId"])
        })
        .ok_or_else(|| {
            err(
                &format!("{}_not_found", key.trim_end_matches("Id")),
                "Requested semantic identity does not exist",
            )
        })?;
    if collection == "masks" {
        let graph = store
            .meta("discovery")
            .ok()
            .map(|v| v["maskGraphSchema"].clone())
            .unwrap_or(Value::Null);
        item["schema"] = graph
            .pointer("/properties/masks/items/oneOf")
            .and_then(Value::as_array)
            .and_then(|branches| {
                branches
                    .iter()
                    .find(|v| v["properties"]["kind"]["const"] == item["kind"])
            })
            .map(|v| v["properties"]["value"].clone())
            .unwrap_or(Value::Null);
        item["graphSchema"] = graph;
    }
    Ok(item)
}
