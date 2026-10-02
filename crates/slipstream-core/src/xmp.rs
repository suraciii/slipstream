use crate::{ComposableEditRecipe, EditRecipeSettings, WhiteBalanceIntent};
use sha2::{Digest, Sha256};

pub const XMP_RETENTION_SECONDS: i64 = 7 * 24 * 60 * 60;
pub const XMP_CONTENT_TYPE: &str = "application/rdf+xml";

#[derive(Clone, Debug, PartialEq)]
pub struct XmpExportRecord {
    pub export_id: String,
    pub photo_id: String,
    pub recipe_version: String,
    pub source_revision: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub byte_length: usize,
    pub sha256: String,
    pub filename: String,
    pub document: Vec<u8>,
    pub exposure_ev: f64,
    pub white_balance: WhiteBalanceIntent,
}
#[derive(Clone, Debug, PartialEq)]
pub enum XmpCreateOutcome {
    Created(XmpExportRecord),
    Replay(XmpExportRecord),
    Conflict,
    Stale,
    Expired,
    NotFound,
    MissingRecipe,
    MissingStep,
    UnsupportedModule,
    UnsupportedParameters,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SemanticSettingsError {
    MissingStep,
    UnsupportedModule,
    UnsupportedParameters,
}

impl From<SemanticSettingsError> for XmpCreateOutcome {
    fn from(error: SemanticSettingsError) -> Self {
        match error {
            SemanticSettingsError::MissingStep => Self::MissingStep,
            SemanticSettingsError::UnsupportedModule => Self::UnsupportedModule,
            SemanticSettingsError::UnsupportedParameters => Self::UnsupportedParameters,
        }
    }
}
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// XMP carries only the unambiguous semantic exposure and white-balance
/// subset. Other darktable controls have no portable XMP representation.
pub(crate) fn semantic_settings(
    recipe: &ComposableEditRecipe,
) -> Result<EditRecipeSettings, SemanticSettingsError> {
    let step = recipe
        .current_step()
        .ok_or(SemanticSettingsError::MissingStep)?;
    if step.module.as_str() != "darktable" {
        return Err(SemanticSettingsError::UnsupportedModule);
    }
    if step.parameters.schema_version != "darktable-params-1" {
        return Err(SemanticSettingsError::UnsupportedParameters);
    }
    let mut settings = EditRecipeSettings {
        exposure_ev: 0.0,
        white_balance: WhiteBalanceIntent::AsShot,
    };
    let mut exposure_seen = false;
    let mut balance_seen = false;
    if let Some(stack) = step.parameters.tree.get("stack") {
        let stack = stack
            .as_array()
            .ok_or(SemanticSettingsError::UnsupportedParameters)?;
        for entry in stack {
            if entry.get("enabled").and_then(serde_json::Value::as_bool) == Some(false) {
                continue;
            }
            let params = &entry["params"];
            match entry["operation"].as_str() {
                Some("exposure") => {
                    if exposure_seen || params["mode"] != "EXPOSURE_MODE_MANUAL" {
                        return Err(SemanticSettingsError::UnsupportedParameters);
                    }
                    settings.exposure_ev = params["exposure"]
                        .as_f64()
                        .filter(|value| value.is_finite())
                        .ok_or(SemanticSettingsError::UnsupportedParameters)?;
                    exposure_seen = true;
                }
                Some("temperature") => {
                    if balance_seen {
                        return Err(SemanticSettingsError::UnsupportedParameters);
                    }
                    let integer = |key: &str| {
                        params[key]
                            .as_i64()
                            .and_then(|value| i32::try_from(value).ok())
                            .ok_or(SemanticSettingsError::UnsupportedParameters)
                    };
                    settings.white_balance = WhiteBalanceIntent::TemperatureTint {
                        temperature_kelvin: integer("temperatureKelvin")?,
                        tint_milli: integer("tintMilli")?,
                    };
                    if !settings.white_balance.within_payload_bounds() {
                        return Err(SemanticSettingsError::UnsupportedParameters);
                    }
                    balance_seen = true;
                }
                _ => {}
            }
        }
    }
    Ok(settings)
}

pub(crate) fn document(recipe: &ComposableEditRecipe, settings: &EditRecipeSettings) -> Vec<u8> {
    let step = recipe.current_step().expect("validated selected XMP step");
    let wb = match settings.white_balance {
        WhiteBalanceIntent::AsShot => "<crs:WhiteBalance>As Shot</crs:WhiteBalance>".to_owned(),
        WhiteBalanceIntent::TemperatureTint {
            temperature_kelvin,
            tint_milli,
        } => format!(
            "<slip:WhiteBalance>Custom</slip:WhiteBalance><slip:TemperatureKelvin>{temperature_kelvin}</slip:TemperatureKelvin><slip:TintMilli>{tint_milli}</slip:TintMilli>"
        ),
    };
    // Source revisions contain NUL separators. Encode every UTF-8 byte so the
    // opaque revision round trips without introducing forbidden XML characters.
    let mut source = String::with_capacity(recipe.source_revision.len() * 2);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in recipe.source_revision.bytes() {
        source.push(char::from(HEX[usize::from(byte >> 4)]));
        source.push(char::from(HEX[usize::from(byte & 15)]));
    }
    let snapshot = escape(&crate::persistence::serialize_recipe_snapshot(recipe));
    format!(
        "<?xpacket begin=\"﻿\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?><x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\" xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" xmlns:slip=\"https://slipstream.app/ns/edit-state/1.0/\"><crs:Exposure2012>{}</crs:Exposure2012>{wb}<slip:PhotoId>{}</slip:PhotoId><slip:RecipeVersion>{}</slip:RecipeVersion><slip:SourceRevision>{source}</slip:SourceRevision><slip:SourceRevisionEncoding>hex-utf8</slip:SourceRevisionEncoding><slip:StepId>{}</slip:StepId><slip:Module>darktable</slip:Module><slip:ParameterSchemaVersion>{}</slip:ParameterSchemaVersion><slip:RecipeSnapshot>{snapshot}</slip:RecipeSnapshot><slip:RecipeSnapshotEncoding>json-utf8</slip:RecipeSnapshotEncoding><slip:UnsupportedControls>Arbitrary darktable controls</slip:UnsupportedControls></rdf:Description></rdf:RDF></x:xmpmeta><?xpacket end=\"w\"?>",
        settings.exposure_ev, escape(&recipe.photo_id), escape(&recipe.revision),
        escape(step.step_id.as_str()), escape(&step.parameters.schema_version)
    ).into_bytes()
}

pub fn digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn recipe(tree: serde_json::Value) -> ComposableEditRecipe {
        let step_id = crate::ProcessingStepId::new("selected").unwrap();
        ComposableEditRecipe {
            photo_id: "photo<&".into(),
            revision: "recipe-1".into(),
            source_revision: "_C2_1520.ARW\u{0}70242304\u{0}1790765898469.3801".into(),
            steps: vec![crate::ProcessingStep {
                step_id: step_id.clone(),
                module: crate::ProcessingModuleId::new("darktable").unwrap(),
                input: crate::ProcessingInput::Original {
                    photo_id: "photo<&".into(),
                    source_revision: "source".into(),
                },
                parameters: crate::ProcessingParameterSnapshot::new("darktable-params-1", tree)
                    .unwrap(),
            }],
            current_step_id: Some(step_id),
        }
    }

