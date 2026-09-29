//! Bounded finished-JPEG validation for the `film-jpeg` output.
//!
//! The launcher validates its own engine result before offering it on an
//! Output request: a JPEG stream with exactly one full-resolution frame of
//! three 8-bit components and the exact pinned embedded sRGB ICC bytes from
//! the shared Film identity (`tools/development/film_identity.py`). The
//! pinned profile hash is byte identity; a profile name or appearance is not
//! evidence. The entropy-coded scan is not decoded: decoded-sample identity
//! is the adapter's gate, structural and profile identity is this one.

use crate::protocol::ErrorCode;
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, os::unix::fs::MetadataExt, path::Path};

/// The pinned sRGB profile bytes embedded by the fixed Finished JPEG encoder.
/// This is the shared `OUTPUT_ICC_SHA256` of `film_identity.py`, which is the
/// byte identity of the pinned encoder's sRGB profile.
pub(crate) const OUTPUT_ICC_SHA256: &str =
    "b44e86e44d44993a3a9a880626f9832e9c37e2234caba501548f9114bada6d21";

/// Identity of one validated finished JPEG.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct JpegIdentity {
    pub size: u64,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
}

const ICC_BYTES_MAX: usize = 16384;
/// All frame geometry, tables and the embedded profile precede the first
/// scan. The pinned producer emits a small header, so this bound keeps the
/// parsed region small while staying far above any qualified artifact head.
const HEAD_MAX: usize = 1024 * 1024;

/// One parsed JPEG marker segment.
struct Segment {
    marker: u8,
    /// Segment payload including the big-endian length; `None` for the
    /// standalone markers (SOI, EOI, RST, TEM) that carry no length field.
    payload: Option<Vec<u8>>,
}

fn read_header(path: &Path) -> Result<Vec<u8>, ErrorCode> {
    let file = File::open(path).map_err(|_| ErrorCode::Uncertain)?;
    // Read no more than the bounded marker region. Parsing stops at the first
    // scan marker, so the entropy-coded body of a valid image may be far
    // larger than this bound; a marker segment that crosses the bound is
    // refused.
    let mut buffer = Vec::new();
    file.take(HEAD_MAX as u64)
        .read_to_end(&mut buffer)
        .map_err(|_| ErrorCode::Uncertain)?;
    Ok(buffer)
}

fn segments(head: &[u8]) -> Result<Vec<Segment>, ErrorCode> {
    let mut segments = Vec::new();
    if head.first() != Some(&0xFF) || head.get(1) != Some(&0xD8) {
        return Err(ErrorCode::Uncertain);
    }
    segments.push(Segment {
        marker: 0xD8,
        payload: None,
    });
    let mut at = 2usize;
    while at < head.len() {
        // Every segment starts with one or more fill bytes followed by the
        // marker byte.
        if head[at] != 0xFF {
            return Err(ErrorCode::Uncertain);
        }
        while at < head.len() && head[at] == 0xFF {
            at += 1;
        }
        if at >= head.len() {
            return Err(ErrorCode::Uncertain);
        }
        let marker = head[at];
        at += 1;
        match marker {
            // Standalone markers carry no length and no payload.
            0x01 | 0xD0..=0xD7 | 0xD9 => {
                segments.push(Segment {
                    marker,
                    payload: None,
                });
                if marker == 0xD9 {
                    break;
                }
            }
            // Stuffed bytes and a repeated SOI are never marker positions.
            0x00 | 0xD8 => return Err(ErrorCode::Uncertain),
            _ => {
                if at + 2 > head.len() {
                    return Err(ErrorCode::Uncertain);
                }
                let length = usize::from(u16::from_be_bytes([head[at], head[at + 1]]));
                // The length counts itself but never less than two bytes.
                if length < 2 || at + length > head.len() {
                    return Err(ErrorCode::Uncertain);
                }
                segments.push(Segment {
                    marker,
                    payload: Some(head[at + 2..at + length].to_vec()),
                });
                at += length;
                // The first scan ends the structured header region: what
                // follows is entropy-coded data with no marker semantics.
                if marker == 0xDA {
                    break;
                }
            }
        }
    }
    Ok(segments)
}

