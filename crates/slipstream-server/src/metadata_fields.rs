//! Typed metadata field precedence and checked XMP changes.
use crate::metadata_wire::{
    MetadataCaptureFact, MetadataChange, MetadataError, MetadataErrorCode, MetadataField,
    MetadataFieldState, MetadataProvenance, MetadataSourceValue, MetadataValue,
};
use slipstream_core::metadata::{
    embedded::{
        CaptureField, EmbeddedMetadata, FieldState as EmbeddedState, IimField, PacketState,
    },
    xmp::{FieldPatch, FieldState, PatchValue, XmpDocument, XmpParseError},
};
use std::collections::BTreeMap;

pub(crate) struct XmpSource {
    pub document: Option<XmpDocument>,
    pub state: MetadataFieldState,
    pub problem: Option<String>,
}

pub(crate) fn xmp_source(bytes: Option<&[u8]>) -> XmpSource {
    match bytes {
        None => XmpSource {
            document: None,
            state: MetadataFieldState::Absent,
            problem: None,
        },
        Some(bytes) => match XmpDocument::parse_for_read(bytes) {
            Ok(document) => {
                let problem = document.preservation_error().map(|error| error.to_string());
                XmpSource {
                    document: Some(document),
                    state: MetadataFieldState::Present,
                    problem,
                }
            }
            Err(error) => XmpSource {
                document: None,
                state: if error == XmpParseError::ResourceLimit {
                    MetadataFieldState::ResourceLimit
                } else {
                    MetadataFieldState::Invalid
                },
                problem: Some(error.to_string()),
            },
        },
    }
}

fn embedded_state(state: EmbeddedState) -> MetadataFieldState {
    match state {
        EmbeddedState::Present => MetadataFieldState::Present,
        EmbeddedState::Absent => MetadataFieldState::Absent,
        EmbeddedState::Invalid => MetadataFieldState::Invalid,
        EmbeddedState::Unavailable => MetadataFieldState::Unavailable,
        EmbeddedState::ResourceLimit => MetadataFieldState::ResourceLimit,
    }
}

fn problem(state: MetadataFieldState) -> Option<String> {
    match state {
        MetadataFieldState::Invalid => Some("Invalid metadata value".into()),
        MetadataFieldState::Unavailable => Some("Metadata source unavailable".into()),
        MetadataFieldState::ResourceLimit => Some("Metadata resource limit".into()),
        _ => None,
    }
}

fn typed<T>(
    value: FieldState<T>,
    convert: impl FnOnce(T) -> MetadataValue,
) -> (MetadataFieldState, Option<MetadataValue>) {
    match value {
        FieldState::Present(value) => (MetadataFieldState::Present, Some(convert(value))),
        FieldState::Absent => (MetadataFieldState::Absent, None),
        FieldState::Invalid => (MetadataFieldState::Invalid, None),
    }
}

pub(crate) fn xmp_value(
    document: &XmpDocument,
    name: &str,
) -> (MetadataFieldState, Option<MetadataValue>) {
    match name {
        "dc:title" => typed(document.title(), MetadataValue::Languages),
        "dc:description" => typed(document.description(), MetadataValue::Languages),
        "photoshop:Headline" => typed(document.headline(), MetadataValue::Text),
        "dc:subject" => typed(document.keywords(), MetadataValue::List),
        "xmp:Label" => typed(document.label(), MetadataValue::Text),
        "xmp:Rating" => typed(document.rating(), |value| {
            MetadataValue::Number(
                serde_json::Number::from_f64(value).expect("core validates finite ratings"),
            )
        }),
        "dc:creator" => typed(document.creators(), MetadataValue::List),
        "photoshop:AuthorsPosition" => typed(document.creators_position(), MetadataValue::Text),
        "photoshop:Credit" => typed(document.credit(), MetadataValue::Text),
        "photoshop:Source" => typed(document.source(), MetadataValue::Text),
        "dc:rights" => typed(document.rights(), MetadataValue::Languages),
        "xmpRights:UsageTerms" => typed(document.usage_terms(), MetadataValue::Languages),
        "xmpRights:Marked" => typed(document.marked(), MetadataValue::Boolean),
        "xmpRights:WebStatement" => typed(document.web_statement(), MetadataValue::Text),
        _ => unreachable!("supported descriptive property"),
    }
}

