use crate::confinement::{ConfinementError, OpenedOriginal};
use std::fmt;

const MAXIMUM_JPEG_BYTES: u64 = 128 * 1024 * 1024;
const MAXIMUM_PIXELS: u64 = 100_000_000;
const MAXIMUM_LIBRAW_MEMORY_MB: u32 = 256;

#[repr(C)]
struct NativeResult {
    candidate_index: i32,
    width: u32,
    height: u32,
    container_orientation: i32,
    bytes: *mut u8,
    length: u64,
}

#[repr(C)]
struct NativeCaptureTimeResult {
    has_timestamp: i32,
    year: i32,
    month: i32,
    day: i32,
    hour: i32,
    minute: i32,
    second: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct NativeCaptureTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

unsafe extern "C" {
    fn slipstream_inspect_jpeg_fd(
        fd: i32,
        maximum_bytes: u64,
        maximum_pixels: u64,
        result: *mut NativeResult,
    ) -> i32;
    fn slipstream_extract_embedded_jpeg_fd(
        fd: i32,
        maximum_jpeg_bytes: u64,
        maximum_pixels: u64,
        maximum_libraw_memory_mb: u32,
        result: *mut NativeResult,
    ) -> i32;
    fn slipstream_inspect_raw_capture_time_fd(
        fd: i32,
        maximum_libraw_memory_mb: u32,
        result: *mut NativeCaptureTimeResult,
    ) -> i32;
    fn slipstream_preview_result_free(result: *mut NativeResult);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativePreviewError {
    Unsupported,
    Malformed,
    Io,
    ResourceLimit,
    Internal,
    NoUsablePreview,
}

impl fmt::Display for NativePreviewError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unsupported => "Preview source is unsupported",
            Self::Malformed => "Preview source is malformed",
            Self::Io => "Preview source could not be read safely",
            Self::ResourceLimit => "Preview source exceeds resource limits",
            Self::Internal => "Preview extraction failed internally",
            Self::NoUsablePreview => "The RAW contains no usable embedded JPEG",
        })
    }
}

