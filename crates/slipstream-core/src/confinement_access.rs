//! Revision-checked read access to one confined Original File.
//!
//! Every read streams through the descriptor retained for the operation and is
//! bracketed by two stats of that same descriptor, so a file replaced or
//! resized while it is being read is reported as `Changed` instead of yielding
//! unusable bytes or digests. The module is deliberately separate from
//! [`crate::confinement::LibraryRoot`], which owns Library scanning and
//! `openat2` path confinement.

use super::{ConfinementError, OriginalCapability, OriginalFacts, sys};
use std::{
    fs::File,
    io::Write,
    os::fd::{AsRawFd, RawFd},
};

#[cfg(test)]
use super::{TestHookPoint, test_hook};

pub(super) const MAXIMUM_RANGE_BYTES: usize = 16 * 1024 * 1024;
pub(super) const MAXIMUM_WHOLE_BYTES: u64 = 128 * 1024 * 1024;
/// Largest Original File the streaming fingerprint reader accepts. Bounded
/// memory use is independent of this limit: hashing streams fixed-size chunks.
const MAXIMUM_DIGEST_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const DIGEST_CHUNK_BYTES: usize = 8 * 1024 * 1024;

impl OriginalCapability {
    pub fn read_range(&self, offset: u64, length: usize) -> Result<Vec<u8>, ConfinementError> {
        if length > MAXIMUM_RANGE_BYTES || offset > i64::MAX as u64 {
            return Err(ConfinementError::ResourceLimit(
                "Confined read exceeds limits",
            ));
        }
        let file = self.root.open_confined(&self.path, false)?;
        let before = stat_regular(file.as_raw_fd())?;
        #[cfg(test)]
        test_hook(TestHookPoint::RangeAfterInitialStat);
        let mut bytes = vec![0; length];
        let count = sys::pread(file.as_raw_fd(), &mut bytes, offset)
            .map_err(|_| ConfinementError::Io("Original File could not be read safely"))?;
        bytes.truncate(count);
        let after = stat_regular(file.as_raw_fd())?;
        if !same_revision(&before, &after) {
            return Err(ConfinementError::Changed);
        }
        Ok(bytes)
    }

    pub fn read_whole(&self, maximum_bytes: u64) -> Result<RevisionCheckedBytes, ConfinementError> {
        if maximum_bytes > MAXIMUM_WHOLE_BYTES {
            return Err(ConfinementError::ResourceLimit(
                "Confined whole-file read exceeds limits",
            ));
        }
        let file = self.root.open_confined(&self.path, false)?;
        let before = stat_regular(file.as_raw_fd())?;
        #[cfg(test)]
        test_hook(TestHookPoint::WholeAfterInitialStat);
        let size = validated_size(&before)?;
        if size > maximum_bytes {
            return Err(ConfinementError::ResourceLimit(
                "Original File exceeds whole-file read limit",
            ));
        }
        let mut bytes = vec![
            0;
            usize::try_from(size).map_err(|_| {
                ConfinementError::ResourceLimit("Original File exceeds whole-file read limit")
            })?
        ];
        let mut consumed = 0;
        while consumed < bytes.len() {
            let count = match sys::pread(file.as_raw_fd(), &mut bytes[consumed..], consumed as u64)
            {
                Ok(count) => count,
                Err(_) => {
                    return Err(read_failure_after_revision_check(
                        file.as_raw_fd(),
                        &before,
                        "Original File could not be read completely",
                    ));
                }
            };
            if count == 0 {
                return Err(read_failure_after_revision_check(
                    file.as_raw_fd(),
                    &before,
                    "Original File could not be read completely",
                ));
            }
            consumed += count;
        }
        let after = stat_regular(file.as_raw_fd())?;
        if !same_revision(&before, &after) {
            return Err(ConfinementError::Changed);
        }
        Ok(RevisionCheckedBytes {
            bytes,
            facts: facts_from_stat(&before)?,
        })
    }

    /// Streams one complete, revision-checked SHA-256 digest of the Original
    /// File with bounded memory. The digest is computed between two stats of
    /// the same descriptor; a file that changes while being read yields
    /// `Changed` instead of an unusable digest.
    pub fn digest_file(&self) -> Result<RevisionCheckedDigest, ConfinementError> {
        let file = self.root.open_confined(&self.path, false)?;
        digest_open_file(&file)
    }

