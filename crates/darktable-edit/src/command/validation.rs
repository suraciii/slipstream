use crate::protocol::ToolError;
use serde_json::{Map, Value};

type Result<T, E = ToolError> = std::result::Result<T, E>;
fn invalid(message: impl Into<String>) -> ToolError {
    ToolError::new("invalid_input", message, 2)
}
fn identifier(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}
fn token(value: &str) -> bool {
    (1..=512).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}
fn object<'a>(
    value: &'a Value,
    allowed: &[&str],
    required: &[&str],
) -> Result<&'a Map<String, Value>> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("Command input must be a JSON object"))?;
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(invalid(format!("Unknown JSON key: {key}")));
        }
    }
    for key in required {
        if !object.contains_key(*key) {
            return Err(invalid(format!("Missing JSON key: {key}")));
        }
    }
    Ok(object)
}
fn text(value: &Value, name: &str) -> Result<()> {
    if value.as_str().is_some() {
        Ok(())
    } else {
        Err(invalid(format!("{name} must be a string")))
    }
}
fn id(value: &Value, name: &str, refs: bool, nullable: bool) -> Result<()> {
    if nullable && value.is_null() {
        return Ok(());
    }
    let valid = value.as_str().is_some_and(|value| {
        identifier(value) || refs && value.strip_prefix('@').is_some_and(reference)
    });
    if valid {
        Ok(())
    } else {
        Err(invalid(format!("{name} must be an admitted identifier")))
    }
}
fn reference(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
}
fn boolean(value: &Value, name: &str) -> Result<()> {
    if value.is_boolean() {
        Ok(())
    } else {
        Err(invalid(format!("{name} must be boolean")))
    }
}
fn enum_value(value: &Value, name: &str, choices: &[&str]) -> Result<()> {
    if value.as_str().is_some_and(|value| choices.contains(&value)) {
        Ok(())
    } else {
        Err(invalid(format!(
            "Invalid {name}; expected {}",
            choices.join("|")
        )))
    }
}
fn ids(value: &Value, name: &str, refs: bool) -> Result<()> {
    let values = value
        .as_array()
        .ok_or_else(|| invalid(format!("{name} must be an array")))?;
    if values.is_empty() || values.len() > 256 {
        return Err(invalid(format!("{name} must contain 1–256 IDs")));
    }
    for value in values {
        id(value, name, refs, false)?;
    }
    Ok(())
}
fn object_value(value: &Value, name: &str) -> Result<()> {
    if value.is_object() {
        Ok(())
    } else {
        Err(invalid(format!("{name} must be an object")))
    }
}
fn selection(value: &Value, style: bool) -> Result<()> {
    let allowed = if style {
        vec![
            "name",
            "instances",
            "includeOutput",
            "initialization",
            "maskPolicy",
        ]
    } else {
        vec![
            "name",
            "parameterIds",
            "includeBlend",
            "initialization",
            "maskPolicy",
        ]
    };
    let fields = object(value, &allowed, &allowed)?;
    text(&fields["name"], "name")?;
    if fields["name"]
        .as_str()
        .is_some_and(|name| name.is_empty() || name.len() > 128)
    {
        return Err(invalid("name must contain 1–128 UTF-8 bytes"));
    }
    enum_value(
        &fields["initialization"],
        "initialization",
        &["literal", "target-camera"],
    )?;
    enum_value(&fields["maskPolicy"], "maskPolicy", &["omit", "compatible"])?;
    if style {
        boolean(&fields["includeOutput"], "includeOutput")?;
        let instances = fields["instances"]
            .as_array()
            .ok_or_else(|| invalid("instances must be an array"))?;
        if instances.is_empty() || instances.len() > 256 {
            return Err(invalid("instances must contain 1–256 entries"));
        }
        for instance in instances {
            let entry = object(
                instance,
                &["instanceId", "parameterIds", "includeBlend"],
                &["instanceId", "parameterIds", "includeBlend"],
            )?;
            id(&entry["instanceId"], "instanceId", false, false)?;
            parameter_selection(entry)?;
        }
    } else {
        parameter_selection(fields)?;
    }
    Ok(())
}
fn parameter_selection(fields: &Map<String, Value>) -> Result<()> {
    boolean(&fields["includeBlend"], "includeBlend")?;
    let parameters = fields["parameterIds"]
        .as_array()
        .ok_or_else(|| invalid("parameterIds must be an array"))?;
    if parameters.is_empty() && fields["includeBlend"] != Value::Bool(true) {
        return Err(invalid("Select at least one parameter or blend settings"));
    }
    for parameter in parameters {
        id(parameter, "parameterIds", false, false)?;
    }
    Ok(())
}
fn mapping(value: &Value) -> Result<()> {
    let entries = value
        .as_array()
        .ok_or_else(|| invalid("mapping must be an array"))?;
    if entries.is_empty() || entries.len() > 256 {
        return Err(invalid("mapping must contain 1–256 entries"));
    }
    for entry in entries {
        let entry = object(
            entry,
            &["entryId", "instanceId"],
            &["entryId", "instanceId"],
        )?;
        id(&entry["entryId"], "entryId", false, false)?;
        id(&entry["instanceId"], "instanceId", true, true)?;
    }
    Ok(())
}
fn style_apply(value: &Value, atomic: bool) -> Result<()> {
    let allowed = if atomic {
        vec![
            "type",
            "styleId",
            "version",
            "mode",
            "mapping",
            "initialization",
            "maskPolicy",
            "includeOutput",
        ]
    } else {
        vec![
            "mode",
            "mapping",
            "initialization",
            "maskPolicy",
            "includeOutput",
        ]
    };
    let fields = object(value, &allowed, &allowed)?;
    if atomic {
        id(&fields["styleId"], "styleId", false, false)?;
        id(&fields["version"], "version", false, false)?;
    }
    enum_value(&fields["mode"], "mode", &["merge", "replace"])?;
    enum_value(
        &fields["initialization"],
        "initialization",
        &["literal", "target-camera"],
    )?;
    enum_value(&fields["maskPolicy"], "maskPolicy", &["omit", "compatible"])?;
    boolean(&fields["includeOutput"], "includeOutput")?;
    mapping(&fields["mapping"])
}
fn change(value: &Value) -> Result<()> {
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("Each change requires a string type"))?;
    let keys: &[&str] = match kind {
        "set" => &["type", "instanceId", "parameterId", "value"],
        "reset" => &["type", "scope", "instanceId", "parameterId"],
        "instance-add" => &["type", "moduleId", "ref", "name"],
        "instance-clone" => &["type", "instanceId", "ref", "name"],
        "instance-enable" => &["type", "instanceId", "enabled"],
        "instance-remove" => &["type", "instanceId"],
        "instance-reorder" => &["type", "instanceIds"],
        "mask-create" => &["type", "ref", "kind", "frameId", "value"],
        "mask-set" => &["type", "maskId", "value"],
        "mask-remove" => &["type", "maskId"],
        "blend-set" => &["type", "instanceId", "value"],
        "action" => &["type", "actionId", "instanceId", "input"],
        "output-set" => &["type", "fieldId", "value"],
        "preset-apply" => &[
            "type",
            "presetId",
            "version",
            "instanceId",
            "mode",
            "initialization",
            "maskPolicy",
        ],
        "style-apply" => return style_apply(value, true),
        _ => return Err(invalid(format!("Unknown change type: {kind}"))),
    };
    let required: Vec<_> = if kind == "reset" {
        vec!["type", "scope"]
    } else {
        keys.to_vec()
    };
    let fields = object(value, keys, &required)?;
    for (key, value) in fields {
        if key.ends_with("Id") || key == "version" {
            id(value, key, true, kind == "action" && key == "instanceId")?;
        }
    }
    if let Some(value) = fields.get("name")
        && !value.is_null()
    {
        text(value, "name")?;
    }
    if let Some(value) = fields.get("ref")
        && !value.as_str().is_some_and(reference)
    {
        return Err(invalid(
            "ref must contain 1–64 ASCII letters, digits, underscores or hyphens",
        ));
    }
    match kind {
        "reset" => {
            enum_value(&fields["scope"], "scope", &["all", "instance", "parameter"])?;
            let scope = fields["scope"].as_str().unwrap();
            if scope == "all"
                && (fields.contains_key("instanceId") || fields.contains_key("parameterId"))
                || scope == "instance" && fields.contains_key("parameterId")
            {
                return Err(invalid("Reset contains irrelevant fields"));
            }
            if scope != "all" && !fields.contains_key("instanceId")
                || scope == "parameter" && !fields.contains_key("parameterId")
            {
                return Err(invalid("Reset lacks its target"));
            }
        }
        "instance-enable" => boolean(&fields["enabled"], "enabled")?,
        "instance-reorder" => ids(&fields["instanceIds"], "instanceIds", true)?,
        "mask-create" => {
            id(&fields["kind"], "kind", false, false)?;
            object_value(&fields["value"], "value")?;
        }
        "mask-set" | "blend-set" => object_value(&fields["value"], "value")?,
        "action" => object_value(&fields["input"], "input")?,
        "preset-apply" => {
            enum_value(&fields["mode"], "mode", &["update", "replace"])?;
            enum_value(
                &fields["initialization"],
                "initialization",
                &["literal", "target-camera"],
            )?;
            enum_value(&fields["maskPolicy"], "maskPolicy", &["omit", "compatible"])?;
        }
        _ => (),
    }
    Ok(())
}
pub(super) fn validate_arguments(operation: &str, arguments: &Map<String, Value>) -> Result<()> {
    for (name, value) in arguments {
        if name.ends_with("Id")
            || matches!(
                name.as_str(),
                "version" | "revision" | "kind" | "schemaVersion"
            )
        {
            id(value, name, false, false)?;
        }
        if matches!(name.as_str(), "ifState" | "migrationToken" | "cursor")
            && !value.as_str().is_some_and(token)
        {
            return Err(invalid(format!(
                "{name} must be a base64url token of 1–512 bytes"
            )));
        }
    }
    if operation == "edit masks set" {
        object_value(&arguments["value"], "value")?;
    }
    let Some(input) = arguments.get("input") else {
        return Ok(());
    };
    match operation {
        "edit apply" | "edit validate" => {
            let fields = object(input, &["changes"], &["changes"])?;
            let changes = fields["changes"]
                .as_array()
                .ok_or_else(|| invalid("changes must be an array"))?;
            if changes.is_empty() || changes.len() > 100 {
                return Err(invalid("changes must contain 1–100 changes"));
            }
            for value in changes {
                change(value)?;
            }
        }
        "edit instances reorder" => {
            let fields = object(input, &["instanceIds"], &["instanceIds"])?;
            ids(&fields["instanceIds"], "instanceIds", false)?;
        }
        "edit masks create" => {
            let fields = object(
                input,
                &["kind", "frameId", "value"],
                &["kind", "frameId", "value"],
            )?;
            id(&fields["kind"], "kind", false, false)?;
            id(&fields["frameId"], "frameId", false, false)?;
            object_value(&fields["value"], "value")?;
        }
        "edit actions run" => object_value(input, "input")?,
        "edit rebind" => {
            let fields = object(input, &["inputPath"], &["inputPath"])?;
            text(&fields["inputPath"], "inputPath")?;
            if fields["inputPath"].as_str() == Some("") {
                return Err(invalid("inputPath cannot be empty"));
            }
        }
        "presets save" => selection(input, false)?,
        "styles save" => selection(input, true)?,
        "styles apply" => style_apply(input, false)?,
        "preview render" => {
            let fields = object(input, &["comparison", "region"], &[])?;
            if let Some(comparison) = fields.get("comparison")
                && !comparison.is_null()
                && comparison.as_str() != Some("baseline")
            {
                let compared = object(comparison, &["revision"], &["revision"])?;
                id(&compared["revision"], "revision", false, false)?;
            }
            if let Some(region) = fields.get("region")
                && !region.is_null()
            {
                object_value(region, "region")?;
            }
        }
        _ => return Err(invalid(format!("{operation} does not accept JSON input"))),
    }
    Ok(())
}
