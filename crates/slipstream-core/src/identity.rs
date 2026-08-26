use crate::domain::OriginalKind;
use sha2::{Digest, Sha256};
use std::fmt;

const RAW_EXTENSIONS: &[&str] = &[
    "3fr", "ari", "arw", "bay", "bmq", "cap", "cine", "cr2", "cr3", "cs1", "dc2", "dcr", "dng",
    "drf", "eip", "erf", "fff", "gpr", "iiq", "k25", "kc2", "kdc", "mdc", "mef", "mos", "mrw",
    "nef", "nrw", "obm", "orf", "pef", "ptx", "pxn", "qtk", "r3d", "raf", "raw", "rdc", "rw2",
    "rwl", "rwz", "sr2", "srf", "srw", "x3f",
];
const JPEG_EXTENSIONS: &[&str] = &["jpg", "jpeg"];

pub fn classify_name(name: &str) -> Option<OriginalKind> {
    let dot = name.rfind('.')?;
    if dot == 0 || dot + 1 == name.len() {
        return None;
    }
    classify_extension(&name.as_bytes()[dot + 1..])
}

pub(crate) fn classify_extension(extension: &[u8]) -> Option<OriginalKind> {
    let extension = extension
        .iter()
        .map(u8::to_ascii_lowercase)
        .collect::<Vec<_>>();
    if RAW_EXTENSIONS
        .iter()
        .any(|supported| supported.as_bytes() == extension)
    {
        Some(OriginalKind::Raw)
    } else if JPEG_EXTENSIONS
        .iter()
        .any(|supported| supported.as_bytes() == extension)
    {
        Some(OriginalKind::Jpeg)
    } else {
        None
    }
}

pub fn pairing_stem(name: &str) -> &str {
    name.rfind('.')
        .map_or(name, |dot| if dot > 0 { &name[..dot] } else { name })
}

fn digest(parts: &[&[u8]]) -> String {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update(part);
    }
    format!("{:x}", hash.finalize())
}

pub fn original_id(path: &str) -> String {
    digest(&[b"original\0", path.as_bytes()])
}

pub fn standalone_photo_id(original_id: &str) -> String {
    digest(&[b"photo\0", original_id.as_bytes()])
}

pub fn paired_photo_id(raw_original_id: &str, jpeg_original_id: &str) -> String {
    digest(&[
        b"photo\0",
        raw_original_id.as_bytes(),
        b"\0",
        jpeg_original_id.as_bytes(),
    ])
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidModificationTime;

impl fmt::Display for InvalidModificationTime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Original modification time is invalid")
    }
}

impl std::error::Error for InvalidModificationTime {}

pub fn source_revision(
    path: &str,
    size: u64,
    mtime_ms: f64,
) -> Result<String, InvalidModificationTime> {
    if !mtime_ms.is_finite() || mtime_ms < 0.0 {
        return Err(InvalidModificationTime);
    }
    Ok(format!("{path}\0{size}\0{mtime_ms}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::{fs, path::PathBuf};

    #[derive(Deserialize)]
    struct Contract {
        vectors: Vec<Vector>,
        paired: Pair,
    }

    #[derive(Deserialize)]
    struct Vector {
        path: String,
        size: u64,
        #[serde(rename = "mtimeMs")]
        mtime_ms: f64,
        #[serde(rename = "originalId")]
        original_id: String,
        #[serde(rename = "photoId")]
        photo_id: String,
        #[serde(rename = "sourceRevision")]
        source_revision: String,
    }

    #[derive(Deserialize)]
    struct Pair {
        #[serde(rename = "rawOriginalId")]
        raw_original_id: String,
        #[serde(rename = "jpegOriginalId")]
        jpeg_original_id: String,
        #[serde(rename = "photoId")]
        photo_id: String,
    }

    fn contract_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../compatibility/identity/vectors.json")
    }

    #[test]
    fn classifies_the_complete_supported_extension_set() {
        for extension in RAW_EXTENSIONS {
            assert_eq!(
                classify_name(&format!("photo.{extension}")),
                Some(OriginalKind::Raw)
            );
            assert_eq!(
                classify_name(&format!("photo.{}", extension.to_ascii_uppercase())),
                Some(OriginalKind::Raw)
            );
        }
        for extension in JPEG_EXTENSIONS {
            assert_eq!(
                classify_name(&format!("photo.{extension}")),
                Some(OriginalKind::Jpeg)
            );
            assert_eq!(
                classify_name(&format!("photo.{}", extension.to_ascii_uppercase())),
                Some(OriginalKind::Jpeg)
            );
        }
        for name in ["photo", ".jpg", "photo.", "photo.jpg.txt"] {
            assert_eq!(classify_name(name), None);
        }
    }

    #[test]
    fn pairing_removes_only_the_last_extension_and_preserves_case() {
        assert_eq!(pairing_stem("Case.final.ArW"), "Case.final");
        assert_eq!(pairing_stem(".hidden"), ".hidden");
        assert_eq!(pairing_stem("plain"), "plain");
    }

    #[test]
    fn renders_filesystem_milliseconds_like_the_javascript_authority() {
        let cases = [
            (0, 0, "0"),
            (0, 1, "0.000001"),
            (0, 999_999_999, "999.999999"),
            (1_700_000_000, 0, "1700000000000"),
            (1_700_000_000, 123_456_789, "1700000000123.4568"),
        ];
        for (seconds, nanoseconds, expected) in cases {
            let milliseconds = seconds as f64 * 1000.0 + nanoseconds as f64 / 1_000_000.0;
            assert_eq!(
                source_revision("photo.JPG", 1, milliseconds).unwrap(),
                format!("photo.JPG\0{size}\0{expected}", size = 1)
            );
        }
    }

    #[test]
    fn rejects_invalid_modification_times_without_panicking() {
        assert_eq!(
            source_revision("photo.JPG", 1, f64::NAN),
            Err(InvalidModificationTime)
        );
        assert_eq!(
            source_revision("photo.JPG", 1, f64::INFINITY),
            Err(InvalidModificationTime)
        );
        assert_eq!(
            source_revision("photo.JPG", 1, -0.001),
            Err(InvalidModificationTime)
        );
    }

    #[test]
    fn matches_shared_identity_vectors() {
        let contract: Contract =
            serde_json::from_slice(&fs::read(contract_path()).unwrap()).unwrap();
        for vector in contract.vectors {
            assert_eq!(original_id(&vector.path), vector.original_id);
            assert_eq!(standalone_photo_id(&vector.original_id), vector.photo_id);
            assert_eq!(
                source_revision(&vector.path, vector.size, vector.mtime_ms).unwrap(),
                vector.source_revision
            );
        }
        assert_eq!(
            paired_photo_id(
                &contract.paired.raw_original_id,
                &contract.paired.jpeg_original_id
            ),
            contract.paired.photo_id
        );
    }
}
