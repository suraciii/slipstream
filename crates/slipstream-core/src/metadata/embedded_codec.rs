//! Bounded TIFF/EXIF and IPTC IIM decoding for embedded metadata.
//!
//! The owner walks JPEG and TIFF containers, owns field state, and verifies the
//! retained descriptor; this module only decodes bounded payloads.

use super::{
    CaptureField, EmbeddedExtractionError, EmbeddedMetadata, FieldState, IimEncoding, IimSource,
    MAX_ENTRIES, MAX_VALUE, PacketState, Reader,
};

#[derive(Clone, Copy)]
enum Order {
    Little,
    Big,
}
fn u16_at(data: &[u8], order: Order) -> u16 {
    match order {
        Order::Little => u16::from_le_bytes([data[0], data[1]]),
        Order::Big => u16::from_be_bytes([data[0], data[1]]),
    }
}
fn u32_at(data: &[u8], order: Order) -> u32 {
    match order {
        Order::Little => u32::from_le_bytes([data[0], data[1], data[2], data[3]]),
        Order::Big => u32::from_be_bytes([data[0], data[1], data[2], data[3]]),
    }
}
#[derive(Clone, Copy)]
struct Entry {
    tag: u16,
    ty: u16,
    count: u32,
    offset: u32,
    inline: [u8; 4],
}
struct Tiff<'a, 'b> {
    reader: &'a mut Reader<'b>,
    start: u64,
    end: u64,
    order: Order,
}
impl Tiff<'_, '_> {
    fn read(&mut self, offset: u32, len: usize) -> Result<Vec<u8>, EmbeddedExtractionError> {
        let position = self
            .start
            .checked_add(u64::from(offset))
            .ok_or(EmbeddedExtractionError::InvalidInput)?;
        if position
            .checked_add(len as u64)
            .filter(|end| *end <= self.end)
            .is_none()
        {
            return Err(EmbeddedExtractionError::InvalidInput);
        }
        self.reader.read(position, len)
    }
    fn directory(&mut self, offset: u32) -> Result<Vec<Entry>, EmbeddedExtractionError> {
        let count = usize::from(u16_at(&self.read(offset, 2)?, self.order));
        if count > MAX_ENTRIES {
            return Err(EmbeddedExtractionError::ResourceLimit);
        }
        let data = self.read(
            offset
                .checked_add(2)
                .ok_or(EmbeddedExtractionError::InvalidInput)?,
            count * 12,
        )?;
        Ok(data
            .chunks_exact(12)
            .map(|bytes| {
                let mut inline = [0; 4];
                inline.copy_from_slice(&bytes[8..12]);
                Entry {
                    tag: u16_at(bytes, self.order),
                    ty: u16_at(&bytes[2..], self.order),
                    count: u32_at(&bytes[4..], self.order),
                    offset: u32_at(&bytes[8..], self.order),
                    inline,
                }
            })
            .collect())
    }
    fn value_size(entry: Entry) -> Option<u64> {
        let unit: u64 = match entry.ty {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 => 4,
            5 | 10 | 12 => 8,
            _ => return None,
        };
        Some(u64::from(entry.count) * unit)
    }
    fn value(&mut self, entry: Entry) -> Result<Vec<u8>, FieldState> {
        let len = Self::value_size(entry).ok_or(FieldState::Invalid)?;
        if len > MAX_VALUE as u64 {
            return Err(FieldState::ResourceLimit);
        }
        if len <= 4 {
            return Ok(entry.inline[..len as usize].to_vec());
        }
        self.read(entry.offset, len as usize).map_err(|e| match e {
            EmbeddedExtractionError::ResourceLimit => FieldState::ResourceLimit,
            _ => FieldState::Invalid,
        })
    }
}
pub(super) fn parse_tiff(
    reader: &mut Reader<'_>,
    start: u64,
    end: u64,
    output: &mut EmbeddedMetadata,
) -> Result<(), EmbeddedExtractionError> {
    if start.checked_add(8).filter(|n| *n <= end).is_none() {
        return Err(EmbeddedExtractionError::InvalidInput);
    }
    let header = reader.read(start, 8)?;
    let order = match &header[..2] {
        b"II" => Order::Little,
        b"MM" => Order::Big,
        _ => return Err(EmbeddedExtractionError::InvalidInput),
    };
    if u16_at(&header[2..], order) != 42 {
        return Err(EmbeddedExtractionError::InvalidInput);
    }
    let mut tiff = Tiff {
        reader,
        start,
        end,
        order,
    };
    let entries = tiff.directory(u32_at(&header[4..], order))?;
    for &entry in &entries {
        if start == 0 || !matches!(entry.tag, 0x02bc | 0x83bb) {
            apply_entry(&mut tiff, entry, false, output);
        }
    }
    let pointers: Vec<_> = entries.iter().filter(|entry| entry.tag == 0x8769).collect();
    if pointers.len() > 1 {
        output.capture.mark_exif(FieldState::Invalid);
    } else if let Some(pointer) = pointers.first() {
        if pointer.count != 1 || !matches!(pointer.ty, 3 | 4) {
            output.capture.mark_exif(FieldState::Invalid);
        } else if let Ok(bytes) = tiff.value(**pointer) {
            let offset = if pointer.ty == 3 {
                u32::from(u16_at(&bytes, order))
            } else {
                u32_at(&bytes, order)
            };
            match tiff.directory(offset) {
                Ok(entries) => {
                    for entry in entries {
                        apply_entry(&mut tiff, entry, true, output);
                    }
                }
                Err(EmbeddedExtractionError::ResourceLimit) => {
                    output.capture.mark_exif(FieldState::ResourceLimit)
                }
                Err(_) => output.capture.mark_exif(FieldState::Invalid),
            }
        } else {
            output.capture.mark_exif(FieldState::Invalid);
        }
    }
    Ok(())
}
fn assign<T>(
    field: &mut CaptureField<T>,
    entry: Entry,
    tiff: &mut Tiff<'_, '_>,
    types: &[u16],
    parse: impl FnOnce(&[u8], Order) -> Option<T>,
) {
    if field.state != FieldState::Absent || !types.contains(&entry.ty) {
        field.value = None;
        field.state = FieldState::Invalid;
        return;
    }
    match tiff.value(entry) {
        Ok(bytes) => match parse(&bytes, tiff.order) {
            Some(value) => {
                field.value = Some(value);
                field.state = FieldState::Present;
            }
            None => field.state = FieldState::Invalid,
        },
        Err(state) => field.state = state,
    }
}
/// Scalar capture values must declare exactly one value of an allowed type;
/// a multi-value payload is never reinterpreted by its byte length. A declared
/// size beyond the extraction limit still reports the limit.
fn assign_scalar<T>(
    field: &mut CaptureField<T>,
    entry: Entry,
    tiff: &mut Tiff<'_, '_>,
    types: &[u16],
    parse: impl FnOnce(&[u8], Order) -> Option<T>,
) {
    match Tiff::value_size(entry) {
        None => assign(field, entry, tiff, types, parse),
        Some(len) if len > MAX_VALUE as u64 => field.state = FieldState::ResourceLimit,
        Some(_) if entry.count != 1 => {
            field.value = None;
            field.state = FieldState::Invalid;
        }
        Some(_) => assign(field, entry, tiff, types, parse),
    }
}
fn ascii(bytes: &[u8], _: Order) -> Option<String> {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    if bytes[end..].iter().any(|byte| *byte != 0)
        || bytes[..end]
            .iter()
            .any(|byte| !byte.is_ascii_graphic() && *byte != b' ')
    {
        return None;
    }
    std::str::from_utf8(&bytes[..end]).ok().map(str::to_owned)
}
fn number(bytes: &[u8], order: Order) -> Option<u32> {
    match bytes.len() {
        2 => Some(u32::from(u16_at(bytes, order))),
        4 => Some(u32_at(bytes, order)),
        _ => None,
    }
}
fn dimension(bytes: &[u8], order: Order) -> Option<u32> {
    number(bytes, order).filter(|value| *value != 0)
}