/// Assemble the embedded ICC profile from the ordered APP2 `ICC_PROFILE`
/// segments. Chunks must be complete, unique and contiguous from one.
fn icc_profile(segments: &[Segment]) -> Result<Vec<u8>, ErrorCode> {
    const SIGNATURE: &[u8] = b"ICC_PROFILE\0";
    let mut chunks: Vec<(u8, &[u8])> = Vec::new();
    for segment in segments {
        let Some(payload) = &segment.payload else {
            continue;
        };
        if segment.marker != 0xE2 || payload.len() < SIGNATURE.len() + 2 {
            continue;
        }
        if &payload[..SIGNATURE.len()] != SIGNATURE {
            continue;
        }
        let number = payload[SIGNATURE.len()];
        let total = payload[SIGNATURE.len() + 1];
        if total == 0 || number == 0 || number > total {
            return Err(ErrorCode::Uncertain);
        }
        chunks.push((number, &payload[SIGNATURE.len() + 2..]));
    }
    if chunks.is_empty() {
        return Err(ErrorCode::Uncertain);
    }
    let mut profile = Vec::new();
    for (index, (number, data)) in chunks.iter().enumerate() {
        if *number != index as u8 + 1 {
            return Err(ErrorCode::Uncertain);
        }
        profile.extend_from_slice(data);
        if profile.len() > ICC_BYTES_MAX {
            return Err(ErrorCode::Uncertain);
        }
    }
    Ok(profile)
}

/// The exactly-one frame geometry declared before the first scan.
fn frame(segments: &[Segment]) -> Result<(u32, u32), ErrorCode> {
    let mut frame: Option<(u32, u32)> = None;
    for segment in segments {
        match segment.marker {
            // Start of scan: the frame and profile must already be complete.
            0xDA => break,
            0xC4 | 0xC8 | 0xCC | 0xD8 | 0xD9 | 0x01 | 0xD0..=0xD7 => {}
            marker if (0xC0..=0xCF).contains(&marker) => {
                if frame.is_some() {
                    return Err(ErrorCode::Uncertain);
                }
                let Some(payload) = &segment.payload else {
                    return Err(ErrorCode::Uncertain);
                };
                // Precision, height, width, component count, then exactly
                // three bytes per component.
                if payload.len() < 6 || payload[0] != 8 {
                    return Err(ErrorCode::Uncertain);
                }
                let height = u32::from(u16::from_be_bytes([payload[1], payload[2]]));
                let width = u32::from(u16::from_be_bytes([payload[3], payload[4]]));
                if width == 0 || height == 0 || payload[5] != 3 {
                    return Err(ErrorCode::Uncertain);
                }
                if payload.len() != 6 + usize::from(payload[5]) * 3 {
                    return Err(ErrorCode::Uncertain);
                }
                frame = Some((width, height));
            }
            _ => {}
        }
    }
    frame.ok_or(ErrorCode::Uncertain)
}

/// Validate one launcher-owned finished JPEG and return its identity.
pub(crate) fn validate(path: &Path, output_bytes_max: u64) -> Result<JpegIdentity, ErrorCode> {
    validate_for_icc(path, output_bytes_max, OUTPUT_ICC_SHA256)
}

