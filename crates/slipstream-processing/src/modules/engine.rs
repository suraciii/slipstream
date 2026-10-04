use super::*;
use crate::photo_profile::{APPROVED_EXPOSURE_MILLI_EV_MAX, APPROVED_EXPOSURE_MILLI_EV_MIN};

const QUALIFIED_EXPOSURE_EV_MIN: f64 = APPROVED_EXPOSURE_MILLI_EV_MIN as f64 / 1000.0;
const QUALIFIED_EXPOSURE_EV_MAX: f64 = APPROVED_EXPOSURE_MILLI_EV_MAX as f64 / 1000.0;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EngineRefusal {
    pub code: String,
    pub reason: String,
    pub recovery_action: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EngineQualification {
    pub adapter_version: String,
    pub schema_version: String,
    pub bundle_id: Option<String>,
    pub qualification_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EngineControlDescription {
    pub name: String,
    pub meaning: String,
    pub unit: Option<String>,
    pub schema: Value,
    pub default: Value,
    pub reset: Value,
    pub source_support: Vec<String>,
    pub readable: bool,
    pub editable: bool,
    pub executable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<EngineRefusal>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EngineModuleDescription {
    pub name: String,
    pub meaning: String,
    pub controls: Vec<EngineControlDescription>,
    pub ordering: Value,
    pub interaction: Value,
    pub qualification: EngineQualification,
    pub source_support: Vec<String>,
    pub executable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refusal: Option<EngineRefusal>,
}

pub fn apply_darktable_control(
    tree: Option<&Value>,
    control: &str,
    value: &Value,
) -> Result<Value, ModuleError> {
    if control != "darktable.exposure.ev" {
        return Err(unsupported(
            DARKTABLE_MODULE,
            format!("control `{control}` is not qualified"),
        ));
    }
    let ev = value.as_f64().ok_or_else(|| {
        malformed(
            DARKTABLE_MODULE,
            "darktable.exposure.ev must be a JSON number".into(),
        )
    })?;
    if !ev.is_finite() {
        return Err(malformed(
            DARKTABLE_MODULE,
            "darktable.exposure.ev must be finite".into(),
        ));
    }
    if !(QUALIFIED_EXPOSURE_EV_MIN..=QUALIFIED_EXPOSURE_EV_MAX).contains(&ev) {
        return Err(unsupported(
            DARKTABLE_MODULE,
            "darktable.exposure.ev is outside the qualified range 0..=1".into(),
        ));
    }
    let mut result = tree.cloned().unwrap_or_else(|| {
        darktable_description(ModuleAvailability::ready()).parameter_schema["default"].clone()
    });
    validate_saved_tree(DARKTABLE_MODULE, &result)?;
    let stack = result
        .get_mut("stack")
        .and_then(Value::as_array_mut)
        .ok_or_else(|| malformed(DARKTABLE_MODULE, "darktable default has no stack".into()))?;
    let exposure = stack
        .iter_mut()
        .find(|entry| entry.get("operation") == Some(&Value::String("exposure".into())))
        .ok_or_else(|| unsupported(DARKTABLE_MODULE, "exposure module is not present".into()))?;
    exposure
        .get_mut("params")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| malformed(DARKTABLE_MODULE, "exposure module has no params".into()))?
        .insert("exposure".into(), json!(ev));
    validate_saved_tree(DARKTABLE_MODULE, &result)?;
    Ok(result)
}

pub fn reset_darktable_control(tree: Option<&Value>, control: &str) -> Result<Value, ModuleError> {
    apply_darktable_control(tree, control, &json!(0.0))
}

fn qualification(qualification_id: &str, schema_version: &str) -> EngineQualification {
    EngineQualification {
        adapter_version: DARKTABLE_ADAPTER_VERSION.into(),
        schema_version: schema_version.into(),
        bundle_id: None,
        qualification_id: qualification_id.into(),
    }
}

fn refusal(code: &str, reason: &str, recovery_action: &str) -> EngineRefusal {
    EngineRefusal {
        code: code.into(),
        reason: reason.into(),
        recovery_action: recovery_action.into(),
    }
}

pub(super) fn darktable_engine_modules() -> Vec<EngineModuleDescription> {
    vec![
        EngineModuleDescription {
            name: "exposure".into(),
            meaning: "Manual exposure compensation in EV".into(),
            controls: vec![EngineControlDescription {
                name: "ev".into(),
                meaning: "Exposure compensation relative to the qualified baseline".into(),
                unit: Some("EV".into()),
                schema: json!({"type": "number", "minimum": QUALIFIED_EXPOSURE_EV_MIN, "maximum": QUALIFIED_EXPOSURE_EV_MAX, "finite": true}),
                default: json!(0.0),
                reset: json!(0.0),
                source_support: vec!["sony-arw".into()],
                readable: true,
                editable: true,
                executable: true,
                refusal_reason: None,
                refusal: None,
            }],
            ordering: json!({"before": [], "after": []}),
            interaction: json!({"preserves": ["qualified-baseline"]}),
            qualification: qualification("darktable.exposure.ev-v1", DARKTABLE_PARAMETER_VERSION),
            source_support: vec!["sony-arw".into()],
            executable: true,
            refusal_reason: None,
            refusal: None,
        },
        EngineModuleDescription {
            name: "white-balance".into(),
            meaning: "Camera white-balance interpretation".into(),
            controls: Vec::new(),
            ordering: json!({"before": [], "after": []}),
            interaction: json!({"requires": ["camera-metadata"]}),
            qualification: qualification("darktable.white-balance-v0", DARKTABLE_PARAMETER_VERSION),
            source_support: vec!["sony-arw".into()],
            executable: false,
            refusal_reason: Some("no stateful white-balance mapping is qualified".into()),
            refusal: Some(refusal(
                "not-qualified",
                "No stateful white-balance mapping is qualified",
                "Use retained complete-recipe intent or wait for a qualified mapping",
            )),
        },
    ]
}

pub(super) fn spektrafilm_engine_modules() -> Vec<EngineModuleDescription> {
    vec![EngineModuleDescription {
        name: "fixed-recipe".into(),
        meaning: "The pinned standalone SpektraFilm recipe".into(),
        controls: Vec::new(),
        ordering: json!({"before": [], "after": []}),
        interaction: json!({"requires": ["explicit-artifact-input"]}),
        qualification: EngineQualification {
            adapter_version: SPEKTRAFILM_ADAPTER_VERSION.into(),
            schema_version: SPEKTRAFILM_PARAMETER_VERSION.into(),
            bundle_id: None,
            qualification_id: "spektrafilm.fixed-recipe-v1".into(),
        },
        source_support: vec!["linear-prophoto-tiff".into()],
        executable: false,
        refusal_reason: Some(
            "SpektraFilm is a fixed recipe; no stateful controls are qualified".into(),
        ),
        refusal: Some(refusal(
            "fixed-recipe",
            "SpektraFilm exposes no mutable stateful controls",
            "Use the complete Processing Recipe surface",
        )),
    }]
}
