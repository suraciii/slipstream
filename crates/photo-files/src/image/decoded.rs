use super::{jpeg, tiff};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, BufReader, Read},
    os::fd::AsRawFd,
    path::{Path, PathBuf},
};

const DECODED_MAX: usize = 1024 * 1024 * 1024;
#[derive(Clone, Debug)]
pub struct ImageFacts {
    pub format: &'static str,
    pub precision: &'static str,
    pub width: u32,
    pub height: u32,
    pub channels: u8,
    pub bytes: u64,
    pub sha256: String,
    pub icc_sha256: Option<String>,
}
fn rejected(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
fn geometry(width: u32, height: u32, sample_bytes: usize) -> io::Result<()> {
    if width == 0
        || height == 0
        || (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4 * sample_bytes))
            .filter(|n| *n <= DECODED_MAX)
            .is_none()
    {
        return Err(rejected("image decoded geometry exceeds bound"));
    }
    Ok(())
}
/// Validate actual bytes, embedded profile identity and every decoded sample.
/// Format and precision are observed from the file, never from its filename.
pub fn validate(path: &Path, maximum: u64, expected_icc: &str) -> io::Result<ImageFacts> {
    let facts = inspect_path(path, maximum)?;
    if facts.channels != 3 {
        return Err(rejected("qualified output must have three RGB channels"));
    }
    if facts.icc_sha256.as_deref() != Some(expected_icc) {
        return Err(rejected("embedded ICC identity differs"));
    }
    if facts.format == "tiff" && facts.precision == "float32" {
        tiff::validate_output_for_icc(path, maximum, expected_icc)
            .map_err(|_| rejected("float TIFF structure/profile refused"))?;
    }
    if facts.format == "jpeg" {
        jpeg::validate_for_icc(path, maximum, expected_icc)
            .map_err(|_| rejected("JPEG structure/profile refused"))?;
    }
    Ok(facts)
}
/// Inspect one retained descriptor without imposing a qualified output profile.
pub fn inspect_image(file: &mut File, maximum: u64) -> io::Result<ImageFacts> {
    let before = crate::Observation::of(file)?;
    let facts = inspect_path(
        &PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd())),
        maximum,
    )?;
    before.verify(file)?;
    Ok(facts)
}
fn inspect_path(path: &Path, maximum: u64) -> io::Result<ImageFacts> {
    let mut file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > maximum {
        return Err(rejected("image file exceeds bound"));
    }
    let mut magic = [0; 8];
    file.read_exact(&mut magic)?;
    let (format, precision, width, height, channels, icc) = if &magic == b"\x89PNG\r\n\x1a\n" {
        let mut decoder = png::Decoder::new(BufReader::new(File::open(path)?));
        decoder.set_limits(png::Limits { bytes: DECODED_MAX });
        decoder.set_transformations(png::Transformations::EXPAND);
        let mut reader = decoder.read_info().map_err(|e| rejected(e.to_string()))?;
        let info = reader.info();
        if info.animation_control.is_some() {
            return Err(rejected("animated PNG is not one image"));
        }
        let (color, depth) = reader.output_color_type();
        let channels = color.samples() as u8;
        let precision = match depth {
            png::BitDepth::Eight => "uint8",
            png::BitDepth::Sixteen => "uint16",
            _ => return Err(rejected("PNG precision unsupported")),
        };
        let (width, height) = (info.width, info.height);
        let interlaced = info.interlaced;
        geometry(width, height, if precision == "uint16" { 2 } else { 1 })?;
        let icc = info.icc_profile.as_ref().map(|v| v.to_vec());
        let mut rows = 0u32;
        while reader
            .next_row()
            .map_err(|e| rejected(e.to_string()))?
            .is_some()
        {
            rows = rows
                .checked_add(1)
                .ok_or_else(|| rejected("PNG rows overflow"))?;
        }
        if !interlaced && rows != height {
            return Err(rejected("PNG decoded rows differ"));
        }
        reader.finish().map_err(|e| rejected(e.to_string()))?;
        ("png", precision, width, height, channels, icc)
    } else if magic[..2] == [0xff, 0xd8] {
        let mut decoder = jpeg_decoder::Decoder::new(BufReader::new(File::open(path)?));
        decoder.set_max_decoding_buffer_size(DECODED_MAX);
        decoder.read_info().map_err(|e| rejected(e.to_string()))?;
        let declared = decoder
            .info()
            .ok_or_else(|| rejected("JPEG declared metadata missing"))?;
        geometry(u32::from(declared.width), u32::from(declared.height), 1)?;
        let pixels = decoder.decode().map_err(|e| rejected(e.to_string()))?;
        let info = decoder
            .info()
            .ok_or_else(|| rejected("JPEG decoded metadata missing"))?;
        let (width, height) = (u32::from(info.width), u32::from(info.height));
        geometry(width, height, 1)?;
        let channels = match info.pixel_format {
            jpeg_decoder::PixelFormat::RGB24 => 3,
            jpeg_decoder::PixelFormat::L8 => 1,
            jpeg_decoder::PixelFormat::CMYK32 => 4,
            _ => return Err(rejected("JPEG precision unsupported")),
        };
        if pixels.len() != width as usize * height as usize * usize::from(channels) {
            return Err(rejected("JPEG decoded contract differs"));
        }
        (
            "jpeg",
            "uint8",
            width,
            height,
            channels,
            decoder.icc_profile(),
        )
    } else if magic[..4] == *b"II\x2a\x00" || magic[..4] == *b"MM\x00\x2a" {
        use ::tiff::{
            ColorType,
            decoder::{Decoder, DecodingResult, Limits},
            tags::Tag,
        };
        let mut limits = Limits::default();
        limits.decoding_buffer_size = DECODED_MAX;
        limits.intermediate_buffer_size = DECODED_MAX;
        let mut decoder = Decoder::new(BufReader::new(File::open(path)?))
            .map_err(|e| rejected(e.to_string()))?
            .with_limits(limits);
        let (width, height) = decoder.dimensions().map_err(|e| rejected(e.to_string()))?;
        let (bits, channels) = match decoder.colortype().map_err(|e| rejected(e.to_string()))? {
            ColorType::RGB(bits) => (bits, 3),
            ColorType::RGBA(bits) => (bits, 4),
            ColorType::Gray(bits) => (bits, 1),
            ColorType::GrayA(bits) => (bits, 2),
            _ => return Err(rejected("TIFF color type unsupported")),
        };
        geometry(width, height, usize::from(bits) / 8)?;
        let icc = decoder
            .find_tag(Tag::Unknown(34675))
            .map_err(|e| rejected(e.to_string()))?
            .map(|v| v.into_u8_vec())
            .transpose()
            .map_err(|e| rejected(e.to_string()))?;
        let count = if decoder
            .find_tag(Tag::TileOffsets)
            .map_err(|e| rejected(e.to_string()))?
            .is_some()
        {
            decoder.tile_count()
        } else {
            decoder.strip_count()
        }
        .map_err(|e| rejected(e.to_string()))?;
        if count == 0 || count > 1_000_000 {
            return Err(rejected("TIFF chunk count exceeds bound"));
        }
        let mut samples = 0usize;
        let mut precision = None;
        for index in 0..count {
            let chunk = decoder
                .read_chunk(index)
                .map_err(|e| rejected(e.to_string()))?;
            let (kind, length) = match chunk {
                DecodingResult::F32(values) => {
                    if values.iter().any(|v| !v.is_finite()) {
                        return Err(rejected("TIFF has nonfinite samples"));
                    }
                    ("float32", values.len())
                }
                DecodingResult::U8(values) => ("uint8", values.len()),
                DecodingResult::U16(values) => ("uint16", values.len()),
                _ => return Err(rejected("TIFF sample type unsupported")),
            };
            if precision.is_some_and(|p| p != kind) {
                return Err(rejected("TIFF mixed sample types"));
            }
            precision = Some(kind);
            samples = samples
                .checked_add(length)
                .ok_or_else(|| rejected("TIFF samples overflow"))?;
        }
        if samples != width as usize * height as usize * usize::from(channels)
            || decoder.more_images()
        {
            return Err(rejected("TIFF decoded extent differs"));
        }
        let precision = precision.ok_or_else(|| rejected("TIFF has no samples"))?;
        ("tiff", precision, width, height, channels, icc)
    } else {
        return Err(rejected("image signature unsupported"));
    };
    let icc_sha256 = icc.as_ref().map(|icc| format!("{:x}", Sha256::digest(icc)));
    let mut hasher = Sha256::new();
    let mut file = File::open(path)?;
    let mut buffer = [0; 65536];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(ImageFacts {
        format,
        precision,
        width,
        height,
        channels,
        bytes: metadata.len(),
        sha256: format!("{:x}", hasher.finalize()),
        icc_sha256,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn decoded_geometry_is_bounded_without_allocation() {
        assert!(geometry(100, 200, 4).is_ok());
        assert!(geometry(0, 200, 4).is_err());
        assert!(geometry(u32::MAX, u32::MAX, 4).is_err());
    }
    #[test]
    fn unprofiled_png_is_inspected_but_never_qualified() {
        let directory = crate::Directory::open("/tmp").unwrap();
        let name = crate::identity("image-test").unwrap();
        let mut file = directory.create_file(&name, 0o600).unwrap();
        {
            let mut encoder = png::Encoder::new(&mut file, 2, 1);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[0, 1, 2, 3, 4, 5])
                .unwrap();
        }
        let facts = inspect_image(&mut file, 1024 * 1024).unwrap();
        assert_eq!((facts.width, facts.height, facts.channels), (2, 1, 3));
        assert_eq!(facts.icc_sha256, None);
        let path = directory.path().join(&name);
        assert!(validate(&path, 1024 * 1024, &"a".repeat(64)).is_err());
        file.set_len(40).unwrap();
        assert!(inspect_image(&mut file, 1024 * 1024).is_err());
        directory.remove_file(&name).unwrap();
    }
}
