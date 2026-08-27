//! Descriptor-confined EXIF Capture Time inspection.
//!
//! This module reads only EXIF/TIFF structures needed for review ordering. It
//! never passes a path to a parser, opens another descriptor, develops RAW
//! pixels, or follows metadata in an embedded Preview.

use crate::{
    OriginalCapability, OriginalFacts, OriginalKind,
    confinement::{ConfinementError, OpenedOriginal},
    source_revision,
};
use std::fmt;

/// Total metadata bytes one Capture Time inspection may read or allocate.
pub const MAXIMUM_CAPTURE_METADATA_BYTES: u64 = 16 * 1024 * 1024;
const MAXIMUM_TAG_VALUE_BYTES: usize = 64 * 1024;
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
}

impl CaptureFact {
    pub fn pending() -> Self {
        Self {
            state: CaptureMetadataState::Pending,
            order_key: None,
            field: None,
            offset_minutes: None,
            source_revision: None,
        }
    }

    pub fn failed(source_revision: Option<String>) -> Self {
        Self {
            state: CaptureMetadataState::Failed,
            order_key: None,
            field: None,
            offset_minutes: None,
            source_revision,
        }
    }

    fn completed(
        state: CaptureMetadataState,
        order_key: Option<String>,
        field: Option<CaptureTimeField>,
        offset_minutes: Option<i16>,
        source_revision: String,
    ) -> Self {
        Self {
            state,
            order_key,
            field,
            offset_minutes,
            source_revision: Some(source_revision),
        }
    }

