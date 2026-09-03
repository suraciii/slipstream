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