// A present higher-priority source wins even when invalid or resource-limited.
pub(super) fn select_dimension_source(
    field: &mut CaptureField<u32>,
    identifier: &'static str,
) -> bool {
    let priority = |id| match id {
        "PixelXDimension" | "PixelYDimension" => 3,
        "ImageWidth" | "ImageLength" => 2,
        _ => 1,
    };
    if field.state != FieldState::Absent {
        match priority(identifier).cmp(&priority(field.exif_identifier)) {
            std::cmp::Ordering::Less => return false,
            std::cmp::Ordering::Equal => {
                field.value = None;
                field.state = FieldState::Invalid;
                return false;
            }
            std::cmp::Ordering::Greater => {}
        }
    }
    field.value = None;
    field.state = FieldState::Absent;
    field.exif_identifier = identifier;
    true
}
fn short(bytes: &[u8], order: Order) -> Option<u16> {
    if bytes.len() != 2 {
        return None;
    }
    let value = u16_at(bytes, order);
    (1..=8).contains(&value).then_some(value)
}
fn rational(bytes: &[u8], order: Order) -> Option<String> {
    if bytes.len() != 8 {
        return None;
    }
    let denominator = u32_at(&bytes[4..], order);
    (denominator != 0).then(|| format!("{}/{denominator}", u32_at(bytes, order)))
}
fn datetime(bytes: &[u8], order: Order) -> Option<String> {
    let value = ascii(bytes, order)?;
    let v = value.as_bytes();
    if v.len() != 19
        || v[4] != b':'
        || v[7] != b':'
        || v[10] != b' '
        || v[13] != b':'
        || v[16] != b':'
    {
        return None;
    }
    let part = |start: usize, end: usize| value[start..end].parse::<u32>().ok();
    let (year, month, day, hour, minute, second) = (
        part(0, 4)?,
        part(5, 7)?,
        part(8, 10)?,
        part(11, 13)?,
        part(14, 16)?,
        part(17, 19)?,
    );
    let leap = year.is_multiple_of(400) || (year.is_multiple_of(4) && !year.is_multiple_of(100));
    let days = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if year == 0 || day == 0 || day > days || hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some(value)
}
fn subseconds(bytes: &[u8], order: Order) -> Option<String> {
    let value = ascii(bytes, order)?;
    (!value.is_empty() && value.bytes().all(|c| c.is_ascii_digit())).then_some(value)
}
fn offset(bytes: &[u8], order: Order) -> Option<String> {
    let value = ascii(bytes, order)?;
    let b = value.as_bytes();
    if b.len() != 6 || !matches!(b[0], b'+' | b'-') || b[3] != b':' {
        return None;
    }
    let hour = value[1..3].parse::<u32>().ok()?;
    let minute = value[4..6].parse::<u32>().ok()?;
    (minute <= 59 && hour * 60 + minute <= 840).then_some(value)
}
fn apply_entry(tiff: &mut Tiff<'_, '_>, entry: Entry, exif: bool, output: &mut EmbeddedMetadata) {
    let c = &mut output.capture;
    match (exif, entry.tag) {
        (false, 0x0100) | (true, 0xa002) => {
            let identifier = if exif {
                "PixelXDimension"
            } else {
                "ImageWidth"
            };
            if select_dimension_source(&mut c.image_width, identifier) {
                assign_scalar(&mut c.image_width, entry, tiff, &[3, 4], dimension);
            }
        }
        (false, 0x0101) | (true, 0xa003) => {
            let identifier = if exif {
                "PixelYDimension"
            } else {
                "ImageLength"
            };
            if select_dimension_source(&mut c.image_height, identifier) {
                assign_scalar(&mut c.image_height, entry, tiff, &[3, 4], dimension);
            }
        }
        (false, 0x0112) => assign_scalar(&mut c.orientation, entry, tiff, &[3], short),
        (false, 0x010f) => assign(&mut c.camera_make, entry, tiff, &[2], ascii),
        (false, 0x0110) => assign(&mut c.camera_model, entry, tiff, &[2], ascii),
        (true, 0x9003) => assign(&mut c.capture_time, entry, tiff, &[2], datetime),
        (true, 0x9290 | 0x9291) => {
            if entry.tag == 0x9290
                && c.capture_subseconds.exif_identifier == "SubSecTimeOriginal"
                && c.capture_subseconds.state != FieldState::Absent
            {
                return;
            }
            if entry.tag == 0x9291 && c.capture_subseconds.exif_identifier == "SubSecTime" {
                c.capture_subseconds.value = None;
                c.capture_subseconds.state = FieldState::Absent;
            }
            c.capture_subseconds.exif_identifier = if entry.tag == 0x9290 {
                "SubSecTime"
            } else {
                "SubSecTimeOriginal"
            };
            assign(&mut c.capture_subseconds, entry, tiff, &[2], subseconds);
        }
        (true, 0x9010 | 0x9011) => {
            if entry.tag == 0x9010
                && c.capture_offset.exif_identifier == "OffsetTimeOriginal"
                && c.capture_offset.state != FieldState::Absent
            {
                return;
            }
            if entry.tag == 0x9011 && c.capture_offset.exif_identifier == "OffsetTime" {
                c.capture_offset.value = None;
                c.capture_offset.state = FieldState::Absent;
            }
            c.capture_offset.exif_identifier = if entry.tag == 0x9010 {
                "OffsetTime"
            } else {
                "OffsetTimeOriginal"
            };
            assign(&mut c.capture_offset, entry, tiff, &[2], offset);
        }
        (true, 0xa434) => assign(&mut c.lens_model, entry, tiff, &[2], ascii),
        (true, 0x829a) => assign_scalar(&mut c.exposure_time, entry, tiff, &[5], rational),
        (true, 0x829d) => assign_scalar(&mut c.aperture, entry, tiff, &[5], rational),
        (true, 0x8827) => assign_scalar(&mut c.iso, entry, tiff, &[3, 4], number),
        (true, 0x920a) => assign_scalar(&mut c.focal_length, entry, tiff, &[5], rational),
        (false, 0x02bc) => {
            output.xmp_packet.state =
                if output.xmp_packet.state != PacketState::Absent || !matches!(entry.ty, 1 | 7) {
                    PacketState::Invalid
                } else {
                    match tiff.value(entry) {
                        Ok(value) => PacketState::Present(value),
                        Err(FieldState::ResourceLimit) => PacketState::ResourceLimit,
                        Err(_) => PacketState::Invalid,
                    }
                };
        }
        (false, 0x83bb) => {
            if output.iim.state != FieldState::Absent || !matches!(entry.ty, 1 | 7) {
                output.iim.mark(FieldState::Invalid);
            } else {
                match tiff.value(entry) {
                    Ok(value) => parse_iim(&value, &mut output.iim),
                    Err(state) => output.iim.mark(state),
                }
            }
        }
        _ => {}
    }
}

