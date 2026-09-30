//! Closed validation of launcher-received export artifacts.

use slipstream_core::{ExportError, ExportTarget};
use slipstream_processing::photo::OutputReceipt;
use std::{
    fs,
    io::{self, Read, Seek, SeekFrom},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

fn open_read_only(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
}

/// The one actionable reason every output refusal carries. The launcher
/// retains its result for reconciliation either way.
pub(crate) const OUTPUT_VALIDATION_FAILED: &str =
    "received output failed closed artifact validation";

pub(crate) fn validate_output(
    path: &Path,
    target: ExportTarget,
) -> Result<DevelopmentTiffFacts, ExportError> {
    match target {
        ExportTarget::DevelopmentTiff => validate_development_tiff(path),
        ExportTarget::FilmJpeg => validate_finished_jpeg(path),
    }
}

/// Verifies the received output against the launcher receipt and target
/// contract before any acknowledgement or publication.
pub(crate) fn verify_received_output(
    path: &Path,
    receipt: &OutputReceipt,
    target: ExportTarget,
) -> Result<DevelopmentTiffFacts, ExportError> {
    use sha2::{Digest, Sha256};
    let file = open_read_only(path).map_err(|_| ExportError::InvalidArtifact)?;
    let metadata = file.metadata().map_err(ExportError::Io)?;
    if metadata.len() != receipt.size || receipt.size == 0 {
        return Err(ExportError::InvalidArtifact);
    }
    let mut hasher = Sha256::new();
    let mut file = file;
    io::copy(&mut file, &mut hasher).map_err(ExportError::Io)?;
    if format!("{:x}", hasher.finalize()) != receipt.sha256 {
        return Err(ExportError::InvalidArtifact);
    }
    validate_output(path, target)
}

/// Validated Development TIFF facts the wire contract discloses with the
/// artifact: declared geometry and the embedded-profile identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DevelopmentTiffFacts {
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// SHA-256 of the embedded ICC profile bytes.
    pub(crate) profile_identity: String,
}

