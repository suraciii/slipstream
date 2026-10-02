use crate::{EditRecipe, WhiteBalanceIntent};
use sha2::{Digest, Sha256};

pub const XMP_RETENTION_SECONDS: i64 = 7 * 24 * 60 * 60;
pub const XMP_CONTENT_TYPE: &str = "application/rdf+xml";
// Pinned recipe identity from tools/development/film_identity.py.
pub const XMP_FILM_RECIPE_SHA256: &str =
    "8efdd28d3a49fea7e71835dea82dbc416d9f95e4ae4216ae5fb7535087ec5cf8";
pub const XMP_FILM_PROCEDURE: &str = "film-once-empty-cache-v1";

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
}
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub fn document(recipe: &EditRecipe) -> Vec<u8> {
    let wb = match recipe.settings.white_balance {
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
    format!(
        "<?xpacket begin=\"﻿\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?><x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\" xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\" xmlns:slip=\"https://slipstream.app/ns/edit-state/1.0/\"><crs:Exposure2012>{}</crs:Exposure2012>{wb}<slip:PhotoId>{}</slip:PhotoId><slip:RecipeVersion>{}</slip:RecipeVersion><slip:SourceRevision>{source}</slip:SourceRevision><slip:SourceRevisionEncoding>hex-utf8</slip:SourceRevisionEncoding><slip:FilmRecipeSha256>{XMP_FILM_RECIPE_SHA256}</slip:FilmRecipeSha256><slip:FilmProcedure>{XMP_FILM_PROCEDURE}</slip:FilmProcedure></rdf:Description></rdf:RDF></x:xmpmeta><?xpacket end=\"w\"?>",
        recipe.settings.exposure_ev, escape(&recipe.photo_id), escape(&recipe.revision)
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
    #[test]
    fn custom_balance_and_opaque_provenance_are_valid_xml() {
        let recipe = EditRecipe {
            photo_id: "photo<&".into(),
            revision: "recipe-1".into(),
            source_revision: "_C2_1520.ARW\u{0}70242304\u{0}1790765898469.3801".into(),
            settings: crate::EditRecipeSettings {
                exposure_ev: 0.5,
                white_balance: WhiteBalanceIntent::TemperatureTint {
                    temperature_kelvin: 5200,
                    tint_milli: -250,
                },
            },
        };
        let bytes = document(&recipe);
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
        assert!(text.contains(XMP_FILM_RECIPE_SHA256));
        assert!(text.contains("<slip:TintMilli>-250</slip:TintMilli>"));
        assert!(!text.contains("<crs:Temperature>"));
        assert!(!text.contains("<crs:Tint>"));
        assert!(!text.contains("<crs:WhiteBalance>"));
    }
}
