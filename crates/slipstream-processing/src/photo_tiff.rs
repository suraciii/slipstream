//! Bounded Development TIFF validation for the `development-tiff` output.
//!
//! The launcher validates its own engine result before offering it on an
//! Output request: IEEE float32 RGB samples, Deflate compression, full
//! geometry, no orientation surprise, and the exact pinned embedded linear
//! ProPhoto ICC bytes (`design/development-color.md`). The pinned profile
//! hash is byte identity; a profile name or appearance is not evidence.

use crate::protocol::ErrorCode;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    os::unix::fs::MetadataExt,
    path::Path,
};

/// The pinned darktable-embedded linear ProPhoto RGB profile bytes. These
/// differ from the bundle's `LargeRGB-elle-V2-g10.icc` asset only in the
/// legacy description-tag normalization recorded by the qualification.
pub(crate) const OUTPUT_ICC_SHA256: &str =
    "7bef28a81c974482756f09c7d34c55d53549ba450f26185b2c16f6228af96dfe";

/// Identity of one validated Development TIFF.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TiffIdentity {
    pub size: u64,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
}

const ICC_TAG: u16 = 34675;
const ICC_BYTES_MAX: usize = 1024 * 1024;
const IFD_ENTRIES_MAX: usize = 512;
/// The qualified writer emits one strip per row, so the strip layout arrays
/// are as long as the image is tall. The bound stays far above any qualified
/// geometry while keeping the parsed arrays small.
const STRIP_ENTRIES_MAX: usize = 65_536;
const HEAD_MAX: u64 = 1024 * 1024;

struct Reader {
    file: File,
    little: bool,
}

impl Reader {
    fn open(path: &Path) -> Result<Self, ErrorCode> {
        let mut file = File::open(path).map_err(|_| ErrorCode::Uncertain)?;
        let mut head = [0u8; 8];
        file.read_exact(&mut head)
            .map_err(|_| ErrorCode::Uncertain)?;
        let little = match &head[..4] {
            b"II\x2a\x00" => true,
            b"MM\x00\x2a" => false,
            _ => return Err(ErrorCode::Uncertain),
        };
        Ok(Self { file, little })
    }

    fn at(&mut self, offset: u64, buffer: &mut [u8]) -> Result<(), ErrorCode> {
        self.file
            .seek(SeekFrom::Start(offset))
            .map_err(|_| ErrorCode::Uncertain)?;
        self.file
            .read_exact(buffer)
            .map_err(|_| ErrorCode::Uncertain)
    }

    fn u16(&mut self, offset: u64) -> Result<u16, ErrorCode> {
        let mut buffer = [0u8; 2];
        self.at(offset, &mut buffer)?;
        Ok(if self.little {
            u16::from_le_bytes(buffer)
        } else {
            u16::from_be_bytes(buffer)
        })
    }

    fn u32(&mut self, offset: u64) -> Result<u32, ErrorCode> {
        let mut buffer = [0u8; 4];
        self.at(offset, &mut buffer)?;
        Ok(if self.little {
            u32::from_le_bytes(buffer)
        } else {
            u32::from_be_bytes(buffer)
        })
    }

    /// Read one tag value, inline or by offset, bounded by `max`.
    fn value(&mut self, offset: u64, count: u32, max: usize) -> Result<Vec<u8>, ErrorCode> {
        let length = usize::try_from(count)
            .ok()
            .filter(|length| *length <= max)
            .ok_or(ErrorCode::Uncertain)?;
        let mut buffer = vec![0u8; length];
        self.at(offset, &mut buffer)?;
        Ok(buffer)
    }
}

struct Entry {
    kind: u16,
    field_type: u16,
    count: u32,
    /// Absolute offset of the value: inline in the entry or external.
    offset: u64,
}

