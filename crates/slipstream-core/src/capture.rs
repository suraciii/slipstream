//! Descriptor-confined EXIF Capture Time inspection.
//!
//! This module reads only EXIF/TIFF structures needed for review ordering. It
//! never passes a path to a parser, opens another descriptor, develops RAW
//! pixels, or follows metadata in an embedded Preview.

#[path = "capture_metadata.rs"]
mod metadata;

use metadata::{MetadataError, ParseOutcome, parse_metadata};

use crate::{
    OriginalCapability, OriginalFacts, OriginalKind, confinement::ConfinementError, source_revision,
};
use std::fmt;

/// Total metadata bytes one Capture Time inspection may read or allocate.
pub const MAXIMUM_CAPTURE_METADATA_BYTES: u64 = 16 * 1024 * 1024;
const MAXIMUM_TAG_VALUE_BYTES: usize = 64 * 1024;
/// Camera identity strings are short; a longer value is not a camera name.
const MAXIMUM_TEXT_TAG_BYTES: usize = 128;
const MAXIMUM_TIFF_DIRECTORY_ENTRIES: usize = 1024;
const MAXIMUM_JPEG_MARKERS: usize = 4096;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureMetadataState {
    Pending,
    Known,
    Missing,
    Invalid,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureTimeField {
    DateTimeOriginal,
    DateTimeDigitized,
}

impl CaptureTimeField {
    pub(crate) fn database_name(self) -> &'static str {
        match self {
            Self::DateTimeOriginal => "date-time-original",
            Self::DateTimeDigitized => "date-time-digitized",
        }
    }

    pub(crate) fn parse_database_name(value: &str) -> Option<Self> {
        match value {
            "date-time-original" => Some(Self::DateTimeOriginal),
            "date-time-digitized" => Some(Self::DateTimeDigitized),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureFact {
    pub state: CaptureMetadataState,
    pub order_key: Option<String>,
    pub field: Option<CaptureTimeField>,
    pub offset_minutes: Option<i16>,
    pub source_revision: Option<String>,
    pub identity: CameraIdentity,
}

/// The camera identity published by one completed capture inspection of a
/// source revision. The identity names the source class of a RAW Original,
/// so it is captured by the same bounded read as the Capture Time and is
/// published atomically with the revision it describes.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum CameraIdentity {
    /// No completed identity observation is bound to the current source
    /// revision yet: the inspection has not run, or it has not completed
    /// for the revision that the persisted facts describe.
    #[default]
    Pending,
    /// A completed observation. `make` and `model` are the recorded camera
    /// strings, or `None` when the completed bounded read found no camera
    /// identity, so the class cannot be named.
    Observed {
        make: Option<String>,
        model: Option<String>,
    },
}

impl CameraIdentity {
    /// True when a completed observation is bound to the current revision.
    pub fn is_observed(&self) -> bool {
        matches!(self, Self::Observed { .. })
    }
}

/// The camera facts needed during Photo review. This is intentionally a
/// small, read-only view rather than a general EXIF model.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CaptureReviewMetadata {
    pub capture_time: Option<String>,
    pub aperture: Option<String>,
    pub iso: Option<u32>,
    pub shutter_speed: Option<String>,
    pub focal_length: Option<String>,
    /// Camera make and model as recorded by the camera. They identify the
    /// source class of a RAW Original and are not user-editable facts.
    pub make: Option<String>,
    pub model: Option<String>,
}

impl CaptureFact {
    pub fn pending() -> Self {
        Self {
            state: CaptureMetadataState::Pending,
            order_key: None,
            field: None,
            offset_minutes: None,
            source_revision: None,
            identity: CameraIdentity::Pending,
        }
    }

    pub fn failed(source_revision: Option<String>) -> Self {
        Self {
            state: CaptureMetadataState::Failed,
            order_key: None,
            field: None,
            offset_minutes: None,
            source_revision,
            identity: CameraIdentity::Pending,
        }
    }

    fn completed(
        state: CaptureMetadataState,
        order_key: Option<String>,
        field: Option<CaptureTimeField>,
        offset_minutes: Option<i16>,
        source_revision: String,
        identity: CameraIdentity,
    ) -> Self {
        Self {
            state,
            order_key,
            field,
            offset_minutes,
            source_revision: Some(source_revision),
            identity,
        }
    }

    /// A completed capture fact is reusable for one source revision when its
    /// completed inspection is bound to that revision and its camera
    /// identity observation is published. A pending identity is not
    /// reusable, so the next scan re-inspects and publishes it instead of
    /// carrying an unobserved identity forward.
    pub(crate) fn is_reusable_for(&self, source_revision: &str) -> bool {
        matches!(
            self.state,
            CaptureMetadataState::Known
                | CaptureMetadataState::Missing
                | CaptureMetadataState::Invalid
        ) && self.source_revision.as_deref() == Some(source_revision)
            && self.identity.is_observed()
    }
}

#[derive(Debug)]
pub enum CaptureInspectionError {
    Confinement(ConfinementError),
    ResourceLimit,
}

impl fmt::Display for CaptureInspectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Confinement(error) => error.fmt(formatter),
            Self::ResourceLimit => {
                formatter.write_str("Capture metadata inspection exceeds limits")
            }
        }
    }
}