    #[test]
    fn semantic_export_uses_only_selected_step_and_refuses_ambiguous_intent() {
        let mut saved = recipe(serde_json::json!({"stack": [
            {"operation":"exposure","enabled":true,"params":{"mode":"EXPOSURE_MODE_MANUAL","exposure":0.5}},
            {"operation":"contrast","enabled":true,"params":{"contrast":0.9}}
        ]}));
        let mut other = saved.steps[0].clone();
        other.step_id = crate::ProcessingStepId::new("other").unwrap();
        other.parameters.tree["stack"][0]["params"]["exposure"] = serde_json::json!(1.5);
        saved.steps.push(other);
        assert_eq!(semantic_settings(&saved).unwrap().exposure_ev, 0.5);
        saved.current_step_id = Some(crate::ProcessingStepId::new("other").unwrap());
        assert_eq!(semantic_settings(&saved).unwrap().exposure_ev, 1.5);
        let duplicate = saved.steps[1].parameters.tree["stack"][0].clone();
        saved.steps[1].parameters.tree["stack"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        assert_eq!(
            semantic_settings(&saved),
            Err(SemanticSettingsError::UnsupportedParameters)
        );
        saved.steps[1].parameters.tree["stack"][2]["enabled"] = serde_json::json!(false);
        assert_eq!(semantic_settings(&saved).unwrap().exposure_ev, 1.5);
        saved.steps[1].parameters.tree["stack"][0]["params"]["mode"] =
            serde_json::json!("automatic");
        assert_eq!(
            semantic_settings(&saved),
            Err(SemanticSettingsError::UnsupportedParameters)
        );
        saved.steps[1].module = crate::ProcessingModuleId::new("spektrafilm").unwrap();
        assert_eq!(
            semantic_settings(&saved),
            Err(SemanticSettingsError::UnsupportedModule)
        );
        saved.steps.clear();
        saved.current_step_id = None;
        assert_eq!(
            semantic_settings(&saved),
            Err(SemanticSettingsError::MissingStep)
        );
    }

    #[test]
    fn xmp_snapshot_round_trips_complete_recipe_and_selected_semantics() {
        let mut saved = recipe(serde_json::json!({"stack": [
            {"operation":"exposure","enabled":true,"params":{"mode":"EXPOSURE_MODE_MANUAL","exposure":0.75,"black":0.02}},
            {"operation":"contrast","enabled":true,"params":{"contrast":0.9}}
        ], "output":{"format":"tiff","precisionBits":32,"geometry":"source-preserving"}}));
        saved.steps[0].input = crate::ProcessingInput::Original {
            photo_id: saved.photo_id.clone(),
            source_revision: saved.source_revision.clone(),
        };
        saved.steps.push(crate::ProcessingStep {
            step_id: crate::ProcessingStepId::new("film").unwrap(),
            module: crate::ProcessingModuleId::new("spektrafilm").unwrap(),
            input: crate::ProcessingInput::Artifact {
                artifact_id: crate::ProcessingArtifactId::new("retained-input").unwrap(),
                contract: crate::ProcessingImageContract {
                    format: "image/tiff".into(), precision: "float32".into(),
                    color_space: "prophoto-rgb".into(), transfer: "linear".into(),
                    geometry: crate::ProcessingGeometry::new(6000, 4000).unwrap(), encoding: "deflate".into(),
                },
            },
            parameters: crate::ProcessingParameterSnapshot::new("spektrafilm-params-1",
                serde_json::json!({"FilmRecipe":{"stock":"film<&","strength":0.65},"output":{"format":"jpeg","quality":91}})).unwrap(),
        });
        saved
            .steps
            .sort_by(|left, right| left.step_id.as_str().cmp(right.step_id.as_str()));
        let settings = semantic_settings(&saved).unwrap();
        assert_eq!(settings.exposure_ev, 0.75);
        let bytes = document(&saved, &settings);
        let text = std::str::from_utf8(&bytes).unwrap();
        assert!(!text.contains('\0'));
        let mut reader = quick_xml::Reader::from_str(text);
        let snapshot = loop {
            match reader.read_event().unwrap() {
                quick_xml::events::Event::Start(start)
                    if start.name().as_ref() == "slip:RecipeSnapshot" =>
                {
                    let encoded = reader.read_text(start.name()).unwrap();
                    break quick_xml::escape::unescape(&encoded).unwrap().into_owned();
                }
                quick_xml::events::Event::Eof => panic!("complete recipe snapshot missing"),
                _ => {}
            }
        };
        let decoded = crate::persistence::deserialize_recipe_snapshot(&snapshot);
        assert_eq!(decoded, saved);
        assert_eq!(semantic_settings(&decoded).unwrap(), settings);
    }

    #[test]
    fn custom_balance_and_opaque_provenance_are_valid_xml() {
        let recipe = recipe(serde_json::json!({"stack": [
            {"operation":"exposure","enabled":true,"params":{"mode":"EXPOSURE_MODE_MANUAL","exposure":0.5}},
            {"operation":"temperature","enabled":true,"params":{"temperatureKelvin":5200,"tintMilli":-250}}
        ]}));
        let settings = semantic_settings(&recipe).unwrap();
        assert_eq!(
            settings.white_balance,
            WhiteBalanceIntent::TemperatureTint {
                temperature_kelvin: 5200,
                tint_milli: -250,
            }
        );
        let bytes = document(&recipe, &settings);
        let mut reader = quick_xml::Reader::from_reader(bytes.as_slice());
        let mut buffer = Vec::new();
        loop {
            match reader.read_event_into(&mut buffer).unwrap() {
                quick_xml::events::Event::Eof => break,
                quick_xml::events::Event::Text(text) => {
                    quick_xml::escape::unescape(
                        text.xml_content(quick_xml::XmlVersion::Implicit1_0)
                            .as_ref(),
                    )
                    .unwrap();
                }
                _ => {}
            }
            buffer.clear();
        }
        let text = String::from_utf8(bytes).unwrap();
        assert!(!text.contains('\0'));
        let encoded = text
            .split("<slip:SourceRevision>")
            .nth(1)
            .unwrap()
            .split("</slip:SourceRevision>")
            .next()
            .unwrap();
        let decoded: Vec<u8> = encoded
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
        assert_eq!(decoded, recipe.source_revision.as_bytes());
        assert!(text.contains("<slip:PhotoId>photo&lt;&amp;</slip:PhotoId>"));
        assert!(text.contains("<slip:StepId>selected</slip:StepId>"));
        assert!(text.contains("<slip:Module>darktable</slip:Module>"));
        assert!(text.contains(
            "<slip:UnsupportedControls>Arbitrary darktable controls</slip:UnsupportedControls>"
        ));
        assert!(!text.contains("FilmRecipe"));
        assert!(text.contains("<slip:TintMilli>-250</slip:TintMilli>"));
        assert!(!text.contains("<crs:Temperature>"));
        assert!(!text.contains("<crs:Tint>"));
        assert!(!text.contains("<crs:WhiteBalance>"));
    }
}