    /// Streams the revision-checked digest of the Original File only when one
    /// exists at the Location. `None` reports an absent Location, so a caller
    /// distinguishing absence from an unreadable entry never reads a failed
    /// open of nothing as unreadable content.
    pub fn digest_file_if_present(
        &self,
    ) -> Result<Option<RevisionCheckedDigest>, ConfinementError> {
        match self.root.open_confined_if_present(&self.path)? {
            Some(file) => digest_open_file(&file).map(Some),
            None => Ok(None),
        }
    }

    /// Copies the complete Original through the retained confined descriptor
    /// into a caller-owned writer while hashing the exact bytes that were
    /// written. The caller must discard partial output when this returns an
    /// error and publish only after the returned digest and facts are accepted.
    /// The bounded buffer keeps staging memory independent of the Original's
    /// size, and a source revision change before or after the copy is rejected.
    pub fn copy_revision_checked<W: Write>(
        &self,
        mut writer: W,
    ) -> Result<RevisionCheckedDigest, ConfinementError> {
        let file = self.root.open_confined(&self.path, false)?;
        digest_revision_checked(
            &file,
            "Original File exceeds staging read limit",
            |step| -> Result<(), ConfinementError> {
                match step {
                    StreamStep::Chunk(chunk) => writer
                        .write_all(chunk)
                        .map_err(|_| ConfinementError::Io("Staging output could not be written")),
                    StreamStep::Complete => writer
                        .flush()
                        .map_err(|_| ConfinementError::Io("Staging output could not be flushed")),
                }
            },
        )
    }

    pub(crate) fn open_revision_checked(&self) -> Result<OpenedOriginal, ConfinementError> {
        let file = self.root.open_confined(&self.path, false)?;
        let revision = stat_regular(file.as_raw_fd())?;
        Ok(OpenedOriginal { file, revision })
    }
}

/// One delivery from a revision-checked stream. `Complete` follows the final
/// chunk so a staging sink flushes before the source revision is re-checked.
enum StreamStep<'a> {
    /// One chunk of the exact bytes that are hashed.
    Chunk(&'a [u8]),
    /// The final chunk has been delivered.
    Complete,
}

/// Streams the complete confined File between two revisions of the same
/// descriptor, hashing exactly the bytes delivered to `sink`. A file larger
/// than the streaming limit fails with `limit_message`, so each caller names
/// its own limit, and `sink` observes `Complete` after the final chunk and
/// before the source revision is checked a second time.
fn digest_revision_checked<S>(
    file: &File,
    limit_message: &'static str,
    mut sink: S,
) -> Result<RevisionCheckedDigest, ConfinementError>
where
    S: FnMut(StreamStep<'_>) -> Result<(), ConfinementError>,
{
    use sha2::{Digest as _, Sha256};

    let before = stat_regular(file.as_raw_fd())?;
    let size = validated_size(&before)?;
    if size > MAXIMUM_DIGEST_BYTES {
        return Err(ConfinementError::ResourceLimit(limit_message));
    }
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; DIGEST_CHUNK_BYTES];
    let mut consumed = 0_u64;
    while consumed < size {
        let length = buffer.len().min(
            usize::try_from(size - consumed)
                .map_err(|_| ConfinementError::ResourceLimit(limit_message))?,
        );
        let chunk = &mut buffer[..length];
        let count = match sys::pread(file.as_raw_fd(), chunk, consumed) {
            Ok(count) => count,
            Err(_) => {
                return Err(read_failure_after_revision_check(
                    file.as_raw_fd(),
                    &before,
                    "Original File could not be read completely",
                ));
            }
        };
        if count == 0 {
            return Err(read_failure_after_revision_check(
                file.as_raw_fd(),
                &before,
                "Original File could not be read completely",
            ));
        }
        sink(StreamStep::Chunk(&chunk[..count]))?;
        hasher.update(&chunk[..count]);
        consumed += u64::try_from(count)
            .map_err(|_| ConfinementError::Io("Original File could not be read completely"))?;
    }
    sink(StreamStep::Complete)?;
    let after = stat_regular(file.as_raw_fd())?;
    if !same_revision(&before, &after) {
        return Err(ConfinementError::Changed);
    }
    Ok(RevisionCheckedDigest {
        digest: format!("{:x}", hasher.finalize()),
        facts: facts_from_stat(&before)?,
    })
}

