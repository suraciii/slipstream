use super::*;
use std::collections::{BTreeMap, BTreeSet};
pub(super) fn admitted(schema: &Value, value: &Value) -> Result<(), ToolError> {
    let validator = jsonschema::validator_for(schema)
        .map_err(|e| error("invalid_definition", format!("invalid native schema: {e}")))?;
    validator
        .validate(value)
        .map_err(|e| error("invalid_definition", e.to_string()))?;
    Ok(())
}
pub(super) fn module<'a>(discovery: &'a Value, id: &str) -> Result<&'a Value, ToolError> {
    array(discovery, "modules")?
        .iter()
        .find(|m| m["moduleId"] == id)
        .ok_or_else(|| {
            error(
                "incompatible_definition",
                format!("operation {id} unavailable in bundle"),
            )
        })
}
pub(super) fn properties(module: &Value) -> Result<&serde_json::Map<String, Value>, ToolError> {
    module["parameters"]["properties"]
        .as_object()
        .ok_or_else(|| {
            error(
                "incompatible_definition",
                "operation parameter schema unavailable",
            )
        })
}
pub(super) fn check_settings(
    settings: &Value,
    entry: bool,
    discovery: &Value,
    initialization: &str,
) -> Result<(), ToolError> {
    keys(
        settings,
        if entry {
            &[
                "entryId",
                "moduleId",
                "parameterSchemaVersion",
                "parameters",
                "blend",
            ]
        } else {
            &["moduleId", "parameterSchemaVersion", "parameters", "blend"]
        },
    )?;
    let m = module(discovery, text(settings, "moduleId")?)?;
    if settings["parameterSchemaVersion"] != m["parameterSchemaVersion"] {
        return Err(error(
            "incompatible_definition",
            "parameter schema version differs",
        ));
    }
    let props = properties(m)?;
    let mut seen = BTreeSet::new();
    for p in array(settings, "parameters")? {
        keys(p, &["parameterId", "value"])?;
        let id = text(p, "parameterId")?;
        if !seen.insert(id) {
            return Err(error("invalid_definition", "duplicate captured parameter"));
        }
        let schema = props
            .get(id)
            .ok_or_else(|| error("invalid_definition", "unknown captured parameter"))?;
        if initialization == "target-camera"
            && p["value"].get("mode") == Some(&json!("target-camera"))
        {
            admitted(
                schema.get("initializationSchema").ok_or_else(|| {
                    error(
                        "unsupported_initialization",
                        "parameter does not admit target initialization",
                    )
                })?,
                &p["value"],
            )?;
        } else {
            admitted(schema, &p["value"])?;
        }
    }
    if !settings["blend"].is_null() {
        let mut schema = m["blendSchema"].clone();
        omit_required(&mut schema);
        admitted(&schema, &settings["blend"])?;
    }
    Ok(())
}
pub(super) const MASK_FIELDS: &[&str] = &[
    "mode",
    "maskId",
    "maskIds",
    "parametric",
    "raster",
    "combineInclusive",
    "inverted",
    "drawnPositive",
    "featheringRadius",
    "featheringGuide",
    "blurRadius",
    "contrast",
    "brightness",
    "details",
    "featherVersion",
    "colorSpace",
    "rasterSourceInstanceId",
    "sourceInstanceId",
];
fn omit_required(schema: &mut Value) {
    match schema {
        Value::Object(o) => {
            if let Some(required) = o.get_mut("required").and_then(Value::as_array_mut) {
                required.retain(|v| !v.as_str().is_some_and(|key| MASK_FIELDS.contains(&key)));
            }
            for v in o.values_mut() {
                omit_required(v);
            }
        }
        Value::Array(a) => {
            for v in a {
                omit_required(v)
            }
        }
        _ => (),
    }
}
pub(super) fn typed_relationships(
    value: &Value,
    mask_ids: &BTreeSet<String>,
    entry_ids: &BTreeSet<String>,
) -> Result<(), ToolError> {
    match value {
        Value::Object(o) => {
            for (k, v) in o {
                if ["instanceId", "sourceInstanceId", "rasterSourceInstanceId"]
                    .contains(&k.as_str())
                {
                    if let Some(id) = v.as_str()
                        && !entry_ids.contains(id)
                    {
                        return Err(error(
                            "invalid_definition",
                            "dangling record-local instance reference",
                        ));
                    }
                } else if k == "maskId" {
                    if let Some(id) = v.as_str()
                        && !mask_ids.contains(id)
                    {
                        return Err(error(
                            "invalid_definition",
                            "dangling record-local mask reference",
                        ));
                    }
                } else if k == "maskIds" {
                    for id in v
                        .as_array()
                        .ok_or_else(|| error("invalid_definition", "maskIds must be array"))?
                    {
                        if !mask_ids.contains(
                            id.as_str().ok_or_else(|| {
                                error("invalid_definition", "mask ID must be string")
                            })?,
                        ) {
                            return Err(error(
                                "invalid_definition",
                                "dangling record-local mask reference",
                            ));
                        }
                    }
                } else {
                    typed_relationships(v, mask_ids, entry_ids)?;
                }
            }
        }
        Value::Array(a) => {
            for v in a {
                typed_relationships(v, mask_ids, entry_ids)?;
            }
        }
        _ => (),
    }
    Ok(())
}
pub(super) fn validate(
    record: &Value,
    kind: &str,
    discovery: &Value,
    bundle_id: &str,
) -> Result<(), ToolError> {
    let common = [
        "id",
        "version",
        "kind",
        "name",
        "bundleId",
        "schemaVersion",
        "createdAt",
        "availability",
        "initialization",
        "maskPolicy",
        "resources",
        "masks",
    ];
    crate::catalog_interchange::validate_record(record, kind)
        .map_err(|e| error("invalid_definition", e))?;
    let mut allowed = common.to_vec();
    if kind == "preset" {
        allowed.push("settings");
    } else {
        allowed.extend(["entries", "output"]);
    }
    keys(record, &allowed)?;
    for key in common {
        if !record.as_object().unwrap().contains_key(key) {
            return Err(error(
                "invalid_definition",
                format!("missing definition field {key}"),
            ));
        }
    }
    for key in ["id", "version", "bundleId", "schemaVersion", "createdAt"] {
        text(record, key)?;
    }
    time::OffsetDateTime::parse(
        text(record, "createdAt")?,
        &time::format_description::well_known::Rfc3339,
    )
    .map_err(|_| error("invalid_definition", "createdAt must be RFC3339"))?;
    keys(
        &record["availability"],
        &["read", "write", "execute", "reason", "recovery"],
    )?;
    for key in ["read", "write", "execute"] {
        if !record["availability"][key].is_boolean() {
            return Err(error(
                "invalid_definition",
                "capability fields must be boolean",
            ));
        }
    }
    for key in ["reason", "recovery"] {
        if !record["availability"][key].is_null() && !record["availability"][key].is_string() {
            return Err(error(
                "invalid_definition",
                "capability reason/recovery must be nullable strings",
            ));
        }
    }
    if record["kind"] != kind || text(record, "schemaVersion")? != "1" {
        return Err(error(
            "invalid_definition",
            "record kind or representation schema differs",
        ));
    }
    if text(record, "bundleId")? != bundle_id {
        return Err(error(
            "incompatible_definition",
            "definition bundle differs from current native bundle",
        ));
    }
    let init = text(record, "initialization")?;
    if !["literal", "target-camera"].contains(&init) {
        return Err(error("invalid_definition", "invalid initialization mode"));
    }
    let policy = text(record, "maskPolicy")?;
    if !["omit", "compatible"].contains(&policy) {
        return Err(error("invalid_definition", "invalid mask policy"));
    }
    if text(record, "name")?.is_empty() {
        return Err(error("invalid_definition", "definition name is empty"));
    }
    let mut entries = BTreeSet::new();
    if kind == "preset" {
        check_settings(&record["settings"], false, discovery, init)?;
        entries.insert("self".to_owned());
    } else {
        let settings = array(record, "entries")?;
        if settings.is_empty() {
            return Err(error(
                "invalid_definition",
                "style entries must be nonempty",
            ));
        }
        for s in settings {
            let id = text(s, "entryId")?;
            if !entries.insert(id.to_owned()) {
                return Err(error("invalid_definition", "duplicate entry identity"));
            }
            check_settings(s, true, discovery, init)?;
        }
        if !record["output"].is_null() {
            admitted(&discovery["output"]["schema"], &record["output"])?;
        }
    }
    let mut declared = BTreeSet::new();
    for r in array(record, "resources")? {
        keys(r, &["resourceId", "version"])?;
        if !declared.insert((
            text(r, "resourceId")?.to_owned(),
            text(r, "version")?.to_owned(),
        )) {
            return Err(error("invalid_definition", "duplicate dependency"));
        }
    }
    if policy == "omit" && !record["masks"].is_null() {
        return Err(error("invalid_definition", "omitted masks must be null"));
    }
    if !record["masks"].is_null() {
        keys(&record["masks"], &["schema", "value"])?;
        if record["masks"]["schema"] != discovery["maskGraphSchema"] {
            return Err(error(
                "incompatible_definition",
                "mask graph schema differs from native bundle",
            ));
        }
        admitted(&record["masks"]["schema"], &record["masks"]["value"])?;
        graph(&record["masks"]["value"], &entries)?;
    }
    if policy == "compatible" {
        let settings = if kind == "preset" {
            vec![&record["settings"]]
        } else {
            array(record, "entries")?.iter().collect::<Vec<_>>()
        };
        for settings in settings {
            if !settings["blend"].is_null() {
                let m = module(discovery, text(settings, "moduleId")?)?;
                admitted(&m["blendSchema"], &settings["blend"])?;
            }
        }
    }
    let mask_ids = record["masks"]["value"]["masks"]
        .as_array()
        .map(|masks| {
            masks
                .iter()
                .filter_map(|m| m["maskId"].as_str().map(str::to_owned))
                .collect::<BTreeSet<_>>()
        })
        .unwrap_or_default();
    if kind == "preset" {
        typed_relationships(&record["settings"], &mask_ids, &entries)?;
    } else {
        typed_relationships(&record["entries"], &mask_ids, &entries)?;
        typed_relationships(&record["output"], &mask_ids, &entries)?;
    }
    if !record["masks"].is_null() {
        for mask in array(&record["masks"]["value"], "masks")? {
            typed_relationships(&mask["value"], &mask_ids, &entries)?;
        }
    }
    let mut actual = Vec::new();
    let mut content = record.clone();
    content.as_object_mut().unwrap().remove("resources");
    refs(&content, &mut actual);
    if actual.into_iter().collect::<BTreeSet<_>>() != declared {
        return Err(error(
            "invalid_definition",
            "resource list does not match complete captured closure",
        ));
    }
    Ok(())
}
fn graph(value: &Value, entries: &BTreeSet<String>) -> Result<(), ToolError> {
    let masks = array(value, "masks")?;
    let mut ids = BTreeSet::new();
    for m in masks {
        if !ids.insert(text(m, "maskId")?.to_owned()) {
            return Err(error("invalid_definition", "duplicate mask identity"));
        }
    }
    let mut edges = BTreeMap::<String, Vec<String>>::new();
    for m in masks {
        let mut found = Vec::new();
        mask_refs(&m["value"], &mut found);
        for id in &found {
            if !ids.contains(id) {
                return Err(error(
                    "invalid_definition",
                    "dangling captured mask reference",
                ));
            }
        }
        for r in array(m, "referencedBy")? {
            let id = r
                .as_str()
                .ok_or_else(|| error("invalid_definition", "mask relationship must be string"))?;
            if !entries.contains(id) && !ids.contains(id) {
                return Err(error(
                    "invalid_definition",
                    "dangling captured relationship",
                ));
            }
        }
        edges.insert(text(m, "maskId")?.to_owned(), found);
    }
    fn visit(
        id: &str,
        edges: &BTreeMap<String, Vec<String>>,
        active: &mut BTreeSet<String>,
        done: &mut BTreeSet<String>,
    ) -> Result<(), ToolError> {
        if done.contains(id) {
            return Ok(());
        }
        if !active.insert(id.into()) {
            return Err(error("invalid_definition", "cyclic mask graph"));
        }
        for child in &edges[id] {
            visit(child, edges, active, done)?;
        }
        active.remove(id);
        done.insert(id.into());
        Ok(())
    }
    let mut active = BTreeSet::new();
    let mut done = BTreeSet::new();
    for id in &ids {
        visit(id, &edges, &mut active, &mut done)?;
    }
    Ok(())
}
pub(super) fn mask_refs(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(o) => {
            for (k, v) in o {
                if k == "maskId" {
                    if let Some(id) = v.as_str()
                        && !out.iter().any(|x| x == id)
                    {
                        out.push(id.into());
                    }
                } else if k == "maskIds" {
                    if let Some(a) = v.as_array() {
                        for id in a.iter().filter_map(Value::as_str) {
                            if !out.iter().any(|x| x == id) {
                                out.push(id.into());
                            }
                        }
                    }
                } else {
                    mask_refs(v, out);
                }
            }
        }
        Value::Array(a) => {
            for v in a {
                mask_refs(v, out)
            }
        }
        _ => (),
    }
}
pub(super) fn rewrite(
    value: &mut Value,
    masks: &BTreeMap<String, String>,
    instances: &BTreeMap<String, String>,
) {
    match value {
        Value::Object(o) => {
            for (k, v) in o.iter_mut() {
                if [
                    "maskId",
                    "instanceId",
                    "sourceInstanceId",
                    "rasterSourceInstanceId",
                ]
                .contains(&k.as_str())
                {
                    if let Some(s) = v.as_str()
                        && let Some(new) = if k == "maskId" {
                            masks.get(s)
                        } else {
                            instances.get(s)
                        }
                    {
                        *v = json!(new);
                    }
                } else if ["maskIds", "referencedBy"].contains(&k.as_str()) {
                    if let Some(a) = v.as_array_mut() {
                        for v in a {
                            if let Some(s) = v.as_str()
                                && let Some(new) = if k == "maskIds" {
                                    masks.get(s)
                                } else {
                                    instances.get(s).or_else(|| masks.get(s))
                                }
                            {
                                *v = json!(new);
                            }
                        }
                    }
                } else {
                    rewrite(v, masks, instances);
                }
            }
        }
        Value::Array(a) => {
            for v in a {
                rewrite(v, masks, instances)
            }
        }
        _ => (),
    }
}
pub(super) fn omit(value: &mut Value) {
    match value {
        Value::Object(o) => {
            o.retain(|k, _| !MASK_FIELDS.contains(&k.as_str()));
            for v in o.values_mut() {
                omit(v);
            }
        }
        Value::Array(a) => {
            for v in a {
                omit(v)
            }
        }
        _ => (),
    }
}
pub(super) fn capture(
    kind: &str,
    args: &Value,
    edit: &Value,
    discovery: &Value,
    bundle_id: &str,
) -> Result<(Value, Option<Vec<u8>>), ToolError> {
    let selection = &args["input"];
    keys(
        selection,
        if kind == "preset" {
            &[
                "name",
                "parameterIds",
                "includeBlend",
                "initialization",
                "maskPolicy",
            ]
        } else {
            &[
                "name",
                "instances",
                "includeOutput",
                "initialization",
                "maskPolicy",
            ]
        },
    )?;
    let init = text(selection, "initialization")?;
    let policy = text(selection, "maskPolicy")?;
    let snapshot = &edit["snapshot"];
    let instances = array(snapshot, "instances")?;
    let selections = if kind == "preset" {
        vec![
            json!({"instanceId":text(args,"instanceId")?,"parameterIds":selection["parameterIds"],"includeBlend":selection["includeBlend"]}),
        ]
    } else {
        array(selection, "instances")?.clone()
    };
    if selections.is_empty() {
        return Err(error(
            "invalid_request",
            "capture selection must be nonempty",
        ));
    }
    let mut captured = Vec::new();
    let mut instance_map = BTreeMap::new();
    let mut selected_ids = BTreeSet::new();
    let mut ordered = Vec::new();
    for s in selections {
        keys(&s, &["instanceId", "parameterIds", "includeBlend"])?;
        let id = text(&s, "instanceId")?;
        if !selected_ids.insert(id.to_owned()) {
            return Err(error("invalid_request", "duplicate selected instance"));
        }
        let instance = instances
            .iter()
            .find(|i| i["instanceId"] == id)
            .ok_or_else(|| error("instance_not_found", "capture instance missing"))?;
        let eid = if kind == "preset" {
            "self".to_owned()
        } else {
            photo_files::identity("entry").map_err(io)?
        };
        instance_map.insert(id.to_owned(), eid.clone());
        ordered.push((instance["order"].as_f64().unwrap_or(0.0), s, instance, eid));
    }
    ordered.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut used_masks = Vec::new();
    for (_, s, i, eid) in ordered {
        let m = module(discovery, text(i, "moduleId")?)?;
        let props = properties(m)?;
        let ids = array(&s, "parameterIds")?;
        if ids.is_empty() && s["includeBlend"] != true {
            return Err(error(
                "invalid_request",
                "capture selection must contain parameters or blend",
            ));
        }
        let mut seen = BTreeSet::new();
        let mut parameters = Vec::new();
        for id in ids {
            let id = id
                .as_str()
                .ok_or_else(|| error("invalid_request", "parameter ID must be string"))?;
            if !seen.insert(id) {
                return Err(error("invalid_request", "duplicate selected parameter"));
            }
            let schema = props
                .get(id)
                .ok_or_else(|| error("invalid_request", "unknown selected parameter"))?;
            let value = if init == "target-camera" && schema.get("initializationSchema").is_some() {
                let directive = json!({"mode":"target-camera"});
                admitted(&schema["initializationSchema"], &directive)?;
                directive
            } else {
                i["parameters"].get(id).cloned().ok_or_else(|| {
                    error("invalid_request", "selected parameter value unavailable")
                })?
            };
            if policy == "omit" {
                let mut refs = Vec::new();
                mask_refs(&value, &mut refs);
                if !refs.is_empty() {
                    continue;
                }
            } else {
                mask_refs(&value, &mut used_masks);
            }
            parameters.push(json!({"parameterId":id,"value":value}));
        }
        let include = s["includeBlend"]
            .as_bool()
            .ok_or_else(|| error("invalid_request", "includeBlend must be boolean"))?;
        let mut blend = if include {
            i["blend"].clone()
        } else {
            Value::Null
        };
        if policy == "omit" {
            omit(&mut blend);
        } else if include {
            mask_refs(i, &mut used_masks);
        }
        if parameters.is_empty()
            && (blend.is_null() || blend.as_object().is_some_and(|o| o.is_empty()))
        {
            return Err(error(
                "invalid_request",
                "capture has no fields after mask omission",
            ));
        }
        let mut setting = json!({"moduleId":i["moduleId"],"parameterSchemaVersion":m["parameterSchemaVersion"],"parameters":parameters,"blend":blend});
        if kind == "style" {
            setting["entryId"] = json!(eid);
        }
        captured.push(setting);
    }
    let mut mask_map = BTreeMap::new();
    let mut masks = Vec::new();
    if policy == "compatible" && !used_masks.is_empty() {
        let source = array(snapshot, "masks")?;
        let mut n = 0;
        while n < used_masks.len() {
            if used_masks.len() > 4096 {
                return Err(error("invalid_definition", "captured mask limit exceeded"));
            }
            let id = &used_masks[n];
            let mask = source
                .iter()
                .find(|m| m["maskId"] == *id)
                .ok_or_else(|| error("invalid_definition", "dangling source mask"))?;
            let mut dependencies = Vec::new();
            mask_refs(&mask["value"], &mut dependencies);
            for dep in dependencies {
                if !used_masks.contains(&dep) {
                    used_masks.push(dep);
                }
            }
            n += 1;
        }
        for id in &used_masks {
            mask_map.insert(id.clone(), photo_files::identity("mask").map_err(io)?);
        }
        for id in used_masks {
            let mut m = source.iter().find(|m| m["maskId"] == id).unwrap().clone();
            m["referencedBy"] = json!(
                m["referencedBy"]
                    .as_array()
                    .unwrap_or(&Vec::new())
                    .iter()
                    .filter_map(|id| id.as_str())
                    .filter(|id| instance_map.contains_key(*id) || mask_map.contains_key(*id))
                    .collect::<Vec<_>>()
            );
            rewrite(&mut m, &mask_map, &instance_map);
            masks.push(m);
        }
    }
    for s in &mut captured {
        rewrite(s, &mask_map, &instance_map);
    }
    let graph = if masks.is_empty() {
        Value::Null
    } else {
        json!({"schema":discovery["maskGraphSchema"],"value":{"masks":masks,"frames":snapshot["frames"],"input":{"fingerprint":edit["input"]["fingerprint"],"byteLength":edit["input"]["byteLength"],"boundRevision":edit["input"]["boundRevision"]}}})
    };
    let mut record = json!({"id":"pending","version":"pending","kind":kind,"name":text(selection,"name")?,"bundleId":bundle_id,"schemaVersion":"1","createdAt":utc(now())?,"availability":available(),"initialization":init,"maskPolicy":policy,"resources":[],"masks":graph});
    if kind == "preset" {
        record["settings"] = captured.remove(0);
    } else {
        record["entries"] = json!(captured);
        record["output"] = if selection["includeOutput"]
            .as_bool()
            .ok_or_else(|| error("invalid_request", "includeOutput must be boolean"))?
        {
            snapshot["output"]["settings"].clone()
        } else {
            Value::Null
        };
    }
    let mut resources = Vec::new();
    refs(&record, &mut resources);
    record["resources"] = json!(
        resources
            .into_iter()
            .map(|(id, version)| json!({"resourceId":id,"version":version}))
            .collect::<Vec<_>>()
    );
    validate(&record, kind, discovery, bundle_id)?;
    Ok((record, None))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn graph_rejects_cycle_and_dangling() {
        let entries = BTreeSet::from(["self".into()]);
        assert!(
            graph(
                &json!({"masks":[{"maskId":"a","referencedBy":["self"],"value":{"maskId":"a"}}]}),
                &entries
            )
            .is_err()
        );
        assert!(
            graph(
                &json!({"masks":[{"maskId":"a","referencedBy":["missing"],"value":{}}]}),
                &entries
            )
            .is_err()
        );
    }
    #[test]
    fn shared_reference_rewrite_is_consistent() {
        let mut value =
            json!({"maskIds":["a","a"],"sourceInstanceId":"old","nested":{"maskId":"a"}});
        rewrite(
            &mut value,
            &BTreeMap::from([("a".into(), "b".into())]),
            &BTreeMap::from([("old".into(), "entry".into())]),
        );
        assert_eq!(
            value,
            json!({"maskIds":["b","b"],"sourceInstanceId":"entry","nested":{"maskId":"b"}})
        );
    }
}
