use std::fmt;
use std::os::fd::RawFd;
use std::sync::OnceLock;

const MAXIMUM_INPUT_BYTES: u64 = 128 * 1024 * 1024;
const MAXIMUM_PIXELS: u64 = 100_000_000;
const MAXIMUM_OUTPUT_BYTES: u64 = 64 * 1024 * 1024;

/// A Development TIFF is machine-generated and can be larger than a camera
/// JPEG, so it gets its own bounded ceiling. The 63 MP full-size artifact of
/// the qualified RAW run is about 640 MB.
const MAXIMUM_DEVELOPMENT_TIFF_BYTES: u64 = 1024 * 1024 * 1024;

/// Versioned identity of the Development Result to sRGB display conversion.
/// The matrix below is that identity; changing either requires a new version
/// and a new qualification record.
pub const DISPLAY_TRANSFORM_VERSION: &str = "display-transform-v1";

/// Linear ProPhoto RGB to linear sRGB, Bradford D50-to-D65 adaptation, derived
/// from the two pinned ICC profile assets and pinned by
/// `design/development-color.md#display-and-comparison`. Every row sums to 1.0,
/// so linear ProPhoto white maps to linear sRGB `[1, 1, 1]` without
/// normalization.
const DISPLAY_TRANSFORM_MATRIX: [[f64; 3]; 3] = [
    [2.034390615588929, -0.727658826450461, -0.306731789138469],
    [-0.228838423556639, 1.231758946896254, -0.002920523339614],
    [-0.008543280951811, -0.153257113180394, 1.161800394132204],
];

/// The pinned destination profile asset, embedded in every display derivative.
const DESTINATION_PROFILE_ASSET: &[u8] = include_bytes!("../assets/srgb-iec61966-2-1.icc");

/// Source profile digests accepted for a Development Result: the pinned asset
/// and the legacy-normalized profile the qualified darktable run embeds. They
/// differ only in description-tag bytes; colorants, white point, and the linear
/// transfer curves are identical, so both describe the same display transform.
const ACCEPTED_SOURCE_PROFILE_DIGESTS: [&str; 2] = [
    "df7b2c677645f1ca5364b52e62f8db04ca61f80163792942f3e409a84a6b12ed",
    "7bef28a81c974482756f09c7d34c55d53549ba450f26185b2c16f6228af96dfe",
];

#[repr(C)]
struct NativeResult {
    width: u32,
    height: u32,
    profile: i32,
    bytes: *mut u8,
    length: u64,
}

#[repr(C)]
struct NativeLinearResult {
    width: u32,
    height: u32,
    pixels: *mut f32,
    length_bytes: usize,
    profile: *mut u8,
    profile_length: usize,
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
    fn slipstream_vips_linear_from_fd(
        fd: i32,
        target_long_edge: u32,
        maximum_bytes: u64,
        maximum_pixels: u64,
        result: *mut NativeLinearResult,
    ) -> i32;
    fn slipstream_vips_linear_result_free(result: *mut NativeLinearResult);
    fn slipstream_vips_encode_srgb8(
        pixels: *const u8,
        width: u32,
        height: u32,
        profile: *const u8,
        profile_length: usize,
        maximum_output_bytes: u64,
        result: *mut NativeResult,
    ) -> i32;
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
    take_encoded_result(status, &mut result)
}