    pub(crate) fn is_reusable_for(&self, source_revision: &str) -> bool {
        matches!(
            self.state,
            CaptureMetadataState::Known
                | CaptureMetadataState::Missing
                | CaptureMetadataState::Invalid
        ) && self.source_revision.as_deref() == Some(source_revision)
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

/// Stable Capture Time reuse identity, including the discovery descriptor's
/// device and inode so same-size/same-mtime path replacement cannot reuse an
/// old metadata fact.
pub(crate) fn capture_source_revision(
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
    let expected_source_revision =
        capture_source_revision(capability.path().as_str(), expected_facts)
            .map_err(|_| CaptureInspectionError::Confinement(ConfinementError::Changed))?;
    let opened = capability
        .open_revision_checked()
        .map_err(CaptureInspectionError::Confinement)?;
    let result =
        MetadataReader::new(&opened).and_then(|mut reader| parse_metadata(&mut reader, kind));
    // A metadata classification is useful only when it describes the same
    // bytes that were opened. This check intentionally wins over malformed,
    // I/O, and resource results from the parser.
    let facts = opened
        .verify_unchanged()
        .map_err(CaptureInspectionError::Confinement)?;
    // The discovery identity includes device and inode in addition to the
    // durable size/mtime facts. A same-size, same-mtime path replacement must
    // never publish metadata for the replacement under the discovery result.
    if facts != expected_facts {
        return Err(CaptureInspectionError::Confinement(
            ConfinementError::Changed,
        ));
    }
    let outcome = result.map_err(CaptureInspectionError::from)?;
    Ok(match outcome {
        ParseOutcome::Known {
            order_key,
            field,
            offset_minutes,
        } => CaptureFact::completed(
            CaptureMetadataState::Known,
            Some(order_key),
            Some(field),
            offset_minutes,
            expected_source_revision.clone(),
        ),
        ParseOutcome::Missing => CaptureFact::completed(
            CaptureMetadataState::Missing,
            None,
            None,
            None,
            expected_source_revision.clone(),
        ),
        ParseOutcome::Invalid => CaptureFact::completed(
            CaptureMetadataState::Invalid,
            None,
            None,
            None,
            expected_source_revision,
        ),
    })
}

#[derive(Debug)]
enum MetadataError {
    Invalid,
    Resource,
    Confinement(ConfinementError),
}

impl From<ConfinementError> for MetadataError {
    fn from(value: ConfinementError) -> Self {
        Self::Confinement(value)
    }
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

struct MetadataReader<'a> {
    opened: &'a OpenedOriginal,
    file_size: u64,
    bytes_read: u64,
}

impl<'a> MetadataReader<'a> {
    fn new(opened: &'a OpenedOriginal) -> Result<Self, MetadataError> {
        Ok(Self {
            opened,
            file_size: opened.size()?,
            bytes_read: 0,
        })
    }

    fn size(&self) -> u64 {
        self.file_size
    }

    fn read(&mut self, offset: u64, length: usize) -> Result<Vec<u8>, MetadataError> {
        let length_u64 = u64::try_from(length).map_err(|_| MetadataError::Resource)?;
        let end = offset
            .checked_add(length_u64)
            .ok_or(MetadataError::Invalid)?;
        if end > self.file_size {
            return Err(MetadataError::Invalid);
        }
        self.bytes_read = self
            .bytes_read
            .checked_add(length_u64)
            .filter(|value| *value <= MAXIMUM_CAPTURE_METADATA_BYTES)
            .ok_or(MetadataError::Resource)?;
        let bytes = self.opened.pread_range(offset, length)?;
        if bytes.len() != length {
            return Err(MetadataError::Invalid);
        }
        Ok(bytes)
    }
}

enum ParseOutcome {
    Known {
        order_key: String,
        field: CaptureTimeField,
        offset_minutes: Option<i16>,
    },
    Missing,
    Invalid,
}

#[derive(Clone, Debug)]
enum TagValue {
    Absent,
    Valid(Vec<u8>),
    Invalid,
}

#[derive(Clone, Debug)]
struct ExifFields {
    original: TagValue,
    digitized: TagValue,
    subsec_original: TagValue,
    subsec_digitized: TagValue,
    offset_original: TagValue,
    offset_digitized: TagValue,
}

impl Default for ExifFields {
    fn default() -> Self {
        Self {
            original: TagValue::Absent,
            digitized: TagValue::Absent,
            subsec_original: TagValue::Absent,
            subsec_digitized: TagValue::Absent,
            offset_original: TagValue::Absent,
            offset_digitized: TagValue::Absent,
        }
    }
}

fn parse_metadata(
    reader: &mut MetadataReader<'_>,
    kind: OriginalKind,
) -> Result<ParseOutcome, MetadataError> {
    match kind {
        OriginalKind::Jpeg => Ok(parse_jpeg_exif(reader)?
            .as_ref()
            .map_or(ParseOutcome::Missing, select_capture_fact)),
        OriginalKind::Raw => match parse_raw_tiff(reader)? {
            // TIFF fields retain selected EXIF field, subseconds, and camera
            // offset exactly. LibRaw is never consulted for a TIFF container.
            Some(fields) => Ok(select_capture_fact(&fields)),
            // Non-TIFF RAW containers have no bounded TIFF structure at byte
            // zero. The narrow native fallback opens only the retained
            // descriptor and reads LibRaw's metadata timestamp.
            None => parse_non_tiff_raw_capture_time(reader.opened),
        },
    }
}

fn parse_non_tiff_raw_capture_time(opened: &OpenedOriginal) -> Result<ParseOutcome, MetadataError> {
    match crate::native::inspect_raw_capture_time(opened) {
        Ok(Some(time)) => Ok(ParseOutcome::Known {
            order_key: format!(
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000000000",
                time.year, time.month, time.day, time.hour, time.minute, time.second
            ),
            // LibRaw's timestamp does not identify original versus digitized.
            field: CaptureTimeField::DateTimeOriginal,
            offset_minutes: None,
        }),
        Ok(None) | Err(crate::NativePreviewError::Unsupported) => Ok(ParseOutcome::Missing),
        Err(crate::NativePreviewError::Malformed) => Ok(ParseOutcome::Invalid),
        Err(crate::NativePreviewError::ResourceLimit) => Err(MetadataError::Resource),
        Err(crate::NativePreviewError::Io | crate::NativePreviewError::Internal) => {
            Err(MetadataError::Confinement(ConfinementError::Io(
                "RAW Capture Time metadata could not be inspected",
            )))
        }
        Err(crate::NativePreviewError::NoUsablePreview) => Ok(ParseOutcome::Missing),
    }
}

fn parse_jpeg_exif(reader: &mut MetadataReader<'_>) -> Result<Option<ExifFields>, MetadataError> {
    if reader.size() < 2 || reader.read(0, 2)? != [0xff, 0xd8] {
        return Ok(None);
    }
    let mut position = 2_u64;
    for _ in 0..MAXIMUM_JPEG_MARKERS {
        if position >= reader.size() {
            return Ok(None);
        }
        if reader.read(position, 1)?[0] != 0xff {
            return Err(MetadataError::Invalid);
        }
        position = position.checked_add(1).ok_or(MetadataError::Invalid)?;
        let marker = loop {
            if position >= reader.size() {
                return Err(MetadataError::Invalid);
            }
            let value = reader.read(position, 1)?[0];
            position = position.checked_add(1).ok_or(MetadataError::Invalid)?;
            if value != 0xff {
                break value;
            }
        };
        match marker {
            0xd9 | 0xda => return Ok(None),
            0xd8 | 0x01 | 0xd0..=0xd7 => continue,
            0x00 => return Err(MetadataError::Invalid),
            _ => {}
        }
        let length = reader.read(position, 2)?;
        position = position.checked_add(2).ok_or(MetadataError::Invalid)?;
        let segment_length = u64::from(u16::from_be_bytes([length[0], length[1]]));
        if segment_length < 2 {
            return Err(MetadataError::Invalid);
        }
        let payload_start = position;
        let payload_length = segment_length - 2;
        let segment_end = payload_start
            .checked_add(payload_length)
            .ok_or(MetadataError::Invalid)?;
        if segment_end > reader.size() {
            return Err(MetadataError::Invalid);
        }
        if marker == 0xe1 && payload_length > 0 {
            let prefix_length =
                usize::try_from(payload_length.min(6)).map_err(|_| MetadataError::Resource)?;
            let prefix = reader.read(payload_start, prefix_length)?;
            if prefix == b"Exif\0\0" {
                return parse_tiff(reader, payload_start + 6, segment_end);
            }
            if prefix.starts_with(b"Exif") {
                return Err(MetadataError::Invalid);
            }
        }
        position = segment_end;
    }
    Err(MetadataError::Resource)
}

fn parse_raw_tiff(reader: &mut MetadataReader<'_>) -> Result<Option<ExifFields>, MetadataError> {
    if reader.size() < 4 {
        return Ok(None);
    }
    let header = reader.read(0, 4)?;
    if !matches!(header.as_slice(), b"II*\0" | b"MM\0*") {
        return Ok(None);
    }
    parse_tiff(reader, 0, reader.size())
}

#[derive(Clone, Copy)]
enum ByteOrder {
    Little,
    Big,
}

#[derive(Clone, Copy)]
struct TiffEntry {
    tag: u16,
    field_type: u16,
    count: u32,
    value_offset: u32,
    inline: [u8; 4],
}

struct TiffReader<'a, 'b> {
    reader: &'a mut MetadataReader<'b>,
    start: u64,
    end: u64,
}

impl TiffReader<'_, '_> {
    fn read_relative(&mut self, offset: u32, length: usize) -> Result<Vec<u8>, MetadataError> {
        let position = self
            .start
            .checked_add(u64::from(offset))
            .ok_or(MetadataError::Invalid)?;
        let end = position
            .checked_add(u64::try_from(length).map_err(|_| MetadataError::Resource)?)
            .ok_or(MetadataError::Invalid)?;
        if end > self.end {
            return Err(MetadataError::Invalid);
        }
        self.reader.read(position, length)
    }
}

fn parse_tiff(
    reader: &mut MetadataReader<'_>,
    start: u64,
    end: u64,
) -> Result<Option<ExifFields>, MetadataError> {
    let header_end = start.checked_add(8).ok_or(MetadataError::Invalid)?;
    if header_end > end {
        return Err(MetadataError::Invalid);
    }
    let header = reader.read(start, 8)?;
    let order = match &header[..2] {
        b"II" => ByteOrder::Little,
        b"MM" => ByteOrder::Big,
        _ => return Ok(None),
    };
    if read_u16(&header[2..4], order) != 42 {
        return Err(MetadataError::Invalid);
    }
    let mut tiff = TiffReader { reader, start, end };
    let entries = read_directory(&mut tiff, read_u32(&header[4..8], order), order)?;
    let mut fields = ExifFields::default();
    collect_capture_tags(&mut tiff, order, &entries, &mut fields)?;
    if let Some(exif_offset) = single_exif_offset(&mut tiff, order, &entries)? {
        let entries = read_directory(&mut tiff, exif_offset, order)?;
        collect_capture_tags(&mut tiff, order, &entries, &mut fields)?;
    }
    Ok(Some(fields))
}

fn read_directory(
    reader: &mut TiffReader<'_, '_>,
    offset: u32,
    order: ByteOrder,
) -> Result<Vec<TiffEntry>, MetadataError> {
    let count = reader.read_relative(offset, 2)?;
    let count = usize::from(read_u16(&count, order));
    if count > MAXIMUM_TIFF_DIRECTORY_ENTRIES {
        return Err(MetadataError::Resource);
    }
    let entries_offset = offset.checked_add(2).ok_or(MetadataError::Invalid)?;
    let length = count.checked_mul(12).ok_or(MetadataError::Resource)?;
    let data = reader.read_relative(entries_offset, length)?;
    let mut entries = Vec::with_capacity(count);
    for item in data.chunks_exact(12) {
        let mut inline = [0; 4];
        inline.copy_from_slice(&item[8..12]);
        entries.push(TiffEntry {
            tag: read_u16(&item[..2], order),
            field_type: read_u16(&item[2..4], order),
            count: read_u32(&item[4..8], order),
            value_offset: read_u32(&item[8..12], order),
            inline,
        });
    }
    Ok(entries)
}

fn single_exif_offset(
    reader: &mut TiffReader<'_, '_>,
    order: ByteOrder,
    entries: &[TiffEntry],
) -> Result<Option<u32>, MetadataError> {
    let matches = entries
        .iter()
        .filter(|entry| entry.tag == 0x8769)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [entry] => scalar_u32(reader, order, entry).map(Some),
        _ => Err(MetadataError::Invalid),
    }
}

fn scalar_u32(
    reader: &mut TiffReader<'_, '_>,
    order: ByteOrder,
    entry: &TiffEntry,
) -> Result<u32, MetadataError> {
    if entry.count != 1 || !matches!(entry.field_type, 3 | 4) {
        return Err(MetadataError::Invalid);
    }
    let value = field_bytes(reader, entry)?;
    match entry.field_type {
        3 => Ok(u32::from(read_u16(&value[..2], order))),
        4 => Ok(read_u32(&value[..4], order)),
        _ => Err(MetadataError::Invalid),
    }
}

fn collect_capture_tags(
    reader: &mut TiffReader<'_, '_>,
    _order: ByteOrder,
    entries: &[TiffEntry],
    fields: &mut ExifFields,
) -> Result<(), MetadataError> {
    for entry in entries {
        let target = match entry.tag {
            0x9003 => Some(&mut fields.original),
            0x9004 => Some(&mut fields.digitized),
            0x9291 => Some(&mut fields.subsec_original),
            0x9292 => Some(&mut fields.subsec_digitized),
            0x9011 => Some(&mut fields.offset_original),
            0x9012 => Some(&mut fields.offset_digitized),
            _ => None,
        };
        let Some(target) = target else { continue };
        if !matches!(target, TagValue::Absent) || entry.field_type != 2 {
            *target = TagValue::Invalid;
            continue;
        }
        *target = match field_bytes(reader, entry) {
            Ok(value) => TagValue::Valid(value),
            Err(MetadataError::Invalid) => TagValue::Invalid,
            Err(error) => return Err(error),
        };
    }
    Ok(())
}

fn field_bytes(
    reader: &mut TiffReader<'_, '_>,
    entry: &TiffEntry,
) -> Result<Vec<u8>, MetadataError> {
    let unit = match entry.field_type {
        1 | 2 | 6 | 7 => 1_usize,
        3 | 8 => 2,
        4 | 9 | 11 => 4,
        5 | 10 | 12 => 8,
        _ => return Err(MetadataError::Invalid),
    };
    let length = usize::try_from(entry.count)
        .ok()
        .and_then(|count| count.checked_mul(unit))
        .ok_or(MetadataError::Resource)?;
    if length > MAXIMUM_TAG_VALUE_BYTES {
        return Err(MetadataError::Resource);
    }
    if length <= 4 {
        return Ok(entry.inline[..length].to_vec());
    }
    reader.read_relative(entry.value_offset, length)
}

fn select_capture_fact(fields: &ExifFields) -> ParseOutcome {
    for (base, subsecond, offset, field) in [
        (
            &fields.original,
            &fields.subsec_original,
            &fields.offset_original,
            CaptureTimeField::DateTimeOriginal,
        ),
        (
            &fields.digitized,
            &fields.subsec_digitized,
            &fields.offset_digitized,
            CaptureTimeField::DateTimeDigitized,
        ),
    ] {
        match parse_base(base) {
            BaseValue::Absent | BaseValue::Invalid => continue,
            BaseValue::Valid((year, month, day, hour, minute, second)) => {
                let fraction = parse_subsecond(subsecond);
                return ParseOutcome::Known {
                    order_key: format!(
                        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{fraction:09}"
                    ),
                    field,
                    offset_minutes: parse_offset(offset),
                };
            }
        }
    }
    if matches!(fields.original, TagValue::Invalid | TagValue::Valid(_))
        || matches!(fields.digitized, TagValue::Invalid | TagValue::Valid(_))
    {
        ParseOutcome::Invalid
    } else {
        ParseOutcome::Missing
    }
}

enum BaseValue {
    Absent,
    Invalid,
    Valid((u16, u8, u8, u8, u8, u8)),
}

fn parse_base(value: &TagValue) -> BaseValue {
    let TagValue::Valid(value) = value else {
        return if matches!(value, TagValue::Absent) {
            BaseValue::Absent
        } else {
            BaseValue::Invalid
        };
    };
    let value = trim_terminal_nuls(value);
    if value.len() != 19
        || value[4] != b':'
        || value[7] != b':'
        || value[10] != b' '
        || value[13] != b':'
        || value[16] != b':'
    {
        return BaseValue::Invalid;
    }
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        decimal(&value[0..4]),
        decimal(&value[5..7]),
        decimal(&value[8..10]),
        decimal(&value[11..13]),
        decimal(&value[14..16]),
        decimal(&value[17..19]),
    ) else {
        return BaseValue::Invalid;
    };
    let (year, month, day, hour, minute, second) = (
        u16::try_from(year).ok(),
        u8::try_from(month).ok(),
        u8::try_from(day).ok(),
        u8::try_from(hour).ok(),
        u8::try_from(minute).ok(),
        u8::try_from(second).ok(),
    );
    let (Some(year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) =
        (year, month, day, hour, minute, second)
    else {
        return BaseValue::Invalid;
    };
    if year == 0
        || !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return BaseValue::Invalid;
    }
    BaseValue::Valid((year, month, day, hour, minute, second))
}

