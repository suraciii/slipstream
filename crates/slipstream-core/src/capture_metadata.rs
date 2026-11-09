//! Bounded JPEG/TIFF capture metadata parsing from a retained Original descriptor.
//!
//! The capture owner opens and verifies the descriptor; this parser only reads it.

use super::{
    CaptureReviewMetadata, CaptureTimeField, MAXIMUM_CAPTURE_METADATA_BYTES, MAXIMUM_JPEG_MARKERS,
    MAXIMUM_TAG_VALUE_BYTES, MAXIMUM_TEXT_TAG_BYTES, MAXIMUM_TIFF_DIRECTORY_ENTRIES,
};
use crate::{
    OriginalKind,
    confinement::{ConfinementError, OpenedOriginal},
};

#[derive(Debug)]
pub(super) enum MetadataError {
    Invalid,
    Resource,
    Confinement(ConfinementError),
}

impl From<ConfinementError> for MetadataError {
    fn from(value: ConfinementError) -> Self {
        Self::Confinement(value)
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

pub(super) enum ParseOutcome {
    Known {
        order_key: String,
        field: CaptureTimeField,
        offset_minutes: Option<i16>,
        review: CaptureReviewMetadata,
    },
    Missing {
        review: CaptureReviewMetadata,
    },
    Invalid {
        review: CaptureReviewMetadata,
    },
}

#[derive(Clone, Debug)]
enum TagValue {
    Absent,
    Valid(Vec<u8>),
    Invalid,
}

#[derive(Clone, Debug)]
struct ExifFields {
    make: TagValue,
    model: TagValue,
    original: TagValue,
    digitized: TagValue,
    subsec_original: TagValue,
    subsec_digitized: TagValue,
    offset_original: TagValue,
    offset_digitized: TagValue,
    aperture: TagValue,
    iso: TagValue,
    exposure_time: TagValue,
    focal_length: TagValue,
    order: Option<ByteOrder>,
}

impl Default for ExifFields {
    fn default() -> Self {
        Self {
            make: TagValue::Absent,
            model: TagValue::Absent,
            original: TagValue::Absent,
            digitized: TagValue::Absent,
            subsec_original: TagValue::Absent,
            subsec_digitized: TagValue::Absent,
            offset_original: TagValue::Absent,
            offset_digitized: TagValue::Absent,
            aperture: TagValue::Absent,
            iso: TagValue::Absent,
            exposure_time: TagValue::Absent,
            focal_length: TagValue::Absent,
            order: None,
        }
    }
}

pub(super) fn parse_metadata(
    opened: &OpenedOriginal,
    kind: OriginalKind,
) -> Result<ParseOutcome, MetadataError> {
    let mut reader = MetadataReader::new(opened)?;
    match kind {
        OriginalKind::Jpeg => Ok(parse_jpeg_exif(&mut reader)?.as_ref().map_or(
            ParseOutcome::Missing {
                review: CaptureReviewMetadata::default(),
            },
            select_capture_fact,
        )),
        OriginalKind::Raw => match parse_raw_tiff(&mut reader)? {
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
            review: CaptureReviewMetadata {
                capture_time: Some(format!(
                    "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000000000",
                    time.year, time.month, time.day, time.hour, time.minute, time.second
                )),
                ..CaptureReviewMetadata::default()
            },
        }),
        Ok(None) | Err(crate::NativePreviewError::Unsupported) => Ok(ParseOutcome::Missing {
            review: CaptureReviewMetadata::default(),
        }),
        Err(crate::NativePreviewError::Malformed) => Ok(ParseOutcome::Invalid {
            review: CaptureReviewMetadata::default(),
        }),
        Err(crate::NativePreviewError::ResourceLimit) => Err(MetadataError::Resource),
        Err(crate::NativePreviewError::Io | crate::NativePreviewError::Internal) => {
            Err(MetadataError::Confinement(ConfinementError::Io(
                "RAW Capture Time metadata could not be inspected",
            )))
        }
        Err(crate::NativePreviewError::NoUsablePreview) => Ok(ParseOutcome::Missing {
            review: CaptureReviewMetadata::default(),
        }),
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

#[derive(Clone, Copy, Debug)]
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
    let mut fields = ExifFields {
        order: Some(order),
        ..ExifFields::default()
    };
    collect_capture_tags(
        &mut tiff,
        order,
        &entries,
        &mut fields,
        CaptureDirectory::Primary,
    )?;
    if let Some(exif_offset) = single_exif_offset(&mut tiff, order, &entries)? {
        let entries = read_directory(&mut tiff, exif_offset, order)?;
        collect_capture_tags(
            &mut tiff,
            order,
            &entries,
            &mut fields,
            CaptureDirectory::Exif,
        )?;
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

/// Which TIFF directory a collection pass reads.
#[derive(Clone, Copy, Eq, PartialEq)]
enum CaptureDirectory {
    /// The primary image directory.
    Primary,
    /// The Exif subdirectory referenced from the primary directory.
    Exif,
}

/// Camera identity tags, which are only meaningful in the primary directory.
fn is_camera_identity(tag: u16) -> bool {
    matches!(tag, 0x010f | 0x0110)
}

fn collect_capture_tags(
    reader: &mut TiffReader<'_, '_>,
    _order: ByteOrder,
    entries: &[TiffEntry],
    fields: &mut ExifFields,
    directory: CaptureDirectory,
) -> Result<(), MetadataError> {
    for entry in entries {
        // A Make or Model in the Exif subdirectory is not the camera identity:
        // accepting it would let a file with no primary identity be classified
        // as an approved camera, so it is ignored rather than invalidated.
        if directory == CaptureDirectory::Exif && is_camera_identity(entry.tag) {
            continue;
        }
        let (target, expected_types): (Option<&mut TagValue>, &[u16]) = match entry.tag {
            0x010f => (Some(&mut fields.make), &[2]),
            0x0110 => (Some(&mut fields.model), &[2]),
            0x9003 => (Some(&mut fields.original), &[2]),
            0x9004 => (Some(&mut fields.digitized), &[2]),
            0x9291 => (Some(&mut fields.subsec_original), &[2]),
            0x9292 => (Some(&mut fields.subsec_digitized), &[2]),
            0x9011 => (Some(&mut fields.offset_original), &[2]),
            0x9012 => (Some(&mut fields.offset_digitized), &[2]),
            0x829d => (Some(&mut fields.aperture), &[5]),
            0x8827 | 0x8833 => (Some(&mut fields.iso), &[3, 4]),
            0x829a => (Some(&mut fields.exposure_time), &[5]),
            0x920a => (Some(&mut fields.focal_length), &[5]),
            _ => (None, &[]),
        };
        let Some(target) = target else { continue };
        if !matches!(target, TagValue::Absent) || !expected_types.contains(&entry.field_type) {
            *target = TagValue::Invalid;
            continue;
        }
        *target = match field_bytes(reader, entry) {
            Ok(value) => TagValue::Valid(value),
            Err(MetadataError::Invalid) => TagValue::Invalid,
            // A camera identity larger than the bounded tag ceiling is
            // unusable, but it must not discard the rest of the bounded
            // metadata: report the identity as unavailable and keep the facts.
            Err(MetadataError::Resource) if is_camera_identity(entry.tag) => TagValue::Invalid,
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
    let review = review_metadata(fields);
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
                    review: CaptureReviewMetadata {
                        capture_time: Some(format!(
                            "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{fraction:09}"
                        )),
                        ..review.clone()
                    },
                };
            }
        }
    }
    if matches!(fields.original, TagValue::Invalid | TagValue::Valid(_))
        || matches!(fields.digitized, TagValue::Invalid | TagValue::Valid(_))
    {
        ParseOutcome::Invalid { review }
    } else {
        ParseOutcome::Missing { review }
    }
}

fn review_metadata(fields: &ExifFields) -> CaptureReviewMetadata {
    let Some(order) = fields.order else {
        return CaptureReviewMetadata::default();
    };
    CaptureReviewMetadata {
        capture_time: None,
        aperture: parse_rational(&fields.aperture, order).map(|(numerator, denominator)| {
            format!("f/{}", format_decimal(numerator, denominator))
        }),
        iso: parse_integer(&fields.iso, order),
        shutter_speed: parse_rational(&fields.exposure_time, order).map(
            |(numerator, denominator)| format!("{} s", format_fraction(numerator, denominator)),
        ),
        focal_length: parse_rational(&fields.focal_length, order).map(
            |(numerator, denominator)| format!("{} mm", format_decimal(numerator, denominator)),
        ),
        make: parse_text(&fields.make),
        model: parse_text(&fields.model),
    }
}

/// One bounded ASCII tag value: camera strings are NUL-terminated and may carry
/// padding. A value that is not printable ASCII is reported as absent rather
/// than guessed.
fn parse_text(value: &TagValue) -> Option<String> {
    let TagValue::Valid(value) = value else {
        return None;
    };
    let end = value
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(value.len());
    let text = value.get(..end)?;
    if text.is_empty() || text.len() > MAXIMUM_TEXT_TAG_BYTES {
        return None;
    }
    if !text
        .iter()
        .all(|byte| byte.is_ascii_graphic() || *byte == b' ')
    {
        return None;
    }
    let text = String::from_utf8_lossy(text);
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn parse_integer(value: &TagValue, order: ByteOrder) -> Option<u32> {
    let TagValue::Valid(value) = value else {
        return None;
    };
    match value.len() {
        2 => Some(u32::from(read_u16(value, order))),
        4 => Some(read_u32(value, order)),
        _ => None,
    }
}

fn parse_rational(value: &TagValue, order: ByteOrder) -> Option<(u32, u32)> {
    let TagValue::Valid(value) = value else {
        return None;
    };
    if value.len() != 8 {
        return None;
    }
    let numerator = read_u32(&value[..4], order);
    let denominator = read_u32(&value[4..], order);
    (denominator != 0).then_some((numerator, denominator))
}

fn format_fraction(numerator: u32, denominator: u32) -> String {
    if numerator == 0 {
        return "0".to_owned();
    }
    if numerator.is_multiple_of(denominator) {
        return (numerator / denominator).to_string();
    }
    format!("{numerator}/{denominator}")
}

fn format_decimal(numerator: u32, denominator: u32) -> String {
    if numerator.is_multiple_of(denominator) {
        return (numerator / denominator).to_string();
    }
    let scaled = (u64::from(numerator) * 100 + u64::from(denominator) / 2) / u64::from(denominator);
    let whole = scaled / 100;
    let fraction = scaled % 100;
    if fraction == 0 {
        whole.to_string()
    } else if fraction % 10 == 0 {
        format!("{whole}.{}", fraction / 10)
    } else {
        format!("{whole}.{fraction:02}")
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