/// Digests one already-opened Original File between two revisions of the same
/// descriptor.
fn digest_open_file(file: &File) -> Result<RevisionCheckedDigest, ConfinementError> {
    digest_revision_checked(
        file,
        "Original File exceeds fingerprint read limit",
        |_step| -> Result<(), ConfinementError> { Ok(()) },
    )
}

pub(crate) struct OpenedOriginal {
    file: File,
    revision: libc::stat,
}

impl OpenedOriginal {
    /// Reads one bounded range through the descriptor retained for this
    /// inspection. A short result is meaningful to format parsers (for
    /// example, a truncated EXIF segment); callers verify the same descriptor
    /// once their complete inspection is finished.
    pub(crate) fn pread_range(
        &self,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, ConfinementError> {
        if length > MAXIMUM_RANGE_BYTES || offset > i64::MAX as u64 {
            return Err(ConfinementError::ResourceLimit(
                "Confined read exceeds limits",
            ));
        }
        let mut bytes = vec![0; length];
        let count = sys::pread(self.file.as_raw_fd(), &mut bytes, offset).map_err(|_| {
            read_failure_after_revision_check(
                self.file.as_raw_fd(),
                &self.revision,
                "Original File could not be read safely",
            )
        })?;
        bytes.truncate(count);
        Ok(bytes)
    }

    pub(crate) fn size(&self) -> Result<u64, ConfinementError> {
        validated_size(&self.revision)
    }

    pub(crate) fn descriptor(&self) -> RawFd {
        self.file.as_raw_fd()
    }

    pub(crate) fn verify_unchanged(&self) -> Result<OriginalFacts, ConfinementError> {
        #[cfg(test)]
        test_hook(TestHookPoint::DescriptorBeforeVerification);
        let after = stat_regular(self.file.as_raw_fd())?;
        if !same_revision(&self.revision, &after) {
            return Err(ConfinementError::Changed);
        }
        facts_from_stat(&self.revision)
    }
}

#[derive(Debug)]
pub struct RevisionCheckedBytes {
    pub bytes: Vec<u8>,
    pub facts: OriginalFacts,
}

/// A complete-content SHA-256 digest together with the file facts observed
/// while hashing. Equal digests prove equal bytes at the hashed revision.
#[derive(Debug, PartialEq)]
pub struct RevisionCheckedDigest {
    pub digest: String,
    pub facts: OriginalFacts,
}

pub(super) fn stat_regular(fd: RawFd) -> Result<libc::stat, ConfinementError> {
    let value =
        sys::fstat(fd).map_err(|_| ConfinementError::Io("Original File could not be inspected"))?;
    if value.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(ConfinementError::NotRegular);
    }
    Ok(value)
}

pub(super) fn validated_size(value: &libc::stat) -> Result<u64, ConfinementError> {
    u64::try_from(value.st_size)
        .map_err(|_| ConfinementError::Io("Original File facts are invalid"))
}

pub(super) fn facts_from_stat(value: &libc::stat) -> Result<OriginalFacts, ConfinementError> {
    let size = validated_size(value)?;
    let mtime_ms = value.st_mtime as f64 * 1000.0 + value.st_mtime_nsec as f64 / 1_000_000.0;
    if !mtime_ms.is_finite() || mtime_ms < 0.0 {
        return Err(ConfinementError::Io("Original File facts are invalid"));
    }
    Ok(OriginalFacts {
        size,
        mtime_ms,
        device: value.st_dev,
        inode: value.st_ino,
    })
}

fn read_failure_after_revision_check(
    fd: RawFd,
    before: &libc::stat,
    message: &'static str,
) -> ConfinementError {
    match stat_regular(fd) {
        Ok(after) if !same_revision(before, &after) => ConfinementError::Changed,
        _ => ConfinementError::Io(message),
    }
}

fn same_revision(left: &libc::stat, right: &libc::stat) -> bool {
    left.st_dev == right.st_dev
        && left.st_ino == right.st_ino
        && left.st_size == right.st_size
        && left.st_mtime == right.st_mtime
        && left.st_mtime_nsec == right.st_mtime_nsec
}
