use serde_json::Value;
use std::collections::BTreeSet;
fn object<'a>(
    v: &'a Value,
    allowed: &[&str],
    required: &[&str],
) -> Result<&'a serde_json::Map<String, Value>, String> {
    let o = v.as_object().ok_or("expected object")?;
    if let Some(k) = o.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(format!("unknown field {k}"));
    }
    if let Some(k) = required.iter().find(|k| !o.contains_key(**k)) {
        return Err(format!("missing field {k}"));
    }
    Ok(o)
}
fn string<'a>(v: &'a Value, k: &str) -> Result<&'a str, String> {
    v[k].as_str().ok_or_else(|| format!("{k} must be string"))
}
fn identifier(v: &Value, k: &str) -> Result<(), String> {
    let s = string(v, k)?;
    if s.is_empty()
        || s.len() > 128
        || !s
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return Err(format!("{k} invalid identifier"));
    }
    Ok(())
}
fn array<'a>(v: &'a Value, k: &str) -> Result<&'a Vec<Value>, String> {
    v[k].as_array().ok_or_else(|| format!("{k} must be array"))
}
fn settings(v: &Value, entry: bool) -> Result<(), String> {
    let mut keys = vec!["moduleId", "parameterSchemaVersion", "parameters", "blend"];
    if entry {
        keys.push("entryId");
    }
    object(v, &keys, &keys)?;
    identifier(v, "moduleId")?;
    string(v, "parameterSchemaVersion")?;
    if entry {
        identifier(v, "entryId")?;
    }
    let mut ids = BTreeSet::new();
    for p in array(v, "parameters")? {
        object(p, &["parameterId", "value"], &["parameterId", "value"])?;
        identifier(p, "parameterId")?;
        if !ids.insert(string(p, "parameterId")?) {
            return Err("duplicate parameter ID".into());
        }
    }
    if !v["blend"].is_null() && !v["blend"].is_object() {
        return Err("blend must be object or null".into());
    }
    Ok(())
}
/// Structural interchange admission shared by standalone and remote ingress.
/// Native schemas, qualification and immutable dependency closure remain native
/// catalog authority and are checked again before any record is created.
pub fn validate(value: &Value) -> Result<(), String> {
    object(
        value,
        &["schemaVersion", "kind", "record"],
        &["schemaVersion", "kind", "record"],
    )?;
    if value["schemaVersion"] != 1 {
        return Err("interchange schemaVersion must be numeric 1".into());
    }
    let kind = string(value, "kind")?;
    if !["preset", "style"].contains(&kind) {
        return Err("interchange kind must be preset or style".into());
    }
    validate_record(&value["record"], kind)
}
pub fn validate_record(v: &Value, kind: &str) -> Result<(), String> {
    let mut keys = vec![
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
    if kind == "preset" {
        keys.push("settings");
    } else if kind == "style" {
        keys.extend(["entries", "output"]);
    } else {
        return Err("invalid definition kind".into());
    }
    object(v, &keys, &keys)?;
    if v["kind"] != kind || v["schemaVersion"] != "1" {
        return Err("definition kind or representation schemaVersion differs".into());
    }
    for k in ["id", "version"] {
        identifier(v, k)?;
    }
    for k in ["name", "bundleId", "createdAt"] {
        if string(v, k)?.is_empty() {
            return Err(format!("{k} empty"));
        }
    }
    if !["literal", "target-camera"].contains(&string(v, "initialization")?) {
        return Err("invalid initialization".into());
    }
    if !["omit", "compatible"].contains(&string(v, "maskPolicy")?) {
        return Err("invalid maskPolicy".into());
    }
    object(
        &v["availability"],
        &["read", "write", "execute", "reason", "recovery"],
        &["read", "write", "execute", "reason", "recovery"],
    )?;
    for k in ["read", "write", "execute"] {
        if !v["availability"][k].is_boolean() {
            return Err("availability flags must be boolean".into());
        }
    }
    for k in ["reason", "recovery"] {
        if !v["availability"][k].is_null() && !v["availability"][k].is_string() {
            return Err("availability reason/recovery nullable string".into());
        }
    }
    let mut resources = BTreeSet::new();
    for r in array(v, "resources")? {
        object(r, &["resourceId", "version"], &["resourceId", "version"])?;
        identifier(r, "resourceId")?;
        identifier(r, "version")?;
        if !resources.insert((string(r, "resourceId")?, string(r, "version")?)) {
            return Err("duplicate resource reference".into());
        }
    }
    if !v["masks"].is_null() {
        object(&v["masks"], &["schema", "value"], &["schema", "value"])?;
        if !v["masks"]["schema"].is_object() || !v["masks"]["value"].is_object() {
            return Err("mask schema/value must be objects".into());
        }
        if v["maskPolicy"] == "omit" {
            return Err("omit policy cannot capture masks".into());
        }
    }
    if kind == "preset" {
        settings(&v["settings"], false)?;
    } else {
        let entries = array(v, "entries")?;
        if entries.is_empty() {
            return Err("style entries must be nonempty".into());
        }
        let mut ids = BTreeSet::new();
        for entry in entries {
            settings(entry, true)?;
            if !ids.insert(string(entry, "entryId")?) {
                return Err("duplicate entry ID".into());
            }
        }
        if !v["output"].is_null() && !v["output"].is_object() {
            return Err("output must be object or null".into());
        }
    }
    Ok(())
}
