//! Development TIFF publication regression tests.

use super::*;
use sha2::{Digest, Sha256};

/// Writes a structurally valid Development TIFF whose single Deflate
/// strip carries `payload`, so only the decoded content can differ
/// between a good and a corrupt artifact. The embedded profile is the
/// pinned accepted engine output profile.
pub(crate) fn write_development_tiff(path: &Path, payload: &[u8]) {
    let icc: &[u8] =
        include_bytes!("../../slipstream-core/assets/prophoto-linear-g10-darktable.icc");
    let mut bytes = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
    bytes.extend_from_slice(&14_u16.to_le_bytes());
    let mut externals: Vec<u8> = Vec::new();
    let base: usize = 8 + 2 + 14 * 12 + 4;
    let mut at = base as u32;
    let entry = |tag: u16,
                 kind: u16,
                 count: u32,
                 value: u32,
                 extra: Option<&[u8]>,
                 out: &mut Vec<u8>,
                 externals: &mut Vec<u8>,
                 at: &mut u32| {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        match extra {
            Some(blob) => {
                out.extend_from_slice(&at.to_le_bytes());
                externals.extend_from_slice(blob);
                if blob.len() % 2 == 1 {
                    externals.push(0);
                }
                *at += u32::try_from(blob.len() + blob.len() % 2).unwrap();
            }
            None => out.extend_from_slice(&value.to_le_bytes()),
        }
    };
    let three_shorts =
        |a: u16, b: u16, c: u16| [a.to_le_bytes(), b.to_le_bytes(), c.to_le_bytes()].concat();
    entry(256, 4, 1, 2, None, &mut bytes, &mut externals, &mut at);
    entry(257, 4, 1, 1, None, &mut bytes, &mut externals, &mut at);
    entry(
        258,
        3,
        3,
        0,
        Some(&three_shorts(32, 32, 32)),
        &mut bytes,
        &mut externals,
        &mut at,
    );
    entry(259, 4, 1, 8, None, &mut bytes, &mut externals, &mut at);
    entry(262, 3, 1, 2, None, &mut bytes, &mut externals, &mut at);
    let strip_patch = bytes.len() + 8;
    entry(273, 4, 1, 0, None, &mut bytes, &mut externals, &mut at);
    entry(277, 3, 1, 3, None, &mut bytes, &mut externals, &mut at);
    entry(278, 4, 1, 1, None, &mut bytes, &mut externals, &mut at);
    entry(
        279,
        4,
        1,
        u32::try_from(payload.len()).unwrap(),
        None,
        &mut bytes,
        &mut externals,
        &mut at,
    );
    entry(284, 3, 1, 1, None, &mut bytes, &mut externals, &mut at);
    entry(
        339,
        3,
        3,
        0,
        Some(&three_shorts(3, 3, 3)),
        &mut bytes,
        &mut externals,
        &mut at,
    );
    // Resolution entries are RATIONAL, a type this walk does not read but
    // a real engine artifact always carries. A walk that refuses an unread
    // type refuses every development artifact.
    let resolution = [0x2c_u8, 0x01, 0, 0, 1, 0, 0, 0];
    entry(
        282,
        5,
        1,
        0,
        Some(&resolution),
        &mut bytes,
        &mut externals,
        &mut at,
    );
    entry(
        283,
        5,
        1,
        0,
        Some(&resolution),
        &mut bytes,
        &mut externals,
        &mut at,
    );
    entry(
        34675,
        7,
        u32::try_from(icc.len()).unwrap(),
        0,
        Some(icc),
        &mut bytes,
        &mut externals,
        &mut at,
    );
    bytes.extend_from_slice(&[0, 0, 0, 0]);
    bytes.extend_from_slice(&externals);
    let strip_offset: u32 = bytes.len() as u32;
    let patch_end = strip_patch + 4;
    bytes[strip_patch..patch_end].copy_from_slice(&strip_offset.to_le_bytes());
    bytes.extend_from_slice(payload);
    fs::write(path, bytes).unwrap();
}

/// One zlib stream made of stored deflate blocks, so the test does not
/// need a compressor to produce a payload that must decode cleanly.
pub(crate) fn stored_zlib(content: &[u8]) -> Vec<u8> {
    let mut stream = vec![0x78, 0x01];
    for chunk in content.chunks(65_535) {
        let length = chunk.len() as u16;
        stream.push(if chunk.len() == content.len() { 1 } else { 0 });
        stream.extend_from_slice(&length.to_le_bytes());
        stream.extend_from_slice(&(!length).to_le_bytes());
        stream.extend_from_slice(chunk);
    }
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in content {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    stream.extend_from_slice(&((b << 16) | a).to_be_bytes());
    stream
}

#[test]
fn publication_requires_a_developed_payload_that_really_inflates() {
    let base = std::env::temp_dir().join(format!(
        "export-decode-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    fs::create_dir_all(&base).unwrap();

    // A structurally valid TIFF whose strip does not inflate: refused by
    // the real Development TIFF reader.
    // A stream whose stored-block header claims more bytes than follow.
    let mut corrupt = vec![0x78_u8, 0x01, 1];
    corrupt.extend_from_slice(&24_u16.to_le_bytes());
    corrupt.extend_from_slice(&(!24_u16).to_le_bytes());
    corrupt.extend_from_slice(&[0_u8; 6]);
    let corrupt_path = base.join("corrupt.tif");
    write_development_tiff(&corrupt_path, &corrupt);
    assert!(validate_development_tiff(&corrupt_path).is_err());

    // The publication path refuses before any artifact is published.
    let originals = base.join("originals");
    fs::create_dir_all(&originals).unwrap();
    fs::create_dir_all(base.join("exports")).unwrap();
    let workspace = ExportWorkspace::open(base.join("exports"), &originals).unwrap();
    let writer = workspace.begin_development_tiff("exp-corrupt").unwrap();
    fs::copy(&corrupt_path, writer.temporary_path()).unwrap();
    assert!(
        writer
            .publish(|path| validate_development_tiff(path).map(|_| ()))
            .is_err()
    );
    let artifacts = base.join("exports/artifacts");
    assert_eq!(fs::read_dir(&artifacts).unwrap().count(), 0);

    // A well-formed stored-block payload with the exact float32 RGB
    // geometry decodes through the reader and publishes; the validation
    // yields the disclosed geometry and the embedded-profile identity.
    let pixels = vec![0_u8; 2 * 3 * 4];
    let good_path = base.join("good.tif");
    write_development_tiff(&good_path, &stored_zlib(&pixels));
    let good_facts = validate_development_tiff(&good_path).unwrap();
    assert_eq!(good_facts.width, 2);
    assert_eq!(good_facts.height, 1);
    let icc: &[u8] =
        include_bytes!("../../slipstream-core/assets/prophoto-linear-g10-darktable.icc");
    assert_eq!(
        good_facts.profile_identity,
        format!("{:x}", Sha256::digest(icc))
    );
    let writer = workspace.begin_development_tiff("exp-good").unwrap();
    fs::copy(&good_path, writer.temporary_path()).unwrap();
    let published = writer
        .publish(|path| validate_development_tiff(path).map(|_| ()))
        .unwrap();
    assert_eq!(
        fs::read(&published.path).unwrap(),
        fs::read(&good_path).unwrap()
    );

    // A truncated stream that decodes to fewer samples is also refused.
    let short = stored_zlib(&pixels[..12]);
    let short_path = base.join("short.tif");
    write_development_tiff(&short_path, &short);
    assert!(validate_development_tiff(&short_path).is_err());

    let _ = fs::remove_dir_all(base);
}