impl std::error::Error for NativePreviewError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativePreview {
    pub candidate_index: Option<u32>,
    pub width: u32,
    pub height: u32,
    /// Standard EXIF orientation supplied by the containing RAW, if any.
    /// JPEG Originals and RAW containers with unknown transforms use `None`.
    pub container_orientation: Option<u8>,
    pub jpeg: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InspectedPreviewSource {
    MatchingJpeg,
    EmbeddedRawJpeg,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InspectedPreview {
    pub source: InspectedPreviewSource,
    pub preview: NativePreview,
}

#[derive(Debug)]
pub enum PreviewError {
    Confinement(ConfinementError),
    Native(NativePreviewError),
}

impl fmt::Display for PreviewError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Confinement(error) => error.fmt(formatter),
            Self::Native(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for PreviewError {}

fn error_for_status(status: i32) -> NativePreviewError {
    match status {
        1 => NativePreviewError::Unsupported,
        2 => NativePreviewError::Malformed,
        3 => NativePreviewError::Io,
        4 => NativePreviewError::ResourceLimit,
        6 => NativePreviewError::NoUsablePreview,
        _ => NativePreviewError::Internal,
    }
}

fn take_result(
    status: i32,
    result: &mut NativeResult,
) -> Result<NativePreview, NativePreviewError> {
    if status != 0 {
        // SAFETY: the shim initializes the result before returning and accepts an empty result.
        unsafe { slipstream_preview_result_free(result) };
        return Err(error_for_status(status));
    }
    if result.length == 0 || result.bytes.is_null() || result.width == 0 || result.height == 0 {
        // SAFETY: releases any allocation returned with an invalid success result.
        unsafe { slipstream_preview_result_free(result) };
        return Err(NativePreviewError::Internal);
    }
    let candidate_index = (result.candidate_index >= 0).then_some(result.candidate_index as u32);
    let container_orientation = match result.container_orientation {
        0 => None,
        value @ 1..=8 => Some(value as u8),
        _ => {
            // SAFETY: release an allocation returned with an invalid ABI value.
            unsafe { slipstream_preview_result_free(result) };
            return Err(NativePreviewError::Internal);
        }
    };
    let width = result.width;
    let height = result.height;
    // SAFETY: a successful shim call owns `length` initialized bytes until the matching free call.
    let jpeg = unsafe { std::slice::from_raw_parts(result.bytes, result.length as usize) }.to_vec();
    // SAFETY: matching release for the native allocation after copying into Rust ownership.
    unsafe { slipstream_preview_result_free(result) };
    Ok(NativePreview {
        candidate_index,
        width,
        height,
        container_orientation,
        jpeg,
    })
}

fn empty_result() -> NativeResult {
    NativeResult {
        candidate_index: -1,
        width: 0,
        height: 0,
        container_orientation: 0,
        bytes: std::ptr::null_mut(),
        length: 0,
    }
}

pub(crate) fn inspect_raw_capture_time(
    opened: &OpenedOriginal,
) -> Result<Option<NativeCaptureTime>, NativePreviewError> {
    let mut result = NativeCaptureTimeResult {
        has_timestamp: 0,
        year: 0,
        month: 0,
        day: 0,
        hour: 0,
        minute: 0,
        second: 0,
    };
    // SAFETY: the retained descriptor remains open for LibRaw's metadata-only
    // open, and the native result is valid writable storage.
    let status = unsafe {
        slipstream_inspect_raw_capture_time_fd(
            opened.descriptor(),
            MAXIMUM_LIBRAW_MEMORY_MB,
            &mut result,
        )
    };
    if status != 0 {
        return Err(error_for_status(status));
    }
    if result.has_timestamp == 0 {
        return Ok(None);
    }
    let valid = result.has_timestamp == 1
        && (1..=9999).contains(&result.year)
        && (1..=12).contains(&result.month)
        && (1..=31).contains(&result.day)
        && (0..=23).contains(&result.hour)
        && (0..=59).contains(&result.minute)
        && (0..=59).contains(&result.second);
    if !valid {
        return Err(NativePreviewError::Internal);
    }
    Ok(Some(NativeCaptureTime {
        year: result.year as u16,
        month: result.month as u8,
        day: result.day as u8,
        hour: result.hour as u8,
        minute: result.minute as u8,
        second: result.second as u8,
    }))
}

fn inspect_opened(opened: &OpenedOriginal) -> Result<NativePreview, PreviewError> {
    let mut result = empty_result();
    // SAFETY: `opened` retains the borrowed descriptor and `result` is writable for the call.
    let status = unsafe {
        slipstream_inspect_jpeg_fd(
            opened.descriptor(),
            MAXIMUM_JPEG_BYTES,
            MAXIMUM_PIXELS,
            &mut result,
        )
    };
    let outcome = take_result(status, &mut result).map_err(PreviewError::Native);
    // The descriptor check is mandatory on every native path, including
    // malformed and resource-limited failures. A changed Original wins over
    // the native classification because the bytes are no longer trustworthy.
    opened
        .verify_unchanged()
        .map_err(PreviewError::Confinement)?;
    outcome
}

fn extract_opened(opened: &OpenedOriginal) -> Result<NativePreview, PreviewError> {
    let mut result = empty_result();
    // SAFETY: `opened` retains the borrowed descriptor and `result` is writable for the call.
    let status = unsafe {
        slipstream_extract_embedded_jpeg_fd(
            opened.descriptor(),
            MAXIMUM_JPEG_BYTES,
            MAXIMUM_PIXELS,
            MAXIMUM_LIBRAW_MEMORY_MB,
            &mut result,
        )
    };
    let outcome = take_result(status, &mut result).map_err(PreviewError::Native);
    opened
        .verify_unchanged()
        .map_err(PreviewError::Confinement)?;
    outcome
}

pub fn inspect_matching_jpeg(
    capability: &crate::OriginalCapability,
) -> Result<NativePreview, PreviewError> {
    let opened = capability
        .open_revision_checked()
        .map_err(PreviewError::Confinement)?;
    inspect_opened(&opened)
}

pub fn extract_embedded_jpeg(
    capability: &crate::OriginalCapability,
) -> Result<NativePreview, PreviewError> {
    let opened = capability
        .open_revision_checked()
        .map_err(PreviewError::Confinement)?;
    extract_opened(&opened)
}

pub fn inspect_preview_source(
    matching_jpeg: Option<&crate::OriginalCapability>,
    raw: Option<&crate::OriginalCapability>,
) -> Result<InspectedPreview, PreviewError> {
    if let Some(jpeg) = matching_jpeg {
        match inspect_matching_jpeg(jpeg) {
            Ok(preview) => {
                return Ok(InspectedPreview {
                    source: InspectedPreviewSource::MatchingJpeg,
                    preview,
                });
            }
            Err(PreviewError::Native(
                NativePreviewError::Malformed
                | NativePreviewError::Unsupported
                | NativePreviewError::NoUsablePreview,
            )) => {}
            Err(error) => return Err(error),
        }
    }
    let raw = raw.ok_or(PreviewError::Native(NativePreviewError::NoUsablePreview))?;
    Ok(InspectedPreview {
        source: InspectedPreviewSource::EmbeddedRawJpeg,
        preview: extract_embedded_jpeg(raw)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        LibraryRoot, RelativeOriginalPath,
        test_support::{generated_dng, generated_dng_candidates, original_snapshot, raw_sample},
    };
    use image::{
        DynamicImage, ExtendedColorType,
        codecs::jpeg::{JpegDecoder, JpegEncoder},
    };
    use std::{
        fs,
        io::Cursor,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let pixels = vec![127_u8; width as usize * height as usize * 3];
        let mut bytes = Vec::new();
        JpegEncoder::new_with_quality(&mut bytes, 85)
            .encode(&pixels, width, height, ExtendedColorType::Rgb8)
            .unwrap();
        bytes
    }

    fn directional_jpeg() -> Vec<u8> {
        const COLORS: [[u8; 3]; 4] = [[230, 20, 20], [20, 220, 20], [20, 20, 230], [230, 220, 20]];
        let (width, height) = (120_usize, 80_usize);
        let mut pixels = vec![0; width * height * 3];
        for y in 0..height {
            for x in 0..width {
                let quadrant = usize::from(x >= width / 2) + 2 * usize::from(y >= height / 2);
                let offset = (y * width + x) * 3;
                pixels[offset..offset + 3].copy_from_slice(&COLORS[quadrant]);
            }
        }
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, 100)
            .encode(
                &pixels,
                width as u32,
                height as u32,
                ExtendedColorType::Rgb8,
            )
            .unwrap();
        jpeg
    }

    fn insert_exif_orientation(jpeg: &[u8], orientation: Option<u16>) -> Vec<u8> {
        let mut exif = b"Exif\0\0II\x2a\0\x08\0\0\0\0\0\0\0\0\0\0\0".to_vec();
        if let Some(value) = orientation {
            exif =
                b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x01\0\0\0\0\0\0\0\0\0\0\0"
                    .to_vec();
            exif[24..26].copy_from_slice(&value.to_le_bytes());
        }
        let length = u16::try_from(exif.len() + 2).unwrap();
        let mut result = jpeg[..2].to_vec();
        result.extend_from_slice(&[0xff, 0xe1]);
        result.extend_from_slice(&length.to_be_bytes());
        result.extend_from_slice(&exif);
        result.extend_from_slice(&jpeg[2..]);
        result
    }

    fn corner_directions(jpeg: &[u8]) -> [usize; 4] {
        const COLORS: [[i32; 3]; 4] = [[230, 20, 20], [20, 220, 20], [20, 20, 230], [230, 220, 20]];
        let decoded = DynamicImage::from_decoder(JpegDecoder::new(Cursor::new(jpeg)).unwrap())
            .unwrap()
            .to_rgb8();
        [
            (decoded.width() / 4, decoded.height() / 4),
            (decoded.width() * 3 / 4, decoded.height() / 4),
            (decoded.width() / 4, decoded.height() * 3 / 4),
            (decoded.width() * 3 / 4, decoded.height() * 3 / 4),
        ]
        .map(|(x, y)| {
            let pixel = decoded.get_pixel(x, y).0.map(i32::from);
            COLORS
                .iter()
                .enumerate()
                .min_by_key(|(_, color)| {
                    pixel
                        .iter()
                        .zip(color.iter())
                        .map(|(actual, expected)| (actual - expected).pow(2))
                        .sum::<i32>()
                })
                .unwrap()
                .0
        })
    }

    fn with_capability<T>(
        name: &str,
        bytes: &[u8],
        action: impl FnOnce(&crate::OriginalCapability) -> T,
    ) -> T {
        let base = std::env::temp_dir().join(format!(
            "slipstream-native-{}-{}-{}",
            std::process::id(),
            NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed),
            name.replace('.', "-")
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir(&base).unwrap();
        fs::write(base.join(name), bytes).unwrap();
        let root = LibraryRoot::open(&base).unwrap();
        let capability = root
            .original(RelativeOriginalPath::parse(name).unwrap())
            .unwrap();
        let result = action(&capability);
        drop(root);
        let _ = fs::remove_dir_all(base);
        result
    }

    #[test]
    fn validates_matching_jpeg_and_rejects_malformed_bytes() {
        let valid = with_capability("valid.JPG", &jpeg(13, 7), inspect_matching_jpeg).unwrap();
        assert_eq!((valid.width, valid.height), (13, 7));
        assert_eq!(valid.candidate_index, None);
        assert_eq!(valid.container_orientation, None);
        assert!(matches!(
            with_capability("invalid.JPG", b"not jpeg", inspect_matching_jpeg),
            Err(PreviewError::Native(NativePreviewError::Malformed))
        ));
    }

    #[test]
    fn generated_raw_container_exposes_embedded_jpeg_and_orientation() {
        let source = jpeg(120, 80);
        let raw = generated_dng(&source, 6);
        let preview = with_capability("generated.DNG", &raw, extract_embedded_jpeg).unwrap();
        assert_eq!(preview.candidate_index, Some(0));
        assert_eq!((preview.width, preview.height), (120, 80));
        assert_eq!(preview.container_orientation, Some(6));
        assert_eq!(preview.jpeg, source);
    }

    #[test]
    fn generated_raw_selects_largest_complete_jpeg_and_falls_back() {
        let small = jpeg(60, 40);
        let large = jpeg(120, 80);
        let raw = generated_dng_candidates(&[(60, 40, &small), (120, 80, &large)], 1);
        let selected = with_capability("largest.DNG", &raw, extract_embedded_jpeg).unwrap();
        assert_eq!(selected.candidate_index, Some(1));
        assert_eq!((selected.width, selected.height), (120, 80));

        let truncated = &large[..large.len() - 7];
        let raw = generated_dng_candidates(&[(120, 80, truncated), (60, 40, &small)], 1);
        let fallback = with_capability("fallback.DNG", &raw, extract_embedded_jpeg).unwrap();
        assert_eq!(fallback.candidate_index, Some(1));
        assert_eq!((fallback.width, fallback.height), (60, 40));
        assert_eq!(fallback.jpeg, small);
    }

    #[test]
    fn generated_raw_extraction_to_derivative_preserves_orientation_precedence() {
        let source = directional_jpeg();
        let expected_corners = [
            [0, 1, 2, 3],
            [1, 0, 3, 2],
            [3, 2, 1, 0],
            [2, 3, 0, 1],
            [0, 2, 1, 3],
            [2, 0, 3, 1],
            [3, 1, 2, 0],
            [1, 3, 0, 2],
        ];
        for orientation in 1..=8_u16 {
            let raw = generated_dng(&source, orientation);
            let preview = with_capability("direction.DNG", &raw, extract_embedded_jpeg).unwrap();
            // The generated candidate records tflip=0. The container transform
            // must still cross the extraction boundary.
            assert_eq!(preview.container_orientation, Some(orientation as u8));
            if orientation == 6 {
                // This is the prior pipeline behavior: JPEG-only normalization
                // loses the portrait container transform and stays landscape.
                let legacy =
                    crate::process_jpeg(&preview.jpeg, crate::DerivativeTarget::Thumbnail512)
                        .unwrap();
                assert_eq!((legacy.width, legacy.height), (120, 80));
                assert_ne!(corner_directions(&legacy.jpeg), expected_corners[5]);
            }
            for target in [
                crate::DerivativeTarget::Thumbnail512,
                crate::DerivativeTarget::Review2560,
            ] {
                let derivative = crate::derivative::process_jpeg_with_orientation(
                    &preview.jpeg,
                    preview.container_orientation,
                    target,
                )
                .unwrap();
                let expected_dimensions = if orientation >= 5 {
                    (80, 120)
                } else {
                    (120, 80)
                };
                assert_eq!((derivative.width, derivative.height), expected_dimensions);
                assert_eq!(
                    corner_directions(&derivative.jpeg),
                    expected_corners[orientation as usize - 1]
                );
            }
        }

        for (jpeg, expected) in [
            (insert_exif_orientation(&source, None), [2, 0, 3, 1]),
            (insert_exif_orientation(&source, Some(9)), [2, 0, 3, 1]),
            (insert_exif_orientation(&source, Some(1)), [0, 1, 2, 3]),
        ] {
            let raw = generated_dng(&jpeg, 6);
            let preview = with_capability("precedence.DNG", &raw, extract_embedded_jpeg).unwrap();
            let derivative = crate::derivative::process_jpeg_with_orientation(
                &preview.jpeg,
                preview.container_orientation,
                crate::DerivativeTarget::Thumbnail512,
            )
            .unwrap();
            assert_eq!(corner_directions(&derivative.jpeg), expected);
        }
    }

    #[test]
    fn raw_capture_time_wrapper_reports_a_native_status_for_non_raw_descriptor() {
        let result = with_capability("not-raw.ARW", b"not a raw container", |capability| {
            let opened = capability.open_revision_checked().unwrap();
            inspect_raw_capture_time(&opened)
        });
        assert!(matches!(
            result,
            Err(NativePreviewError::Unsupported
                | NativePreviewError::Malformed
                | NativePreviewError::Io
                | NativePreviewError::Internal)
        ));
    }

    #[test]
    fn matching_jpeg_wins_and_unusable_matching_jpeg_falls_back_only_to_raw() {
        let valid = jpeg(9, 5);
        with_capability("valid.JPG", &valid, |jpeg_capability| {
            let selected = inspect_preview_source(Some(jpeg_capability), None).unwrap();
            assert_eq!(selected.source, InspectedPreviewSource::MatchingJpeg);
        });
        let malformed = with_capability("invalid.JPG", b"bad", |jpeg_capability| {
            inspect_preview_source(Some(jpeg_capability), None)
        });
        assert!(matches!(
            malformed,
            Err(PreviewError::Native(NativePreviewError::NoUsablePreview))
        ));
    }

    #[test]
    #[ignore = "requires SLIPSTREAM_RAW_SAMPLE"]
    fn sony_embedded_preview_is_largest_usable_candidate_and_original_is_unchanged() {
        let (path, before) = raw_sample();
        let root = LibraryRoot::open(path.parent().unwrap()).unwrap();
        let relative = RelativeOriginalPath::parse(
            path.file_name()
                .unwrap()
                .to_str()
                .expect("sample path must be UTF-8"),
        )
        .unwrap();
        let capability = root.original(relative).unwrap();
        let preview = extract_embedded_jpeg(&capability).unwrap();
        assert_eq!(preview.candidate_index, Some(2));
        assert_eq!((preview.width, preview.height), (9504, 6336));
        assert_eq!(preview.container_orientation, Some(6));
        assert_eq!(original_snapshot(&path), before);
        let mut decoder = image::ImageReader::new(Cursor::new(preview.jpeg));
        decoder.set_format(image::ImageFormat::Jpeg);
        assert_eq!(decoder.into_dimensions().unwrap(), (9504, 6336));
    }
}