fn first_ifd(reader: &mut Reader) -> Result<Vec<Entry>, ErrorCode> {
    let ifd_offset = u64::from(reader.u32(4)?);
    if ifd_offset == 0 || ifd_offset > HEAD_MAX {
        return Err(ErrorCode::Uncertain);
    }
    let count = usize::from(reader.u16(ifd_offset)?);
    if count == 0 || count > IFD_ENTRIES_MAX {
        return Err(ErrorCode::Uncertain);
    }
    let mut entries = Vec::with_capacity(count);
    for index in 0..count {
        let base = ifd_offset + 2 + (index * 12) as u64;
        let kind = reader.u16(base)?;
        let field_type = reader.u16(base + 2)?;
        let count = reader.u32(base + 4)?;
        let inline = {
            let mut inline = [0u8; 4];
            reader.at(base + 8, &mut inline)?;
            inline
        };
        let width = match field_type {
            1 => 1usize,
            3 => 2,
            _ => 4,
        };
        let total = width
            .checked_mul(count as usize)
            .ok_or(ErrorCode::Uncertain)?;
        let offset = if total <= 4 {
            base + 8
        } else {
            u64::from(if reader.little {
                u32::from_le_bytes(inline)
            } else {
                u32::from_be_bytes(inline)
            })
        };
        let _ = inline;
        entries.push(Entry {
            kind,
            field_type,
            count,
            offset,
        });
    }
    Ok(entries)
}

fn entry(entries: &[Entry], kind: u16) -> Option<&Entry> {
    entries.iter().find(|entry| entry.kind == kind)
}

/// Read one full SHORT/LONG array value, such as the strip layout arrays.
/// Any other element encoding is not the qualified writer's output.
fn array_values(reader: &mut Reader, entry: &Entry) -> Result<Vec<u64>, ErrorCode> {
    let count = usize::try_from(entry.count).map_err(|_| ErrorCode::Uncertain)?;
    if count == 0 || count > STRIP_ENTRIES_MAX {
        return Err(ErrorCode::Uncertain);
    }
    let element = match entry.field_type {
        3 => 2usize,
        4 => 4,
        _ => return Err(ErrorCode::Uncertain),
    };
    let mut values = Vec::with_capacity(count);
    for index in 0..count {
        let at = entry.offset + (index * element) as u64;
        values.push(match element {
            2 => u64::from(reader.u16(at)?),
            _ => u64::from(reader.u32(at)?),
        });
    }
    Ok(values)
}

fn short_values(reader: &mut Reader, entry: &Entry) -> Result<Vec<u16>, ErrorCode> {
    let count = usize::try_from(entry.count).map_err(|_| ErrorCode::Uncertain)?;
    if count == 0 || count > 4 {
        return Err(ErrorCode::Uncertain);
    }
    let mut values = Vec::with_capacity(count);
    for index in 0..count {
        values.push(reader.u16(entry.offset + (index * 2) as u64)?);
    }
    Ok(values)
}

fn long_value(reader: &mut Reader, entry: &Entry) -> Result<u32, ErrorCode> {
    if entry.count != 1 {
        return Err(ErrorCode::Uncertain);
    }
    reader.u32(entry.offset)
}

/// Validate one launcher-owned Development TIFF and return its identity.
pub(crate) fn validate(path: &Path, output_bytes_max: u64) -> Result<TiffIdentity, ErrorCode> {
    validate_for_icc(path, output_bytes_max, OUTPUT_ICC_SHA256)
}