/// Validate against an explicit embedded profile identity; the production
/// entrypoint pins the qualified bytes.
fn validate_for_icc(
    path: &Path,
    output_bytes_max: u64,
    icc_sha256: &str,
) -> Result<JpegIdentity, ErrorCode> {
    let metadata = std::fs::metadata(path).map_err(|_| ErrorCode::Uncertain)?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.len() == 0
        || metadata.len() > output_bytes_max
    {
        return Err(ErrorCode::Uncertain);
    }
    let head = read_header(path)?;
    let parsed = segments(&head)?;
    let profile = icc_profile(&parsed)?;
    if format!("{:x}", Sha256::digest(&profile)) != icc_sha256 {
        return Err(ErrorCode::Uncertain);
    }
    let (width, height) = frame(&parsed)?;
    let mut file = File::open(path).map_err(|_| ErrorCode::Uncertain)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|_| ErrorCode::Uncertain)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(JpegIdentity {
        size: metadata.len(),
        sha256: format!("{:x}", hasher.finalize()),
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Assemble a JPEG marker stream from named segments. Every payload is a
    /// complete APPn/SOFn segment body without its two length bytes.
    struct Stream {
        bytes: Vec<u8>,
    }

    impl Stream {
        fn new() -> Self {
            Self {
                bytes: vec![0xFF, 0xD8],
            }
        }

        fn segment(mut self, marker: u8, payload: &[u8]) -> Self {
            self.bytes.push(0xFF);
            self.bytes.push(marker);
            let length = payload.len() + 2;
            self.bytes.extend_from_slice(&(length as u16).to_be_bytes());
            self.bytes.extend_from_slice(payload);
            self
        }

        fn icc(self, number: u8, total: u8, data: &[u8]) -> Self {
            let mut payload = b"ICC_PROFILE\0".to_vec();
            payload.push(number);
            payload.push(total);
            payload.extend_from_slice(data);
            self.segment(0xE2, &payload)
        }

        fn frame(self, width: u16, height: u16, components: u8, precision: u8) -> Self {
            let mut payload = vec![precision];
            payload.extend_from_slice(&height.to_be_bytes());
            payload.extend_from_slice(&width.to_be_bytes());
            payload.push(components);
            for component in 1..=components {
                payload.extend_from_slice(&[component, 0x11, 0]);
            }
            self.segment(0xC0, &payload)
        }

        fn scan(mut self) -> Self {
            self.bytes
                .extend_from_slice(&[0xFF, 0xDA, 0x00, 0x02, 0x01, 0xA1]);
            self
        }

        fn finish(mut self) -> Vec<u8> {
            self.bytes.extend_from_slice(&[0xFF, 0xD9]);
            self.bytes
        }
    }

    fn profile_bytes(length: usize, seed: u8) -> Vec<u8> {
        vec![seed; length]
    }

    fn digest(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    fn write(name: &str, bytes: &[u8]) -> (std::path::PathBuf, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "slipstream-photo-jpeg-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = path.with_extension("dir");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&path, bytes).unwrap();
        (root, path)
    }

    #[test]
    fn a_wellformed_stream_with_the_pinned_profile_yields_its_identity() {
        let profile = profile_bytes(620, 7);
        let bytes = Stream::new()
            .segment(0xE0, b"JFIF\0")
            .icc(1, 1, &profile)
            .frame(4021, 3071, 3, 8)
            .scan()
            .finish();
        let (_root, path) = write("wellformed", &bytes);
        let identity = validate_for_icc(&path, 64 * 1024 * 1024, &digest(&profile)).unwrap();
        assert_eq!(identity.width, 4021);
        assert_eq!(identity.height, 3071);
        assert_eq!(identity.size, bytes.len() as u64);
        assert_eq!(identity.sha256, digest(&bytes));
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir_all(&_root).unwrap();
    }

    #[test]
    fn multi_chunk_profiles_assemble_in_chunk_order() {
        let first = profile_bytes(300, 1);
        let second = profile_bytes(200, 2);
        let mut profile = first.clone();
        profile.extend_from_slice(&second);
        let bytes = Stream::new()
            .icc(1, 2, &first)
            .icc(2, 2, &second)
            .frame(64, 48, 3, 8)
            .scan()
            .finish();
        let (_root, path) = write("chunks", &bytes);
        let identity = validate_for_icc(&path, 64 * 1024 * 1024, &digest(&profile)).unwrap();
        assert_eq!((identity.width, identity.height), (64, 48));
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir_all(&_root).unwrap();
    }

    #[test]
    fn a_foreign_embedded_profile_is_not_the_pinned_output() {
        let profile = profile_bytes(620, 7);
        let bytes = Stream::new()
            .icc(1, 1, &profile)
            .frame(64, 48, 3, 8)
            .scan()
            .finish();
        let (_root, path) = write("foreign-profile", &bytes);
        let other = profile_bytes(620, 9);
        assert!(validate_for_icc(&path, 64 * 1024 * 1024, &digest(&other)).is_err());
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir_all(&_root).unwrap();
    }

    #[test]
    fn streams_without_one_complete_profile_or_frame_are_refused() {
        let profile = profile_bytes(64, 3);
        let pinned = digest(&profile);
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("no-icc", Stream::new().frame(64, 48, 3, 8).scan().finish()),
            (
                "no-frame",
                Stream::new().icc(1, 1, &profile).scan().finish(),
            ),
            (
                "two-frames",
                Stream::new()
                    .icc(1, 1, &profile)
                    .frame(64, 48, 3, 8)
                    .frame(32, 24, 3, 8)
                    .scan()
                    .finish(),
            ),
            (
                "zero-width",
                Stream::new()
                    .icc(1, 1, &profile)
                    .frame(0, 48, 3, 8)
                    .scan()
                    .finish(),
            ),
            (
                "four-components",
                Stream::new()
                    .icc(1, 1, &profile)
                    .frame(64, 48, 4, 8)
                    .scan()
                    .finish(),
            ),
            (
                "sixteen-bit",
                Stream::new()
                    .icc(1, 1, &profile)
                    .frame(64, 48, 3, 16)
                    .scan()
                    .finish(),
            ),
            (
                "chunk-gap",
                Stream::new()
                    .icc(1, 3, &profile)
                    .icc(3, 3, &profile)
                    .frame(64, 48, 3, 8)
                    .scan()
                    .finish(),
            ),
            (
                "chunk-count-zero",
                Stream::new()
                    .icc(1, 0, &profile)
                    .frame(64, 48, 3, 8)
                    .scan()
                    .finish(),
            ),
            ("not-jpeg", b"GIF89a".to_vec()),
            ("truncated", vec![0xFF, 0xD8, 0xFF]),
        ];
        for (name, bytes) in cases {
            let (_root, path) = write(name, &bytes);
            assert!(
                validate_for_icc(&path, 64 * 1024 * 1024, &pinned).is_err(),
                "{name} must be refused"
            );
            std::fs::remove_file(&path).unwrap();
            std::fs::remove_dir_all(&_root).unwrap();
        }
    }

    #[test]
    fn oversized_or_sparse_files_never_reach_the_marker_parser() {
        let profile = profile_bytes(620, 7);
        let bytes = Stream::new()
            .icc(1, 1, &profile)
            .frame(64, 48, 3, 8)
            .scan()
            .finish();
        let (_root, path) = write("oversized", &bytes);
        assert!(validate_for_icc(&path, 16, &digest(&profile)).is_err());
        std::fs::remove_file(&path).unwrap();
        let empty = write("empty", b"").1;
        assert!(validate_for_icc(&empty, 64 * 1024 * 1024, &digest(&profile)).is_err());
        std::fs::remove_file(&empty).unwrap();
        std::fs::remove_dir_all(&_root).unwrap();
    }

    #[test]
    fn pinned_header_admits_an_encoded_body_larger_than_the_header_limit() {
        let asset = include_bytes!("../../slipstream-core/assets/srgb-iec61966-2-1.icc");
        let mut bytes = Stream::new()
            .icc(1, 1, asset)
            .frame(6376, 9568, 3, 8)
            .scan()
            .finish();
        bytes.splice(bytes.len() - 2..bytes.len() - 2, vec![0x33; HEAD_MAX]);
        let (root, path) = write("large-encoded-body", &bytes);
        let identity = validate(&path, bytes.len() as u64).unwrap();
        assert_eq!((identity.width, identity.height), (6376, 9568));
        assert_eq!(identity.sha256, digest(&bytes));
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }

    /// The pinned output identity is the byte identity of the repository's
    /// pinned sRGB asset. A drift on either side must fail this test and
    /// force a conscious re-pin of the shared Film identity.
    #[test]
    fn the_pinned_output_identity_is_the_pinned_srgb_asset_bytes() {
        let asset = include_bytes!("../../slipstream-core/assets/srgb-iec61966-2-1.icc");
        assert_eq!(digest(asset), OUTPUT_ICC_SHA256);
        let bytes = Stream::new()
            .icc(1, 1, asset)
            .frame(64, 48, 3, 8)
            .scan()
            .finish();
        let (_root, path) = write("pinned-asset", &bytes);
        let identity = validate(&path, 64 * 1024 * 1024).unwrap();
        assert_eq!((identity.width, identity.height), (64, 48));
        assert_eq!(identity.sha256, digest(&bytes));
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_dir_all(&_root).unwrap();
    }
}