fn days_in_month(year: u16, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(400) || (year.is_multiple_of(4) && !year.is_multiple_of(100)) => {
            29
        }
        2 => 28,
        _ => 0,
    }
}

fn parse_subsecond(value: &TagValue) -> u32 {
    let TagValue::Valid(value) = value else {
        return 0;
    };
    let value = trim_terminal_nuls(value);
    if value.is_empty() || !value.iter().all(u8::is_ascii_digit) {
        return 0;
    }
    let mut digits = value.iter().copied().take(9).collect::<Vec<_>>();
    digits.resize(9, b'0');
    decimal(&digits).unwrap_or(0)
}

fn parse_offset(value: &TagValue) -> Option<i16> {
    let TagValue::Valid(value) = value else {
        return None;
    };
    let value = trim_terminal_nuls(value);
    if value.len() != 6 || !matches!(value[0], b'+' | b'-') || value[3] != b':' {
        return None;
    }
    let hours = decimal(&value[1..3])?;
    let minutes = decimal(&value[4..6])?;
    if minutes > 59 {
        return None;
    }
    let total = hours.checked_mul(60)?.checked_add(minutes)?;
    if total > 840 {
        return None;
    }
    let total = i16::try_from(total).ok()?;
    Some(if value[0] == b'-' { -total } else { total })
}

