use super::*;
use std::collections::{BTreeMap, BTreeSet};
pub(super) fn apply(
    state: &Path,
    snapshot: &Value,
    change: &Value,
    scope: &str,
    engine: &Engine,
    input: &Path,
) -> Result<Value, ToolError> {
    plan(state, snapshot, change, scope, engine, input, false)
}
pub(super) fn validate_apply(
    state: &Path,
    snapshot: &Value,
    change: &Value,
    scope: &str,
    engine: &Engine,
    input: &Path,
) -> Result<Value, ToolError> {
    plan(state, snapshot, change, scope, engine, input, true)
}
fn plan(
    state: &Path,
    snapshot: &Value,
    change: &Value,
    scope: &str,
    engine: &Engine,
    input: &Path,
    planning: bool,
) -> Result<Value, ToolError> {
    let kind = match text(change, "type")? {
        "preset-apply" => "preset",
        "style-apply" => "style",
        _ => {
            return Err(error(
                "invalid_request",
                "catalog apply requires preset/style change",
            ));
        }
    };
    keys(
        change,
        if kind == "preset" {
            &[
                "type",
                "presetId",
                "version",
                "instanceId",
                "mode",
                "initialization",
                "maskPolicy",
            ]
        } else {
            &[
                "type",
                "styleId",
                "version",
                "mode",
                "mapping",
                "initialization",
                "maskPolicy",
                "includeOutput",
            ]
        },
    )?;
    let id = text(
        change,
        if kind == "preset" {
            "presetId"
        } else {
            "styleId"
        },
    )?;
    let catalog = Catalog::open(Some(state), scope, false)?;
    let record = catalog.record(id, text(change, "version")?, kind)?;
    let discovery = engine.discover()?;
    definitions::validate(&record, kind, &discovery, engine.bundle_id())?;
    let mode = text(change, "mode")?;
    if !if kind == "preset" {
        ["update", "replace"].contains(&mode)
    } else {
        ["merge", "replace"].contains(&mode)
    } {
        return Err(error("invalid_request", "invalid application mode"));
    }
    let init = text(change, "initialization")?;
    if !["literal", "target-camera"].contains(&init) {
        return Err(error("invalid_request", "invalid initialization mode"));
    }
    let mask_policy = text(change, "maskPolicy")?;
    if !["omit", "compatible"].contains(&mask_policy) {
        return Err(error("invalid_request", "invalid mask policy"));
    }
    if init != text(&record, "initialization")? {
        return Err(error(
            "incompatible_definition",
            "application initialization differs from captured representation",
        ));
    }
    let settings = if kind == "preset" {
        vec![record["settings"].clone()]
    } else {
        array(&record, "entries")?.clone()
    };
    let mapping = if kind == "preset" {
        vec![json!({"entryId":"self","instanceId":text(change,"instanceId")?})]
    } else {
        array(change, "mapping")?.clone()
    };
    let mut targets = BTreeMap::<String, Option<String>>::new();
    let mut used = BTreeSet::new();
    for m in mapping {
        keys(&m, &["entryId", "instanceId"])?;
        let entry = text(&m, "entryId")?;
        let target = if m.get("instanceId").is_some_and(Value::is_null) {
            None
        } else {
            Some(text(&m, "instanceId")?.to_owned())
        };
        if targets.insert(entry.to_owned(), target.clone()).is_some() {
            return Err(error("invalid_mapping", "duplicate captured entry mapping"));
        }
        if let Some(t) = target
            && !used.insert(t)
        {
            return Err(error(
                "invalid_mapping",
                "destination mapped more than once",
            ));
        }
    }
    let entries: BTreeSet<_> = settings
        .iter()
        .map(|s| {
            if kind == "preset" {
                Ok("self".to_owned())
            } else {
                text(s, "entryId").map(str::to_owned)
            }
        })
        .collect::<Result<_, _>>()?;
    if entries != targets.keys().cloned().collect() {
        return Err(error(
            "invalid_mapping",
            "mapping must cover every entry exactly once",
        ));
    }
    let original = array(snapshot, "instances")?;
    for s in &settings {
        let entry = if kind == "preset" {
            "self"
        } else {
            text(s, "entryId")?
        };
        if let Some(id) = targets[entry].as_deref() {
            let target = original
                .iter()
                .find(|i| i["instanceId"] == id)
                .ok_or_else(|| error("invalid_mapping", "mapped target instance missing"))?;
            if target["moduleId"] != s["moduleId"] {
                return Err(error("invalid_mapping", "mapped operation differs"));
            }
        }
    }
    let (reset, preserved) = reset_scope(original, &targets, mode);
    let existing_resources = super::validate_dependencies(state, snapshot, scope)?;
    let bound_engine = engine.with_resources(&existing_resources);
    let engine = &bound_engine;
    let mut candidate = snapshot.clone();
    let mut instance_map = BTreeMap::new();
    let mut created = Vec::new();
    let mut previous_target: Option<String> = None;
    for (setting_index, s) in settings.iter().enumerate() {
        let entry = if kind == "preset" {
            "self"
        } else {
            text(s, "entryId")?
        };
        let module = text(s, "moduleId")?;
        let target = if let Some(id) = targets[entry].clone() {
            if mode == "replace" {
                let old = array(&candidate, "instances")?
                    .iter()
                    .find(|i| i["instanceId"] == id)
                    .unwrap()
                    .clone();
                candidate = engine.construction_action(
                    input,
                    &candidate,
                    &json!({"actionId":"instance-reset","instanceId":id}),
                )?;
                let target = candidate["instances"]
                    .as_array_mut()
                    .ok_or_else(|| error("native_failure", "reset omitted instances"))?
                    .iter_mut()
                    .find(|i| i["instanceId"] == id)
                    .ok_or_else(|| error("native_failure", "reset changed target identity"))?;
                target["order"] = old["order"].clone();
                if mask_policy == "omit" {
                    preserve_relationships(target, &old);
                }
            }
            id
        } else {
            let id = if planning {
                format!("planned-{entry}")
            } else {
                photo_files::identity("instance").map_err(io)?
            };
            let mut action = json!({"actionId":"instance-create","moduleId":module,"newInstanceId":id,"name":null});
            if let Some(previous) = &previous_target {
                action["after"] = json!(previous);
            }
            if let Some(next) = settings[setting_index + 1..].iter().find_map(|next| {
                next["entryId"]
                    .as_str()
                    .and_then(|entry| targets[entry].as_ref())
            }) {
                action["before"] = json!(next);
            }
            candidate = engine.construction_action(input, &candidate, &action)?;
            let target = array(&candidate, "instances")?
                .iter()
                .find(|i| i["instanceId"] == id)
                .ok_or_else(|| {
                    error(
                        "native_failure",
                        "native creation omitted requested identity",
                    )
                })?;
            created.push(json!({"entryId":entry,"instanceId":if planning{Value::Null}else{json!(id)},"moduleId":module,"order":target["order"]}));
            id
        };
        previous_target = Some(target.clone());
        instance_map.insert(entry.into(), target);
    }
    let mut initialized = BTreeMap::new();
    for s in &settings {
        if array(s, "parameters")?
            .iter()
            .any(|p| p["value"].get("mode") == Some(&json!("target-camera")))
        {
            if init != "target-camera" {
                return Err(error(
                    "unsupported_initialization",
                    "literal representation contains initialization directive",
                ));
            }
            let entry = if kind == "preset" {
                "self"
            } else {
                text(s, "entryId")?
            };
            let id = &instance_map[entry];
            let reset = engine.construction_action(
                input,
                &candidate,
                &json!({"actionId":"instance-reset","instanceId":id}),
            )?;
            initialized.insert(id.clone(), reset);
        }
    }
    // Validate coordinates before changing any relationships. The native validator
    // then checks source-stage evidence and complete relationships atomically.
    let mut mask_map = BTreeMap::new();
    if mask_policy == "compatible" && !record["masks"].is_null() {
        let graph = &record["masks"]["value"];
        if graph["frames"] != snapshot["frames"] {
            return Err(error(
                "incompatible_geometry",
                "captured mask coordinate frames differ from bound target",
            ));
        }
        for m in array(graph, "masks")? {
            let source = text(m, "maskId")?;
            mask_map.insert(
                source.into(),
                if planning {
                    format!("planned-{source}")
                } else {
                    photo_files::identity("mask").map_err(io)?
                },
            );
        }
        for m in array(graph, "masks")? {
            let mut m = m.clone();
            definitions::rewrite(&mut m, &mask_map, &instance_map);
            candidate["masks"]
                .as_array_mut()
                .ok_or_else(|| error("native_failure", "target masks unavailable"))?
                .push(m);
        }
    }
    let mut changed = Vec::new();
    for s in &settings {
        let entry = if kind == "preset" {
            "self"
        } else {
            text(s, "entryId")?
        };
        let id = &instance_map[entry];
        let target = candidate["instances"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|i| i["instanceId"] == *id)
            .unwrap();
        let mut parameter_ids = Vec::new();
        for p in array(s, "parameters")? {
            let pid = text(p, "parameterId")?;
            let mut value = if p["value"].get("mode") == Some(&json!("target-camera")) {
                initialized_parameter(&initialized[id], id, pid)?
            } else {
                p["value"].clone()
            };
            if mask_policy == "omit" {
                let mut refs = Vec::new();
                definitions::mask_refs(&value, &mut refs);
                if !refs.is_empty() {
                    continue;
                }
            }
            definitions::rewrite(&mut value, &mask_map, &instance_map);
            target["parameters"]
                .as_object_mut()
                .ok_or_else(|| error("native_failure", "target parameters unavailable"))?
                .insert(pid.into(), value);
            parameter_ids.push(pid.to_owned());
        }
        if !s["blend"].is_null() {
            let mut blend = s["blend"].clone();
            if mask_policy == "omit" {
                merge_omitted(&mut target["blend"], &mut blend);
            } else {
                definitions::rewrite(&mut blend, &mask_map, &instance_map);
                target["blend"] = blend;
                let mut ids = Vec::new();
                definitions::mask_refs(&target["blend"], &mut ids);
                target["maskIds"] = json!(ids);
            }
        }
        changed.push(
            json!({"instanceId":id,"parameterIds":parameter_ids,"blend":!s["blend"].is_null()}),
        );
    }
    let include_output = if kind == "style" {
        change["includeOutput"]
            .as_bool()
            .ok_or_else(|| error("invalid_request", "includeOutput must be boolean"))?
    } else {
        false
    };
    let output_changed = include_output && !record["output"].is_null();
    if output_changed {
        let mut output = record["output"].clone();
        definitions::rewrite(&mut output, &mask_map, &instance_map);
        candidate["output"]["settings"] = output;
    }
    if kind == "style" {
        validate_captured_order(&candidate, &settings, &instance_map)?;
    }
    // Native validation runs once on the complete atomic group in state dispatch.
    // A later change may repair this intermediate candidate's output combination.
    super::validate_new_dependencies(state, snapshot, &candidate, scope)?;
    let resources = super::validate_dependencies(state, &candidate, scope)?;
    super::validate_kind_refs(&candidate, &resources, &discovery)?;
    let instances:Vec<_>=instance_map.iter().map(|(entry,id)|json!({"entryId":entry,"instanceId":if planning&&targets[entry].is_none(){Value::Null}else{json!(id)}})).collect();
    let masks:Vec<_>=mask_map.iter().map(|(source,id)|json!({"maskId":source,"targetMaskId":if planning{Value::Null}else{json!(id)}})).collect();
    if planning {
        for target in &mut changed {
            if let Some(id) = target["instanceId"].as_str()
                && instance_map
                    .iter()
                    .any(|(entry, mapped)| mapped == id && targets[entry].is_none())
            {
                target["instanceId"] = Value::Null;
            }
        }
    }
    Ok(
        json!({"snapshot":candidate,"catalog":catalog.descriptor,"catalogMapping":{"catalog":catalog.descriptor,"id":id,"version":record["version"],"instances":instances,"masks":masks},"effects":{"resetInstanceIds":reset,"preservedInstanceIds":preserved,"createdInstances":created,"changedTargets":changed,"relationships":{"policy":mask_policy,"clonedMasks":mask_map.len(),"preservedExistingMasks":true},"output":{"changed":output_changed}}}),
    )
}
fn validate_captured_order(
    candidate: &Value,
    settings: &[Value],
    instance_map: &BTreeMap<String, String>,
) -> Result<(), ToolError> {
    let instances = array(candidate, "instances")?;
    let mut previous_order = None;
    for setting in settings {
        let id = &instance_map[text(setting, "entryId")?];
        let order = instances
            .iter()
            .find(|instance| instance["instanceId"] == *id)
            .and_then(|instance| instance["order"].as_f64())
            .ok_or_else(|| error("native_failure", "native target ordering unavailable"))?;
        if previous_order.is_some_and(|previous| previous >= order) {
            return Err(error(
                "invalid_mapping",
                "mapped target positions cannot preserve captured relative ordering",
            ));
        }
        previous_order = Some(order);
    }
    Ok(())
}
fn preserve_relationships(target: &mut Value, old: &Value) {
    target["maskIds"] = old["maskIds"].clone();
    restore_mask_fields(&mut target["blend"], &old["blend"]);
    if let (Some(t), Some(o)) = (
        target["parameters"].as_object_mut(),
        old["parameters"].as_object(),
    ) {
        for (k, v) in o {
            let mut masks = Vec::new();
            definitions::mask_refs(v, &mut masks);
            if !masks.is_empty() {
                t.insert(k.clone(), v.clone());
            }
        }
    }
}
fn restore_mask_fields(target: &mut Value, old: &Value) {
    if let (Some(t), Some(o)) = (target.as_object_mut(), old.as_object()) {
        for (k, v) in o {
            if definitions::MASK_FIELDS.contains(&k.as_str()) {
                t.insert(k.clone(), v.clone());
            } else if let Some(target) = t.get_mut(k) {
                restore_mask_fields(target, v);
            }
        }
    }
}
fn merge_omitted(target: &mut Value, source: &mut Value) {
    definitions::omit(source);
    if let (Some(t), Some(s)) = (target.as_object_mut(), source.as_object()) {
        for (k, v) in s {
            if v.is_object() {
                merge_omitted(t.entry(k.clone()).or_insert(json!({})), &mut v.clone());
            } else {
                t.insert(k.clone(), v.clone());
            }
        }
    }
}
fn initialized_parameter(
    reset: &Value,
    instance_id: &str,
    parameter_id: &str,
) -> Result<Value, ToolError> {
    let source = array(reset, "instances")?
        .iter()
        .find(|i| i["instanceId"] == instance_id)
        .ok_or_else(|| {
            error(
                "unsupported_initialization",
                "native reset omitted target occurrence",
            )
        })?;
    source["parameters"]
        .get(parameter_id)
        .cloned()
        .ok_or_else(|| {
            error(
                "unsupported_initialization",
                "target occurrence initialization unavailable",
            )
        })
}
#[cfg(test)]
mod initialization_tests {
    use super::*;
    #[test]
    fn captured_order_keeps_new_entries_between_mapped_neighbors() {
        let settings = vec![
            json!({"entryId":"first"}),
            json!({"entryId":"new"}),
            json!({"entryId":"last"}),
        ];
        let mapping = BTreeMap::from([
            ("first".into(), "mapped-first".into()),
            ("new".into(), "created".into()),
            ("last".into(), "mapped-last".into()),
        ]);
        let mut candidate = json!({"instances":[
            {"instanceId":"mapped-first","order":2},
            {"instanceId":"created","order":3},
            {"instanceId":"mapped-last","order":4}
        ]});
        validate_captured_order(&candidate, &settings, &mapping).unwrap();
        candidate["instances"][1]["order"] = json!(1);
        assert_eq!(
            validate_captured_order(&candidate, &settings, &mapping)
                .unwrap_err()
                .code,
            "invalid_mapping"
        );
        candidate["instances"][1]["order"] = json!(5);
        assert!(validate_captured_order(&candidate, &settings, &mapping).is_err());
        candidate["instances"][0]["order"] = json!(6);
        assert!(validate_captured_order(&candidate, &settings, &mapping).is_err());
    }
    #[test]
    fn initializes_nonbaseline_operation_by_exact_occurrence() {
        let reset = json!({"instances":[{"instanceId":"baseline-temperature","moduleId":"temperature","parameters":{"coeffs":[1,1,1]}},{"instanceId":"other-channelmixer","moduleId":"channelmixerrgb","parameters":{"temperature":4500}},{"instanceId":"target-channelmixer","moduleId":"channelmixerrgb","parameters":{"temperature":6500,"adaptation":1}}]});
        assert_eq!(
            initialized_parameter(&reset, "target-channelmixer", "temperature").unwrap(),
            6500
        );
        assert_eq!(
            initialized_parameter(&reset, "target-channelmixer", "adaptation").unwrap(),
            1
        );
        assert!(initialized_parameter(&reset, "missing", "temperature").is_err());
        assert!(initialized_parameter(&reset, "target-channelmixer", "missing").is_err());
    }
}
fn reset_scope(
    original: &[Value],
    targets: &BTreeMap<String, Option<String>>,
    mode: &str,
) -> (Vec<String>, Vec<String>) {
    let reset = if mode == "replace" {
        targets
            .values()
            .filter_map(Clone::clone)
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let preserved = original
        .iter()
        .filter_map(|i| i["instanceId"].as_str())
        .filter(|id| !reset.iter().any(|r| r == id))
        .map(str::to_owned)
        .collect();
    (reset, preserved)
}
#[cfg(test)]
mod scope_tests {
    use super::*;
    #[test]
    fn replace_resets_only_a_and_c_preserves_b_and_d() {
        let original = [
            json!({"instanceId":"A"}),
            json!({"instanceId":"B"}),
            json!({"instanceId":"C"}),
            json!({"instanceId":"D"}),
        ];
        let targets = BTreeMap::from([
            ("entry-one".into(), Some("A".into())),
            ("entry-two".into(), Some("C".into())),
        ]);
        let (reset, preserved) = reset_scope(&original, &targets, "replace");
        assert_eq!(reset, vec!["A", "C"]);
        assert_eq!(preserved, vec!["B", "D"]);
    }
    #[test]
    fn all_null_replace_has_no_reset_or_deletion_scope() {
        let original = [json!({"instanceId":"A"}), json!({"instanceId":"B"})];
        let targets = BTreeMap::from([("entry".into(), None)]);
        let (reset, preserved) = reset_scope(&original, &targets, "replace");
        assert!(reset.is_empty());
        assert_eq!(preserved, vec!["A", "B"]);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn omit_preserves_target_mask_relationships() {
        let mut target =
            json!({"opacity":20,"maskId":"target","nested":{"maskIds":["shared"],"amount":1}});
        let mut source =
            json!({"opacity":80,"maskId":"source","nested":{"maskIds":["source"],"amount":2}});
        merge_omitted(&mut target, &mut source);
        assert_eq!(
            target,
            json!({"opacity":80,"maskId":"target","nested":{"maskIds":["shared"],"amount":2}})
        );
    }
    #[test]
    fn replace_restores_only_relationship_fields() {
        let old = json!({"maskIds":["shared"],"parameters":{"x":9},"blend":{"maskId":"shared","opacity":90}});
        let mut reset = json!({"maskIds":[],"parameters":{"x":0},"blend":{"opacity":100}});
        preserve_relationships(&mut reset, &old);
        assert_eq!(
            reset,
            json!({"maskIds":["shared"],"parameters":{"x":0},"blend":{"maskId":"shared","opacity":100}})
        );
    }
}
