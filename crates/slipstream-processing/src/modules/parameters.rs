//! Deterministic validation of each peer's parameter tree.
use super::*;

/// Dispatch the module-owned deterministic parameter tree validation. Each
/// peer validates its own tree shape against its pinned adapter contract;
/// nothing is executed and no cross-module merging happens first.
pub(super) fn validate_parameter_tree(module: &str, tree: &Value) -> Result<(), ModuleError> {
    match module {
        DARKTABLE_MODULE => validate_darktable_tree(module, tree),
        SPEKTRAFILM_MODULE => validate_spektrafilm_tree(module, tree),
        // The registry only ever holds the two peer constructors.
        _ => Ok(()),
    }
}

fn reject_unknown_keys(
    module: &str,
    object: &Map<String, Value>,
    container: &str,
    allowed: &[&str],
) -> Result<(), ModuleError> {
    for key in object.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(malformed(
                module,
                format!("{container} has the unknown field `{key}`"),
            ));
        }
    }
    Ok(())
}

fn required<'v>(
    module: &str,
    object: &'v Map<String, Value>,
    container: &str,
    key: &str,
) -> Result<&'v Value, ModuleError> {
    object.get(key).ok_or_else(|| {
        malformed(
            module,
            format!("{container} is missing the required field `{key}`"),
        )
    })
}

/// A required control that must equal one pinned string value. A missing or
/// non-string field is malformed; a well-formed value that differs from the
/// pin is an unsupported control, not a conversion target.
fn pinned_string(
    module: &str,
    object: &Map<String, Value>,
    key: &str,
    pinned: &str,
) -> Result<(), ModuleError> {
    match object.get(key) {
        Some(Value::String(value)) if value == pinned => Ok(()),
        Some(Value::String(value)) => Err(unsupported(
            module,
            format!("`{key}` = `{value}`, not the pinned `{pinned}`"),
        )),
        Some(_) => Err(malformed(module, format!("`{key}` is not a string"))),
        None => Err(malformed(
            module,
            format!("`{key}` is missing; the pinned value is `{pinned}`"),
        )),
    }
}

/// A required numeric control that must equal one pinned value.
fn pinned_number(
    module: &str,
    object: &Map<String, Value>,
    key: &str,
    pinned: u64,
) -> Result<(), ModuleError> {
    match object.get(key) {
        Some(value) if value.as_u64() == Some(pinned) => Ok(()),
        Some(value) if value.as_u64().is_some() => Err(unsupported(
            module,
            format!("`{key}` = {value}, not the pinned {pinned}"),
        )),
        Some(_) => Err(malformed(module, format!("`{key}` is not a number"))),
        None => Err(malformed(
            module,
            format!("`{key}` is missing; the pinned value is {pinned}"),
        )),
    }
}

/// An optional control that must equal one pinned string value when present.
fn optional_pinned_string(
    module: &str,
    object: &Map<String, Value>,
    key: &str,
    pinned: &str,
) -> Result<(), ModuleError> {
    match object.get(key) {
        None => Ok(()),
        Some(Value::String(value)) if value == pinned => Ok(()),
        Some(Value::String(value)) => Err(unsupported(
            module,
            format!("`{key}` = `{value}`, not the pinned `{pinned}`"),
        )),
        Some(_) => Err(malformed(module, format!("`{key}` is not a string"))),
    }
}

/// An optional identifier control bounded like the adapter's own names.
fn optional_identifier(
    module: &str,
    object: &Map<String, Value>,
    key: &str,
) -> Result<(), ModuleError> {
    match object.get(key) {
        None => Ok(()),
        Some(Value::String(value)) => {
            if identifier(value, MODULE_NAME_BYTES) {
                Ok(())
            } else {
                Err(malformed(
                    module,
                    format!("`{key}` is not a bounded identifier"),
                ))
            }
        }
        Some(_) => Err(malformed(module, format!("`{key}` is not a string"))),
    }
}