fn trim_terminal_nuls(value: &[u8]) -> &[u8] {
    let mut end = value.len();
    while end > 0 && value[end - 1] == 0 {
        end -= 1;
    }
    &value[..end]
}

fn decimal(value: &[u8]) -> Option<u32> {
    value.iter().try_fold(0_u32, |result, byte| {
        byte.is_ascii_digit()
            .then(|| result.checked_mul(10)?.checked_add(u32::from(*byte - b'0')))
            .flatten()
    })
}

fn read_u16(value: &[u8], order: ByteOrder) -> u16 {
    let array: [u8; 2] = value.try_into().expect("TIFF field is exact width");
    match order {
        ByteOrder::Little => u16::from_le_bytes(array),
        ByteOrder::Big => u16::from_be_bytes(array),
    }
}

fn read_u32(value: &[u8], order: ByteOrder) -> u32 {
    let array: [u8; 4] = value.try_into().expect("TIFF field is exact width");
    match order {
        ByteOrder::Little => u32::from_le_bytes(array),
        ByteOrder::Big => u32::from_be_bytes(array),
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

    fn tiff(tags: &[(u16, Vec<u8>)]) -> Vec<u8> {
        let data_start = 8 + 2 + tags.len() * 12 + 4;
        let mut bytes = b"II*\0\x08\0\0\0".to_vec();
        bytes.extend_from_slice(&(tags.len() as u16).to_le_bytes());
        let mut data = Vec::new();
        for (tag, value) in tags {
            bytes.extend_from_slice(&tag.to_le_bytes());
            bytes.extend_from_slice(&2_u16.to_le_bytes());
            bytes.extend_from_slice(&(value.len() as u32).to_le_bytes());
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
    fn parses_selected_field_subseconds_and_offset_without_applying_offset() {
        let fact = inspect_bytes(
            OriginalKind::Jpeg,
            &jpeg(&tiff(&[
                (0x9003, b"2026:02:03 04:05:06\0".to_vec()),
                (0x9291, b"12\0".to_vec()),
                (0x9011, b"+01:30\0".to_vec()),
            ])),
        )
        .unwrap();
        assert_eq!(
            fact.order_key.as_deref(),
            Some("2026-02-03T04:05:06.120000000")
        );
        assert_eq!(fact.field, Some(CaptureTimeField::DateTimeOriginal));
        assert_eq!(fact.offset_minutes, Some(90));
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