pub(super) fn parse_photoshop(bytes: &[u8], output: &mut IimSource) {
    let mut position = 0;
    while position < bytes.len() {
        if bytes.len() - position < 7 || &bytes[position..position + 4] != b"8BIM" {
            output.mark(FieldState::Invalid);
            return;
        }
        let id = u16::from_be_bytes([bytes[position + 4], bytes[position + 5]]);
        position += 6;
        let name_len = usize::from(bytes[position]);
        let name_size = (name_len + 2) & !1;
        if position
            .checked_add(name_size + 4)
            .is_none_or(|end| end > bytes.len())
        {
            output.mark(FieldState::Invalid);
            return;
        }
        position += name_size;
        let len = u32::from_be_bytes([
            bytes[position],
            bytes[position + 1],
            bytes[position + 2],
            bytes[position + 3],
        ]) as usize;
        position += 4;
        if len > MAX_VALUE {
            if id == 0x0404 {
                output.mark(FieldState::ResourceLimit);
            }
            return;
        }
        if position
            .checked_add(len + (len & 1))
            .is_none_or(|end| end > bytes.len())
        {
            output.mark(FieldState::Invalid);
            return;
        }
        if id == 0x0404 {
            if output.state != FieldState::Absent {
                output.mark(FieldState::Invalid);
                return;
            }
            parse_iim(&bytes[position..position + len], output);
        }
        position += len + (len & 1);
    }
}
fn parse_iim(bytes: &[u8], output: &mut IimSource) {
    output.state = FieldState::Present;
    let mut position = 0;
    let mut utf8 = false;
    while position < bytes.len() {
        if bytes.len() - position < 5 || bytes[position] != 0x1c {
            output.mark(FieldState::Invalid);
            return;
        }
        let record = bytes[position + 1];
        let dataset = bytes[position + 2];
        let size = u16::from_be_bytes([bytes[position + 3], bytes[position + 4]]);
        position += 5;
        let len = if size & 0x8000 != 0 {
            let width = usize::from(size & 0x7fff);
            if width == 0 || width > 4 {
                output.mark(FieldState::Invalid);
                return;
            }
            if position
                .checked_add(width)
                .is_none_or(|end| end > bytes.len())
            {
                output.mark(FieldState::Invalid);
                return;
            }
            let mut length = 0usize;
            for byte in &bytes[position..position + width] {
                length = (length << 8) | usize::from(*byte);
            }
            position += width;
            length
        } else {
            usize::from(size)
        };
        if len > MAX_VALUE {
            output.mark(FieldState::ResourceLimit);
            return;
        }
        if position
            .checked_add(len)
            .is_none_or(|end| end > bytes.len())
        {
            output.mark(FieldState::Invalid);
            return;
        }
        let value = &bytes[position..position + len];
        position += len;
        if record == 1 && dataset == 90 {
            if value == b"\x1b%G" {
                utf8 = true;
            }
            continue;
        }
        if record == 2 && value == b"\x1b%G" {
            utf8 = true;
            continue;
        }
        if record != 2 {
            continue;
        }
        let Some(field) = output
            .fields()
            .into_iter()
            .find(|field| field.dataset == dataset)
        else {
            continue;
        };
        let encoding = if utf8 || value.starts_with(b"\x1b%G") {
            IimEncoding::Utf8
        } else {
            IimEncoding::Latin1
        };
        let text_bytes = if value.starts_with(b"\x1b%G") {
            &value[3..]
        } else {
            value
        };
        let maximum_length = match dataset {
            120 => 2000,
            105 => 256,
            116 => 128,
            5 | 25 | 80 | 85 | 110 | 115 => 64,
            _ => MAX_VALUE,
        };
        if text_bytes.len() > maximum_length {
            field.state = FieldState::ResourceLimit;
            field.values.clear();
            continue;
        }
        let decoded = match encoding {
            IimEncoding::Utf8 => std::str::from_utf8(text_bytes).map(str::to_owned).ok(),
            IimEncoding::Latin1 => Some(text_bytes.iter().map(|byte| char::from(*byte)).collect()),
        };
        let Some(text) = decoded else {
            field.state = FieldState::Invalid;
            field.values.clear();
            continue;
        };
        if field.state == FieldState::Invalid || field.state == FieldState::ResourceLimit {
            continue;
        }
        if (!matches!(dataset, 25 | 80) && !field.values.is_empty()) || field.values.len() >= 1024 {
            field.state = FieldState::ResourceLimit;
            field.values.clear();
            continue;
        }
        field.encoding = encoding;
        field.values.push(text);
        field.state = FieldState::Present;
    }
    let states = output.fields().map(|field| field.state);
    if states.contains(&FieldState::ResourceLimit) {
        output.state = FieldState::ResourceLimit;
    } else if states.contains(&FieldState::Invalid) {
        output.state = FieldState::Invalid;
    }
}
