use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

pub(crate) const SONY_SAMPLE_SHA256: &str =
    "d577d59901a4aff3ad6f35a1121fe1f3c0345890a1cadc2d33fe7ddaadd3fa74";

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct OriginalSnapshot {
    pub(crate) sha256: String,
    pub(crate) length: u64,
    device: u64,
    inode: u64,
    mode: u32,
    owner: u32,
    group: u32,
    modified_seconds: i64,
    modified_nanoseconds: i64,
}

pub(crate) fn raw_sample() -> (PathBuf, OriginalSnapshot) {
    let path = PathBuf::from(
        std::env::var_os("SLIPSTREAM_RAW_SAMPLE")
            .expect("SLIPSTREAM_RAW_SAMPLE is required for the opt-in RAW safety gate"),
    );
    let snapshot = original_snapshot(&path);
    assert!(
        snapshot.sha256 == SONY_SAMPLE_SHA256,
        "SLIPSTREAM_RAW_SAMPLE does not match the configured Sony safety sample"
    );
    (path, snapshot)
}

/// Builds a minimal redistributable DNG with one embedded JPEG Preview.
/// The fixture has a synthetic 128-by-128 CFA plane and is accepted by LibRaw;
/// no camera file or sensor data is involved.
pub(crate) fn generated_dng(jpeg: &[u8], orientation: u16) -> Vec<u8> {
    generated_dng_candidates(&[(120, 80, jpeg)], orientation)
}

