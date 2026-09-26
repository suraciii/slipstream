//! Bounded extraction of EXIF facts, XMP packets, and IPTC IIM from an Original.
use crate::{
    confinement::{ConfinementError, OpenedOriginal, OriginalCapability},
    domain::OriginalKind,
};
use std::fmt;

const MAX_VALUE: usize = 64 * 1024;
const MAX_ENTRIES: usize = 1024;
const MAX_MARKERS: usize = 4096;
const XMP_HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const PS_HEADER: &[u8] = b"Photoshop 3.0\0";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldState {
    Present,
    Absent,
    Invalid,
    Unavailable,
    ResourceLimit,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureField<T> {
    pub value: Option<T>,
    pub state: FieldState,
    pub exif_identifier: &'static str,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddedCaptureFacts {
    pub capture_time: CaptureField<String>,
    pub capture_subseconds: CaptureField<String>,
    pub capture_offset: CaptureField<String>,
    pub camera_make: CaptureField<String>,
    pub camera_model: CaptureField<String>,
    pub lens_model: CaptureField<String>,
    pub image_width: CaptureField<u32>,
    pub image_height: CaptureField<u32>,
    pub orientation: CaptureField<u16>,
    pub exposure_time: CaptureField<String>,
    pub aperture: CaptureField<String>,
    pub iso: CaptureField<u32>,
    pub focal_length: CaptureField<String>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PacketProvenance {
    JpegApp1,
    TiffIfd0Tag02bc,
}
impl PacketProvenance {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::JpegApp1 => "jpeg-app1",
            Self::TiffIfd0Tag02bc => "tiff-ifd0-tag-0x02bc",
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PacketState {
    Present(Vec<u8>),
    Absent,
    Invalid,
    ResourceLimit,
    Unavailable,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PacketSource {
    pub provenance: PacketProvenance,
    pub state: PacketState,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IimProvenance {
    JpegApp13Resource0404,
    TiffIfd0Tag83bb,
}
impl IimProvenance {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::JpegApp13Resource0404 => "jpeg-app13-resource-0x0404",
            Self::TiffIfd0Tag83bb => "tiff-ifd0-tag-0x83bb",
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IimEncoding {
    Utf8,
    Latin1,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IimField {
    pub dataset: u8,
    pub values: Vec<String>,
    pub encoding: IimEncoding,
    pub state: FieldState,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IimSource {
    pub provenance: IimProvenance,
    pub state: FieldState,
    pub title: IimField,
    pub description: IimField,
    pub headline: IimField,
    pub keywords: IimField,
    pub creators: IimField,
    pub creator_job_title: IimField,
    pub credit: IimField,
    pub source: IimField,
    pub copyright_notice: IimField,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddedMetadata {
    pub capture: EmbeddedCaptureFacts,
    pub xmp_packet: PacketSource,
    pub iim: IimSource,
}
#[derive(Debug)]
pub enum EmbeddedExtractionError {
    Confinement(ConfinementError),
    ResourceLimit,
    InvalidInput,
}
impl fmt::Display for EmbeddedExtractionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Confinement(e) => e.fmt(f),
            Self::ResourceLimit => f.write_str("embedded metadata exceeds resource limits"),
            Self::InvalidInput => f.write_str("embedded metadata is invalid"),
        }
    }
}
impl std::error::Error for EmbeddedExtractionError {}

fn field<T>(id: &'static str) -> CaptureField<T> {
    CaptureField {
        value: None,
        state: FieldState::Absent,
        exif_identifier: id,
    }
}
fn empty_capture() -> EmbeddedCaptureFacts {
    EmbeddedCaptureFacts {
        capture_time: field("DateTimeOriginal"),
        capture_subseconds: field("SubSecTimeOriginal"),
        capture_offset: field("OffsetTimeOriginal"),
        camera_make: field("Make"),
        camera_model: field("Model"),
        lens_model: field("LensModel"),
        image_width: field("ImageWidth"),
        image_height: field("ImageLength"),
        orientation: field("Orientation"),
        exposure_time: field("ExposureTime"),
        aperture: field("FNumber"),
        iso: field("ISOSpeedRatings"),
        focal_length: field("FocalLength"),
    }
}
fn iim_field(dataset: u8) -> IimField {
    IimField {
        dataset,
        values: Vec::new(),
        encoding: IimEncoding::Latin1,
        state: FieldState::Absent,
    }
}
fn empty_iim(provenance: IimProvenance) -> IimSource {
    IimSource {
        provenance,
        state: FieldState::Absent,
        title: iim_field(5),
        description: iim_field(120),
        headline: iim_field(105),
        keywords: iim_field(25),
        creators: iim_field(80),
        creator_job_title: iim_field(85),
        credit: iim_field(110),
        source: iim_field(115),
        copyright_notice: iim_field(116),
    }
}
impl IimSource {
    fn fields(&mut self) -> [&mut IimField; 9] {
        [
            &mut self.title,
            &mut self.description,
            &mut self.headline,
            &mut self.keywords,
            &mut self.creators,
            &mut self.creator_job_title,
            &mut self.credit,
            &mut self.source,
            &mut self.copyright_notice,
        ]
    }
    fn mark(&mut self, state: FieldState) {
        self.state = state;
        for field in self.fields() {
            field.state = state;
            field.values.clear();
        }
    }
}
impl EmbeddedCaptureFacts {
    fn mark(&mut self, state: FieldState) {
        self.capture_time.value = None;
        self.capture_subseconds.value = None;
        self.capture_offset.value = None;
        self.camera_make.value = None;
        self.camera_model.value = None;
        self.lens_model.value = None;
        self.image_width.value = None;
        self.image_height.value = None;
        self.orientation.value = None;
        self.exposure_time.value = None;
        self.aperture.value = None;
        self.iso.value = None;
        self.focal_length.value = None;
        self.capture_time.state = state;
        self.capture_subseconds.state = state;
        self.capture_offset.state = state;
        self.camera_make.state = state;
        self.camera_model.state = state;
        self.lens_model.state = state;
        self.image_width.state = state;
        self.image_height.state = state;
        self.orientation.state = state;
        self.exposure_time.state = state;
        self.aperture.state = state;
        self.iso.state = state;
        self.focal_length.state = state;
    }
    fn mark_exif(&mut self, state: FieldState) {
        for field in [
            &mut self.capture_time,
            &mut self.capture_subseconds,
            &mut self.capture_offset,
            &mut self.lens_model,
            &mut self.exposure_time,
            &mut self.aperture,
            &mut self.focal_length,
        ] {
            field.value = None;
            field.state = state;
        }
        self.iso.value = None;
        self.iso.state = state;
    }
}
fn empty(kind: OriginalKind) -> EmbeddedMetadata {
    let jpeg = kind == OriginalKind::Jpeg;
    EmbeddedMetadata {
        capture: empty_capture(),
        xmp_packet: PacketSource {
            provenance: if jpeg {
                PacketProvenance::JpegApp1
            } else {
                PacketProvenance::TiffIfd0Tag02bc
            },
            state: PacketState::Absent,
        },
        iim: empty_iim(if jpeg {
            IimProvenance::JpegApp13Resource0404
        } else {
            IimProvenance::TiffIfd0Tag83bb
        }),
    }
}

struct Reader<'a> {
    opened: &'a OpenedOriginal,
    size: u64,
    remaining: u64,
    failure: Option<ConfinementError>,
}
impl Reader<'_> {
    fn read(&mut self, offset: u64, len: usize) -> Result<Vec<u8>, EmbeddedExtractionError> {
        if len > MAX_VALUE {
            return Err(EmbeddedExtractionError::ResourceLimit);
        }
        let end = offset
            .checked_add(len as u64)
            .ok_or(EmbeddedExtractionError::InvalidInput)?;
        if end > self.size {
            return Err(EmbeddedExtractionError::InvalidInput);
        }
        self.remaining = self
            .remaining
            .checked_sub(len as u64)
            .ok_or(EmbeddedExtractionError::ResourceLimit)?;
        let bytes = match self.opened.pread_range(offset, len) {
            Ok(bytes) => bytes,
            Err(ConfinementError::ResourceLimit(_)) => {
                return Err(EmbeddedExtractionError::ResourceLimit);
            }
            Err(error) => {
                self.failure = Some(error.clone());
                return Err(EmbeddedExtractionError::Confinement(error));
            }
        };
        if bytes.len() != len {
            return Err(EmbeddedExtractionError::InvalidInput);
        }
        Ok(bytes)
    }
}

/// Reads only bounded ranges through one retained, revision-checked Original descriptor.
pub fn extract(
    capability: &OriginalCapability,
    kind: OriginalKind,
    maximum_bytes: u64,
) -> Result<EmbeddedMetadata, EmbeddedExtractionError> {
    let opened = capability
        .open_revision_checked()
        .map_err(EmbeddedExtractionError::Confinement)?;
    let size = opened
        .size()
        .map_err(EmbeddedExtractionError::Confinement)?;
    let mut reader = Reader {
        opened: &opened,
        size,
        remaining: maximum_bytes.min(16 * 1024 * 1024),
        failure: None,
    };
    let result = if kind == OriginalKind::Jpeg {
        extract_jpeg(&mut reader)
    } else {
        extract_raw(&mut reader)
    };
    if let Some(error) = reader.failure.take() {
        opened
            .verify_unchanged()
            .map_err(EmbeddedExtractionError::Confinement)?;
        return Err(EmbeddedExtractionError::Confinement(error));
    }
    // A changed file must not yield facts from a revision that no longer exists.
    opened
        .verify_unchanged()
        .map_err(EmbeddedExtractionError::Confinement)?;
    result
}

fn extract_raw(reader: &mut Reader<'_>) -> Result<EmbeddedMetadata, EmbeddedExtractionError> {
    let mut output = empty(OriginalKind::Raw);
    if reader.size < 4 || !matches!(reader.read(0, 4)?.as_slice(), b"II*\0" | b"MM\0*") {
        output.capture.mark(FieldState::Unavailable);
        match crate::native::inspect_raw_capture_time(reader.opened) {
            Ok(Some(time)) => {
                output.capture.capture_time.value = Some(format!(
                    "{:04}:{:02}:{:02} {:02}:{:02}:{:02}",
                    time.year, time.month, time.day, time.hour, time.minute, time.second
                ));
                output.capture.capture_time.state = FieldState::Present;
            }
            Ok(None)
            | Err(
                crate::NativePreviewError::Unsupported | crate::NativePreviewError::NoUsablePreview,
            ) => {}
            Err(crate::NativePreviewError::Malformed) => {
                output.capture.capture_time.state = FieldState::Invalid
            }
            Err(crate::NativePreviewError::ResourceLimit) => {
                output.capture.capture_time.state = FieldState::ResourceLimit
            }
            Err(crate::NativePreviewError::Io | crate::NativePreviewError::Internal) => {
                output.capture.capture_time.state = FieldState::Unavailable;
            }
        }
        output.xmp_packet.state = PacketState::Unavailable;
        output.iim.mark(FieldState::Unavailable);
        return Ok(output);
    }
    if let Err(error) = parse_tiff(reader, 0, reader.size, &mut output) {
        match error {
            EmbeddedExtractionError::ResourceLimit => {
                output.capture.mark(FieldState::ResourceLimit);
                output.xmp_packet.state = PacketState::ResourceLimit;
                output.iim.mark(FieldState::ResourceLimit);
            }
            EmbeddedExtractionError::InvalidInput => {
                output.capture.mark(FieldState::Invalid);
                output.xmp_packet.state = PacketState::Invalid;
                output.iim.mark(FieldState::Invalid);
            }
            other => return Err(other),
        }
    }
    Ok(output)
}
fn extract_jpeg(reader: &mut Reader<'_>) -> Result<EmbeddedMetadata, EmbeddedExtractionError> {
    let mut output = empty(OriginalKind::Jpeg);
    if reader.size < 2 || reader.read(0, 2)? != [0xff, 0xd8] {
        output.capture.mark(FieldState::Invalid);
        return Ok(output);
    }
    let mut position = 2_u64;
    let mut marker_limit = true;
    for _ in 0..MAX_MARKERS {
        if position >= reader.size {
            marker_limit = false;
            break;
        }
        if reader.read(position, 1)?[0] != 0xff {
            marker_limit = false;
            output.capture.mark(FieldState::Invalid);
            break;
        }
        position += 1;
        let marker = loop {
            if position >= reader.size {
                return Ok(output);
            }
            let value = reader.read(position, 1)?[0];
            position += 1;
            if value != 0xff {
                break value;
            }
        };
        if matches!(marker, 0xd9 | 0xda) {
            return Ok(output);
        }
        if matches!(marker, 0xd8 | 0x01 | 0xd0..=0xd7) {
            continue;
        }
        if position + 2 > reader.size {
            marker_limit = false;
            output.capture.mark(FieldState::Invalid);
            break;
        }
        let length = reader.read(position, 2)?;
        let length = usize::from(u16::from_be_bytes([length[0], length[1]]));
        position += 2;
        if length < 2 || position + (length - 2) as u64 > reader.size {
            marker_limit = false;
            output.capture.mark(FieldState::Invalid);
            break;
        }
        let end = position + (length - 2) as u64;
        match marker {
            0xe1 => {
                let prefix = reader.read(position, (length - 2).min(XMP_HEADER.len()))?;
                if prefix.starts_with(b"Exif\0\0")
                    && output.capture.capture_time.state == FieldState::Absent
                {
                    if let Err(error) = parse_tiff(reader, position + 6, end, &mut output) {
                        match error {
                            EmbeddedExtractionError::ResourceLimit => {
                                output.capture.mark(FieldState::ResourceLimit)
                            }
                            EmbeddedExtractionError::InvalidInput => {
                                output.capture.mark(FieldState::Invalid)
                            }
                            other => return Err(other),
                        }
                    }
                } else if prefix == XMP_HEADER && output.xmp_packet.state == PacketState::Absent {
                    let packet_len = length - 2 - XMP_HEADER.len();
                    output.xmp_packet.state = if packet_len > MAX_VALUE {
                        PacketState::ResourceLimit
                    } else {
                        PacketState::Present(
                            reader.read(position + XMP_HEADER.len() as u64, packet_len)?,
                        )
                    };
                } else if prefix.starts_with(b"Exif") && !prefix.starts_with(b"Exif\0\0") {
                    output.capture.mark(FieldState::Invalid);
                }
            }
            0xed => {
                let prefix = reader.read(position, (length - 2).min(PS_HEADER.len()))?;
                if prefix == PS_HEADER && output.iim.state == FieldState::Absent {
                    let len = length - 2 - PS_HEADER.len();
                    if len > MAX_VALUE {
                        output.iim.mark(FieldState::ResourceLimit);
                    } else {
                        parse_photoshop(
                            &reader.read(position + PS_HEADER.len() as u64, len)?,
                            &mut output.iim,
                        );
                    }
                }
            }
            0xc0 | 0xc2 | 0xc3 if length >= 7 => {
                let dimensions = reader.read(position, 5)?;
                if output.capture.image_height.state == FieldState::Absent {
                    output.capture.image_height.value = Some(u32::from(u16::from_be_bytes([
                        dimensions[1],
                        dimensions[2],
                    ])));
                    output.capture.image_height.state = FieldState::Present;
                }
                if output.capture.image_width.state == FieldState::Absent {
                    output.capture.image_width.value = Some(u32::from(u16::from_be_bytes([
                        dimensions[3],
                        dimensions[4],
                    ])));
                    output.capture.image_width.state = FieldState::Present;
                }
            }
            _ => {}
        }
        position = end;
    }
    if marker_limit {
        for field in [
            &mut output.capture.capture_time,
            &mut output.capture.capture_subseconds,
            &mut output.capture.capture_offset,
            &mut output.capture.camera_make,
            &mut output.capture.camera_model,
            &mut output.capture.lens_model,
            &mut output.capture.exposure_time,
            &mut output.capture.aperture,
            &mut output.capture.focal_length,
        ] {
            if field.state == FieldState::Absent {
                field.state = FieldState::ResourceLimit;
            }
        }
        for state in [
            &mut output.capture.image_width.state,
            &mut output.capture.image_height.state,
            &mut output.capture.orientation.state,
            &mut output.capture.iso.state,
        ] {
            if *state == FieldState::Absent {
                *state = FieldState::ResourceLimit;
            }
        }
        if output.xmp_packet.state == PacketState::Absent {
            output.xmp_packet.state = PacketState::ResourceLimit;
        }
        if output.iim.state == FieldState::Absent {
            output.iim.mark(FieldState::ResourceLimit);
        }
    }
    Ok(output)
}

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
    fn value(&mut self, entry: Entry) -> Result<Vec<u8>, FieldState> {
        let unit: u64 = match entry.ty {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 => 4,
            5 | 10 | 12 => 8,
            _ => return Err(FieldState::Invalid),
        };
        let len = u64::from(entry.count) * unit;
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
fn parse_tiff(
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
        (false, 0x0100) => assign(&mut c.image_width, entry, tiff, &[3, 4], number),
        (false, 0x0101) => assign(&mut c.image_height, entry, tiff, &[3, 4], number),
        (false, 0x0112) => assign(&mut c.orientation, entry, tiff, &[3], short),
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
        (true, 0x829a) => assign(&mut c.exposure_time, entry, tiff, &[5], rational),
        (true, 0x829d) => assign(&mut c.aperture, entry, tiff, &[5], rational),
        (true, 0x8827) => assign(&mut c.iso, entry, tiff, &[3, 4], number),
        (true, 0x920a) => assign(&mut c.focal_length, entry, tiff, &[5], rational),
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

fn parse_photoshop(bytes: &[u8], output: &mut IimSource) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LibraryRoot, RelativeOriginalPath};
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Tree(PathBuf);
    impl Tree {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "slipstream-embedded-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn inspect(
        kind: OriginalKind,
        bytes: &[u8],
        limit: u64,
    ) -> Result<EmbeddedMetadata, EmbeddedExtractionError> {
        let tree = Tree::new();
        let name = if kind == OriginalKind::Jpeg {
            "fixture.JPG"
        } else {
            "fixture.ARW"
        };
        fs::write(tree.0.join(name), bytes).unwrap();
        let root = LibraryRoot::open(&tree.0).unwrap();
        let capability = root
            .original(RelativeOriginalPath::parse(name).unwrap())
            .unwrap();
        extract(&capability, kind, limit)
    }
    fn segment(marker: u8, data: &[u8]) -> Vec<u8> {
        let mut result = vec![0xff, marker];
        result.extend_from_slice(&u16::try_from(data.len() + 2).unwrap().to_be_bytes());
        result.extend_from_slice(data);
        result
    }
    fn jpeg(exif: &[u8], xmp: &[u8], iim: &[u8]) -> Vec<u8> {
        let mut result = vec![0xff, 0xd8];
        if !exif.is_empty() {
            result.extend(segment(0xe1, &[b"Exif\0\0".as_slice(), exif].concat()));
        }
        if !xmp.is_empty() {
            result.extend(segment(0xe1, &[XMP_HEADER, xmp].concat()));
        }
        if !iim.is_empty() {
            let mut resource = b"8BIM\x04\x04\0\0".to_vec();
            resource.extend_from_slice(&(iim.len() as u32).to_be_bytes());
            resource.extend_from_slice(iim);
            if iim.len() % 2 != 0 {
                resource.push(0);
            }
            result.extend(segment(0xed, &[PS_HEADER, &resource].concat()));
        }
        result.extend_from_slice(&[0xff, 0xd9]);
        result
    }
    fn dataset(record: u8, number: u8, value: &[u8]) -> Vec<u8> {
        let mut result = vec![0x1c, record, number];
        result.extend_from_slice(&(value.len() as u16).to_be_bytes());
        result.extend_from_slice(value);
        result
    }
    fn directory(entries: &[(u16, u16, Vec<u8>)], offset: usize) -> (Vec<u8>, Vec<u8>) {
        let mut dir = (entries.len() as u16).to_le_bytes().to_vec();
        let mut data = Vec::new();
        for (tag, ty, value) in entries {
            dir.extend_from_slice(&tag.to_le_bytes());
            dir.extend_from_slice(&ty.to_le_bytes());
            let unit = match ty {
                3 => 2,
                4 => 4,
                5 => 8,
                _ => 1,
            };
            dir.extend_from_slice(&((value.len() / unit) as u32).to_le_bytes());
            if value.len() <= 4 {
                let mut inline = [0; 4];
                inline[..value.len()].copy_from_slice(value);
                dir.extend_from_slice(&inline);
            } else {
                dir.extend_from_slice(
                    &((offset + 2 + entries.len() * 12 + 4 + data.len()) as u32).to_le_bytes(),
                );
                data.extend_from_slice(value);
            }
        }
        dir.extend_from_slice(&0u32.to_le_bytes());
        (dir, data)
    }
    fn tiff(primary: &[(u16, u16, Vec<u8>)], exif: &[(u16, u16, Vec<u8>)]) -> Vec<u8> {
        let mut primary = primary.to_vec();
        if !exif.is_empty() {
            primary.push((0x8769, 4, vec![0; 4]));
        }
        let (mut ifd, data) = directory(&primary, 8);
        let next = 8 + ifd.len() + data.len();
        if !exif.is_empty() {
            let pos = ifd.len() - 8;
            ifd[pos..pos + 4].copy_from_slice(&(next as u32).to_le_bytes());
        }
        let mut result = b"II*\0\x08\0\0\0".to_vec();
        result.extend(ifd);
        result.extend(data);
        if !exif.is_empty() {
            let (dir, data) = directory(exif, next);
            result.extend(dir);
            result.extend(data);
        }
        result
    }
    fn fraction(n: u32, d: u32) -> Vec<u8> {
        [n.to_le_bytes(), d.to_le_bytes()].concat()
    }
    #[test]
    fn tiff_exposes_every_capture_fact_and_original_sources() {
        let iim = dataset(2, 5, b"Landscape");
        let bytes = tiff(
            &[
                (0x0100, 4, 640u32.to_le_bytes().to_vec()),
                (0x0101, 3, 480u16.to_le_bytes().to_vec()),
                (0x0112, 3, 6u16.to_le_bytes().to_vec()),
                (0x010f, 2, b"Sony\0".to_vec()),
                (0x0110, 2, b"Alpha\0".to_vec()),
                (0x02bc, 7, b"<xmp/>".to_vec()),
                (0x83bb, 7, iim),
            ],
            &[
                (0x9003, 2, b"2026:02:03 04:05:06\0".to_vec()),
                (0x9291, 2, b"123\0".to_vec()),
                (0x9010, 2, b"+02:30\0".to_vec()),
                (0xa434, 2, b"24-70\0".to_vec()),
                (0x829a, 5, fraction(1, 125)),
                (0x829d, 5, fraction(28, 10)),
                (0x8827, 3, 400u16.to_le_bytes().to_vec()),
                (0x920a, 5, fraction(50, 1)),
            ],
        );
        let result = inspect(OriginalKind::Raw, &bytes, 16 * 1024 * 1024).unwrap();
        let jpeg_result = inspect(
            OriginalKind::Jpeg,
            &jpeg(&bytes, b"jpeg packet", &dataset(2, 5, b"JPEG title")),
            16 * 1024 * 1024,
        )
        .unwrap();
        assert_eq!(jpeg_result.capture, result.capture);
        assert_eq!(
            jpeg_result.xmp_packet.state,
            PacketState::Present(b"jpeg packet".to_vec())
        );
        assert_eq!(jpeg_result.iim.title.values, ["JPEG title"]);
        let c = result.capture;
        assert_eq!(c.capture_time.value.as_deref(), Some("2026:02:03 04:05:06"));
        assert_eq!(c.capture_subseconds.value.as_deref(), Some("123"));
        assert_eq!(c.capture_offset.value.as_deref(), Some("+02:30"));
        assert_eq!(c.camera_make.value.as_deref(), Some("Sony"));
        assert_eq!(c.camera_model.value.as_deref(), Some("Alpha"));
        assert_eq!(c.lens_model.value.as_deref(), Some("24-70"));
        assert_eq!(c.image_width.value, Some(640));
        assert_eq!(c.image_height.value, Some(480));
        assert_eq!(c.orientation.value, Some(6));
        assert_eq!(c.exposure_time.value.as_deref(), Some("1/125"));
        assert_eq!(c.aperture.value.as_deref(), Some("28/10"));
        assert_eq!(c.iso.value, Some(400));
        assert_eq!(c.focal_length.value.as_deref(), Some("50/1"));
        assert_eq!(
            result.xmp_packet.state,
            PacketState::Present(b"<xmp/>".to_vec())
        );
        assert_eq!(result.iim.title.values, ["Landscape"]);
    }
    #[test]
    fn jpeg_reads_all_three_segments_and_sof_dimensions() {
        let mut iim = dataset(1, 90, b"\x1b%G");
        iim.extend(dataset(2, 25, "été".as_bytes()));
        iim.extend(dataset(2, 25, b"snow"));
        iim.extend(dataset(2, 80, b"Alice"));
        iim.extend(dataset(2, 80, b"Bob"));
        for (id, value) in [
            (5, "Title"),
            (120, "Description"),
            (105, "Headline"),
            (85, "Artist"),
            (110, "Credit"),
            (115, "Source"),
            (116, "Rights"),
        ] {
            iim.extend(dataset(2, id, value.as_bytes()));
        }
        let mut bytes = jpeg(
            &tiff(&[], &[(0x9003, 2, b"2026:02:03 04:05:06\0".to_vec())]),
            b"packet",
            &iim,
        );
        let end = bytes.len() - 2;
        bytes.splice(end..end, segment(0xc2, &[8, 0, 100, 1, 44]));
        let result = inspect(OriginalKind::Jpeg, &bytes, 16 * 1024 * 1024).unwrap();
        assert_eq!(result.capture.image_width.value, Some(300));
        assert_eq!(result.capture.image_height.value, Some(100));
        assert_eq!(
            result.xmp_packet.state,
            PacketState::Present(b"packet".to_vec())
        );
        assert_eq!(result.iim.keywords.values, ["été", "snow"]);
        assert_eq!(result.iim.creators.values, ["Alice", "Bob"]);
        for field in [
            result.iim.title,
            result.iim.description,
            result.iim.headline,
            result.iim.creator_job_title,
            result.iim.credit,
            result.iim.source,
            result.iim.copyright_notice,
        ] {
            assert_eq!(field.state, FieldState::Present);
            assert_eq!(field.encoding, IimEncoding::Utf8);
        }
    }
    #[test]
    fn latin1_iim_and_invalid_encoding_are_not_lossily_decoded() {
        let result = inspect(
            OriginalKind::Jpeg,
            &jpeg(&[], &[], &dataset(2, 5, b"Caf\xe9")),
            16 * 1024 * 1024,
        )
        .unwrap();
        assert_eq!(result.iim.title.values, ["Café"]);
        assert_eq!(result.iim.title.encoding, IimEncoding::Latin1);
        let mut iim = dataset(1, 90, b"\x1b%G");
        iim.extend(dataset(2, 5, b"\xff"));
        let result = inspect(OriginalKind::Jpeg, &jpeg(&[], &[], &iim), 16 * 1024 * 1024).unwrap();
        assert_eq!(result.iim.title.state, FieldState::Invalid);
    }
    #[test]
    fn absent_wrong_type_and_non_tiff_are_distinct() {
        let missing = inspect(OriginalKind::Raw, &tiff(&[], &[]), 16 * 1024 * 1024).unwrap();
        assert_eq!(missing.capture.camera_make.state, FieldState::Absent);
        assert_eq!(missing.xmp_packet.state, PacketState::Absent);
        let wrong = inspect(
            OriginalKind::Raw,
            &tiff(
                &[
                    (0x010f, 3, vec![1, 0]),
                    (0x02bc, 3, vec![1, 0]),
                    (0x83bb, 3, vec![1, 0]),
                ],
                &[(0x8827, 2, b"400\0".to_vec())],
            ),
            16 * 1024 * 1024,
        )
        .unwrap();
        assert_eq!(wrong.capture.camera_make.state, FieldState::Invalid);
        assert_eq!(wrong.capture.iso.state, FieldState::Invalid);
        assert_eq!(wrong.xmp_packet.state, PacketState::Invalid);
        assert_eq!(wrong.iim.state, FieldState::Invalid);
        let unavailable = inspect(OriginalKind::Raw, b"non-TIFF RAW", 16 * 1024 * 1024).unwrap();
        assert_eq!(
            unavailable.capture.image_width.state,
            FieldState::Unavailable
        );
        assert_eq!(unavailable.xmp_packet.state, PacketState::Unavailable);
        assert_eq!(unavailable.iim.title.state, FieldState::Unavailable);
    }
    #[test]
    fn bounds_report_limits_without_truncation() {
        let bytes = tiff(
            &[
                (0x02bc, 7, vec![b'x'; MAX_VALUE + 1]),
                (0x010f, 2, vec![b'N'; MAX_VALUE + 1]),
            ],
            &[],
        );
        let result = inspect(OriginalKind::Raw, &bytes, 16 * 1024 * 1024).unwrap();
        assert_eq!(result.xmp_packet.state, PacketState::ResourceLimit);
        assert_eq!(result.capture.camera_make.state, FieldState::ResourceLimit);
        let mut too_many = Vec::new();
        for _ in 0..1025 {
            too_many.extend(dataset(2, 25, b"x"));
        }
        let result = inspect(
            OriginalKind::Raw,
            &tiff(&[(0x83bb, 7, too_many)], &[]),
            16 * 1024 * 1024,
        )
        .unwrap();
        assert_eq!(result.iim.keywords.state, FieldState::ResourceLimit);
        assert!(matches!(
            inspect(OriginalKind::Jpeg, &jpeg(&[], b"packet", &[]), 4),
            Err(EmbeddedExtractionError::ResourceLimit)
        ));
    }
    #[test]
    fn iim_length_and_singleton_multiplicity_limits_are_visible() {
        let mut iim = dataset(2, 5, &vec![b'x'; 65]);
        iim.extend(dataset(2, 105, b"first"));
        iim.extend(dataset(2, 105, b"second"));
        let result = inspect(OriginalKind::Jpeg, &jpeg(&[], &[], &iim), 16 * 1024 * 1024).unwrap();
        assert_eq!(result.iim.title.state, FieldState::ResourceLimit);
        assert_eq!(result.iim.headline.state, FieldState::ResourceLimit);
        assert!(result.iim.title.values.is_empty());
        assert!(result.iim.headline.values.is_empty());
    }
    #[test]
    fn directory_and_marker_limits_are_visible() {
        let entries = vec![(0x1111, 3, vec![0, 0]); MAX_ENTRIES + 1];
        let result = inspect(OriginalKind::Raw, &tiff(&entries, &[]), 16 * 1024 * 1024).unwrap();
        assert_eq!(result.capture.capture_time.state, FieldState::ResourceLimit);
        assert_eq!(result.xmp_packet.state, PacketState::ResourceLimit);
        let mut bytes = vec![0xff, 0xd8];
        for _ in 0..MAX_MARKERS + 1 {
            bytes.extend_from_slice(&[0xff, 1]);
        }
        let result = inspect(OriginalKind::Jpeg, &bytes, 16 * 1024 * 1024).unwrap();
        assert_eq!(result.iim.state, FieldState::ResourceLimit);
        assert_eq!(result.capture.image_width.state, FieldState::ResourceLimit);
    }
    #[test]
    fn large_original_reads_only_metadata_and_preserves_original_bytes() {
        use std::io::Write;
        let tree = Tree::new();
        let path = tree.0.join("large.JPG");
        let bytes = jpeg(&[], b"packet", &[]);
        let mut file = fs::File::create(&path).unwrap();
        file.write_all(&bytes).unwrap();
        file.set_len(64 * 1024 * 1024).unwrap();
        drop(file);
        let root = LibraryRoot::open(&tree.0).unwrap();
        let capability = root
            .original(RelativeOriginalPath::parse("large.JPG").unwrap())
            .unwrap();
        let before = capability.facts().unwrap();
        let result = extract(&capability, OriginalKind::Jpeg, 16 * 1024 * 1024).unwrap();
        assert_eq!(
            result.xmp_packet.state,
            PacketState::Present(b"packet".to_vec())
        );
        assert_eq!(capability.facts().unwrap(), before);
        assert_eq!(capability.read_range(0, bytes.len()).unwrap(), bytes);
    }
    #[test]
    fn exif_validation_rejects_zero_denominator_and_impossible_dates() {
        let result = inspect(
            OriginalKind::Raw,
            &tiff(
                &[],
                &[
                    (0x9003, 2, b"2026:02:30 04:05:06\0".to_vec()),
                    (0x829a, 5, fraction(1, 0)),
                    (0x9011, 2, b"+14:01\0".to_vec()),
                ],
            ),
            16 * 1024 * 1024,
        )
        .unwrap();
        assert_eq!(result.capture.capture_time.state, FieldState::Invalid);
        assert_eq!(result.capture.exposure_time.state, FieldState::Invalid);
        assert_eq!(result.capture.capture_offset.state, FieldState::Invalid);
        assert_eq!(result.capture.capture_subseconds.state, FieldState::Absent);
    }
    #[test]
    fn iim_extended_lengths_and_truncation_are_checked() {
        let mut iim = vec![0x1c, 2, 5, 0x80, 1, 5];
        iim.extend_from_slice(b"Title");
        let result = inspect(OriginalKind::Jpeg, &jpeg(&[], &[], &iim), 16 * 1024 * 1024).unwrap();
        assert_eq!(result.iim.title.values, ["Title"]);
        let result = inspect(
            OriginalKind::Jpeg,
            &jpeg(&[], &[], &[0x1c, 2, 5, 0, 5, b'x']),
            16 * 1024 * 1024,
        )
        .unwrap();
        assert_eq!(result.iim.state, FieldState::Invalid);
    }
    #[test]
    fn big_endian_primary_fields_and_out_of_range_offsets_are_checked() {
        let mut bytes = b"MM\0*\0\0\0\x08".to_vec();
        bytes.extend_from_slice(&2u16.to_be_bytes());
        for (tag, ty, value) in [
            (0x0100u16, 4u16, 600u32.to_be_bytes()),
            (0x0112, 3, [0, 6, 0, 0]),
        ] {
            bytes.extend_from_slice(&tag.to_be_bytes());
            bytes.extend_from_slice(&ty.to_be_bytes());
            bytes.extend_from_slice(&1u32.to_be_bytes());
            bytes.extend_from_slice(&value);
        }
        bytes.extend_from_slice(&0u32.to_be_bytes());
        let result = inspect(OriginalKind::Raw, &bytes, 16 * 1024 * 1024).unwrap();
        assert_eq!(result.capture.image_width.value, Some(600));
        assert_eq!(result.capture.orientation.value, Some(6));
        let mut bytes = tiff(&[(0x02bc, 7, b"packet".to_vec())], &[]);
        bytes[18..22].copy_from_slice(&u32::MAX.to_le_bytes());
        let result = inspect(OriginalKind::Raw, &bytes, 16 * 1024 * 1024).unwrap();
        assert_eq!(result.xmp_packet.state, PacketState::Invalid);
    }
    #[test]
    fn original_precision_tags_win_without_accepting_subifd_camera_identity() {
        for tags in [
            vec![
                (0x9290, 2, b"1\0".to_vec()),
                (0x9291, 2, b"123\0".to_vec()),
                (0x9010, 2, b"+01:00\0".to_vec()),
                (0x9011, 2, b"+02:00\0".to_vec()),
            ],
            vec![
                (0x9291, 2, b"123\0".to_vec()),
                (0x9290, 2, b"1\0".to_vec()),
                (0x9011, 2, b"+02:00\0".to_vec()),
                (0x9010, 2, b"+01:00\0".to_vec()),
            ],
        ] {
            let result = inspect(OriginalKind::Raw, &tiff(&[], &tags), 16 * 1024 * 1024).unwrap();
            assert_eq!(
                result.capture.capture_subseconds.value.as_deref(),
                Some("123")
            );
            assert_eq!(
                result.capture.capture_subseconds.exif_identifier,
                "SubSecTimeOriginal"
            );
            assert_eq!(
                result.capture.capture_offset.value.as_deref(),
                Some("+02:00")
            );
            assert_eq!(
                result.capture.capture_offset.exif_identifier,
                "OffsetTimeOriginal"
            );
        }
        let result = inspect(
            OriginalKind::Raw,
            &tiff(
                &[],
                &[
                    (0x010f, 2, b"Other\0".to_vec()),
                    (0x0100, 4, 100u32.to_le_bytes().to_vec()),
                ],
            ),
            16 * 1024 * 1024,
        )
        .unwrap();
        assert_eq!(result.capture.camera_make.state, FieldState::Absent);
        assert_eq!(result.capture.image_width.state, FieldState::Absent);
    }
    #[test]
    fn orientation_outside_exif_transform_range_is_invalid() {
        let result = inspect(
            OriginalKind::Raw,
            &tiff(&[(0x0112, 3, 9u16.to_le_bytes().to_vec())], &[]),
            16 * 1024 * 1024,
        )
        .unwrap();
        assert_eq!(result.capture.orientation.state, FieldState::Invalid);
        assert_eq!(result.capture.orientation.value, None);
    }
    #[test]
    fn empty_tiff_reports_all_supported_capture_fields_absent() {
        let result = inspect(OriginalKind::Raw, &tiff(&[], &[]), 16 * 1024 * 1024).unwrap();
        let c = result.capture;
        for state in [
            c.capture_time.state,
            c.capture_subseconds.state,
            c.capture_offset.state,
            c.camera_make.state,
            c.camera_model.state,
            c.lens_model.state,
            c.image_width.state,
            c.image_height.state,
            c.orientation.state,
            c.exposure_time.state,
            c.aperture.state,
            c.iso.state,
            c.focal_length.state,
        ] {
            assert_eq!(state, FieldState::Absent);
        }
        assert_eq!(c.capture_offset.value, None);
    }
}
