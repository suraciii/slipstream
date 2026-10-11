//! Historical XMP export metadata and retained document bytes.
//! Generic processing no longer generates semantic XMP snapshots.
use crate::WhiteBalanceIntent;
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

pub fn digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}