impl std::error::Error for CaptureInspectionError {}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CaptureInspectionTestPoint {
    BeforeOpen,
    BeforeVerification,
}

#[cfg(test)]
type CaptureInspectionTestHook =
    dyn Fn(&crate::RelativeOriginalPath, CaptureInspectionTestPoint) + Send + Sync;

#[cfg(test)]
static CAPTURE_INSPECTION_TEST_HOOK: std::sync::OnceLock<
    std::sync::Mutex<Option<std::sync::Arc<CaptureInspectionTestHook>>>,
> = std::sync::OnceLock::new();

#[cfg(test)]
static CAPTURE_INSPECTION_TEST_HOOK_LEASE: std::sync::OnceLock<std::sync::Mutex<()>> =
    std::sync::OnceLock::new();

#[cfg(test)]
pub(crate) struct CaptureInspectionTestHookGuard {
    _lease: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
pub(crate) fn install_capture_inspection_test_hook(
    hook: impl Fn(&crate::RelativeOriginalPath, CaptureInspectionTestPoint) + Send + Sync + 'static,
) -> CaptureInspectionTestHookGuard {
    let lease = CAPTURE_INSPECTION_TEST_HOOK_LEASE
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap();
    *CAPTURE_INSPECTION_TEST_HOOK
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .unwrap() = Some(std::sync::Arc::new(hook));
    CaptureInspectionTestHookGuard { _lease: lease }
}

#[cfg(test)]
impl Drop for CaptureInspectionTestHookGuard {
    fn drop(&mut self) {
        *CAPTURE_INSPECTION_TEST_HOOK
            .get_or_init(|| std::sync::Mutex::new(None))
            .lock()
            .unwrap() = None;
    }
}

#[cfg(test)]
fn capture_inspection_test_hook(
    capability: &OriginalCapability,
    point: CaptureInspectionTestPoint,
) {
    let hook = CAPTURE_INSPECTION_TEST_HOOK
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .unwrap()
        .clone();
    if let Some(hook) = hook {
        hook(capability.path(), point);
    }
}

/// Stable Capture Time reuse identity, including the discovery descriptor's
/// device and inode so same-size/same-mtime path replacement cannot reuse an
/// old metadata fact.
pub fn capture_source_revision(
    path: &str,
    facts: OriginalFacts,
) -> Result<String, crate::InvalidModificationTime> {
    Ok(format!(
        "{}\0{}\0{}",
        source_revision(path, facts.size, facts.mtime_ms)?,
        facts.device,
        facts.inode
    ))
}

/// Whether a published Capture fact revision belongs to the durable
/// descriptor (path, size, and modification time) named by `current`. A
/// published revision also carries the discovery device and inode, which are
/// not persisted, so a metadata-only read decides currency from the durable
/// leading components alone.
pub fn capture_revision_matches_descriptor(published: &str, current: &str) -> bool {
    published
        .strip_prefix(current)
        .is_some_and(|remainder| remainder.starts_with('\0'))
}

pub(crate) struct CaptureObservation {
    pub(crate) facts: OriginalFacts,
    pub(crate) capture: CaptureFact,
}

struct MetadataObservation {
    facts: OriginalFacts,
    outcome: Result<ParseOutcome, MetadataError>,
}

/// Inspects EXIF metadata from one already-confined Original descriptor.
///
/// JPEGs inspect APP1 segment boundaries only. RAW inspection accepts a TIFF
/// container at byte zero, so an embedded JPEG Preview can never contribute a
/// RAW Capture Time. Both paths use bounded `pread` ranges and verify the same
/// retained descriptor after parsing.
pub(crate) fn inspect_capture(
    capability: &OriginalCapability,
    kind: OriginalKind,
    expected_facts: OriginalFacts,
) -> Result<CaptureFact, CaptureInspectionError> {
    capture_source_revision(capability.path().as_str(), expected_facts)
        .map_err(|_| CaptureInspectionError::Confinement(ConfinementError::Changed))?;
    let observation = inspect_metadata(capability, kind)?;
    // The discovery identity includes device and inode in addition to the
    // durable size/mtime facts. A same-size, same-mtime path replacement must
    // never publish metadata for the replacement under the discovery result.
    // This comparison intentionally wins over parser failures.
    if observation.facts != expected_facts {
        return Err(CaptureInspectionError::Confinement(
            ConfinementError::Changed,
        ));
    }
    complete_capture(capability.path().as_str(), observation).map(|result| result.capture)
}

/// Makes one fresh observation without a discovery-facts comparison. The
/// returned source facts and Capture fact come from the same retained,
/// revision-checked descriptor and must be adopted together by the scanner.
pub(crate) fn inspect_capture_fresh(
    capability: &OriginalCapability,
    kind: OriginalKind,
) -> Result<CaptureObservation, CaptureInspectionError> {
    let observation = inspect_metadata(capability, kind)?;
    complete_capture(capability.path().as_str(), observation)
}

fn inspect_metadata(
    capability: &OriginalCapability,
    kind: OriginalKind,
) -> Result<MetadataObservation, CaptureInspectionError> {
    #[cfg(test)]
    capture_inspection_test_hook(capability, CaptureInspectionTestPoint::BeforeOpen);
    let opened = capability
        .open_revision_checked()
        .map_err(CaptureInspectionError::Confinement)?;
    let outcome = parse_metadata(&opened, kind);
    #[cfg(test)]
    capture_inspection_test_hook(capability, CaptureInspectionTestPoint::BeforeVerification);
    // A metadata classification is useful only when it describes the same
    // bytes that were opened. This check intentionally wins over malformed,
    // I/O, and resource results from the parser.
    let facts = opened
        .verify_unchanged()
        .map_err(CaptureInspectionError::Confinement)?;
    Ok(MetadataObservation { facts, outcome })
}

fn complete_capture(
    path: &str,
    observation: MetadataObservation,
) -> Result<CaptureObservation, CaptureInspectionError> {
    let source_revision = capture_source_revision(path, observation.facts)
        .map_err(|_| CaptureInspectionError::Confinement(ConfinementError::Changed))?;
    let capture = match observation.outcome.map_err(CaptureInspectionError::from)? {
        ParseOutcome::Known {
            order_key,
            field,
            offset_minutes,
            review,
        } => CaptureFact::completed(
            CaptureMetadataState::Known,
            Some(order_key),
            Some(field),
            offset_minutes,
            source_revision,
            camera_identity(&review),
        ),
        ParseOutcome::Missing { review } => CaptureFact::completed(
            CaptureMetadataState::Missing,
            None,
            None,
            None,
            source_revision,
            camera_identity(&review),
        ),
        ParseOutcome::Invalid { review } => CaptureFact::completed(
            CaptureMetadataState::Invalid,
            None,
            None,
            None,
            source_revision,
            camera_identity(&review),
        ),
    };
    Ok(CaptureObservation {
        facts: observation.facts,
        capture,
    })
}

/// The camera identity of one completed parse. Every completed outcome
/// carries the review view built from the same bounded read, so the
/// identity is bound to the same source revision as the Capture Time. A
/// completed read that yields no camera strings is an observed absence,
/// not a pending one: the bounded reader cannot name the class.
fn camera_identity(review: &CaptureReviewMetadata) -> CameraIdentity {
    CameraIdentity::Observed {
        make: review.make.clone(),
        model: review.model.clone(),
    }
}

/// Inspects review facts from one already-confined Original descriptor. A
/// malformed or absent metadata block yields empty fields; only confinement
/// and resource failures are returned as errors.
pub fn inspect_review_metadata(
    capability: &OriginalCapability,
    kind: OriginalKind,
    expected_facts: OriginalFacts,
) -> Result<CaptureReviewMetadata, CaptureInspectionError> {
    let observation = inspect_metadata(capability, kind)?;
    if observation.facts != expected_facts {
        return Err(CaptureInspectionError::Confinement(
            ConfinementError::Changed,
        ));
    }
    let outcome = observation.outcome.map_err(CaptureInspectionError::from)?;
    Ok(match outcome {
        ParseOutcome::Known { review, .. }
        | ParseOutcome::Missing { review }
        | ParseOutcome::Invalid { review } => review,
    })
}

impl From<MetadataError> for CaptureInspectionError {
    fn from(value: MetadataError) -> Self {
        match value {
            MetadataError::Confinement(error) => Self::Confinement(error),
            MetadataError::Resource => Self::ResourceLimit,
            // Structural corruption is represented as an `invalid` fact, so
            // this branch is unreachable at the public boundary.
            MetadataError::Invalid => {
                Self::Confinement(ConfinementError::Io("Capture metadata inspection failed"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LibraryRoot, RelativeOriginalPath};
    use serde::Deserialize;
    use std::{
        ffi::CString,
        fs::{self, File},
        io::{Seek, SeekFrom, Write},
        os::unix::{ffi::OsStrExt, fs::MetadataExt},
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT_FIXTURE: AtomicU64 = AtomicU64::new(0);

    struct TempTree(PathBuf);

    impl TempTree {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "slipstream-capture-{}-{}",
                std::process::id(),
                NEXT_FIXTURE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn inspect_bytes(
        kind: OriginalKind,
        bytes: &[u8],
    ) -> Result<CaptureFact, CaptureInspectionError> {
        let tree = TempTree::new();
        let name = match kind {
            OriginalKind::Raw => "fixture.ARW",
            OriginalKind::Jpeg => "fixture.JPG",
        };
        fs::write(tree.0.join(name), bytes).unwrap();
        let root = LibraryRoot::open(&tree.0).unwrap();
        let capability = root
            .original(RelativeOriginalPath::parse(name).unwrap())
            .unwrap();
        let facts = capability.facts().unwrap();
        let result = inspect_capture(&capability, kind, facts);
        drop(root);
        result
    }

    fn inspect_review_bytes(
        kind: OriginalKind,
        bytes: &[u8],
    ) -> Result<CaptureReviewMetadata, CaptureInspectionError> {
        let tree = TempTree::new();
        let name = match kind {
            OriginalKind::Raw => "fixture.ARW",
            OriginalKind::Jpeg => "fixture.JPG",
        };
        fs::write(tree.0.join(name), bytes).unwrap();
        let root = LibraryRoot::open(&tree.0).unwrap();
        let capability = root
            .original(RelativeOriginalPath::parse(name).unwrap())
            .unwrap();
        let facts = capability.facts().unwrap();
        let result = inspect_review_metadata(&capability, kind, facts);
        drop(root);
        result
    }

    fn tiff(tags: &[(u16, Vec<u8>)]) -> Vec<u8> {
        let typed = tags
            .iter()
            .map(|(tag, value)| (*tag, 2_u16, value.clone()))
            .collect::<Vec<_>>();
        tiff_typed(&typed)
    }

    fn tiff_typed(tags: &[(u16, u16, Vec<u8>)]) -> Vec<u8> {
        let data_start = 8 + 2 + tags.len() * 12 + 4;
        let mut bytes = b"II*\0\x08\0\0\0".to_vec();
        bytes.extend_from_slice(&(tags.len() as u16).to_le_bytes());
        let mut data = Vec::new();
        for (tag, field_type, value) in tags {
            bytes.extend_from_slice(&tag.to_le_bytes());
            bytes.extend_from_slice(&field_type.to_le_bytes());
            let count = if matches!(field_type, 3 | 5) {
                1
            } else {
                value.len() as u32
            };
            bytes.extend_from_slice(&count.to_le_bytes());
            if value.len() <= 4 {
                let mut inline = [0; 4];
                inline[..value.len()].copy_from_slice(value);
                bytes.extend_from_slice(&inline);
            } else {
                bytes.extend_from_slice(&((data_start + data.len()) as u32).to_le_bytes());
                data.extend_from_slice(value);
            }
        }
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        bytes.extend_from_slice(&data);
        bytes
    }

    fn jpeg_segment(marker: u8, payload: &[u8]) -> Vec<u8> {
        let length = u16::try_from(payload.len() + 2).unwrap();
        [
            vec![0xff, marker],
            length.to_be_bytes().to_vec(),
            payload.to_vec(),
        ]
        .concat()
    }

    fn jpeg(tiff: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0xff, 0xd8];
        let mut payload = b"Exif\0\0".to_vec();
        payload.extend_from_slice(tiff);
        bytes.extend_from_slice(&jpeg_segment(0xe1, &payload));
        bytes.extend_from_slice(&[0xff, 0xd9]);
        bytes
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct MetadataVector {
        date_time_original: Option<String>,
        date_time_digitized: Option<String>,
        sub_sec_time_original: Option<String>,
        sub_sec_time_digitized: Option<String>,
        offset_time_original: Option<String>,
        offset_time_digitized: Option<String>,
        expected: ExpectedVector,
    }

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct ExpectedVector {
        state: String,
        field: Option<String>,
        order_key: Option<String>,
        offset_minutes: Option<i16>,
    }

    #[test]
    fn shared_metadata_vectors_cover_precedence_normalization_and_failure_states() {
        let vectors: Vec<MetadataVector> = serde_json::from_str(include_str!(
            "../../../compatibility/metadata/capture-time.json"
        ))
        .unwrap();
        for vector in vectors {
            let mut tags = Vec::new();
            for (tag, value) in [
                (0x9003, vector.date_time_original),
                (0x9004, vector.date_time_digitized),
                (0x9291, vector.sub_sec_time_original),
                (0x9292, vector.sub_sec_time_digitized),
                (0x9011, vector.offset_time_original),
                (0x9012, vector.offset_time_digitized),
            ] {
                if let Some(value) = value {
                    let mut bytes = value.into_bytes();
                    bytes.push(0);
                    tags.push((tag, bytes));
                }
            }
            let bytes = if tags.is_empty() {
                b"not metadata".to_vec()
            } else {
                jpeg(&tiff(&tags))
            };
            let fact = inspect_bytes(OriginalKind::Jpeg, &bytes).unwrap();
            assert_eq!(
                fact.state,
                match vector.expected.state.as_str() {
                    "known" => CaptureMetadataState::Known,
                    "missing" => CaptureMetadataState::Missing,
                    "invalid" => CaptureMetadataState::Invalid,
                    _ => panic!("unexpected vector state"),
                }
            );
            assert_eq!(
                fact.order_key.as_deref(),
                vector.expected.order_key.as_deref()
            );
            assert_eq!(
                fact.field.map(CaptureTimeField::database_name),
                vector.expected.field.as_deref()
            );
            assert_eq!(fact.offset_minutes, vector.expected.offset_minutes);
        }
    }

    #[test]
    fn checked_in_order_vector_keeps_camera_local_offset_out_of_the_order_key() {
        let vectors: Vec<serde_json::Value> = serde_json::from_str(include_str!(
            "../../../compatibility/metadata/capture-order.json"
        ))
        .unwrap();
        let vector = vectors
            .iter()
            .find(|vector| vector["name"] == "camera-local-offset-is-not-applied-to-order-key")
            .unwrap();
        let date = vector["dateTimeOriginal"].as_str().unwrap();
        let offset = vector["offsetTimeOriginal"].as_str().unwrap();
        let fact = inspect_bytes(
            OriginalKind::Jpeg,
            &jpeg(&tiff(&[
                (0x9003, format!("{date}\0").into_bytes()),
                (0x9011, format!("{offset}\0").into_bytes()),
            ])),
        )
        .unwrap();
        assert_eq!(
            fact.order_key.as_deref(),
            vector["expectedOrderKey"].as_str()
        );
        assert_eq!(
            fact.offset_minutes,
            vector["expectedOffsetMinutes"]
                .as_i64()
                .map(|value| value as i16)
        );
    }

    #[test]
    fn parses_review_metadata_with_camera_display_values() {
        let rational = |numerator: u32, denominator: u32| {
            [numerator.to_le_bytes(), denominator.to_le_bytes()].concat()
        };
        let bytes = jpeg(&tiff_typed(&[
            (0x9003, 2, b"2026:02:03 04:05:06\0".to_vec()),
            (0x829d, 5, rational(28, 10)),
            (0x8827, 3, 400_u16.to_le_bytes().to_vec()),
            (0x829a, 5, rational(1, 125)),
            (0x920a, 5, rational(50, 1)),
            (0x010f, 2, b"SONY\0".to_vec()),
            (0x0110, 2, b"ILCE-7RM5\0\0\0".to_vec()),
        ]));
        let metadata = inspect_review_bytes(OriginalKind::Jpeg, &bytes).unwrap();
        assert_eq!(
            metadata.capture_time.as_deref(),
            Some("2026-02-03T04:05:06.000000000")
        );
        assert_eq!(metadata.aperture.as_deref(), Some("f/2.8"));
        assert_eq!(metadata.iso, Some(400));
        assert_eq!(metadata.shutter_speed.as_deref(), Some("1/125 s"));
        assert_eq!(metadata.focal_length.as_deref(), Some("50 mm"));
        assert_eq!(metadata.make.as_deref(), Some("SONY"));
        assert_eq!(metadata.model.as_deref(), Some("ILCE-7RM5"));
    }

    /// Encodes one TIFF directory with its out-of-line data, so a caller can
    /// place the two parts at known offsets.
    fn directory(entries: &[(u16, u16, Vec<u8>)], ifd_offset: usize) -> (Vec<u8>, Vec<u8>) {
        let data_start = ifd_offset + 2 + entries.len() * 12 + 4;
        let mut ifd = Vec::new();
        ifd.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        let mut data = Vec::new();
        for (tag, field_type, value) in entries {
            ifd.extend_from_slice(&tag.to_le_bytes());
            ifd.extend_from_slice(&field_type.to_le_bytes());
            let count = if matches!(field_type, 3..=5) {
                1
            } else {
                value.len() as u32
            };
            ifd.extend_from_slice(&count.to_le_bytes());
            if value.len() <= 4 {
                let mut inline = [0; 4];
                inline[..value.len()].copy_from_slice(value);
                ifd.extend_from_slice(&inline);
            } else {
                ifd.extend_from_slice(&((data_start + data.len()) as u32).to_le_bytes());
                data.extend_from_slice(value);
            }
        }
        ifd.extend_from_slice(&0_u32.to_le_bytes());
        (ifd, data)
    }

    /// One TIFF whose primary directory holds `primary` and whose Exif
    /// subdirectory holds `exif`.
    fn tiff_with_exif_directory(
        primary: &[(u16, u16, Vec<u8>)],
        exif: &[(u16, u16, Vec<u8>)],
    ) -> Vec<u8> {
        let mut primary_entries = primary.to_vec();
        primary_entries.push((0x8769, 4, 0_u32.to_le_bytes().to_vec()));
        let (mut primary_ifd, primary_data) = directory(&primary_entries, 8);
        let exif_offset = (8 + primary_ifd.len() + primary_data.len() + 3) & !3;
        // The Exif pointer is the last primary entry, so its inline value sits
        // immediately before the next-directory offset.
        let value_at = primary_ifd.len() - 8;
        primary_ifd[value_at..value_at + 4].copy_from_slice(&(exif_offset as u32).to_le_bytes());
        let (exif_ifd, exif_data) = directory(exif, exif_offset);
        let mut bytes = b"II*\0\x08\0\0\0".to_vec();
        bytes.extend_from_slice(&primary_ifd);
        bytes.extend_from_slice(&primary_data);
        bytes.resize(exif_offset, 0);
        bytes.extend_from_slice(&exif_ifd);
        bytes.extend_from_slice(&exif_data);
        bytes
    }

    #[test]
    fn camera_identity_is_read_only_from_the_primary_directory() {
        let bytes = jpeg(&tiff_with_exif_directory(
            &[(0x9003, 2, b"2026:02:03 04:05:06\0".to_vec())],
            &[
                (0x010f, 2, b"SONY\0".to_vec()),
                (0x0110, 2, b"ILCE-7RM5\0\0\0".to_vec()),
            ],
        ));
        let metadata = inspect_review_bytes(OriginalKind::Jpeg, &bytes).unwrap();
        assert_eq!(
            metadata.capture_time.as_deref(),
            Some("2026-02-03T04:05:06.000000000")
        );
        assert_eq!(
            metadata.make, None,
            "an Exif-directory Make must not classify the source"
        );
        assert_eq!(metadata.model, None);
    }

    #[test]
    fn an_oversized_camera_identity_is_absent_without_discarding_other_facts() {
        // A JPEG segment cannot carry a value this large, so the oversized tag
        // has to be exercised through a RAW container.
        let bytes = tiff_typed(&[
            (0x010f, 2, vec![b'N'; MAXIMUM_TAG_VALUE_BYTES + 1]),
            (0x0110, 2, b"ILCE-7RM5\0\0\0".to_vec()),
            (0x8827, 3, 400_u16.to_le_bytes().to_vec()),
            (0x9003, 2, b"2026:02:03 04:05:06\0".to_vec()),
        ]);
        let metadata = inspect_review_bytes(OriginalKind::Raw, &bytes).unwrap();
        assert_eq!(metadata.make, None);
        assert_eq!(metadata.model.as_deref(), Some("ILCE-7RM5"));
        assert_eq!(metadata.iso, Some(400));
        assert_eq!(
            metadata.capture_time.as_deref(),
            Some("2026-02-03T04:05:06.000000000")
        );
    }

    #[test]
    fn camera_identity_rejects_non_text_and_oversized_values() {
        let bytes = jpeg(&tiff_typed(&[
            (0x010f, 2, vec![0x01, 0x02, 0x00]),
            (0x0110, 3, 7_u16.to_le_bytes().to_vec()),
        ]));
        let metadata = inspect_review_bytes(OriginalKind::Jpeg, &bytes).unwrap();
        assert_eq!(metadata.make, None);
        assert_eq!(metadata.model, None);

        let oversized = format!("{}\0", "N".repeat(MAXIMUM_TEXT_TAG_BYTES + 1));
        let bytes = jpeg(&tiff_typed(&[(0x010f, 2, oversized.into_bytes())]));
        let metadata = inspect_review_bytes(OriginalKind::Jpeg, &bytes).unwrap();
        assert_eq!(metadata.make, None);
    }

    #[test]
    fn raw_uses_only_its_primary_tiff_container_without_sensor_unpack() {
        let raw = tiff(&[(0x9003, b"2026:02:03 04:05:06\0".to_vec())]);
        assert_eq!(
            inspect_bytes(OriginalKind::Raw, &raw).unwrap().state,
            CaptureMetadataState::Known
        );
        let embedded = jpeg(&tiff(&[(0x9003, b"2026:02:03 04:05:06\0".to_vec())]));
        assert!(
            inspect_bytes(OriginalKind::Raw, &embedded)
                .map(|fact| fact.state != CaptureMetadataState::Known)
                .unwrap_or(true),
            "an embedded JPEG must never become a RAW Capture Time"
        );
    }

    #[test]
    fn jpeg_parser_uses_real_segment_boundaries_not_arbitrary_exif_bytes() {
        let tiff = tiff(&[(0x9003, b"2026:02:03 04:05:06\0".to_vec())]);
        let mut bytes = vec![0xff, 0xd8];
        bytes.extend_from_slice(&jpeg_segment(0xe0, b"JFIF\0\x01\x02\0\0\x01\0\x01\0\0"));
        bytes.extend_from_slice(&jpeg_segment(
            0xe1,
            b"http://ns.adobe.com/xap/1.0/\0Exif\0\0not-tiff",
        ));
        let mut exif = b"Exif\0\0".to_vec();
        exif.extend_from_slice(&tiff);
        bytes.extend_from_slice(&jpeg_segment(0xe1, &exif));
        bytes.extend_from_slice(&[0xff, 0xda]);
        let fact = inspect_bytes(OriginalKind::Jpeg, &bytes).unwrap();
        assert_eq!(fact.state, CaptureMetadataState::Known);
        assert_eq!(
            fact.order_key.as_deref(),
            Some("2026-02-03T04:05:06.000000000")
        );
    }

    #[test]
    fn sparse_large_raw_reads_only_tiff_ranges_and_preserves_original_bytes() {
        let tree = TempTree::new();
        let path = tree.0.join("large.ARW");
        let value_offset = 32_u64 * 1024 * 1024;
        let mut file = File::create(&path).unwrap();
        file.write_all(b"II*\0\x08\0\0\0").unwrap();
        // IFD0 references an Exif IFD; the Exif tag itself points far into a
        // sparse 64 MiB RAW. Inspection must fetch only those structures.
        file.write_all(&1_u16.to_le_bytes()).unwrap();
        file.write_all(&0x8769_u16.to_le_bytes()).unwrap();
        file.write_all(&4_u16.to_le_bytes()).unwrap();
        file.write_all(&1_u32.to_le_bytes()).unwrap();
        file.write_all(&26_u32.to_le_bytes()).unwrap();
        file.write_all(&0_u32.to_le_bytes()).unwrap();
        file.write_all(&1_u16.to_le_bytes()).unwrap();
        file.write_all(&0x9003_u16.to_le_bytes()).unwrap();
        file.write_all(&2_u16.to_le_bytes()).unwrap();
        file.write_all(&20_u32.to_le_bytes()).unwrap();
        file.write_all(&(value_offset as u32).to_le_bytes())
            .unwrap();
        file.write_all(&0_u32.to_le_bytes()).unwrap();
        file.seek(SeekFrom::Start(value_offset)).unwrap();
        file.write_all(b"2026:02:03 04:05:06\0").unwrap();
        file.set_len(64 * 1024 * 1024).unwrap();
        drop(file);
        let before = fs::read(&path).unwrap();
        let root = LibraryRoot::open(&tree.0).unwrap();
        let capability = root
            .original(RelativeOriginalPath::parse("large.ARW").unwrap())
            .unwrap();
        let facts = capability.facts().unwrap();
        let fact = inspect_capture(&capability, OriginalKind::Raw, facts).unwrap();
        assert_eq!(fact.state, CaptureMetadataState::Known);
        assert_eq!(fs::read(&path).unwrap(), before);
    }

    #[test]
    fn changed_between_discovery_and_open_is_rejected_without_old_revision() {
        let tree = TempTree::new();
        let path = tree.0.join("race.JPG");
        fs::write(
            &path,
            jpeg(&tiff(&[(0x9003, b"2026:02:03 04:05:06\0".to_vec())])),
        )
        .unwrap();
        let root = LibraryRoot::open(&tree.0).unwrap();
        let capability = root
            .original(RelativeOriginalPath::parse("race.JPG").unwrap())
            .unwrap();
        let facts = capability.facts().unwrap();
        let expected = facts;
        let mut changed = jpeg(&tiff(&[(0x9003, b"2026:02:03 05:05:06\0".to_vec())]));
        changed.push(0);
        fs::write(&path, changed).unwrap();
        assert!(matches!(
            inspect_capture(&capability, OriginalKind::Jpeg, expected),
            Err(CaptureInspectionError::Confinement(
                ConfinementError::Changed
            ))
        ));
    }

    #[test]
    fn same_size_same_mtime_path_replacement_is_rejected_by_discovery_identity() {
        let tree = TempTree::new();
        let path = tree.0.join("identity.ARW");
        let original = tiff(&[(0x9003, b"2026:02:03 04:05:06\0".to_vec())]);
        let replacement = tiff(&[(0x9003, b"2026:02:03 05:05:06\0".to_vec())]);
        assert_eq!(original.len(), replacement.len());
        fs::write(&path, original).unwrap();
        let root = LibraryRoot::open(&tree.0).unwrap();
        let capability = root
            .original(RelativeOriginalPath::parse("identity.ARW").unwrap())
            .unwrap();
        let expected = capability.facts().unwrap();
        let metadata = fs::metadata(&path).unwrap();
        let replacement_path = tree.0.join("replacement.ARW");
        fs::write(&replacement_path, replacement).unwrap();
        let replacement_name = CString::new(replacement_path.as_os_str().as_bytes()).unwrap();
        let times = [
            libc::timespec {
                tv_sec: 0,
                tv_nsec: libc::UTIME_OMIT,
            },
            libc::timespec {
                tv_sec: metadata.mtime(),
                tv_nsec: metadata.mtime_nsec(),
            },
        ];
        assert_eq!(
            unsafe {
                libc::utimensat(libc::AT_FDCWD, replacement_name.as_ptr(), times.as_ptr(), 0)
            },
            0
        );
        fs::rename(&replacement_path, &path).unwrap();
        let actual = capability.facts().unwrap();
        assert_eq!(actual.size, expected.size);
        assert_eq!(actual.mtime_ms, expected.mtime_ms);
        assert_eq!(actual.device, expected.device);
        assert_ne!(actual.inode, expected.inode);
        assert!(matches!(
            inspect_capture(&capability, OriginalKind::Raw, expected),
            Err(CaptureInspectionError::Confinement(
                ConfinementError::Changed
            ))
        ));
    }

    #[test]
    fn capture_reuse_revision_includes_discovery_device_and_inode() {
        let left = OriginalFacts {
            size: 12,
            mtime_ms: 1_000.0,
            device: 7,
            inode: 8,
        };
        let right = OriginalFacts { inode: 9, ..left };
        assert_ne!(
            capture_source_revision("same.ARW", left).unwrap(),
            capture_source_revision("same.ARW", right).unwrap()
        );
    }

    #[test]
    fn malformed_offsets_are_invalid_and_excessive_metadata_is_rejected() {
        let mut malformed = b"II*\0\x08\0\0\0".to_vec();
        malformed.extend_from_slice(&1_u16.to_le_bytes());
        malformed.extend_from_slice(&0x9003_u16.to_le_bytes());
        malformed.extend_from_slice(&2_u16.to_le_bytes());
        malformed.extend_from_slice(&20_u32.to_le_bytes());
        malformed.extend_from_slice(&4096_u32.to_le_bytes());
        malformed.extend_from_slice(&0_u32.to_le_bytes());
        assert_eq!(
            inspect_bytes(OriginalKind::Raw, &malformed).unwrap().state,
            CaptureMetadataState::Invalid
        );

        let mut excessive = b"II*\0\x08\0\0\0".to_vec();
        excessive.extend_from_slice(&((MAXIMUM_TIFF_DIRECTORY_ENTRIES + 1) as u16).to_le_bytes());
        assert!(matches!(
            inspect_bytes(OriginalKind::Raw, &excessive),
            Err(CaptureInspectionError::ResourceLimit)
        ));
    }
}
