use crate::http::{HttpState, cli_error_empty};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{Response, StatusCode, header},
};
use quick_xml::{events::Event, name::ResolveResult, reader::NsReader};
use slipstream_core::{XMP_CONTENT_TYPE, XmpExportRecord};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn error(status: StatusCode, code: &'static str, message: &'static str) -> Response<Body> {
    cli_error_empty(status, code, message)
}
fn timestamp(value: i64) -> String {
    OffsetDateTime::from_unix_timestamp(value)
        .expect("persisted XMP timestamp")
        .format(&Rfc3339)
        .expect("RFC3339 timestamp")
}
#[cfg(test)]
#[test]
fn retained_parameter_support_resolves_namespaces_and_ignores_text() {
    let document = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:RDF><rdf:Description xmlns:edit="https://slipstream.app/ns/edit-state/1.0/" xmlns:raw="http://ns.adobe.com/camera-raw-settings/1.0/" raw:WhiteBalance="As Shot"><edit:StepId>saved</edit:StepId><edit:RecipeSnapshot>&lt;slip:Module&gt;darktable&lt;/slip:Module&gt;</edit:RecipeSnapshot><!-- <edit:PhotoId>not a property</edit:PhotoId> --><slip:Module xmlns:slip="https://unrelated.example/">other</slip:Module></rdf:Description></rdf:RDF></x:xmpmeta>"#;
    assert_eq!(
        parameter_support(document),
        (vec!["WhiteBalance"], vec!["StepId", "RecipeSnapshot"]),
    );
}

fn parameter_support(document: &[u8]) -> (Vec<&'static str>, Vec<&'static str>) {
    let mut standard = [("Exposure2012", false), ("WhiteBalance", false)];
    let mut slipstream = [
        ("PhotoId", false),
        ("RecipeVersion", false),
        ("SourceRevision", false),
        ("SourceRevisionEncoding", false),
        ("StepId", false),
        ("Module", false),
        ("ParameterSchemaVersion", false),
        ("RecipeSnapshot", false),
        ("RecipeSnapshotEncoding", false),
        ("WhiteBalance", false),
        ("TemperatureKelvin", false),
        ("TintMilli", false),
    ];
    let mut reader = NsReader::from_reader(document);
    loop {
        match reader.read_event() {
            Ok(Event::Start(element) | Event::Empty(element)) => {
                let (namespace, name) = reader.resolver().resolve_element(element.name());
                mark_property(namespace, name.as_ref(), &mut standard, &mut slipstream);
                for attribute in element.attributes() {
                    let Ok(attribute) = attribute else {
                        return (Vec::new(), Vec::new());
                    };
                    let (namespace, name) = reader.resolver().resolve_attribute(attribute.key);
                    mark_property(namespace, name.as_ref(), &mut standard, &mut slipstream);
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => return (Vec::new(), Vec::new()),
            _ => {}
        }
    }
    (
        standard
            .into_iter()
            .filter_map(|(name, present)| present.then_some(name))
            .collect(),
        slipstream
            .into_iter()
            .filter_map(|(name, present)| present.then_some(name))
            .collect(),
    )
}

fn mark_property(
    namespace: ResolveResult<'_>,
    name: &str,
    standard: &mut [(&'static str, bool)],
    slipstream: &mut [(&'static str, bool)],
) {
    let ResolveResult::Bound(namespace) = namespace else {
        return;
    };
    let properties = match namespace.as_ref() {
        "http://ns.adobe.com/camera-raw-settings/1.0/" => standard,
        "https://slipstream.app/ns/edit-state/1.0/" => slipstream,
        _ => return,
    };
    for (property, present) in properties {
        if *property == name {
            *present = true;
        }
    }
}

fn wire(record: &XmpExportRecord) -> serde_json::Value {
    // Read namespace-qualified properties from the acknowledged bytes; the
    // retained document and its digest remain untouched.
    let (standard, slipstream) = parameter_support(&record.document);
    serde_json::json!({
        "exportId": record.export_id, "photoId": record.photo_id,
        "target": "edit-state-xmp", "state": "succeeded",
        "recipeVersion": record.recipe_version, "sourceRevision": record.source_revision,
        "createdAt": timestamp(record.created_at), "expiresAt": timestamp(record.expires_at),
        "artifact": {"filename": record.filename, "contentType": XMP_CONTENT_TYPE, "byteLength": record.byte_length, "sha256": record.sha256},
        "parameterSupport": {"standard": standard, "slipstream": slipstream, "unsupported": ["Arbitrary darktable controls"]}
    })
}
pub(crate) async fn list(
    State(state): State<HttpState>,
    Path(photo_id): Path<String>,
) -> Response<Body> {
    match state.application.library.photo_xmp_exports(&photo_id).await {
        Ok(Some(records)) => crate::http::json_response(
            StatusCode::OK,
            &serde_json::json!({"exports": records.iter().map(wire).collect::<Vec<_>>()}),
        ),
        Ok(None) => error(
            StatusCode::NOT_FOUND,
            "unknown_photo",
            "The Photo is not part of the published Library",
        ),
        Err(_) => error(
            StatusCode::SERVICE_UNAVAILABLE,
            "resource_unavailable",
            "The XMP export state could not be read",
        ),
    }
}
pub(crate) async fn artifact(
    State(state): State<HttpState>,
    Path((photo_id, export_id)): Path<(String, String)>,
) -> Response<Body> {
    let record = match state.application.library.xmp_export(&export_id).await {
        Ok(Some(record)) if record.photo_id == photo_id => record,
        Ok(_) => {
            return error(
                StatusCode::NOT_FOUND,
                "unknown_export",
                "The XMP export identity is unknown",
            );
        }
        Err(_) => {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "resource_unavailable",
                "The XMP export state could not be read",
            );
        }
    };
    if record.expires_at <= now() {
        return error(
            StatusCode::GONE,
            "export_expired",
            "The XMP export artifact has expired",
        );
    }
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, XMP_CONTENT_TYPE)
        .header(header::CONTENT_LENGTH, record.byte_length.to_string())
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", record.filename),
        )
        .header(header::CACHE_CONTROL, "no-store")
        .header("x-content-type-options", "nosniff")
        .header("slipstream-artifact-export-id", &record.export_id)
        .header("slipstream-artifact-target", "edit-state-xmp")
        .header("slipstream-artifact-content-type", XMP_CONTENT_TYPE)
        .header("slipstream-artifact-filename", &record.filename)
        .header(
            "slipstream-artifact-byte-length",
            record.byte_length.to_string(),
        )
        .header("slipstream-artifact-sha256", &record.sha256)
        .header(
            "slipstream-artifact-created-at",
            timestamp(record.created_at),
        )
        .header(
            "slipstream-artifact-expires-at",
            timestamp(record.expires_at),
        )
        .body(Body::from(record.document))
        .expect("valid XMP artifact response")
}