/// Convert one Development Result into the fixed sRGB display derivative.
///
/// `fd` must be a read-only descriptor for a float32 RGB TIFF that carries the
/// pinned linear ProPhoto RGB source profile. The descriptor is read only. The
/// linear samples are resampled in their own light before the pinned matrix,
/// the per-channel clip, and the sRGB transfer function are applied, so this
/// branch is the only place that clips, exactly as
/// `design/development-color.md#display-and-comparison` defines.
///
/// The result is a display derivative of a Development Result. It must never
/// replace, or be written back into, the Development TIFF or the Film input.
///
/// This conversion is also an integrity gate for the reader's inputs. It
/// verifies the embedded source profile identity, refuses anything it cannot
/// decode as a float32 RGB TIFF, and loads with `fail_on` set to
/// `VIPS_FAIL_ON_WARNING`, so malformed input and decode failures inside the
/// container are refused instead of decoding as partial or black data. The
/// byte length and digest of a published artifact are still established by
/// the receipt that publishes it.
pub fn process_development_tiff(
    fd: RawFd,
    target: DerivativeTarget,
) -> Result<Derivative, DerivativeError> {
    initialize()?;
    if fd < 0 {
        return Err(DerivativeError::Internal);
    }
    let mut linear = empty_linear_result();
    // SAFETY: the borrowed descriptor stays open for the complete synchronous
    // native call, and the result is freed through the matching C ABI below.
    let status = unsafe {
        slipstream_vips_linear_from_fd(
            fd,
            target.long_edge(),
            MAXIMUM_DEVELOPMENT_TIFF_BYTES,
            MAXIMUM_PIXELS,
            &mut linear,
        )
    };
    if status != 0 {
        // SAFETY: the shim accepts an empty result on every failure path.
        unsafe { slipstream_vips_linear_result_free(&mut linear) };
        return Err(status_error(status));
    }
    let (width, height, planes, profile) = take_linear_result(&mut linear)?;
    if !ACCEPTED_SOURCE_PROFILE_DIGESTS.contains(&hex_digest(&profile).as_str()) {
        return Err(DerivativeError::Malformed);
    }
    if planes.len() != width as usize * height as usize * 3 {
        return Err(DerivativeError::Internal);
    }
    let encoded = to_srgb8(&planes);
    encode_srgb8(&encoded, width, height)
}

fn take_linear_result(
    linear: &mut NativeLinearResult,
) -> Result<(u32, u32, Vec<f32>, Vec<u8>), DerivativeError> {
    let width = linear.width;
    let height = linear.height;
    let valid = width != 0
        && height != 0
        && height as u64 * u64::from(width) * 3 == (linear.length_bytes / 4) as u64
        && !linear.pixels.is_null()
        && !linear.profile.is_null()
        && linear.profile_length != 0;
    if !valid {
        // SAFETY: release an allocation returned with an invalid success result.
        unsafe { slipstream_vips_linear_result_free(linear) };
        return Err(DerivativeError::Internal);
    }
    // SAFETY: a successful call owns `length_bytes` initialized floats and
    // `profile_length` initialized profile bytes until the matching free.
    let planes = unsafe {
        std::slice::from_raw_parts(
            linear.pixels,
            linear.length_bytes / std::mem::size_of::<f32>(),
        )
    }
    .to_vec();
    // SAFETY: same allocation contract for the embedded profile bytes.
    let profile =
        unsafe { std::slice::from_raw_parts(linear.profile, linear.profile_length) }.to_vec();
    // SAFETY: matching release after copying both buffers into Rust ownership.
    unsafe { slipstream_vips_linear_result_free(linear) };
    Ok((width, height, planes, profile))
}

fn empty_linear_result() -> NativeLinearResult {
    NativeLinearResult {
        width: 0,
        height: 0,
        pixels: std::ptr::null_mut(),
        length_bytes: 0,
        profile: std::ptr::null_mut(),
        profile_length: 0,
    }
}

/// Apply the pinned matrix, clip each linear sRGB channel, apply the sRGB
/// transfer function, and quantize once to 8-bit.
fn to_srgb8(planes: &[f32]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(planes.len());
    for pixel in planes.chunks_exact(3) {
        let [red, green, blue] = [pixel[0], pixel[1], pixel[2]];
        for row in DISPLAY_TRANSFORM_MATRIX {
            let linear =
                row[0] * f64::from(red) + row[1] * f64::from(green) + row[2] * f64::from(blue);
            encoded.push(srgb_byte(linear));
        }
    }
    encoded
}