pub(crate) fn generated_dng_candidates(
    previews: &[(u32, u32, &[u8])],
    orientation: u16,
) -> Vec<u8> {
    assert!(!previews.is_empty());
    assert!((1..=8).contains(&orientation));

    enum Value {
        Inline(Vec<u8>),
        External(Vec<u8>),
        Data(Vec<u8>),
    }
    struct Entry {
        tag: u16,
        kind: u16,
        count: u32,
        value: Value,
    }
    fn inline_short(value: u16) -> Value {
        let mut bytes = value.to_le_bytes().to_vec();
        bytes.resize(4, 0);
        Value::Inline(bytes)
    }
    fn inline_long(value: u32) -> Value {
        Value::Inline(value.to_le_bytes().to_vec())
    }
    fn entry(tag: u16, kind: u16, count: u32, value: Value) -> Entry {
        Entry {
            tag,
            kind,
            count,
            value,
        }
    }
    fn write_ifd(
        output: &mut Vec<u8>,
        entries: Vec<Entry>,
        next_ifd: u32,
        external_offset: u32,
        external: &mut Vec<u8>,
    ) {
        output.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for entry in entries {
            output.extend_from_slice(&entry.tag.to_le_bytes());
            output.extend_from_slice(&entry.kind.to_le_bytes());
            output.extend_from_slice(&entry.count.to_le_bytes());
            match entry.value {
                Value::Inline(bytes) => {
                    assert_eq!(bytes.len(), 4);
                    output.extend_from_slice(&bytes);
                }
                Value::External(bytes) | Value::Data(bytes) => {
                    let offset = external_offset + external.len() as u32;
                    output.extend_from_slice(&offset.to_le_bytes());
                    external.extend_from_slice(&bytes);
                }
            }
        }
        output.extend_from_slice(&next_ifd.to_le_bytes());
    }

    let matrix = [1_i32, 0, 0, 0, 1, 0, 0, 0, 1]
        .into_iter()
        .flat_map(|value| [value.to_le_bytes(), 1_i32.to_le_bytes()].concat())
        .collect::<Vec<_>>();
    let neutral = (0..3)
        .flat_map(|_| [1_u32.to_le_bytes(), 1_u32.to_le_bytes()].concat())
        .collect::<Vec<_>>();
    let ifd0 = vec![
        entry(256, 4, 1, inline_long(128)),
        entry(257, 4, 1, inline_long(128)),
        entry(258, 3, 1, inline_short(16)),
        entry(259, 3, 1, inline_short(1)),
        entry(262, 3, 1, inline_short(32803)),
        entry(271, 2, 10, Value::External(b"Synthetic\0".to_vec())),
        entry(272, 2, 9, Value::External(b"Test DNG\0".to_vec())),
        entry(273, 4, 1, Value::Data(vec![0; 128 * 128 * 2])),
        entry(274, 3, 1, inline_short(orientation)),
        entry(277, 3, 1, inline_short(1)),
        entry(278, 4, 1, inline_long(128)),
        entry(279, 4, 1, inline_long(128 * 128 * 2)),
        entry(
            33421,
            3,
            2,
            Value::Inline([2_u16, 2].map(u16::to_le_bytes).concat()),
        ),
        entry(33422, 1, 4, Value::Inline(vec![0, 1, 1, 2])),
        entry(50706, 1, 4, Value::Inline(vec![1, 4, 0, 0])),
        entry(50707, 1, 4, Value::Inline(vec![1, 3, 0, 0])),
        entry(50708, 2, 14, Value::External(b"Synthetic DNG\0".to_vec())),
        entry(50710, 1, 3, Value::Inline(vec![0, 1, 2, 0])),
        entry(50711, 3, 1, inline_short(1)),
        entry(50717, 4, 1, inline_long(65535)),
        entry(50721, 10, 9, Value::External(matrix)),
        entry(50728, 5, 3, Value::External(neutral)),
        entry(50778, 3, 1, inline_short(21)),
    ];
    let preview_ifds = previews
        .iter()
        .map(|(width, height, jpeg)| {
            vec![
                entry(256, 4, 1, inline_long(*width)),
                entry(257, 4, 1, inline_long(*height)),
                entry(259, 3, 1, inline_short(6)),
                entry(262, 3, 1, inline_short(6)),
                entry(274, 3, 1, inline_short(1)),
                entry(277, 3, 1, inline_short(3)),
                entry(513, 4, 1, Value::Data(jpeg.to_vec())),
                entry(514, 4, 1, inline_long(jpeg.len() as u32)),
            ]
        })
        .collect::<Vec<_>>();
    let ifd0_offset = 8_u32;
    let ifd0_size = 2 + ifd0.len() as u32 * 12 + 4;
    let preview_ifd_size = 2 + 8 * 12 + 4;
    let first_preview_offset = ifd0_offset + ifd0_size;
    let external_offset = first_preview_offset + preview_ifd_size * preview_ifds.len() as u32;
    let mut output = b"II*\0".to_vec();
    output.extend_from_slice(&ifd0_offset.to_le_bytes());
    let mut external = Vec::new();
    write_ifd(
        &mut output,
        ifd0,
        first_preview_offset,
        external_offset,
        &mut external,
    );
    let preview_count = preview_ifds.len();
    for (index, preview_ifd) in preview_ifds.into_iter().enumerate() {
        let next_ifd = if index + 1 == preview_count {
            0
        } else {
            first_preview_offset + preview_ifd_size * (index as u32 + 1)
        };
        write_ifd(
            &mut output,
            preview_ifd,
            next_ifd,
            external_offset,
            &mut external,
        );
    }
    output.extend_from_slice(&external);
    output
}

pub(crate) fn original_snapshot(path: &Path) -> OriginalSnapshot {
    let bytes = fs::read(path).expect("Original safety sample could not be read");
    let metadata = fs::metadata(path).expect("Original safety sample metadata could not be read");
    assert!(
        metadata.is_file(),
        "Original safety sample must be a regular file"
    );
    OriginalSnapshot {
        sha256: format!("{:x}", Sha256::digest(bytes)),
        length: metadata.len(),
        device: metadata.dev(),
        inode: metadata.ino(),
        mode: metadata.mode(),
        owner: metadata.uid(),
        group: metadata.gid(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
    }
}