const FIELDS: [&str; 14] = [
    "dc:title",
    "dc:description",
    "photoshop:Headline",
    "dc:subject",
    "xmp:Label",
    "xmp:Rating",
    "dc:creator",
    "photoshop:AuthorsPosition",
    "photoshop:Credit",
    "photoshop:Source",
    "dc:rights",
    "xmpRights:UsageTerms",
    "xmpRights:Marked",
    "xmpRights:WebStatement",
];

fn source_value(
    source: &XmpSource,
    name: &str,
    provenance: MetadataProvenance,
) -> MetadataSourceValue {
    let (state, value) = source
        .document
        .as_ref()
        .map_or((source.state, None), |document| xmp_value(document, name));
    MetadataSourceValue {
        state,
        provenance,
        value,
        problem: source.problem.clone().or_else(|| problem(state)),
        language_alternatives_available: if matches!(
            name,
            "dc:title" | "dc:description" | "dc:rights" | "xmpRights:UsageTerms"
        ) {
            Some(true)
        } else {
            None
        },
    }
}

fn iim_value(embedded: &EmbeddedMetadata, name: &str) -> Option<MetadataSourceValue> {
    let field: &IimField = match name {
        "dc:title" => &embedded.iim.title,
        "dc:description" => &embedded.iim.description,
        "photoshop:Headline" => &embedded.iim.headline,
        "dc:subject" => &embedded.iim.keywords,
        "dc:creator" => &embedded.iim.creators,
        "photoshop:AuthorsPosition" => &embedded.iim.creator_job_title,
        "photoshop:Credit" => &embedded.iim.credit,
        "photoshop:Source" => &embedded.iim.source,
        "dc:rights" => &embedded.iim.copyright_notice,
        _ => return None,
    };
    let state = embedded_state(
        if matches!(
            embedded.iim.state,
            EmbeddedState::Invalid | EmbeddedState::Unavailable | EmbeddedState::ResourceLimit
        ) {
            embedded.iim.state
        } else {
            field.state
        },
    );
    let value = (state == MetadataFieldState::Present).then(|| {
        if matches!(name, "dc:subject" | "dc:creator") {
            MetadataValue::List(field.values.clone())
        } else {
            MetadataValue::Text(field.values.first().cloned().unwrap_or_default())
        }
    });
    Some(MetadataSourceValue {
        state,
        provenance: MetadataProvenance::IptcIim,
        value,
        problem: problem(state),
        language_alternatives_available: Some(false),
    })
}

pub(crate) fn read_fields(
    sidecar: &XmpSource,
    embedded: &EmbeddedMetadata,
) -> BTreeMap<String, MetadataField> {
    let embedded_xmp = match &embedded.xmp_packet.state {
        PacketState::Present(bytes) => xmp_source(Some(bytes)),
        PacketState::Absent => xmp_source(None),
        state => {
            let state = match state {
                PacketState::Invalid => MetadataFieldState::Invalid,
                PacketState::ResourceLimit => MetadataFieldState::ResourceLimit,
                _ => MetadataFieldState::Unavailable,
            };
            XmpSource {
                document: None,
                state,
                problem: problem(state),
            }
        }
    };
    let mut fields = BTreeMap::new();
    for name in FIELDS {
        let mut sources = vec![
            source_value(sidecar, name, MetadataProvenance::Sidecar),
            source_value(&embedded_xmp, name, MetadataProvenance::EmbeddedXmp),
        ];
        if let Some(source) = iim_value(embedded, name) {
            sources.push(source);
        }
        let effective = sources
            .iter()
            .find(|source| source.state != MetadataFieldState::Absent);
        let state = effective.map_or(MetadataFieldState::Absent, |source| source.state);
        fields.insert(
            name.into(),
            MetadataField {
                state,
                writable: true,
                provenance: effective.map(|source| source.provenance),
                value: effective.and_then(|source| source.value.clone()),
                problem: effective.and_then(|source| source.problem.clone()),
                inferred_value: (name == "xmp:Rating" && state == MetadataFieldState::Absent)
                    .then(|| MetadataValue::Number(0.into())),
                sources,
            },
        );
    }
    let capture_values = sidecar.document.as_ref().map_or_else(
        || {
            [
                "exif:DateTimeOriginal",
                "exif:SubSecTimeOriginal",
                "exif:OffsetTimeOriginal",
                "tiff:Make",
                "tiff:Model",
                "exif:LensModel",
                "exif:PixelXDimension",
                "exif:PixelYDimension",
                "tiff:Orientation",
                "exif:ExposureTime",
                "exif:FNumber",
                "exif:ISOSpeedRatings",
                "exif:FocalLength",
            ]
            .into_iter()
            .map(|name| (name, FieldState::Absent))
            .collect()
        },
        XmpDocument::capture_representations,
    );
    for (name, value) in capture_values {
        let (state, value) = if sidecar.document.is_some() {
            typed(value, MetadataValue::Text)
        } else {
            (sidecar.state, None)
        };
        let problem = sidecar.problem.clone().or_else(|| problem(state));
        fields.insert(
            name.into(),
            MetadataField {
                state,
                writable: false,
                provenance: Some(MetadataProvenance::Sidecar),
                value: value.clone(),
                problem: problem.clone(),
                inferred_value: None,
                sources: vec![MetadataSourceValue {
                    state,
                    provenance: MetadataProvenance::Sidecar,
                    value,
                    problem,
                    language_alternatives_available: None,
                }],
            },
        );
    }
    fields
}