fn srgb_byte(linear: f64) -> u8 {
    let clipped = linear.clamp(0.0, 1.0);
    let display = if clipped <= 0.003_130_8 {
        clipped * 12.92
    } else {
        1.055 * clipped.powf(1.0 / 2.4) - 0.055
    };
    (display * 255.0).round().clamp(0.0, 255.0) as u8
}

fn encode_srgb8(pixels: &[u8], width: u32, height: u32) -> Result<Derivative, DerivativeError> {
    let mut result = empty_result();
    // SAFETY: `pixels` and the committed destination profile outlive the
    // synchronous call, and `result` is writable storage for the C ABI.
    let status = unsafe {
        slipstream_vips_encode_srgb8(
            pixels.as_ptr(),
            width,
            height,
            DESTINATION_PROFILE_ASSET.as_ptr(),
            DESTINATION_PROFILE_ASSET.len(),
            MAXIMUM_OUTPUT_BYTES,
            &mut result,
        )
    };
    take_encoded_result(status, &mut result)
}

fn take_encoded_result(
    status: i32,
    result: &mut NativeResult,
) -> Result<Derivative, DerivativeError> {
    if status != 0 {
        // SAFETY: the shim accepts an empty result on every failure path.
        unsafe { slipstream_vips_result_free(result) };
        return Err(status_error(status));
    }
    if result.bytes.is_null() || result.length == 0 || result.width == 0 || result.height == 0 {
        // SAFETY: release a malformed success result before returning the typed error.
        unsafe { slipstream_vips_result_free(result) };
        return Err(DerivativeError::Internal);
    }
    if result.length > MAXIMUM_OUTPUT_BYTES {
        // SAFETY: release native memory before reporting the output limit.
        unsafe { slipstream_vips_result_free(result) };
        return Err(DerivativeError::OutputLimit);
    }
    let width = result.width;
    let height = result.height;
    let profile = match result.profile {
        0 => DerivativeProfile::Srgb,
        1 => DerivativeProfile::PreservedIcc,
        _ => {
            // SAFETY: release native memory before reporting an unknown ABI value.
            unsafe { slipstream_vips_result_free(result) };
            return Err(DerivativeError::Internal);
        }
    };
    // SAFETY: successful native output owns `length` initialized bytes until free.
    let jpeg = unsafe { std::slice::from_raw_parts(result.bytes, result.length as usize) }.to_vec();
    // SAFETY: matching native deallocation after copying into Rust ownership.
    unsafe { slipstream_vips_result_free(result) };
    Ok(Derivative {
        width,
        height,
        profile,
        jpeg,
    })
}