/// Validate the module-owned tree of `darktable-params-1`: one object of at
/// most `stack` and `output`; every stack entry carries the declared fields of
/// the pinned parameter schema; and `output`, when present, is exactly the
/// pinned development handoff. Anything else is refused before the engine
/// starts.
pub(super) fn validate_darktable_tree(module: &str, tree: &Value) -> Result<(), ModuleError> {
    validate_saved_tree(module, tree)?;
    let object = tree
        .as_object()
        .expect("saved tree validation checks object");
    if let Some(output) = object.get("output") {
        validate_darktable_output(module, output)?;
    }
    if let Some(stack) = object.get("stack") {
        let entries = stack
            .as_array()
            .expect("saved tree validation checks stack array");
        for (index, entry) in entries.iter().enumerate() {
            if entry["operation"] == "temperature"
                && entry["params"].get("temperatureKelvin").is_some()
            {
                return Err(unsupported(
                    module,
                    format!(
                        "stack entry {index} retains temperature/tint intent without a qualified native mapping"
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// The pinned development handoff of the darktable adapter.
fn validate_darktable_output(module: &str, output: &Value) -> Result<(), ModuleError> {
    let object = output
        .as_object()
        .ok_or_else(|| malformed(module, "`output` is not an object".into()))?;
    reject_unknown_keys(
        module,
        object,
        "`output`",
        &[
            "format",
            "precisionBits",
            "colorSpace",
            "transferFunction",
            "geometry",
            "encoding",
        ],
    )?;
    pinned_string(module, object, "format", DARKTABLE_OUTPUT_FORMAT)?;
    pinned_number(
        module,
        object,
        "precisionBits",
        DARKTABLE_OUTPUT_PRECISION_BITS,
    )?;
    pinned_string(module, object, "colorSpace", DARKTABLE_OUTPUT_COLOR_SPACE)?;
    pinned_string(
        module,
        object,
        "transferFunction",
        DARKTABLE_OUTPUT_TRANSFER,
    )?;
    optional_pinned_string(module, object, "geometry", DARKTABLE_OUTPUT_GEOMETRY)?;
    optional_identifier(module, object, "encoding")?;
    Ok(())
}

fn validate_darktable_entry(module: &str, index: usize, entry: &Value) -> Result<(), ModuleError> {
    let container = format!("stack entry {index}");
    let fields = entry
        .as_object()
        .ok_or_else(|| malformed(module, format!("{container} is not an object")))?;
    reject_unknown_keys(
        module,
        fields,
        &container,
        &[
            "operation",
            "multiPriority",
            "enabled",
            "params",
            "before",
            "after",
        ],
    )?;
    let operation = required(module, fields, &container, "operation")?
        .as_str()
        .ok_or_else(|| malformed(module, format!("{container} `operation` is not a string")))?;
    if !identifier(operation, MODULE_NAME_BYTES) {
        return Err(malformed(
            module,
            format!("{container} `operation` is not a bounded identifier"),
        ));
    }
    if !required(module, fields, &container, "multiPriority")?.is_i64() {
        return Err(malformed(
            module,
            format!("{container} `multiPriority` is not an integer"),
        ));
    }
    if !required(module, fields, &container, "enabled")?.is_boolean() {
        return Err(malformed(
            module,
            format!("{container} `enabled` is not a boolean"),
        ));
    }
    if !required(module, fields, &container, "params")?.is_object() {
        return Err(malformed(
            module,
            format!("{container} `params` is not an object"),
        ));
    }
    for key in ["before", "after"] {
        if fields.get(key).is_some_and(|value| !value.is_string()) {
            return Err(malformed(
                module,
                format!("{container} `{key}` is not a string"),
            ));
        }
    }
    Ok(())
}

/// Persistence validates every structural field before qualification. A valid
/// unsupported value remains editing intent and does not become executable.
pub(super) fn validate_saved_tree(module: &str, tree: &Value) -> Result<(), ModuleError> {
    let object = tree
        .as_object()
        .ok_or_else(|| malformed(module, "the parameter tree is not an object".into()))?;
    if module == DARKTABLE_MODULE {
        reject_unknown_keys(module, object, "the parameter tree", &["stack", "output"])?;
        if let Some(stack) = object.get("stack") {
            let entries = stack
                .as_array()
                .ok_or_else(|| malformed(module, "`stack` is not an array".into()))?;
            if entries.len() > DARKTABLE_STACK_OPERATIONS_MAX {
                return Err(malformed(
                    module,
                    "`stack` exceeds the operation bound".into(),
                ));
            }
            for (index, entry) in entries.iter().enumerate() {
                validate_darktable_entry(module, index, entry)?;
                let params = entry["params"]
                    .as_object()
                    .expect("entry checks params object");
                if entry["operation"] == "exposure" {
                    for key in [
                        "black",
                        "exposure",
                        "deflicker_percentile",
                        "deflicker_target_level",
                    ] {
                        if params.get(key).is_some_and(|value| !value.is_number()) {
                            return Err(malformed(
                                module,
                                format!("stack entry {index} `{key}` is not a number"),
                            ));
                        }
                    }
                    for key in ["compensate_exposure_bias", "compensate_hilite_pres"] {
                        if params.get(key).is_some_and(|value| !value.is_boolean()) {
                            return Err(malformed(
                                module,
                                format!("stack entry {index} `{key}` is not a boolean"),
                            ));
                        }
                    }
                    if params.get("mode").is_some_and(|value| !value.is_string()) {
                        return Err(malformed(
                            module,
                            format!("stack entry {index} `mode` is not a string"),
                        ));
                    }
                }
                if entry["operation"] == "temperature"
                    && (params.contains_key("temperatureKelvin")
                        || params.contains_key("tintMilli"))
                {
                    reject_unknown_keys(
                        module,
                        params,
                        "retained white balance",
                        &["temperatureKelvin", "tintMilli"],
                    )?;
                    for (key, minimum, maximum) in [
                        ("temperatureKelvin", 1000, 40000),
                        ("tintMilli", -150000, 150000),
                    ] {
                        let value =
                            required(module, params, "retained white balance", key)?.as_i64();
                        if value.is_none_or(|value| !(minimum..=maximum).contains(&value)) {
                            return Err(malformed(
                                module,
                                format!(
                                    "retained white balance `{key}` must be an integer in {minimum}..={maximum}"
                                ),
                            ));
                        }
                    }
                }
            }
        }
    } else {
        let mut allowed = SPEKTRAFILM_GROUPS.to_vec();
        allowed.push("output");
        reject_unknown_keys(module, object, "the parameter tree", &allowed)?;
        for group in SPEKTRAFILM_GROUPS {
            if !required(module, object, "the parameter tree", group)?.is_object() {
                return Err(malformed(module, format!("`{group}` is not an object")));
            }
        }
    }
    if let Some(output) = object.get("output") {
        let fields = output
            .as_object()
            .ok_or_else(|| malformed(module, "`output` is not an object".into()))?;
        reject_unknown_keys(
            module,
            fields,
            "`output`",
            &[
                "format",
                "precisionBits",
                "colorSpace",
                "transferFunction",
                "geometry",
                "encoding",
            ],
        )?;
        for key in ["format", "colorSpace", "transferFunction"] {
            if !required(module, fields, "`output`", key)?.is_string() {
                return Err(malformed(module, format!("`output.{key}` is not a string")));
            }
        }
        if required(module, fields, "`output`", "precisionBits")?
            .as_u64()
            .is_none()
        {
            return Err(malformed(
                module,
                "`output.precisionBits` is not a nonnegative integer".into(),
            ));
        }
        for key in ["geometry", "encoding"] {
            optional_identifier(module, fields, key)?;
        }
    }
    Ok(())
}

// JSON clients may serialize integral float controls as integers. Compare
// their numerical value while preserving every key, array element and value.
fn same_film_value(value: &Value, pinned: &Value) -> bool {
    match (value, pinned) {
        (Value::Number(value), Value::Number(pinned)) => value.as_f64() == pinned.as_f64(),
        (Value::Array(value), Value::Array(pinned)) => {
            value.len() == pinned.len()
                && value
                    .iter()
                    .zip(pinned)
                    .all(|(value, pinned)| same_film_value(value, pinned))
        }
        (Value::Object(value), Value::Object(pinned)) => {
            value.len() == pinned.len()
                && value.iter().all(|(key, value)| {
                    pinned
                        .get(key)
                        .is_some_and(|pinned| same_film_value(value, pinned))
                })
        }
        _ => value == pinned,
    }
}

/// Validate the module-owned tree of `spektrafilm-params-1`. The
/// standalone runtime's fixed recipe executes only its pinned complete
/// group tree, so every runtime group must be present and exactly the
/// pinned value: an absent group is a malformed tree, and a well-formed
/// group carrying any other value is an unsupported control, never a
/// value the runtime would reinterpret. `output`, when present, must be
/// the pinned finished JPEG. No darktable stack entry is ever admitted
/// or inserted to emulate a group.
pub(super) fn validate_spektrafilm_tree(module: &str, tree: &Value) -> Result<(), ModuleError> {
    let object = tree
        .as_object()
        .ok_or_else(|| malformed(module, "the parameter tree is not an object".into()))?;
    let mut allowed: Vec<&str> = SPEKTRAFILM_GROUPS.to_vec();
    allowed.push("output");
    reject_unknown_keys(module, object, "the parameter tree", &allowed)?;
    let pinned = spektrafilm_default_tree();
    for group in SPEKTRAFILM_GROUPS {
        match object.get(group) {
            None => {
                return Err(malformed(
                    module,
                    format!(
                        "the parameter tree is missing the `{group}` group of the pinned fixed recipe"
                    ),
                ));
            }
            Some(value) if !value.is_object() => {
                return Err(malformed(module, format!("`{group}` is not an object")));
            }
            Some(value)
                if !pinned
                    .get(group)
                    .is_some_and(|pinned| same_film_value(value, pinned)) =>
            {
                return Err(unsupported(
                    module,
                    format!(
                        "`{group}` is not the pinned fixed recipe the standalone runtime executes"
                    ),
                ));
            }
            _ => {}
        }
    }
    if let Some(output) = object.get("output") {
        validate_spektrafilm_output(module, output)?;
    }
    Ok(())
}

/// The pinned finished JPEG of the standalone SpektraFilm adapter: an
/// input-preserving sRGB JPEG at the fixed quality-85 baseline encoding.
fn validate_spektrafilm_output(module: &str, output: &Value) -> Result<(), ModuleError> {
    let object = output
        .as_object()
        .ok_or_else(|| malformed(module, "`output` is not an object".into()))?;
    reject_unknown_keys(
        module,
        object,
        "`output`",
        &[
            "format",
            "precisionBits",
            "colorSpace",
            "transferFunction",
            "geometry",
            "encoding",
        ],
    )?;
    pinned_string(module, object, "format", SPEKTRAFILM_OUTPUT_FORMAT)?;
    pinned_number(
        module,
        object,
        "precisionBits",
        SPEKTRAFILM_OUTPUT_PRECISION_BITS,
    )?;
    pinned_string(module, object, "colorSpace", SPEKTRAFILM_OUTPUT_COLOR_SPACE)?;
    pinned_string(
        module,
        object,
        "transferFunction",
        SPEKTRAFILM_OUTPUT_TRANSFER,
    )?;
    optional_pinned_string(module, object, "geometry", SPEKTRAFILM_OUTPUT_GEOMETRY)?;
    optional_pinned_string(module, object, "encoding", SPEKTRAFILM_OUTPUT_ENCODING)?;
    Ok(())
}