fn capture<T>(
    field: &CaptureField<T>,
    unit: &str,
    convert: impl FnOnce(&T) -> MetadataValue,
) -> MetadataCaptureFact {
    let state = embedded_state(field.state);
    MetadataCaptureFact {
        state,
        identifier: field.exif_identifier.into(),
        unit: unit.into(),
        provenance: MetadataProvenance::Original,
        writable: false,
        value: field.value.as_ref().map(convert),
        problem: problem(state),
    }
}

pub(crate) fn capture_fields(embedded: &EmbeddedMetadata) -> BTreeMap<String, MetadataCaptureFact> {
    let facts = &embedded.capture;
    let text = |value: &String| MetadataValue::Text(value.clone());
    let number = |value: &u32| MetadataValue::Number((*value).into());
    BTreeMap::from([
        (
            "captureTime".into(),
            capture(&facts.capture_time, "local datetime", text),
        ),
        (
            "captureSubseconds".into(),
            capture(&facts.capture_subseconds, "fractional second digits", text),
        ),
        (
            "captureOffset".into(),
            capture(&facts.capture_offset, "UTC offset", text),
        ),
        (
            "cameraMake".into(),
            capture(&facts.camera_make, "text", text),
        ),
        (
            "cameraModel".into(),
            capture(&facts.camera_model, "text", text),
        ),
        ("lensModel".into(), capture(&facts.lens_model, "text", text)),
        (
            "imageWidth".into(),
            capture(&facts.image_width, "pixels", number),
        ),
        (
            "imageHeight".into(),
            capture(&facts.image_height, "pixels", number),
        ),
        (
            "orientation".into(),
            capture(&facts.orientation, "EXIF orientation code", |value| {
                MetadataValue::Number((*value).into())
            }),
        ),
        (
            "exposureTime".into(),
            capture(&facts.exposure_time, "seconds", text),
        ),
        (
            "aperture".into(),
            capture(&facts.aperture, "f-number", text),
        ),
        ("iso".into(), capture(&facts.iso, "ISO", number)),
        (
            "focalLength".into(),
            capture(&facts.focal_length, "mm", text),
        ),
    ])
}

fn invalid(fields: Vec<String>, message: &str) -> MetadataError {
    MetadataError {
        code: MetadataErrorCode::InvalidInput,
        message: message.into(),
        details: serde_json::json!({"fields": fields, "languages": {}}),
    }
}

fn patch<T>(
    change: &MetadataChange,
    convert: impl FnOnce(&MetadataValue) -> Option<T>,
    clear: bool,
) -> Option<PatchValue<T>> {
    match change {
        MetadataChange::Set { value } => convert(value).map(PatchValue::Set),
        MetadataChange::Clear if clear => Some(PatchValue::Clear),
        MetadataChange::Remove => Some(PatchValue::Remove),
        _ => None,
    }
}

