use super::*;
use std::collections::{HashMap, HashSet};
fn keys(v: &Value, allowed: &[&str]) -> Result<(), ToolError> {
    let object = v
        .as_object()
        .ok_or_else(|| err("invalid_input", "Change must be an object"))?;
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(err("invalid_input", format!("Unknown change field {key}")));
        }
    }
    Ok(())
}
fn locate(snapshot: &Value, collection: &str, key: &str, target: &str) -> Result<usize, ToolError> {
    snapshot[collection]
        .as_array()
        .and_then(|a| a.iter().position(|v| v[key] == target))
        .ok_or_else(|| err("target_not_found", format!("Unknown {key} {target}")))
}
fn resolve(v: &mut Value, refs: &HashMap<String, String>) -> Result<(), ToolError> {
    match v {
        Value::String(s) if s.starts_with('@') => {
            *s = refs
                .get(&s[1..])
                .ok_or_else(|| {
                    err(
                        "invalid_reference",
                        "Request-local reference has not been defined",
                    )
                })?
                .clone();
        }
        Value::Array(a) => {
            for v in a {
                resolve(v, refs)?
            }
        }
        Value::Object(o) => {
            for v in o.values_mut() {
                resolve(v, refs)?
            }
        }
        _ => (),
    }
    Ok(())
}
fn new_ref(
    change: &Value,
    refs: &mut HashMap<String, String>,
    prefix: &str,
) -> Result<String, ToolError> {
    let reference = string(change, "ref")?;
    if reference.is_empty()
        || reference.len() > 64
        || !reference
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
        || refs.contains_key(reference)
    {
        return Err(err(
            "invalid_reference",
            "Request-local ref must be unique and valid",
        ));
    }
    let identity = id(prefix)?;
    refs.insert(reference.into(), identity.clone());
    Ok(identity)
}
fn focused(inv: &Invocation) -> Result<Vec<Value>, ToolError> {
    let a = &inv.arguments;
    let instance = a["instanceId"].clone();
    let change = match inv.operation.as_str() {
        "edit set" => {
            json!({"type":"set","instanceId":instance,"parameterId":a["parameterId"],"value":a["value"]})
        }
        "edit reset" => {
            if a["all"] == true {
                json!({"type":"reset","scope":"all"})
            } else if !a["parameterId"].is_null() {
                json!({"type":"reset","scope":"parameter","instanceId":instance,"parameterId":a["parameterId"]})
            } else {
                json!({"type":"reset","scope":"instance","instanceId":instance})
            }
        }
        "edit instances add" => {
            json!({"type":"instance-add","moduleId":a["moduleId"],"ref":"created","name":a["name"]})
        }
        "edit instances clone" => {
            json!({"type":"instance-clone","instanceId":instance,"ref":"created","name":a["name"]})
        }
        "edit instances enable" => {
            json!({"type":"instance-enable","instanceId":instance,"enabled":a["value"]})
        }
        "edit instances remove" => json!({"type":"instance-remove","instanceId":instance}),
        "edit instances reorder" => {
            keys(&a["input"], &["instanceIds"])?;
            json!({"type":"instance-reorder","instanceIds":a["input"]["instanceIds"]})
        }
        "edit masks create" => {
            keys(&a["input"], &["kind", "frameId", "value"])?;
            json!({"type":"mask-create","ref":"created","kind":a["input"]["kind"],"frameId":a["input"]["frameId"],"value":a["input"]["value"]})
        }
        "edit masks set" => json!({"type":"mask-set","maskId":a["maskId"],"value":a["value"]}),
        "edit masks remove" => json!({"type":"mask-remove","maskId":a["maskId"]}),
        "edit actions run" => {
            json!({"type":"action","actionId":a["actionId"],"instanceId":instance,"input":a["input"]})
        }
        "edit output set" => json!({"type":"output-set","fieldId":a["fieldId"],"value":a["value"]}),
        "presets apply" => {
            json!({"type":"preset-apply","presetId":a["presetId"],"version":a["version"],"instanceId":instance,"mode":a["mode"],"initialization":a["initialization"],"maskPolicy":a["maskPolicy"]})
        }
        "styles apply" => {
            let mut c = a["input"].clone();
            c["type"] = json!("style-apply");
            c["styleId"] = a["styleId"].clone();
            c["version"] = a["version"].clone();
            c
        }
        "edit apply" | "edit validate" => {
            keys(&a["input"], &["changes"])?;
            return a["input"]["changes"]
                .as_array()
                .cloned()
                .ok_or_else(|| err("invalid_input", "changes must be an array"));
        }
        _ => return Err(err("invalid_command", "Unknown editing command")),
    };
    Ok(vec![change])
}
struct ApplyContext<'a> {
    store: &'a Store,
    engine: &'a Engine,
    input: &'a Path,
    scope: &'a str,
    validate: bool,
}

