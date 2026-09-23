use std::fmt;
use std::sync::OnceLock;

const MAXIMUM_INPUT_BYTES: u64 = 128 * 1024 * 1024;
const MAXIMUM_PIXELS: u64 = 100_000_000;
const MAXIMUM_OUTPUT_BYTES: u64 = 64 * 1024 * 1024;

#[repr(C)]
struct NativeResult {
    width: u32,
    height: u32,
    profile: i32,
    bytes: *mut u8,
    length: u64,
}

unsafe extern "C" {
    fn slipstream_vips_initialize() -> i32;
    fn slipstream_vips_process_jpeg(
        bytes: *const u8,
        length: usize,
        container_orientation: i32,
        target_long_edge: u32,
        maximum_input_bytes: u64,
        maximum_pixels: u64,
        maximum_output_bytes: u64,
        result: *mut NativeResult,
    ) -> i32;
    fn slipstream_vips_result_free(result: *mut NativeResult);
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DerivativeTarget {
    Thumbnail512,
    Review2560,
}

impl DerivativeTarget {
    pub const fn long_edge(self) -> u32 {
        match self {
            Self::Thumbnail512 => 512,
            Self::Review2560 => 2560,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DerivativeProfile {
    Srgb,
    PreservedIcc,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Derivative {
    pub width: u32,
    pub height: u32,
    pub profile: DerivativeProfile,
    pub jpeg: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DerivativeError {
    Unsupported,
    Malformed,
    ResourceLimit,
    OutputLimit,
    Internal,
}

impl fmt::Display for DerivativeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Unsupported => "Derivative input is unsupported",
            Self::Malformed => "Derivative input is malformed",
            Self::ResourceLimit => "Derivative input exceeds resource limits",
            Self::OutputLimit => "Derivative output exceeds resource limits",
            Self::Internal => "Derivative processing failed internally",
        })
    }
}

impl std::error::Error for DerivativeError {}

static INITIALIZED: OnceLock<Result<(), DerivativeError>> = OnceLock::new();

fn initialize() -> Result<(), DerivativeError> {
    *INITIALIZED.get_or_init(|| {
        // SAFETY: the C ABI performs process-global initialization exactly once;
        // no shutdown operation is exposed, so later Libraries can safely reuse it.
        let status = unsafe { slipstream_vips_initialize() };
        if status == 0 {
            Ok(())
        } else {
            Err(DerivativeError::Internal)
        }
    })
}

fn empty_result() -> NativeResult {
    NativeResult {
        width: 0,
        height: 0,
        profile: 0,
        bytes: std::ptr::null_mut(),
        length: 0,
    }
}

fn status_error(status: i32) -> DerivativeError {
    match status {
        1 => DerivativeError::Unsupported,
        2 => DerivativeError::Malformed,
        3 => DerivativeError::ResourceLimit,
        5 => DerivativeError::OutputLimit,
        _ => DerivativeError::Internal,
    }
}

pub fn process_jpeg(bytes: &[u8], target: DerivativeTarget) -> Result<Derivative, DerivativeError> {
    process_jpeg_with_orientation(bytes, None, target)
}

pub(crate) fn process_jpeg_with_orientation(
    bytes: &[u8],
    container_orientation: Option<u8>,
    target: DerivativeTarget,
) -> Result<Derivative, DerivativeError> {
    initialize()?;
    if bytes.is_empty() || bytes.len() as u64 > MAXIMUM_INPUT_BYTES {
        return Err(DerivativeError::ResourceLimit);
    }
    if container_orientation.is_some_and(|value| !(1..=8).contains(&value)) {
        return Err(DerivativeError::Internal);
    }
    let mut result = empty_result();
    // SAFETY: `bytes` remains borrowed for the complete synchronous native call;
    // `result` is writable and freed through the matching C ABI below.
    let status = unsafe {
        slipstream_vips_process_jpeg(
            bytes.as_ptr(),
            bytes.len(),
            i32::from(container_orientation.unwrap_or(0)),
            target.long_edge(),
            MAXIMUM_INPUT_BYTES,
            MAXIMUM_PIXELS,
            MAXIMUM_OUTPUT_BYTES,
            &mut result,
        )
    };
    if status != 0 {
        // SAFETY: the shim accepts an empty result on every failure path.
        unsafe { slipstream_vips_result_free(&mut result) };
        return Err(status_error(status));
    }
    if result.bytes.is_null() || result.length == 0 || result.width == 0 || result.height == 0 {
        // SAFETY: release a malformed success result before returning the typed error.
        unsafe { slipstream_vips_result_free(&mut result) };
        return Err(DerivativeError::Internal);
    }
    if result.length > MAXIMUM_OUTPUT_BYTES {
        // SAFETY: release native memory before reporting the output limit.
        unsafe { slipstream_vips_result_free(&mut result) };
        return Err(DerivativeError::OutputLimit);
    }
    let width = result.width;
    let height = result.height;
    let profile = match result.profile {
        0 => DerivativeProfile::Srgb,
        1 => DerivativeProfile::PreservedIcc,
        _ => {
            // SAFETY: release native memory before reporting an unknown ABI value.
            unsafe { slipstream_vips_result_free(&mut result) };
            return Err(DerivativeError::Internal);
        }
    };
    // SAFETY: successful native output owns `length` initialized bytes until free.
    let jpeg = unsafe { std::slice::from_raw_parts(result.bytes, result.length as usize) }.to_vec();
    // SAFETY: matching native deallocation after copying into Rust ownership.
    unsafe { slipstream_vips_result_free(&mut result) };
    Ok(Derivative {
        width,
        height,
        profile,
        jpeg,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{
        ColorType, DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder,
        codecs::jpeg::{JpegDecoder, JpegEncoder},
        metadata::Orientation,
    };
    use lcms2::Profile;

    fn pattern(width: u32, height: u32) -> Vec<u8> {
        let mut pixels = vec![0; width as usize * height as usize * 3];
        for y in 0..height {
            for x in 0..width {
                let offset = (y * width + x) as usize * 3;
                pixels[offset] = (x * 19 + y * 7) as u8;
                pixels[offset + 1] = (x * 3 + y * 23) as u8;
                pixels[offset + 2] = if x < width / 2 { 31 } else { 223 };
            }
        }
        pixels
    }

    fn encode(width: u32, height: u32) -> Vec<u8> {
        let mut jpeg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpeg, 100)
            .encode(
                &pattern(width, height),
                width,
                height,
                ExtendedColorType::Rgb8,
            )
            .unwrap();
        jpeg
    }

    fn directional_jpeg() -> Vec<u8> {
        const COLORS: [[u8; 3]; 4] = [[230, 20, 20], [20, 220, 20], [20, 20, 230], [230, 220, 20]];
        let (width, height) = (120, 80);
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

    fn corner_directions(jpeg: &[u8]) -> [usize; 4] {
        const COLORS: [[i32; 3]; 4] = [[230, 20, 20], [20, 220, 20], [20, 20, 230], [230, 220, 20]];
        let decoded =
            DynamicImage::from_decoder(JpegDecoder::new(std::io::Cursor::new(jpeg)).unwrap())
                .unwrap()
                .to_rgb8();
        let samples = [
            (decoded.width() / 4, decoded.height() / 4),
            (decoded.width() * 3 / 4, decoded.height() / 4),
            (decoded.width() / 4, decoded.height() * 3 / 4),
            (decoded.width() * 3 / 4, decoded.height() * 3 / 4),
        ];
        samples.map(|(x, y)| {
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

    fn exif_orientation(value: u16) -> Vec<u8> {
        let mut exif =
            b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x01\0\0\0\0\0\0\0\0\0\0\0\0"
                .to_vec();
        exif[24..26].copy_from_slice(&value.to_le_bytes());
        exif
    }

    fn exif_without_orientation() -> Vec<u8> {
        b"Exif\0\0II\x2a\0\x08\0\0\0\0\0\0\0\0\0\0\0".to_vec()
    }

    fn insert_app1(jpeg: &[u8], payload: &[u8]) -> Vec<u8> {
        let length = u16::try_from(payload.len() + 2).unwrap();
        let mut result = Vec::with_capacity(jpeg.len() + payload.len() + 4);
        result.extend_from_slice(&jpeg[..2]);
        result.extend_from_slice(&[0xff, 0xe1]);
        result.extend_from_slice(&length.to_be_bytes());
        result.extend_from_slice(payload);
        result.extend_from_slice(&jpeg[2..]);
        result
    }

    fn output_metadata(bytes: &[u8]) -> (u32, u32, Option<Orientation>, Vec<u8>) {
        let mut decoder = JpegDecoder::new(std::io::Cursor::new(bytes)).unwrap();
        let orientation = decoder.orientation().unwrap();
        let exif = decoder.exif_metadata().unwrap();
        let dimensions = decoder.dimensions();
        (
            dimensions.0,
            dimensions.1,
            Some(orientation),
            exif.unwrap_or_default(),
        )
    }

    #[test]
    fn process_is_bounded_and_does_not_upscale() {
        let result = process_jpeg(&encode(80, 40), DerivativeTarget::Thumbnail512).unwrap();
        assert_eq!((result.width, result.height), (80, 40));
        assert_eq!(result.profile, DerivativeProfile::Srgb);
        assert!(result.jpeg.len() < MAXIMUM_OUTPUT_BYTES as usize);
        assert!(process_jpeg(&[1, 2, 3], DerivativeTarget::Thumbnail512).is_err());
        assert_eq!(
            process_jpeg(
                &vec![0; MAXIMUM_INPUT_BYTES as usize + 1],
                DerivativeTarget::Thumbnail512
            ),
            Err(DerivativeError::ResourceLimit)
        );
    }

    #[test]
    fn applies_all_exif_orientations_once_and_sanitizes_metadata() {
        for value in 1..=8 {
            let source = insert_app1(&encode(12, 8), &exif_orientation(value));
            let result = process_jpeg(&source, DerivativeTarget::Thumbnail512).unwrap();
            let expected = if matches!(value, 5..=8) {
                (8, 12)
            } else {
                (12, 8)
            };
            assert_eq!(
                (result.width, result.height),
                expected,
                "orientation {value}"
            );
            let (width, height, orientation, exif) = output_metadata(&result.jpeg);
            assert_eq!((width, height), expected);
            assert_eq!(orientation, Some(Orientation::NoTransforms));
            assert!(!exif.windows(2).any(|window| window == [0x12, 0x01]));
        }
    }

    #[test]
    fn raw_container_fallback_preserves_every_direction_for_both_targets() {
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
        for target in [DerivativeTarget::Thumbnail512, DerivativeTarget::Review2560] {
            for orientation in 1..=8_u8 {
                let result =
                    process_jpeg_with_orientation(&source, Some(orientation), target).unwrap();
                let expected_dimensions = if orientation >= 5 {
                    (80, 120)
                } else {
                    (120, 80)
                };
                assert_eq!(
                    (result.width, result.height),
                    expected_dimensions,
                    "target {target:?}, orientation {orientation}"
                );
                assert_eq!(
                    corner_directions(&result.jpeg),
                    expected_corners[usize::from(orientation - 1)],
                    "target {target:?}, orientation {orientation}"
                );
            }
        }
    }

    #[test]
    fn jpeg_direction_wins_and_missing_partial_or_invalid_direction_falls_back() {
        let source = directional_jpeg();
        let explicit_normal = insert_app1(&source, &exif_orientation(1));
        let normal = process_jpeg_with_orientation(
            &explicit_normal,
            Some(6),
            DerivativeTarget::Thumbnail512,
        )
        .unwrap();
        assert_eq!((normal.width, normal.height), (120, 80));
        assert_eq!(corner_directions(&normal.jpeg), [0, 1, 2, 3]);

        for lacking in [
            source.clone(),
            insert_app1(&source, &exif_without_orientation()),
            insert_app1(&source, &exif_orientation(9)),
        ] {
            let rotated =
                process_jpeg_with_orientation(&lacking, Some(6), DerivativeTarget::Thumbnail512)
                    .unwrap();
            assert_eq!((rotated.width, rotated.height), (80, 120));
            assert_eq!(corner_directions(&rotated.jpeg), [2, 0, 3, 1]);
        }

        let encoded_order = process_jpeg(&source, DerivativeTarget::Thumbnail512).unwrap();
        assert_eq!((encoded_order.width, encoded_order.height), (120, 80));
        assert_eq!(corner_directions(&encoded_order.jpeg), [0, 1, 2, 3]);
        assert_eq!(
            process_jpeg_with_orientation(&source, Some(9), DerivativeTarget::Thumbnail512),
            Err(DerivativeError::Internal)
        );
    }

    #[test]
    fn resizes_with_no_upscale_and_keeps_profile_classes_explicit() {
        let result = process_jpeg(&encode(3200, 2000), DerivativeTarget::Thumbnail512).unwrap();
        assert_eq!((result.width, result.height), (512, 320));
        assert_eq!(result.profile, DerivativeProfile::Srgb);

        let mut profiled = Vec::new();
        let mut encoder = JpegEncoder::new_with_quality(&mut profiled, 100);
        encoder
            .set_icc_profile(Profile::new_srgb().icc().unwrap())
            .unwrap();
        encoder
            .write_image(&pattern(32, 16), 32, 16, ColorType::Rgb8.into())
            .unwrap();
        let result = process_jpeg(&profiled, DerivativeTarget::Thumbnail512).unwrap();
        assert_eq!(result.profile, DerivativeProfile::PreservedIcc);
        let (_, _, _, _) = output_metadata(&result.jpeg);
        assert!(
            result
                .jpeg
                .windows(12)
                .any(|window| window == b"ICC_PROFILE\0")
        );
    }

    #[test]
    fn rejects_truncated_jpeg_and_never_exposes_shutdown() {
        let source = encode(24, 12);
        assert_eq!(
            process_jpeg(&source[..source.len() - 7], DerivativeTarget::Thumbnail512),
            Err(DerivativeError::Malformed)
        );
    }

    #[test]
    fn representative_pixels_are_stable_after_lanczos_normalization() {
        let source = encode(3200, 2000);
        let result = process_jpeg(&source, DerivativeTarget::Thumbnail512).unwrap();
        let decoded = DynamicImage::from_decoder(
            JpegDecoder::new(std::io::Cursor::new(&result.jpeg)).unwrap(),
        )
        .unwrap()
        .to_rgb8();
        for (x, y) in [(1, 1), (decoded.width() - 2, 1), (1, decoded.height() - 2)] {
            let pixel = decoded.get_pixel(x, y);
            assert!(pixel.0.iter().any(|channel| *channel > 0));
        }
    }
}
