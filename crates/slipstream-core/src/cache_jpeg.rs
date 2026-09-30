//! JPEG inspection and decode validation for derivative cache admission.
//!
//! [`jpeg_facts`] is a cheap marker walk that rejects truncated or replaced
//! bytes before a manifest is trusted; [`validate_jpeg_bytes`] performs the
//! full decode and ICC checks that gate serving cached bytes.
use super::{MAXIMUM_OUTPUT_BYTES, ManifestFacts, cache_metadata::DerivativeProfileWire};
use crate::derivative::{Derivative, DerivativeError, DerivativeProfile, DerivativeTarget};
use image::{ImageDecoder, codecs::jpeg::JpegDecoder};
use lcms2::Profile;
use std::io::Cursor;

pub(super) fn validate_processed(
    processed: &Derivative,
    target: DerivativeTarget,
) -> Result<(), DerivativeError> {
    if processed.jpeg.is_empty() || processed.jpeg.len() as u64 > MAXIMUM_OUTPUT_BYTES {
        return Err(DerivativeError::OutputLimit);
    }
    if processed.width == 0
        || processed.height == 0
        || processed.width.max(processed.height) > target.long_edge()
    {
        return Err(DerivativeError::Internal);
    }
    let facts = jpeg_facts(&processed.jpeg).ok_or(DerivativeError::Malformed)?;
    if facts.width != processed.width || facts.height != processed.height {
        return Err(DerivativeError::Internal);
    }
    if processed.profile == DerivativeProfile::PreservedIcc && !contains_icc(&processed.jpeg) {
        return Err(DerivativeError::Internal);
    }
    Ok(())
}

pub(super) fn jpeg_facts(bytes: &[u8]) -> Option<ManifestFacts> {
    if bytes.len() < 4 || bytes[0] != 0xff || bytes[1] != 0xd8 {
        return None;
    }
    let mut offset = 2usize;
    let mut dimensions = None;
    let mut saw_scan = false;
    let mut saw_eoi = false;
    while offset + 1 < bytes.len() {
        while offset < bytes.len() && bytes[offset] == 0xff {
            offset += 1;
        }
        if offset >= bytes.len() {
            break;
        }
        let marker = bytes[offset];
        offset += 1;
        if marker == 0xd9 {
            saw_eoi = true;
            break;
        }
        if marker == 0xda {
            saw_scan = true;
            break;
        }
        if marker == 0x00 || (0xd0..=0xd7).contains(&marker) {
            continue;
        }
        if offset + 2 > bytes.len() {
            return None;
        }
        let length = u16::from_be_bytes([bytes[offset], bytes[offset + 1]]) as usize;
        if length < 2 || offset + length > bytes.len() {
            return None;
        }
        if is_jpeg_frame_marker(marker) {
            if length < 7 {
                return None;
            }
            let height = u16::from_be_bytes([bytes[offset + 3], bytes[offset + 4]]) as u32;
            let width = u16::from_be_bytes([bytes[offset + 5], bytes[offset + 6]]) as u32;
            if width == 0 || height == 0 {
                return None;
            }
            dimensions = Some(ManifestFacts { width, height });
        }
        offset += length;
    }
    if saw_scan {
        let mut index = offset;
        while index + 1 < bytes.len() {
            if bytes[index] == 0xff && bytes[index + 1] == 0xd9 {
                saw_eoi = true;
                break;
            }
            index += 1;
        }
    }
    if !saw_eoi && bytes.ends_with(&[0xff, 0xd9]) {
        saw_eoi = true;
    }
    dimensions.filter(|_| saw_eoi)
}

const fn is_jpeg_frame_marker(marker: u8) -> bool {
    matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf)
}

pub(super) fn validate_cached_jpeg(
    bytes: &[u8],
    facts: ManifestFacts,
    profile: DerivativeProfileWire,
) -> Option<()> {
    validate_jpeg_bytes(bytes, facts, profile == DerivativeProfileWire::PreservedIcc)
}

pub(super) fn validate_jpeg_bytes(
    bytes: &[u8],
    facts: ManifestFacts,
    require_icc: bool,
) -> Option<()> {
    let mut decoder = JpegDecoder::new(Cursor::new(bytes)).ok()?;
    if decoder.dimensions() != (facts.width, facts.height) {
        return None;
    }
    let icc = decoder.icc_profile().ok().flatten();
    if require_icc && icc.is_none() {
        return None;
    }
    if let Some(icc) = icc {
        Profile::new_icc(&icc).ok()?;
    }
    let decoded_bytes = usize::try_from(decoder.total_bytes()).ok()?;
    let mut decoded = vec![0; decoded_bytes];
    decoder.read_image(&mut decoded).ok()?;
    Some(())
}

fn contains_icc(bytes: &[u8]) -> bool {
    bytes.windows(12).any(|window| window == b"ICC_PROFILE\0")
}