fn apply_one(
    context: ApplyContext<'_>,
    snapshot: &mut Value,
    change: &Value,
    refs: &mut HashMap<String, String>,
    actions: &mut Vec<Value>,
    mappings: &mut Vec<Value>,
) -> Result<(), ToolError> {
    let ApplyContext {
        store,
        engine,
        input,
        scope,
        validate,
    } = context;
    let mut c = change.clone();
    resolve(&mut c, refs)?;
    let kind = string(&c, "type")?;
    match kind {
        "set" => {
            keys(&c, &["type", "instanceId", "parameterId", "value"])?;
            let i = locate(
                snapshot,
                "instances",
                "instanceId",
                string(&c, "instanceId")?,
            )?;
            let p = string(&c, "parameterId")?;
            if snapshot["instances"][i]["parameters"].get(p).is_none() {
                return Err(err("parameter_not_found", "Parameter is not exposed"));
            }
            snapshot["instances"][i]["parameters"][p] = c["value"].clone();
        }
        "reset" => {
            let scope = string(&c, "scope")?;
            match scope {
                "all" => {
                    keys(&c, &["type", "scope"])?;
                    *snapshot = store
                        .revision(Some(store.meta("baseline")?.as_str().unwrap_or("")))?["snapshot"]
                        .clone();
                }
                "instance" | "parameter" => {
                    keys(
                        &c,
                        if scope == "instance" {
                            &["type", "scope", "instanceId"]
                        } else {
                            &["type", "scope", "instanceId", "parameterId"]
                        },
                    )?;
                    let i = locate(
                        snapshot,
                        "instances",
                        "instanceId",
                        string(&c, "instanceId")?,
                    )?;
                    if scope == "parameter" {
                        let p = string(&c, "parameterId")?;
                        let default = snapshot["instances"][i]["initialized"]
                            .get(p)
                            .cloned()
                            .ok_or_else(|| {
                                err(
                                    "initialization_unavailable",
                                    "Actual reset value is unknown",
                                )
                            })?;
                        snapshot["instances"][i]["parameters"][p] = default;
                    } else {
                        *snapshot = engine.construction_action(
                            input,
                            snapshot,
                            &json!({"actionId":"instance-reset","instanceId":c["instanceId"]}),
                        )?;
                    }
                }
                _ => return Err(err("invalid_input", "Unknown reset scope")),
            }
        }
        "instance-add" => {
            keys(&c, &["type", "moduleId", "ref", "name"])?;
            let identity = new_ref(&c, refs, "instance")?;
            *snapshot=engine.construction_action(input,snapshot,&json!({"actionId":"instance-create","moduleId":c["moduleId"],"newInstanceId":identity,"name":c["name"]}))?;
        }
        "instance-clone" => {
            keys(&c, &["type", "instanceId", "ref", "name"])?;
            let identity = new_ref(&c, refs, "instance")?;
            *snapshot = engine.construction_action(
                input,
                snapshot,
                &json!({"actionId":"instance-clone","instanceId":c["instanceId"],"newInstanceId":identity,"name":c["name"]}),
            )?;
        }
        "instance-enable" => {
            keys(&c, &["type", "instanceId", "enabled"])?;
            if !c["enabled"].is_boolean() {
                return Err(err("invalid_input", "enabled must be boolean"));
            }
            let i = locate(
                snapshot,
                "instances",
                "instanceId",
                string(&c, "instanceId")?,
            )?;
            snapshot["instances"][i]["enabled"] = c["enabled"].clone();
        }
        "instance-remove" => {
            keys(&c, &["type", "instanceId"])?;
            remove_instance(snapshot, string(&c, "instanceId")?)?;
        }
        "instance-reorder" => {
            keys(&c, &["type", "instanceIds"])?;
            let ids = c["instanceIds"]
                .as_array()
                .ok_or_else(|| err("invalid_input", "instanceIds must be an array"))?;
            if ids.len() != snapshot["instances"].as_array().unwrap().len() {
                return Err(err(
                    "invalid_order",
                    "Reorder must contain every instance once",
                ));
            }
            let mut used = HashSet::new();
            let mut reordered = Vec::new();
            for (order, target) in ids.iter().enumerate() {
                let target = target
                    .as_str()
                    .ok_or_else(|| err("invalid_input", "Instance ID must be string"))?;
                if !used.insert(target) {
                    return Err(err("invalid_order", "Duplicate instance"));
                }
                let i = locate(snapshot, "instances", "instanceId", target)?;
                let mut item = snapshot["instances"][i].clone();
                item["order"] = json!(order);
                reordered.push(item);
            }
            snapshot["instances"] = json!(reordered);
        }
        "mask-create" => {
            keys(&c, &["type", "ref", "kind", "frameId", "value"])?;
            if !c["value"].is_object() {
                return Err(err("invalid_input", "mask value must be an object"));
            }
            let mask = json!({"maskId":new_ref(&c,refs,"mask")?,"kind":string(&c,"kind")?,"frameId":string(&c,"frameId")?,"value":c["value"],"referencedBy":[]});
            snapshot["masks"]
                .as_array_mut()
                .ok_or_else(|| err("invalid_state", "Missing masks"))?
                .push(mask);
        }
        "mask-set" => {
            keys(&c, &["type", "maskId", "value"])?;
            if !c["value"].is_object() {
                return Err(err("invalid_input", "mask value must be object"));
            }
            let i = locate(snapshot, "masks", "maskId", string(&c, "maskId")?)?;
            snapshot["masks"][i]["value"] = c["value"].clone();
        }
        "mask-remove" => {
            keys(&c, &["type", "maskId"])?;
            let i = locate(snapshot, "masks", "maskId", string(&c, "maskId")?)?;
            snapshot["masks"].as_array_mut().unwrap().remove(i);
        }
        "blend-set" => {
            keys(&c, &["type", "instanceId", "value"])?;
            if !c["value"].is_object() {
                return Err(err("invalid_input", "blend must be object"));
            }
            let i = locate(
                snapshot,
                "instances",
                "instanceId",
                string(&c, "instanceId")?,
            )?;
            assign_blend(&mut snapshot["instances"][i], &c["value"]);
        }
        "output-set" => {
            keys(&c, &["type", "fieldId", "value"])?;
            let field = string(&c, "fieldId")?;
            snapshot["output"]["settings"][field] = c["value"].clone();
        }
        "action" => {
            keys(&c, &["type", "actionId", "instanceId", "input"])?;
            if validate {
                actions.push(c);
            } else {
                *snapshot = engine.construction_action(input, snapshot, &c)?;
            }
        }
        "preset-apply" | "style-apply" => {
            if validate {
                let result =
                    catalog::validate_apply(store.dir.path(), snapshot, &c, scope, engine, input)?;
                *snapshot = result["snapshot"].clone();
                mappings.push(result);
            } else {
                let result = catalog::apply(store.dir.path(), snapshot, &c, scope, engine, input)?;
                *snapshot = result["snapshot"].clone();
                mappings.push(result);
            }
        }
        _ => return Err(err("invalid_input", format!("Unknown change type {kind}"))),
    }
    if snapshot["instances"]
        .as_array()
        .is_none_or(|a| a.len() > 256)
        || snapshot["masks"].as_array().is_none_or(|a| a.len() > 256)
    {
        return Err(err("limit_exceeded", "Instance/mask bound exceeded"));
    }
    Ok(())
}
pub(super) fn dispatch(
    store: &Store,
    inv: &Invocation,
    revision: &Value,
) -> Result<Value, ToolError> {
    let validate = inv.operation == "edit validate";
    let request = inv.arguments["requestId"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or(id("request")?);
    identifier(&request)?;
    let payload = digest(&json!({"operation":inv.operation,"arguments":inv.arguments}))?;
    if !validate {
        if let Some(replay) = store.replay(&request, &payload)? {
            return Ok(replay);
        }
        store.reject_capture_request(&request)?;
    }
    let required = matches!(
        inv.operation.as_str(),
        "edit apply"
            | "edit rebind"
            | "history restore"
            | "edit instances reorder"
            | "presets apply"
            | "styles apply"
            | "edit migrate"
    );
    store.guard(&inv.arguments, revision, required)?;
    if inv.operation == "edit migrate" {
        return Err(err(
            "migration_unavailable",
            "No complete qualified converter exists for this source/target",
        ));
    }
    let engine = Engine::open()?;
    if engine.bundle_id() != revision["bundleId"].as_str().unwrap_or("")
        || engine.schema_version() != revision["schemaVersion"].as_str().unwrap_or("")
    {
        return Err(err(
            "schema_unsupported",
            "State requires an explicit reviewed migration",
        ));
    }
    let engine = engine.with_resources(&revision["dependencies"]);
    let mut input = revision["input"].clone();
    let _lease = if inv.operation == "edit rebind" {
        let document = &inv.arguments["input"];
        let path = document["inputPath"]
            .as_str()
            .or_else(|| document["input"]["path"].as_str())
            .ok_or_else(|| err("invalid_input", "Rebind requires inputPath"))?;
        let (e, l) = evidence(Path::new(path))?;
        input = e;
        l
    } else {
        verify_input(revision)?
    };
    let path = Path::new(string(&input, "path")?);
    let mut snapshot = revision["snapshot"].clone();
    let mut refs = HashMap::new();
    let mut actions = Vec::new();
    let mut mappings = Vec::new();
    let scope = inv.arguments["catalog"].as_str().unwrap_or("local");
    if inv.operation == "history restore" {
        let historical = store.revision(Some(string(&inv.arguments, "revision")?))?;
        snapshot = historical["snapshot"].clone();
        if snapshot["frames"] != revision["snapshot"]["frames"]
            && !snapshot["masks"].as_array().is_none_or(|m| m.is_empty())
        {
            return Err(err(
                "frame_incompatible",
                "Historical masks use different frames",
            ));
        }
        snapshot["frames"] = revision["snapshot"]["frames"].clone();
    } else if inv.operation == "edit rebind" {
        let baseline = engine.initialize(path)?;
        if snapshot["frames"] != baseline["frames"]
            && !snapshot["masks"].as_array().is_none_or(|m| m.is_empty())
        {
            return Err(err(
                "frame_incompatible",
                "Rebind cannot silently transform masks",
            ));
        }
        snapshot["frames"] = baseline["frames"].clone();
    } else {
        let changes = focused(inv)?;
        if changes.is_empty() || changes.len() > 100 {
            return Err(err("limit_exceeded", "changes must contain 1–100 entries"));
        }
        for change in changes {
            let prepares_native = matches!(
                change["type"].as_str(),
                Some(
                    "instance-add"
                        | "instance-clone"
                        | "reset"
                        | "action"
                        | "preset-apply"
                        | "style-apply"
                )
            );
            let candidate_engine = if prepares_native {
                catalog::validate_new_dependencies(
                    store.dir.path(),
                    &revision["snapshot"],
                    &snapshot,
                    scope,
                )?;
                let resources = catalog::validate_dependencies(store.dir.path(), &snapshot, scope)?;
                catalog::validate_kind_refs(&snapshot, &resources, &engine.discover()?)?;
                engine.with_resources(&resources)
            } else {
                engine.with_resources(&revision["dependencies"])
            };
            apply_one(
                ApplyContext {
                    store,
                    engine: &candidate_engine,
                    input: path,
                    scope,
                    validate,
                },
                &mut snapshot,
                &change,
                &mut refs,
                &mut actions,
                &mut mappings,
            )?;
        }
    }
    if !matches!(inv.operation.as_str(), "history restore" | "edit rebind") {
        catalog::validate_new_dependencies(
            store.dir.path(),
            &revision["snapshot"],
            &snapshot,
            scope,
        )?;
    }
    let dependencies = catalog::validate_dependencies(store.dir.path(), &snapshot, scope)?;
    catalog::validate_kind_refs(&snapshot, &dependencies, &engine.discover()?)?;
    snapshot = engine
        .with_resources(&dependencies)
        .validate(path, &snapshot)?;
    let dependencies = if validate {
        dependencies
    } else {
        catalog::pin_dependencies(store.dir.path(), &snapshot, scope)?
    };
    let targets = changed_targets(&revision["snapshot"], &snapshot);
    let effects = json!({"snapshot":snapshot,"catalogMappings":mappings});
    if validate {
        return Ok(
            json!({"valid":true,"effects":effects,"changedTargets":targets,"catalogMappings":mappings,"actionsRequiringExecution":actions,"generatedIds":refs}),
        );
    }
    let unchanged = snapshot == revision["snapshot"] && input == revision["input"];
    let next = if unchanged {
        revision.clone()
    } else {
        json!({"revision":id("revision")?,"parent":revision["revision"],"createdAt":timestamp(),"requestId":request,"snapshot":snapshot,"input":input,"bundleId":engine.bundle_id(),"schemaVersion":engine.schema_version(),"dependencies":dependencies,"changedTargets":targets})
    };
    let rebound_baseline = if inv.operation == "edit rebind" {
        let mut baseline = engine.initialize(path)?;
        remap_baseline(&mut baseline)?;
        Some(baseline)
    } else {
        None
    };
    let token = store.token(&next)?;
    let result = json!({"stateDirectory":store.dir.path(),"executionHost":host(),"revision":next["revision"],"stateToken":token,"requestId":request,"outcome":if unchanged{"unchanged"}else{"saved"},"retainUntil":deadline(),"changedTargets":targets,"effects":effects,"generatedIds":refs,"catalogMappings":mappings,"current":{"revision":next["revision"],"stateToken":token}});
    store.conn.execute_batch("BEGIN IMMEDIATE").map_err(db)?;
    let settlement = (|| {
        if !unchanged {
            store.insert_revision(&next)?;
            store.set_meta("current", &next["revision"])?;
            if let Some(baseline) = &rebound_baseline {
                let baseline_rev = json!({"revision":id("revision")?,"parent":next["revision"],"createdAt":timestamp(),"requestId":null,"snapshot":baseline,"input":input,"bundleId":engine.bundle_id(),"schemaVersion":engine.schema_version(),"dependencies":[],"changedTargets":[]});
                store.insert_revision(&baseline_rev)?;
                store.set_meta("baseline", &baseline_rev["revision"])?;
            }
            prune_history(store)?;
        }
        store.receipt(&request, &payload, &result)?;
        Ok::<_, ToolError>(())
    })();
    if let Err(e) = settlement {
        let _ = store.conn.execute_batch("ROLLBACK");
        return Err(e);
    }
    store.conn.execute_batch("COMMIT").map_err(db)?;
    Ok(result)
}
fn changed_targets(before: &Value, after: &Value) -> Vec<Value> {
    let mut targets = Vec::new();
    for collection in ["instances", "masks"] {
        let key = if collection == "instances" {
            "instanceId"
        } else {
            "maskId"
        };
        for item in after[collection].as_array().into_iter().flatten() {
            if before[collection]
                .as_array()
                .is_none_or(|a| !a.contains(item))
            {
                targets.push(json!({"kind":collection,"id":item[key]}));
            }
        }
        for item in before[collection].as_array().into_iter().flatten() {
            if after[collection]
                .as_array()
                .is_none_or(|a| !a.iter().any(|i| i[key] == item[key]))
            {
                targets.push(json!({"kind":collection,"id":item[key],"removed":true}));
            }
        }
    }
    if before["output"] != after["output"] {
        targets.push(json!({"kind":"output"}));
    }
    targets
}
fn remove_instance(snapshot: &mut Value, target: &str) -> Result<(), ToolError> {
    let index = locate(snapshot, "instances", "instanceId", target)?;
    let instances = snapshot["instances"].as_array_mut().unwrap();
    instances.remove(index);
    for (order, instance) in instances.iter_mut().enumerate() {
        instance["order"] = json!(order);
    }
    Ok(())
}
fn assign_blend(instance: &mut Value, blend: &Value) {
    instance["blend"] = blend.clone();
    instance["maskIds"] = json!(
        blend["maskId"]
            .as_str()
            .map(|id| vec![id])
            .unwrap_or_default()
    );
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removing_middle_instance_keeps_survivor_identity_and_dense_order() {
        let mut snapshot = json!({"instances":[{"instanceId":"a","order":0,"parameters":{"exposure":1}},{"instanceId":"b","order":1},{"instanceId":"c","order":2,"parameters":{"exposure":3}}]});
        remove_instance(&mut snapshot, "b").unwrap();
        assert_eq!(
            snapshot["instances"],
            json!([{"instanceId":"a","order":0,"parameters":{"exposure":1}},{"instanceId":"c","order":1,"parameters":{"exposure":3}}])
        );
        let retained = snapshot.clone();
        assert_eq!(
            remove_instance(&mut snapshot, "missing").unwrap_err().code,
            "target_not_found"
        );
        assert_eq!(snapshot, retained);
    }
    #[test]
    fn assigning_blend_updates_and_clears_drawn_root_relationship() {
        let mut instance = json!({"instanceId":"instance-a","blend":{},"maskIds":[]});
        assign_blend(&mut instance, &json!({"maskId":"mask-a","opacity":0.8}));
        assert_eq!(instance["maskIds"], json!(["mask-a"]));
        assert_eq!(instance["blend"]["opacity"], 0.8);
        assign_blend(&mut instance, &json!({"maskId":null}));
        assert_eq!(instance["maskIds"], json!([]));
    }
}