fn hex_digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(bytes);
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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

    /// The inputs and expected bytes are the reference vectors recorded for
    /// `display-transform-v1`. They fail if the matrix, the clip, or the
    /// transfer function changes order or value.
    const PINNED_VECTORS: [([f64; 3], [u8; 3]); 10] = [
        ([0.0, 0.0, 0.0], [0, 0, 0]),
        ([0.18, 0.18, 0.18], [118, 118, 118]),
        ([1.0, 1.0, 1.0], [255, 255, 255]),
        ([1.0, 0.0, 0.0], [255, 0, 0]),
        ([0.0, 1.0, 0.0], [0, 255, 0]),
        ([0.0, 0.0, 1.0], [0, 0, 255]),
        ([2.0, 0.5, 0.125], [255, 111, 64]),
        ([-0.25, 0.5, 0.5], [0, 214, 189]),
        ([0.5, 0.5, 2.0], [56, 187, 255]),
        ([0.9, 0.2, 0.05], [255, 57, 38]),
    ];

    /// The pinned source profile asset, used to build fixture artifacts and to
    /// prove the committed asset still carries its recorded identity.
    const SOURCE_PROFILE_ASSET: &[u8] = include_bytes!("../assets/prophoto-linear-g10.icc");
    const SOURCE_PROFILE_ASSET_DIGEST: &str =
        "df7b2c677645f1ca5364b52e62f8db04ca61f80163792942f3e409a84a6b12ed";
    /// The source profile bytes the qualified darktable run embeds. Only the
    /// description tag differs from the pinned asset, so this is the artifact
    /// the accepted-digest list must not refuse.
    const QUALIFIED_RUN_PROFILE_ASSET: &[u8] =
        include_bytes!("../assets/prophoto-linear-g10-darktable.icc");
    const QUALIFIED_RUN_PROFILE_ASSET_DIGEST: &str =
        "7bef28a81c974482756f09c7d34c55d53549ba450f26185b2c16f6228af96dfe";
    const DESTINATION_PROFILE_ASSET_DIGEST: &str =
        "b44e86e44d44993a3a9a880626f9832e9c37e2234caba501548f9114bada6d21";

    struct Fixture {
        bytes: Vec<u8>,
    }

    impl Fixture {
        fn descriptor<T>(&self, use_descriptor: impl FnOnce(RawFd) -> T) -> T {
            use std::io::Write;
            use std::os::fd::AsRawFd;
            use std::sync::atomic::{AtomicU32, Ordering};
            static COUNTER: AtomicU32 = AtomicU32::new(0);
            let path = std::env::temp_dir().join(format!(
                "slipstream-display-fixture-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            let mut file = std::fs::File::create(&path).unwrap();
            file.write_all(&self.bytes).unwrap();
            file.flush().unwrap();
            let outcome = use_descriptor(file.as_raw_fd());
            drop(file);
            std::fs::remove_file(&path).unwrap();
            outcome
        }
    }

    /// Writes one uncompressed little-endian TIFF: `width * height` samples per
    /// band, three bands, one strip, and an embedded ICC profile.
    fn tiff_fixture(
        pixels: &[u8],
        width: u32,
        height: u32,
        bits: u16,
        sample_format: u16,
        profile: &[u8],
    ) -> Fixture {
        let entries: [[u32; 4]; 11] = [
            [256, 4, 1, width],
            [257, 4, 1, height],
            [258, 3, 3, 0],
            [259, 3, 1, 1],
            [262, 3, 1, 2],
            [273, 4, 1, 0],
            [277, 3, 1, 3],
            [278, 4, 1, height],
            [279, 4, 1, pixels.len() as u32],
            [339, 3, 3, 0],
            [34675, 7, profile.len() as u32, 0],
        ];
        let ifd_offset = 8u32;
        let ifd_bytes = 2 + entries.len() as u32 * 12 + 4;
        let bits_offset = align(ifd_offset + ifd_bytes, 2);
        let sample_offset = align(bits_offset + 6, 2);
        let profile_offset = align(sample_offset + 6, 2);
        let pixel_offset = align(profile_offset + profile.len() as u32, 4);

        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II\x2a\0");
        tiff.extend_from_slice(&ifd_offset.to_le_bytes());
        tiff.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for entry in entries {
            let value = match entry[0] {
                258 => bits_offset,
                273 => pixel_offset,
                339 => sample_offset,
                34675 => profile_offset,
                _ => entry[3],
            };
            tiff.extend_from_slice(&(entry[0] as u16).to_le_bytes());
            tiff.extend_from_slice(&(entry[1] as u16).to_le_bytes());
            tiff.extend_from_slice(&entry[2].to_le_bytes());
            if entry[1] == 3 && entry[2] == 1 {
                tiff.extend_from_slice(&(value as u16).to_le_bytes());
                tiff.extend_from_slice(&[0, 0]);
            } else {
                tiff.extend_from_slice(&value.to_le_bytes());
            }
        }
        tiff.extend_from_slice(&0u32.to_le_bytes());
        tiff.resize(bits_offset as usize, 0);
        for _ in 0..3 {
            tiff.extend_from_slice(&bits.to_le_bytes());
        }
        tiff.resize(sample_offset as usize, 0);
        for _ in 0..3 {
            tiff.extend_from_slice(&sample_format.to_le_bytes());
        }
        tiff.resize(profile_offset as usize, 0);
        tiff.extend_from_slice(profile);
        tiff.resize(pixel_offset as usize, 0);
        tiff.extend_from_slice(pixels);
        Fixture { bytes: tiff }
    }

    fn align(value: u32, boundary: u32) -> u32 {
        value + (boundary - value % boundary) % boundary
    }

    const DESCRIPTION_TAG: [u8; 4] = *b"desc";

    /// One flat 16x16 patch per value, so every JPEG block stays uniform and
    /// the decoded patch can be compared with its exact expected byte.
    const PATCH: u32 = 16;

    fn float_fixture(patches: &[[f64; 3]], profile: &[u8]) -> Fixture {
        let mut pixels = Vec::with_capacity(patches.len() * (PATCH * PATCH * 3) as usize * 4);
        for patch in patches {
            for _ in 0..PATCH * PATCH {
                for channel in patch {
                    pixels.extend_from_slice(&(*channel as f32).to_le_bytes());
                }
            }
        }
        tiff_fixture(&pixels, PATCH, PATCH * patches.len() as u32, 32, 3, profile)
    }

    fn decoded_pixels(jpeg: &[u8]) -> image::RgbImage {
        DynamicImage::from_decoder(JpegDecoder::new(std::io::Cursor::new(jpeg)).unwrap())
            .unwrap()
            .to_rgb8()
    }

    /// The ICC tag table of one profile as `(signature, bytes)` pairs.
    fn icc_tags(profile: &[u8]) -> Vec<([u8; 4], Vec<u8>)> {
        let count = u32::from_be_bytes(profile[128..132].try_into().unwrap()) as usize;
        (0..count)
            .map(|index| {
                let entry = 132 + index * 12;
                let signature: [u8; 4] = profile[entry..entry + 4].try_into().unwrap();
                let start =
                    u32::from_be_bytes(profile[entry + 4..entry + 8].try_into().unwrap()) as usize;
                let length =
                    u32::from_be_bytes(profile[entry + 8..entry + 12].try_into().unwrap()) as usize;
                (signature, profile[start..start + length].to_vec())
            })
            .collect()
    }

    #[test]
    fn pinned_profile_assets_keep_their_identity() {
        assert_eq!(
            hex_digest(SOURCE_PROFILE_ASSET),
            SOURCE_PROFILE_ASSET_DIGEST
        );
        assert_eq!(
            hex_digest(DESTINATION_PROFILE_ASSET),
            DESTINATION_PROFILE_ASSET_DIGEST
        );
        assert_eq!(
            hex_digest(QUALIFIED_RUN_PROFILE_ASSET),
            QUALIFIED_RUN_PROFILE_ASSET_DIGEST
        );
        assert!(ACCEPTED_SOURCE_PROFILE_DIGESTS.contains(&SOURCE_PROFILE_ASSET_DIGEST));
        assert!(ACCEPTED_SOURCE_PROFILE_DIGESTS.contains(&QUALIFIED_RUN_PROFILE_ASSET_DIGEST));
    }

    #[test]
    fn qualified_run_profile_differs_from_the_pinned_asset_only_in_its_description() {
        let pinned = icc_tags(SOURCE_PROFILE_ASSET);
        let qualified = icc_tags(QUALIFIED_RUN_PROFILE_ASSET);
        assert_eq!(pinned.len(), qualified.len());
        let mut descriptions = 0;
        for ((pinned_tag, pinned_bytes), (qualified_tag, qualified_bytes)) in
            pinned.iter().zip(&qualified)
        {
            assert_eq!(pinned_tag, qualified_tag);
            if *pinned_tag == DESCRIPTION_TAG {
                descriptions += 1;
                assert_ne!(pinned_bytes, qualified_bytes);
            } else {
                assert_eq!(
                    pinned_bytes,
                    qualified_bytes,
                    "colorimetry tag {:?} differs",
                    std::str::from_utf8(pinned_tag).unwrap()
                );
            }
        }
        assert_eq!(descriptions, 1, "exactly one description tag is expected");
    }

    #[test]
    fn development_tiff_derivative_accepts_the_qualified_run_source_profile() {
        let rows: Vec<[f64; 3]> = PINNED_VECTORS.iter().map(|(input, _)| *input).collect();
        let pinned = float_fixture(&rows, SOURCE_PROFILE_ASSET)
            .descriptor(|fd| process_development_tiff(fd, DerivativeTarget::Thumbnail512))
            .unwrap();
        let qualified = float_fixture(&rows, QUALIFIED_RUN_PROFILE_ASSET)
            .descriptor(|fd| process_development_tiff(fd, DerivativeTarget::Thumbnail512))
            .unwrap();
        // The source profile selects the transform and is never copied into the
        // derivative, so both artifacts must encode the same image.
        assert_eq!(pinned.jpeg, qualified.jpeg);
    }

    #[test]
    fn display_matrix_and_transfer_function_match_pinned_reference_vectors() {
        for (input, expected) in PINNED_VECTORS {
            let planes: Vec<f32> = input.iter().map(|value| *value as f32).collect();
            assert_eq!(to_srgb8(&planes), expected, "input {input:?}");
        }
    }

    #[test]
    fn development_tiff_derivative_matches_pinned_reference_vectors() {
        let rows: Vec<[f64; 3]> = PINNED_VECTORS.iter().map(|(input, _)| *input).collect();
        let fixture = float_fixture(&rows, SOURCE_PROFILE_ASSET);
        let derivative = fixture
            .descriptor(|fd| process_development_tiff(fd, DerivativeTarget::Thumbnail512))
            .unwrap();
        assert_eq!(derivative.profile, DerivativeProfile::Srgb);
        assert_eq!((derivative.width, derivative.height), (PATCH, PATCH * 10));
        assert!(
            derivative
                .jpeg
                .windows(12)
                .any(|window| window == b"ICC_PROFILE\0")
        );
        let decoded = decoded_pixels(&derivative.jpeg);
        assert_eq!((decoded.width(), decoded.height()), (PATCH, PATCH * 10));
        for (index, (_, expected)) in PINNED_VECTORS.iter().enumerate() {
            // JPEG quantizes the transfer-function result, so the flat patch is
            // compared with the small tolerance that encoding can introduce.
            let pixel = decoded
                .get_pixel(PATCH / 2, PATCH * index as u32 + PATCH / 2)
                .0;
            for channel in 0..3 {
                let difference = i32::from(pixel[channel]) - i32::from(expected[channel]);
                assert!(
                    difference.abs() <= 3,
                    "row {index} channel {channel}: decoded {} expected {}",
                    pixel[channel],
                    expected[channel]
                );
            }
        }
    }

    #[test]
    fn development_tiff_derivative_refuses_unpinned_source_profile() {
        let fixture = float_fixture(&[[0.0, 0.0, 0.0]], DESTINATION_PROFILE_ASSET);
        let outcome =
            fixture.descriptor(|fd| process_development_tiff(fd, DerivativeTarget::Thumbnail512));
        assert_eq!(outcome, Err(DerivativeError::Malformed));
    }

    #[test]
    fn development_tiff_derivative_refuses_integer_samples() {
        let pixels = vec![127u8; 8 * 2 * 3];
        let fixture = tiff_fixture(&pixels, 8, 2, 8, 1, SOURCE_PROFILE_ASSET);
        let outcome =
            fixture.descriptor(|fd| process_development_tiff(fd, DerivativeTarget::Thumbnail512));
        assert_eq!(outcome, Err(DerivativeError::Malformed));
    }

    #[test]
    fn development_tiff_derivative_refuses_a_foreign_artifact() {
        // A file that is not a decodable float32 RGB TIFF must be refused
        // rather than displayed. Sample integrity itself is established by the
        // receipt that publishes the artifact, not by this conversion.
        let mut fixture = float_fixture(&[[0.18, 0.18, 0.18]], SOURCE_PROFILE_ASSET);
        fixture.bytes[0..4].copy_from_slice(b"\x89PNG");
        let outcome =
            fixture.descriptor(|fd| process_development_tiff(fd, DerivativeTarget::Thumbnail512));
        assert_eq!(outcome, Err(DerivativeError::Malformed));
    }

    #[test]
    fn development_tiff_derivative_refuses_an_oversized_input() {
        use std::os::fd::AsRawFd;
        // The declared size is checked before anything is decoded, so the
        // fixture is a sparse file and no gigabyte of data touches the disk.
        let path = std::env::temp_dir().join(format!(
            "slipstream-display-oversized-{}",
            std::process::id()
        ));
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAXIMUM_DEVELOPMENT_TIFF_BYTES + 1).unwrap();
        drop(file);
        let file = std::fs::File::open(&path).unwrap();
        let outcome = process_development_tiff(file.as_raw_fd(), DerivativeTarget::Thumbnail512);
        drop(file);
        std::fs::remove_file(&path).unwrap();
        assert_eq!(outcome, Err(DerivativeError::ResourceLimit));
    }

    #[test]
    fn development_tiff_derivative_refuses_an_oversized_raster() {
        // The declared geometry exceeds the pixel ceiling, which is enforced
        // after the header is read and before any sample is decoded, so the
        // strip stays one patch long.
        let pixels = vec![0u8; PATCH as usize * PATCH as usize * 3 * 4];
        let fixture = tiff_fixture(&pixels, 20_000, 20_000, 32, 3, SOURCE_PROFILE_ASSET);
        let outcome =
            fixture.descriptor(|fd| process_development_tiff(fd, DerivativeTarget::Thumbnail512));
        assert_eq!(outcome, Err(DerivativeError::ResourceLimit));
    }

    #[test]
    fn development_tiff_derivative_never_upscales() {
        let fixture = float_fixture(&[[0.18, 0.18, 0.18], [1.0, 1.0, 1.0]], SOURCE_PROFILE_ASSET);
        let derivative = fixture
            .descriptor(|fd| process_development_tiff(fd, DerivativeTarget::Review2560))
            .unwrap();
        assert_eq!((derivative.width, derivative.height), (PATCH, PATCH * 2));
    }

    /// Runs the display branch over a real Development TIFF. Ignored by
    /// default because no fixture can stand in for it: run it with a qualified
    /// artifact and `--ignored`, for example
    /// `SLIPSTREAM_DEVELOPMENT_TIFF_SAMPLE=/path/development.tif cargo test
    /// -p slipstream-core --lib development_tiff_derivative_handles_a_real_artifact
    /// -- --ignored`. It fails rather than passes when the variable is missing.
    #[test]
    #[ignore = "requires a qualified Development TIFF and --ignored"]
    fn development_tiff_derivative_handles_a_real_artifact() {
        use std::os::fd::AsRawFd;
        let path = std::env::var_os("SLIPSTREAM_DEVELOPMENT_TIFF_SAMPLE")
            .expect("SLIPSTREAM_DEVELOPMENT_TIFF_SAMPLE must name a qualified Development TIFF");
        let file = std::fs::File::open(path).unwrap();
        let derivative =
            process_development_tiff(file.as_raw_fd(), DerivativeTarget::Thumbnail512).unwrap();
        assert_eq!(derivative.profile, DerivativeProfile::Srgb);
        assert_eq!(derivative.width.max(derivative.height), 512);
        assert!(
            derivative
                .jpeg
                .windows(12)
                .any(|window| window == b"ICC_PROFILE\0")
        );
        let decoded = decoded_pixels(&derivative.jpeg);
        let (mut dark, mut bright) = (false, false);
        for pixel in decoded.pixels() {
            dark |= pixel.0.iter().all(|channel| *channel < 16);
            bright |= pixel.0.iter().all(|channel| *channel > 240);
        }
        assert!(dark || bright, "derivative is degenerate");
    }
}