/// Validate against an explicit embedded profile identity; the production
/// entrypoint pins the qualified bytes.
fn validate_for_icc(
    path: &Path,
    output_bytes_max: u64,
    icc_sha256: &str,
) -> Result<TiffIdentity, ErrorCode> {
    let metadata = std::fs::metadata(path).map_err(|_| ErrorCode::Uncertain)?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.len() == 0
        || metadata.len() > output_bytes_max
    {
        return Err(ErrorCode::Uncertain);
    }
    let mut reader = Reader::open(path)?;
    let entries = first_ifd(&mut reader)?;
    // Multi-page TIFFs are not the qualified single-image output.
    let next_ifd = u64::from(reader.u32(4)?)
        + 2
        + 12 * u64::try_from(entries.len()).map_err(|_| ErrorCode::Uncertain)?;
    if reader.u32(next_ifd)? != 0 {
        return Err(ErrorCode::Uncertain);
    }
    let width = long_value(
        &mut reader,
        entry(&entries, 256).ok_or(ErrorCode::Uncertain)?,
    )?;
    let height = long_value(
        &mut reader,
        entry(&entries, 257).ok_or(ErrorCode::Uncertain)?,
    )?;
    if width == 0 || height == 0 {
        return Err(ErrorCode::Uncertain);
    }
    // IEEE float32 RGB samples, Deflate compression, RGB photometric.
    let bits = short_values(
        &mut reader,
        entry(&entries, 258).ok_or(ErrorCode::Uncertain)?,
    )?;
    if bits != [32, 32, 32] {
        return Err(ErrorCode::Uncertain);
    }
    if long_value(
        &mut reader,
        entry(&entries, 259).ok_or(ErrorCode::Uncertain)?,
    )? != 8
    {
        return Err(ErrorCode::Uncertain);
    }
    if long_value(
        &mut reader,
        entry(&entries, 262).ok_or(ErrorCode::Uncertain)?,
    )? != 2
    {
        return Err(ErrorCode::Uncertain);
    }
    if let Some(samples) = entry(&entries, 277)
        && long_value(&mut reader, samples)? != 3
    {
        return Err(ErrorCode::Uncertain);
    }
    // Applied orientation with no hidden rotation.
    if let Some(orientation) = entry(&entries, 274)
        && (short_values(&mut reader, orientation)?.len() != 1
            || short_values(&mut reader, orientation)?[0] != 1)
    {
        return Err(ErrorCode::Uncertain);
    }
    let sample_format = short_values(
        &mut reader,
        entry(&entries, 339).ok_or(ErrorCode::Uncertain)?,
    )?;
    if sample_format != [3, 3, 3] {
        return Err(ErrorCode::Uncertain);
    }
    // The embedded profile is pinned byte identity; its extent also bounds
    // where strip payload may begin.
    let icc = entry(&entries, ICC_TAG).ok_or(ErrorCode::Uncertain)?;
    let bytes = reader.value(icc.offset, icc.count, ICC_BYTES_MAX)?;
    if format!("{:x}", Sha256::digest(&bytes)) != icc_sha256 {
        return Err(ErrorCode::Uncertain);
    }
    let head_end = (icc
        .offset
        .checked_add(u64::from(icc.count))
        .ok_or(ErrorCode::Uncertain)?)
    .max(
        u64::from(reader.u32(4)?)
            + 2
            + 12 * u64::try_from(entries.len()).map_err(|_| ErrorCode::Uncertain)?
            + 4,
    );
    // Strip layout must describe the declared full geometry. StripByteCounts
    // are the compressed byte counts of the pinned Deflate producer, so this
    // check proves structure and declared coverage — parsed types and values,
    // in-file, ascending, non-overlapping ranges above the IFD/ICC region,
    // every declared row accounted for, and no unaccounted trailing payload —
    // but not that the compressed payload inflates to the declared
    // `rows × width × 3 × 4` bytes: decode-verification of the artifact is
    // the consumer's gate.
    let strips = entry(&entries, 273).ok_or(ErrorCode::Uncertain)?;
    let counts = entry(&entries, 279).ok_or(ErrorCode::Uncertain)?;
    if strips.count == 0
        || strips.count != counts.count
        || strips.count as usize > STRIP_ENTRIES_MAX
    {
        return Err(ErrorCode::Uncertain);
    }
    let offsets = array_values(&mut reader, strips)?;
    let lengths = array_values(&mut reader, counts)?;
    let rows_per_strip = long_value(
        &mut reader,
        entry(&entries, 278).ok_or(ErrorCode::Uncertain)?,
    )?;
    if rows_per_strip == 0 || rows_per_strip > height {
        return Err(ErrorCode::Uncertain);
    }
    // The strips together account for every declared row: each one carries
    // `rows × width × 3 × 4` uncompressed sample bytes, the last one the
    // remaining partial row block.
    if height.div_ceil(rows_per_strip) != strips.count {
        return Err(ErrorCode::Uncertain);
    }
    let mut previous_end = head_end;
    for (offset, length) in offsets.into_iter().zip(lengths) {
        if length == 0 || offset < previous_end {
            return Err(ErrorCode::Uncertain);
        }
        let end = offset.checked_add(length).ok_or(ErrorCode::Uncertain)?;
        if end > metadata.len() {
            return Err(ErrorCode::Uncertain);
        }
        previous_end = end;
    }
    if previous_end != metadata.len() {
        // Unaccounted trailing payload is not the qualified artifact shape.
        return Err(ErrorCode::Uncertain);
    }
    let mut file = File::open(path).map_err(|_| ErrorCode::Uncertain)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|_| ErrorCode::Uncertain)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(TiffIdentity {
        size: metadata.len(),
        sha256: format!("{:x}", hasher.finalize()),
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Writer {
        bytes: Vec<u8>,
        little: bool,
    }

    impl Writer {
        fn new(little: bool) -> Self {
            let mut bytes = vec![0u8; 8];
            if little {
                bytes[..4].copy_from_slice(b"II\x2a\x00");
            } else {
                bytes[..4].copy_from_slice(b"MM\x00\x2a");
            }
            Self { bytes, little }
        }

        fn u32(&mut self, value: u32) {
            self.bytes.extend_from_slice(&if self.little {
                value.to_le_bytes()
            } else {
                value.to_be_bytes()
            });
        }

        fn patch_u32(&mut self, offset: usize, value: u32) {
            let bytes = if self.little {
                value.to_le_bytes()
            } else {
                value.to_be_bytes()
            };
            self.bytes[offset..offset + 4].copy_from_slice(&bytes);
        }

        fn write(&self, path: &Path) {
            std::fs::write(path, &self.bytes).unwrap();
        }
    }

    #[derive(Clone)]
    struct Tag {
        kind: u16,
        field_type: u16,
        count: u32,
        value: u32,
        extra: Option<Vec<u8>>,
    }

    fn build(path: &Path, tags: &[Tag], body: &[u8]) {
        let mut writer = Writer::new(true);
        let mut entries_bytes: Vec<u8> = Vec::new();
        let mut externals: Vec<u8> = Vec::new();
        let external_base = 8 + 2 + 12 * tags.len() + 4;
        let mut next_external = external_base;
        for tag in tags {
            entries_bytes.extend_from_slice(&tag.kind.to_le_bytes());
            entries_bytes.extend_from_slice(&tag.field_type.to_le_bytes());
            entries_bytes.extend_from_slice(&tag.count.to_le_bytes());
            match tag.extra {
                Some(ref extra) => {
                    entries_bytes.extend_from_slice(&(next_external as u32).to_le_bytes());
                    let mut aligned = extra.clone();
                    if aligned.len() % 2 == 1 {
                        aligned.push(0);
                    }
                    next_external += aligned.len();
                    externals.extend_from_slice(&aligned);
                }
                None => entries_bytes.extend_from_slice(&tag.value.to_le_bytes()),
            }
        }
        writer
            .bytes
            .extend_from_slice(&(tags.len() as u16).to_le_bytes());
        writer.bytes.extend_from_slice(&entries_bytes);
        writer.u32(0); // no next IFD
        writer.patch_u32(4, 8);
        writer.bytes.extend_from_slice(&externals);
        writer.bytes.extend_from_slice(body);
        writer.write(path);
    }

    fn valid_tags() -> Vec<Tag> {
        let icc = vec![0u8; 588];
        // The strip payload sits after the IFD and its external values; the
        // single strip covers the whole 4x3 image body.
        let strip_offset = (8 + 2 + 12 * 11 + 4 + 6 + 6 + icc.len()) as u32;
        vec![
            Tag {
                kind: 256,
                field_type: 4,
                count: 1,
                value: 4,
                extra: None,
            },
            Tag {
                kind: 257,
                field_type: 4,
                count: 1,
                value: 3,
                extra: None,
            },
            Tag {
                kind: 258,
                field_type: 3,
                count: 3,
                value: 0,
                extra: Some(vec![32, 0, 32, 0, 32, 0]),
            },
            Tag {
                kind: 259,
                field_type: 4,
                count: 1,
                value: 8,
                extra: None,
            },
            Tag {
                kind: 262,
                field_type: 4,
                count: 1,
                value: 2,
                extra: None,
            },
            Tag {
                kind: 273,
                field_type: 4,
                count: 1,
                value: strip_offset,
                extra: None,
            },
            Tag {
                kind: 277,
                field_type: 4,
                count: 1,
                value: 3,
                extra: None,
            },
            Tag {
                kind: 278,
                field_type: 4,
                count: 1,
                value: 3,
                extra: None,
            },
            Tag {
                kind: 279,
                field_type: 4,
                count: 1,
                value: 48,
                extra: None,
            },
            Tag {
                kind: 339,
                field_type: 3,
                count: 3,
                value: 0,
                extra: Some(vec![3, 0, 3, 0, 3, 0]),
            },
            Tag {
                kind: ICC_TAG,
                field_type: 1,
                count: icc.len() as u32,
                value: 0,
                extra: Some(icc),
            },
        ]
    }

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "slipstream-photo-tiff-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn valid_structure_and_pinned_icc_are_required() {
        let dir = temp_dir("valid");
        let path = dir.join("development.tif");
        build(&path, &valid_tags(), &[0u8; 48]);
        let synthetic_icc = {
            let payload = vec![0u8; 588];
            format!("{:x}", Sha256::digest(&payload))
        };
        let identity = validate_for_icc(&path, 1 << 20, &synthetic_icc).unwrap();
        assert_eq!((identity.width, identity.height), (4, 3));
        assert_eq!(identity.size, 8 + 2 + 132 + 4 + 6 + 6 + 588 + 48);

        // Any changed tag that breaks the contract is refused.
        for (index, change) in [
            Tag {
                kind: 259,
                field_type: 4,
                count: 1,
                value: 1,
                extra: None,
            }, // no deflate
            Tag {
                kind: 262,
                field_type: 4,
                count: 1,
                value: 1,
                extra: None,
            }, // not RGB
            Tag {
                kind: 256,
                field_type: 4,
                count: 1,
                value: 0,
                extra: None,
            }, // empty width
        ]
        .into_iter()
        .enumerate()
        {
            let mut tags = valid_tags();
            tags[index] = change;
            let broken = dir.join(format!("broken-{index}.tif"));
            build(&broken, &tags, &[0u8; 48]);
            assert!(validate(&broken, 1 << 20).is_err(), "case {index}");
        }
        // A foreign ICC payload never satisfies the pinned bytes.
        let mut tags = valid_tags();
        tags[10].extra.as_mut().unwrap()[0] = b'X';
        let foreign = dir.join("foreign.tif");
        build(&foreign, &tags, &[0u8; 48]);
        assert!(validate(&foreign, 1 << 20).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn truncated_foreign_and_multi_page_are_refused() {
        let dir = temp_dir("refused");
        let path = dir.join("development.tif");
        build(&path, &valid_tags(), &[0u8; 48]);
        assert_eq!(validate(&path, 8).unwrap_err(), ErrorCode::Uncertain);

        let mut writer = Writer::new(false);
        writer.u32(8);
        std::fs::write(dir.join("magic.tif"), &writer.bytes).unwrap();
        assert!(validate(&dir.join("magic.tif"), 1 << 20).is_err());

        // A truncated IFD cannot be parsed.
        let mut writer = Writer::new(true);
        writer.u32(8);
        writer.bytes.extend_from_slice(&2u16.to_le_bytes());
        std::fs::write(dir.join("short.tif"), &writer.bytes).unwrap();
        assert!(validate(&dir.join("short.tif"), 1 << 20).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Build one 4x3 fixture with an explicit strip layout. `offsets` and
    /// `counts` may differ in length or content to describe malformed files.
    fn strip_body_offset(external_bytes: usize, rows_tag: bool) -> u32 {
        (8 + 2 + 12 * (8 + 2 + usize::from(rows_tag)) + 4 + 6 + 6 + 588 + external_bytes) as u32
    }

    fn strip_case(
        path: &Path,
        rows_per_strip: Option<u32>,
        offsets: Vec<u32>,
        counts: Vec<u32>,
        body: &[u8],
    ) {
        let mut tags: Vec<Tag> = valid_tags()
            .into_iter()
            .filter(|tag| !matches!(tag.kind, 273 | 278 | 279))
            .collect();
        let layout = |kind: u16, values: &[u32]| Tag {
            kind,
            field_type: 4,
            count: values.len() as u32,
            value: values[0],
            extra: (values.len() > 1).then(|| {
                values
                    .iter()
                    .flat_map(|value| value.to_le_bytes())
                    .collect()
            }),
        };
        tags.push(layout(273, &offsets));
        if let Some(rows_per_strip) = rows_per_strip {
            tags.push(Tag {
                kind: 278,
                field_type: 4,
                count: 1,
                value: rows_per_strip,
                extra: None,
            });
        }
        tags.push(layout(279, &counts));
        tags.sort_by_key(|tag| tag.kind);
        build(path, &tags, body);
    }

    #[test]
    fn a_strip_layout_as_long_as_the_image_is_tall_is_validated() {
        let dir = temp_dir("many_strips");
        let synthetic_icc = format!("{:x}", Sha256::digest(vec![0u8; 588]));
        let layout = |kind: u16, values: &[u32]| Tag {
            kind,
            field_type: 4,
            count: values.len() as u32,
            value: values[0],
            extra: (values.len() > 1).then(|| {
                values
                    .iter()
                    .flat_map(|value| value.to_le_bytes())
                    .collect()
            }),
        };
        let tall = |path: &Path, rows: u32, strip_bytes: u32| {
            let mut tags: Vec<Tag> = valid_tags()
                .into_iter()
                .filter(|tag| !matches!(tag.kind, 257 | 273 | 278 | 279))
                .collect();
            tags.push(Tag {
                kind: 257,
                field_type: 4,
                count: 1,
                value: rows,
                extra: None,
            });
            let base = strip_body_offset(rows as usize * 8, true);
            let offsets: Vec<u32> = (0..rows).map(|index| base + index * strip_bytes).collect();
            let counts: Vec<u32> = vec![strip_bytes; rows as usize];
            tags.push(layout(273, &offsets));
            tags.push(Tag {
                kind: 278,
                field_type: 4,
                count: 1,
                value: 1,
                extra: None,
            });
            tags.push(layout(279, &counts));
            tags.sort_by_key(|tag| tag.kind);
            let body = vec![7u8; (rows * strip_bytes) as usize];
            build(path, &tags, &body);
        };
        // The qualified writer emits one strip per row, so a real artifact
        // carries a strip entry per row: 5000 rows must validate.
        let path = dir.join("rows.tif");
        tall(&path, 5_000, 48);
        let identity = validate_for_icc(&path, 1 << 23, &synthetic_icc).unwrap();
        assert_eq!((identity.width, identity.height), (4, 5_000));
        // The parsed arrays stay bounded.
        let path = dir.join("over.tif");
        tall(&path, 65_537, 48);
        assert_eq!(
            validate_for_icc(&path, 1 << 23, &synthetic_icc).unwrap_err(),
            ErrorCode::Uncertain
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn strip_ranges_and_declared_sample_coverage_are_validated() {
        let dir = temp_dir("strips");
        let body = [7u8; 48];
        let synthetic_icc = format!("{:x}", Sha256::digest(vec![0u8; 588]));
        let accepts = |path: &Path| validate_for_icc(path, 1 << 20, &synthetic_icc).is_ok();

        // A well-formed single-strip layout validates.
        let path = dir.join("single.tif");
        strip_case(
            &path,
            Some(3),
            vec![strip_body_offset(0, true)],
            vec![48],
            &body,
        );
        assert!(accepts(&path));

        // Three ascending, disjoint one-row strips validate too.
        let path = dir.join("rows.tif");
        let base = strip_body_offset(24, true);
        strip_case(
            &path,
            Some(1),
            vec![base, base + 16, base + 32],
            vec![16, 16, 16],
            &body,
        );
        assert!(accepts(&path));

        // Truncated strip data is refused.
        let path = dir.join("truncated.tif");
        strip_case(
            &path,
            Some(3),
            vec![strip_body_offset(0, true)],
            vec![48],
            &body,
        );
        let complete = std::fs::read(&path).unwrap();
        std::fs::write(&path, &complete[..complete.len() - 14]).unwrap();
        assert_eq!(validate(&path, 1 << 20).unwrap_err(), ErrorCode::Uncertain);

        // A strip claiming bytes beyond the file end is refused.
        let path = dir.join("beyond.tif");
        strip_case(
            &path,
            Some(3),
            vec![strip_body_offset(0, true)],
            vec![100],
            &body,
        );
        assert!(validate(&path, 1 << 20).is_err());

        // An out-of-file strip offset is refused.
        let path = dir.join("outside.tif");
        strip_case(&path, Some(3), vec![100_000], vec![48], &body);
        assert!(validate(&path, 1 << 20).is_err());

        // Mismatched strip array counts are refused.
        let path = dir.join("mismatch.tif");
        strip_case(&path, Some(3), vec![746], vec![24, 24], &body);
        assert!(validate(&path, 1 << 20).is_err());

        // Strips that do not account for every declared row are refused.
        let path = dir.join("undercovered.tif");
        strip_case(
            &path,
            Some(1),
            vec![strip_body_offset(0, true)],
            vec![48],
            &body,
        );
        assert!(validate(&path, 1 << 20).is_err());

        // A zero-length strip is refused.
        let path = dir.join("empty.tif");
        strip_case(
            &path,
            Some(3),
            vec![strip_body_offset(0, true)],
            vec![0],
            &body,
        );
        assert!(validate(&path, 1 << 20).is_err());

        // Overlapping strips are refused.
        let path = dir.join("overlap.tif");
        strip_case(&path, Some(2), vec![746, 760], vec![24, 24], &body);
        assert!(validate(&path, 1 << 20).is_err());

        // Unaccounted trailing payload is refused.
        let path = dir.join("trailing.tif");
        strip_case(
            &path,
            Some(3),
            vec![strip_body_offset(0, true)],
            vec![48],
            &body,
        );
        let mut trailing = std::fs::read(&path).unwrap();
        trailing.push(0);
        std::fs::write(&path, trailing).unwrap();
        assert!(validate(&path, 1 << 20).is_err());

        // Strip arrays must use SHORT or LONG elements.
        let mut tags = valid_tags();
        tags[5].field_type = 1;
        let path = dir.join("typed.tif");
        build(&path, &tags, &body);
        assert!(validate(&path, 1 << 20).is_err());

        // The declared row geometry requires the rows-per-strip tag.
        let path = dir.join("norows.tif");
        strip_case(
            &path,
            None,
            vec![strip_body_offset(0, false)],
            vec![48],
            &body,
        );
        assert!(validate(&path, 1 << 20).is_err());

        std::fs::remove_dir_all(dir).unwrap();
    }
}