fn languages(change: &MetadataChange) -> Option<PatchValue<BTreeMap<String, String>>> {
    match change {
        MetadataChange::SetLanguages { languages } => Some(PatchValue::SetLanguages {
            sets: languages
                .iter()
                .filter_map(|(key, value)| value.as_ref().map(|value| (key.clone(), value.clone())))
                .collect(),
            removes: languages
                .iter()
                .filter(|(_, value)| value.is_none())
                .map(|(key, _)| key.clone())
                .collect(),
        }),
        _ => patch(
            change,
            |value| {
                if let MetadataValue::Languages(value) = value {
                    Some(value.clone())
                } else {
                    None
                }
            },
            true,
        ),
    }
}

pub(crate) fn apply_changes(
    document: &mut XmpDocument,
    changes: &BTreeMap<String, MetadataChange>,
) -> Result<(), MetadataError> {
    let unsupported: Vec<_> = changes
        .keys()
        .filter(|name| !FIELDS.contains(&name.as_str()))
        .cloned()
        .collect();
    if !unsupported.is_empty() {
        return Err(MetadataError {
            code: MetadataErrorCode::UnsupportedField,
            message: "Unsupported or read-only metadata fields".into(),
            details: serde_json::json!({"fields": unsupported, "languages": {}}),
        });
    }
    if let Some(error) = document.preservation_error() {
        return Err(MetadataError {
            code: MetadataErrorCode::MetadataMalformed,
            message: error.to_string(),
            details: serde_json::json!({"fields": [], "languages": {}}),
        });
    }
    let mut patches = Vec::with_capacity(changes.len());
    let mut invalid_fields = Vec::new();
    for (name, change) in changes {
        let text = || {
            patch(
                change,
                |value| {
                    if let MetadataValue::Text(value) = value {
                        Some(value.clone())
                    } else {
                        None
                    }
                },
                true,
            )
        };
        let list = || {
            patch(
                change,
                |value| {
                    if let MetadataValue::List(value) = value {
                        Some(value.clone())
                    } else {
                        None
                    }
                },
                true,
            )
        };
        let candidate = match name.as_str() {
            "dc:title" => languages(change).map(FieldPatch::Title),
            "dc:description" => languages(change).map(FieldPatch::Description),
            "photoshop:Headline" => text().map(FieldPatch::Headline),
            "dc:subject" => list().map(FieldPatch::Keywords),
            "xmp:Label" => text().map(FieldPatch::Label),
            "xmp:Rating" => patch(
                change,
                |value| {
                    if let MetadataValue::Number(value) = value {
                        value.as_f64()
                    } else {
                        None
                    }
                },
                false,
            )
            .map(FieldPatch::Rating),
            "dc:creator" => list().map(FieldPatch::Creators),
            "photoshop:AuthorsPosition" => text().map(FieldPatch::CreatorsPosition),
            "photoshop:Credit" => text().map(FieldPatch::Credit),
            "photoshop:Source" => text().map(FieldPatch::Source),
            "dc:rights" => languages(change).map(FieldPatch::Rights),
            "xmpRights:UsageTerms" => languages(change).map(FieldPatch::UsageTerms),
            "xmpRights:Marked" => patch(
                change,
                |value| {
                    if let MetadataValue::Boolean(value) = value {
                        Some(*value)
                    } else {
                        None
                    }
                },
                false,
            )
            .map(FieldPatch::Marked),
            "xmpRights:WebStatement" => text().map(FieldPatch::WebStatement),
            _ => unreachable!("validated field names"),
        };
        match candidate {
            Some(candidate) => patches.push(candidate),
            None => invalid_fields.push(name.clone()),
        }
    }
    if !invalid_fields.is_empty() {
        return Err(invalid(invalid_fields, "Invalid metadata change shape"));
    }
    document.apply(&patches).map_err(|refusal| MetadataError {
        code: MetadataErrorCode::InvalidInput,
        message: refusal.to_string(),
        details: serde_json::json!({"fields": refusal.fields, "languages": refusal.languages}),
    })
}

pub(crate) fn empty_document() -> XmpDocument {
    XmpDocument::parse(b"<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\"/></rdf:RDF>").expect("static empty XMP document is valid")
}