/// Validates the closed `development-tiff` output contract with a bounded
/// TIFF directory walk: positive geometry, three 32-bit IEEE-float samples per
/// pixel, RGB photometric interpretation, and an embedded RGB ICC profile.
/// Returns the validated facts on success.
pub(crate) fn validate_development_tiff(path: &Path) -> Result<DevelopmentTiffFacts, ExportError> {
    use std::os::fd::AsRawFd;
    macro_rules! invalid {
        () => {{ ExportError::Validation("Output is not a valid Development TIFF") }};
    }
    let mut file = open_read_only(path).map_err(|_| invalid!())?;
    use slipstream_core::derivative::DerivativeTarget;
    let mut header = [0_u8; 8];
    file.read_exact(&mut header).map_err(|_| invalid!())?;
    let little_endian = match &header[..4] {
        b"II\x2a\x00" => true,
        b"MM\x00\x2a" => false,
        _ => return Err(invalid!()),
    };
    let word = |bytes: [u8; 2]| {
        if little_endian {
            u16::from_le_bytes(bytes)
        } else {
            u16::from_be_bytes(bytes)
        }
    };
    let dword = |bytes: [u8; 4]| {
        if little_endian {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        }
    };
    let ifd_offset = dword([header[4], header[5], header[6], header[7]]);
    let mut entry = [0_u8; 2];
    file.seek(SeekFrom::Start(u64::from(ifd_offset)))
        .map_err(|_| invalid!())?;
    file.read_exact(&mut entry).map_err(|_| invalid!())?;
    let entry_count = word([entry[0], entry[1]]);
    if entry_count == 0 || entry_count > 512 {
        return Err(invalid!());
    }
    let mut width = 0_u32;
    let mut height = 0_u32;
    let mut bits_per_sample: Vec<u16> = Vec::new();
    let mut sample_format: Vec<u16> = Vec::new();
    let mut samples_per_pixel = 0_u16;
    let mut photometric = 0_u16;
    let mut compression = 0_u32;
    let mut strip_longs: Vec<u32> = Vec::new();
    let mut strip_counts: Vec<u32> = Vec::new();
    let mut icc: Option<(u32, u32)> = None;
    // Reads a LONG array either inline or at its external offset.
    fn long_array(
        file: &mut std::fs::File,
        little_endian: bool,
        count: u32,
        inline_value: [u8; 4],
        offset: u32,
    ) -> Option<Vec<u32>> {
        if count == 0 || count > 1_048_576 {
            return None;
        }
        if count == 1 {
            let raw = if little_endian {
                u32::from_le_bytes(inline_value)
            } else {
                u32::from_be_bytes(inline_value)
            };
            return Some(vec![raw]);
        }
        let resume = file.stream_position().ok()?;
        let mut bytes = vec![0_u8; count as usize * 4];
        file.seek(SeekFrom::Start(u64::from(offset))).ok()?;
        file.read_exact(&mut bytes).ok()?;
        file.seek(SeekFrom::Start(resume)).ok()?;
        let word = |chunk: &[u8]| {
            let raw: [u8; 4] = chunk.try_into().ok()?;
            Some(if little_endian {
                u32::from_le_bytes(raw)
            } else {
                u32::from_be_bytes(raw)
            })
        };
        bytes.chunks_exact(4).map(word).collect()
    }
    for _ in 0..entry_count {
        let mut raw = [0_u8; 12];
        file.read_exact(&mut raw).map_err(|_| invalid!())?;
        let tag = word([raw[0], raw[1]]);
        let kind = word([raw[2], raw[3]]);
        let count = dword([raw[4], raw[5], raw[6], raw[7]]);
        let type_size = match kind {
            1 | 2 | 6 | 7 => 1, // BYTE | ASCII | SBYTE | UNDEFINED
            3 | 8 => 2,         // SHORT | SSHORT
            4 | 9 => 4,         // LONG | SLONG
            // An engine artifact carries resolution, EXIF, and XMP entries
            // whose types this walk does not constrain. The contract is
            // defined by the tags read below, so an unread entry is skipped
            // rather than refusing a decodable image.
            _ => continue,
        };
        let total = count.checked_mul(type_size).ok_or(ExportError::Validation(
            "Output is not a valid Development TIFF",
        ))?;
        let inline = total <= 4;
        let mut value_bytes = [0_u8; 4];
        value_bytes.copy_from_slice(&raw[8..12]);
        let value_offset = dword(value_bytes);
        let shorts = |bytes: [u8; 4], index: usize| -> u16 {
            let pair = [bytes[index * 2], bytes[index * 2 + 1]];
            word(pair)
        };
        match tag {
            256 | 257 => {
                // LONG dimensions or one SHORT dimension.
                let dimension = if kind == 4 {
                    value_offset
                } else {
                    u32::from(shorts(value_bytes, 0))
                };
                if tag == 256 {
                    width = dimension;
                } else {
                    height = dimension;
                }
            }
            258 | 339 => {
                let values = if inline {
                    (0..count as usize)
                        .map(|index| shorts(value_bytes, index))
                        .collect::<Vec<_>>()
                } else {
                    let resume = file.stream_position().map_err(|_| invalid!())?;
                    let mut bytes = vec![0_u8; total.min(16) as usize];
                    file.seek(SeekFrom::Start(u64::from(value_offset)))
                        .map_err(|_| invalid!())?;
                    file.read_exact(&mut bytes).map_err(|_| invalid!())?;
                    file.seek(SeekFrom::Start(resume)).map_err(|_| invalid!())?;
                    (0..(bytes.len() / 2))
                        .map(|index| word([bytes[index * 2], bytes[index * 2 + 1]]))
                        .collect::<Vec<_>>()
                };
                if tag == 258 {
                    bits_per_sample = values;
                } else {
                    sample_format = values;
                }
            }
            259 => compression = value_offset,
            262 => photometric = shorts(value_bytes, 0),
            273 | 279 => {
                let Some(values) =
                    long_array(&mut file, little_endian, count, value_bytes, value_offset)
                else {
                    return Err(invalid!());
                };
                if tag == 273 {
                    strip_longs = values;
                } else {
                    strip_counts = values;
                }
            }
            277 => samples_per_pixel = shorts(value_bytes, 0),
            34675 => icc = Some((value_offset, count)),
            _ => {}
        }
    }
    if width == 0 || height == 0 {
        return Err(invalid!());
    }
    if samples_per_pixel != 3
        || bits_per_sample != vec![32, 32, 32]
        || sample_format != vec![3, 3, 3]
        || photometric != 2
        || compression != 8
    {
        return Err(invalid!());
    }
    // The embedded profile must describe RGB data; the pixels are scene-linear
    // ProPhoto RGB by the engine contract.
    let Some((offset, size)) = icc else {
        return Err(invalid!());
    };
    if !(128..=1024 * 1024).contains(&size) {
        return Err(invalid!());
    }
    let mut profile = [0_u8; 24];
    file.seek(SeekFrom::Start(u64::from(offset)))
        .map_err(|_| invalid!())?;
    file.read_exact(&mut profile).map_err(|_| invalid!())?;
    if &profile[16..20] != b"RGB " {
        return Err(invalid!());
    }
    // The declared strips must exist, agree on their lengths, and declare the
    // pinned Deflate payload.
    if strip_longs.is_empty() || strip_longs.len() != strip_counts.len() {
        return Err(invalid!());
    }
    if strip_counts.contains(&0) {
        return Err(invalid!());
    }
    // Publication additionally requires that the payload really decodes:
    // structural validity cannot prove the compressed strips inflate to the
    // declared geometry, so the artifact is read through the same bounded
    // Development TIFF reader the preview path uses before anything is
    // published.
    let derivative = slipstream_core::derivative::process_development_tiff(
        file.as_raw_fd(),
        DerivativeTarget::Thumbnail512,
    )
    .map_err(|_| invalid!())?;
    drop(derivative);
    // The profile identity is byte identity: the SHA-256 over the embedded
    // profile bytes, the same discipline the launcher pins at qualification.
    use sha2::{Digest, Sha256};
    file.seek(SeekFrom::Start(u64::from(offset)))
        .map_err(|_| invalid!())?;
    let mut hasher = Sha256::new();
    let mut remaining = usize::try_from(size).map_err(|_| invalid!())?;
    let mut buffer = [0_u8; 8192];
    while remaining > 0 {
        let chunk = remaining.min(buffer.len());
        file.read_exact(&mut buffer[..chunk])
            .map_err(|_| invalid!())?;
        hasher.update(&buffer[..chunk]);
        remaining -= chunk;
    }
    Ok(DevelopmentTiffFacts {
        width,
        height,
        profile_identity: format!("{:x}", hasher.finalize()),
    })
}
/// Validates the bounded JPEG envelope emitted by the fixed Film adapter.
/// The worker performs the complete decode/profile check; this second check
/// binds the transferred artifact to an image shape and one embedded sRGB ICC
/// profile without loading all pixel bytes into the server.
fn validate_finished_jpeg(path: &Path) -> Result<DevelopmentTiffFacts, ExportError> {
    use sha2::{Digest, Sha256};
    const OUTPUT_ICC_SHA256: &str =
        "b44e86e44d44993a3a9a880626f9832e9c37e2234caba501548f9114bada6d21";
    let invalid = || ExportError::Validation("Output is not a valid Finished JPEG");
    let file = open_read_only(path).map_err(|_| invalid())?;
    if file.metadata().map_err(ExportError::Io)?.len() > slipstream_core::MAXIMUM_EXPORT_BYTES {
        return Err(invalid());
    }
    let mut reader = std::io::BufReader::new(file);
    let mut signature = [0_u8; 2];
    reader.read_exact(&mut signature).map_err(|_| invalid())?;
    if signature != [0xff, 0xd8] {
        return Err(invalid());
    }
    let mut width = 0_u32;
    let mut height = 0_u32;
    let mut channels = 0_u8;
    let mut profile = None;
    let mut saw_scan = false;
    loop {
        let mut byte = [0_u8; 1];
        reader.read_exact(&mut byte).map_err(|_| invalid())?;
        if byte[0] != 0xff {
            continue;
        }
        loop {
            reader.read_exact(&mut byte).map_err(|_| invalid())?;
            if byte[0] != 0xff {
                break;
            }
        }
        let marker = byte[0];
        if marker == 0xd9 {
            break;
        }
        if marker == 0xda {
            saw_scan = true;
        }
        if matches!(marker, 0xd8 | 0xd9 | 0x01 | 0xd0..=0xd7) {
            continue;
        }
        let mut length = [0_u8; 2];
        reader.read_exact(&mut length).map_err(|_| invalid())?;
        let length = u16::from_be_bytes(length);
        if length < 2 || usize::from(length) > 1024 * 1024 {
            return Err(invalid());
        }
        let mut segment = vec![0_u8; usize::from(length) - 2];
        reader.read_exact(&mut segment).map_err(|_| invalid())?;
        if (0xc0..=0xcf).contains(&marker) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
            if segment.len() < 6 {
                return Err(invalid());
            }
            height = u32::from(u16::from_be_bytes([segment[1], segment[2]]));
            width = u32::from(u16::from_be_bytes([segment[3], segment[4]]));
            channels = segment[5];
        }
        if marker == 0xe2 && segment.starts_with(b"ICC_PROFILE\0") {
            if segment.len() < 14 || segment[12] != 1 || segment[13] != 1 {
                return Err(invalid());
            }
            profile = Some(segment[14..].to_vec());
        }
        if saw_scan {
            break;
        }
    }
    let profile = profile.ok_or_else(invalid)?;
    if width == 0 || height == 0 || channels != 3 {
        return Err(invalid());
    }
    let identity = format!("{:x}", Sha256::digest(&profile));
    if identity != OUTPUT_ICC_SHA256 {
        return Err(invalid());
    }
    Ok(DevelopmentTiffFacts {
        width,
        height,
        profile_identity: identity,
    })
}
