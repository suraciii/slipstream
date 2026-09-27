//! Confined, observation-only Sidecar association and lease-authorized publication.
use crate::{
    OriginalKind, RelativeOriginalPath,
    confinement::{self, ConfinementError, LibraryRoot},
    identity::{classify_name, pairing_stem},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::CString,
    fs::File,
    io::{self, Write},
    os::fd::AsRawFd,
};

const MAX_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SidecarFacts {
    pub size: u64,
    pub modified_seconds: i64,
    pub modified_nanoseconds: u32,
    pub device: u64,
    pub inode: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SidecarObservation {
    Eligible {
        location: String,
        facts: SidecarFacts,
        sha256: String,
    },
    Absent,
    Ineligible {
        reason: String,
    },
    Ambiguous {
        candidates: Vec<String>,
    },
    Invalid {
        candidate: String,
    },
    Unreadable {
        candidate: String,
    },
    ResourceLimit {
        candidate: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum SidecarEvidence {
    Present { facts: SidecarFacts, sha256: String },
    Absent,
}

/// An opaque attestation supplied by the deployment supervisor, not validated by core.
#[derive(Debug)]
pub struct SupervisorLeaseToken(pub String);

/// Publication authority cannot be constructed without the supervisor attestation.
///
/// ```compile_fail
/// use slipstream_core::metadata::sidecar::ExclusiveSaveLease;
/// let lease = ExclusiveSaveLease {};
/// ```
#[derive(Debug)]
pub struct ExclusiveSaveLease {
    _token: SupervisorLeaseToken,
}
impl ExclusiveSaveLease {
    pub fn from_supervisor_token(token: SupervisorLeaseToken) -> Self {
        Self { _token: token }
    }

    fn staging_name(&self) -> Result<CString, PublishError> {
        // The supervisor's token is the publication identity. Hashing it keeps
        // the runtime record's exact filename bounded and safe even if a
        // future supervisor changes the token representation.
        let digest = Sha256::digest(self._token.0.as_bytes());
        CString::new(format!(".slipstream-sidecar-{digest:x}.tmp"))
            .map_err(|_| PublishError::StorageUnavailable)
    }
}

#[derive(Debug, PartialEq)]
pub struct PublishedSidecar {
    pub facts: SidecarFacts,
    pub sha256: String,
}

#[derive(Debug, PartialEq)]
pub enum PublishError {
    Conflict,
    StorageUnavailable,
    ResourceLimit(String),
    OutcomeUnknown,
}
impl std::fmt::Display for PublishError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict => f.write_str("Sidecar publication conflicts with an existing file"),
            Self::StorageUnavailable => f.write_str("Sidecar storage is unavailable"),
            Self::ResourceLimit(reason) => f.write_str(reason),
            Self::OutcomeUnknown => f.write_str("Sidecar publication outcome is unknown"),
        }
    }
}
impl std::error::Error for PublishError {}

fn parts(original: &RelativeOriginalPath) -> (&str, &str) {
    original
        .as_str()
        .rsplit_once('/')
        .unwrap_or(("", original.as_str()))
}
fn location(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

pub fn observe(
    root: &LibraryRoot,
    original: &RelativeOriginalPath,
    kind: OriginalKind,
) -> Result<SidecarObservation, ConfinementError> {
    let directory = root.sidecar_directory(parts(original).0)?;
    observe_in(directory.as_raw_fd(), original, kind)
}

fn observe_in(
    directory: i32,
    original: &RelativeOriginalPath,
    kind: OriginalKind,
) -> Result<SidecarObservation, ConfinementError> {
    let (parent, name) = parts(original);
    let stem = pairing_stem(name);
    let entries = confinement::sidecar_entries(directory)?;
    let mut raws = Vec::new();
    let mut jpegs = Vec::new();
    let mut candidates = Vec::new();
    for entry in &entries {
        let Some(name) = entry.name.to_str() else {
            continue;
        };
        if pairing_stem(name) != stem {
            continue;
        }
        if name
            .rsplit_once('.')
            .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("xmp"))
        {
            candidates.push(location(parent, name));
        } else if entry.kind == confinement::EntryKind::File {
            match classify_name(name) {
                Some(OriginalKind::Raw) => raws.push(location(parent, name)),
                Some(OriginalKind::Jpeg) => jpegs.push(location(parent, name)),
                None => {}
            }
        }
    }
    if kind == OriginalKind::Jpeg && !raws.is_empty() {
        return Ok(SidecarObservation::Ineligible {
            reason: "A same-basename RAW Original owns the Sidecar".into(),
        });
    }
    let owners = if raws.is_empty() { &jpegs } else { &raws };
    if owners.len() != 1 || !owners.iter().any(|owner| owner == original.as_str()) {
        return Ok(SidecarObservation::Ambiguous { candidates });
    }
    if candidates.len() > 1 {
        return Ok(SidecarObservation::Ambiguous { candidates });
    }
    let Some(candidate) = candidates.pop() else {
        return Ok(SidecarObservation::Absent);
    };
    let filename = candidate.rsplit('/').next().unwrap();
    Ok(match read_candidate(directory, filename) {
        Ok(published) => SidecarObservation::Eligible {
            location: candidate,
            facts: published.facts,
            sha256: published.sha256,
        },
        Err(ReadError::Invalid) => SidecarObservation::Invalid { candidate },
        Err(ReadError::Limit) => SidecarObservation::ResourceLimit { candidate },
        Err(ReadError::Io) => SidecarObservation::Unreadable { candidate },
    })
}

enum ReadError {
    Invalid,
    Limit,
    Io,
}
fn read_candidate(directory: i32, name: &str) -> Result<PublishedSidecar, ReadError> {
    read_candidate_bytes(directory, name).map(|(published, _)| published)
}

fn read_candidate_bytes(
    directory: i32,
    name: &str,
) -> Result<(PublishedSidecar, Vec<u8>), ReadError> {
    let name = CString::new(name).map_err(|_| ReadError::Invalid)?;
    let file = confinement::sidecar_open(directory, &name).map_err(|error| {
        if error.raw_os_error() == Some(libc::ELOOP) {
            ReadError::Invalid
        } else {
            ReadError::Io
        }
    })?;
    let before = confinement::sidecar_stat(file.as_raw_fd()).map_err(|_| ReadError::Io)?;
    if before.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(ReadError::Invalid);
    }
    let size = usize::try_from(before.st_size).map_err(|_| ReadError::Limit)?;
    if size > MAX_BYTES {
        return Err(ReadError::Limit);
    }
    let mut bytes = vec![0; size];
    let mut offset = 0;
    while offset < size {
        match confinement::sidecar_pread(file.as_raw_fd(), &mut bytes[offset..], offset as u64) {
            Ok(0) => return Err(ReadError::Io),
            Ok(count) => offset += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(ReadError::Io),
        }
    }
    let after = confinement::sidecar_stat(file.as_raw_fd()).map_err(|_| ReadError::Io)?;
    if before.st_dev != after.st_dev
        || before.st_ino != after.st_ino
        || before.st_size != after.st_size
        || before.st_mtime != after.st_mtime
        || before.st_mtime_nsec != after.st_mtime_nsec
        || before.st_ctime != after.st_ctime
        || before.st_ctime_nsec != after.st_ctime_nsec
    {
        return Err(ReadError::Io);
    }
    std::str::from_utf8(&bytes).map_err(|_| ReadError::Invalid)?;
    let published = PublishedSidecar {
        facts: SidecarFacts {
            size: size as u64,
            device: before.st_dev,
            inode: before.st_ino,
            modified_seconds: before.st_mtime,
            modified_nanoseconds: before.st_mtime_nsec as u32,
        },
        sha256: format!("{:x}", Sha256::digest(&bytes)),
    };
    Ok((published, bytes))
}

#[derive(Debug, PartialEq)]
pub enum ObservedBytes {
    Bytes(Vec<u8>),
    Absent,
    Changed,
}

/// Returns bounded bytes only while the candidate still matches the observation.
pub fn read_observed(
    root: &LibraryRoot,
    observation: &SidecarObservation,
) -> Result<ObservedBytes, ConfinementError> {
    let SidecarObservation::Eligible {
        location,
        facts,
        sha256,
    } = observation
    else {
        return Ok(if matches!(observation, SidecarObservation::Absent) {
            ObservedBytes::Absent
        } else {
            ObservedBytes::Changed
        });
    };
    let path =
        RelativeOriginalPath::parse(location.clone()).map_err(|_| ConfinementError::InvalidPath)?;
    let (parent, name) = parts(&path);
    let directory = root.sidecar_directory(parent)?;
    let Ok((read, bytes)) = read_candidate_bytes(directory.as_raw_fd(), name) else {
        return Ok(ObservedBytes::Changed);
    };
    Ok(if read.facts == *facts && read.sha256 == *sha256 {
        ObservedBytes::Bytes(bytes)
    } else {
        ObservedBytes::Changed
    })
}

/// Reads one Original's identity facts through its confined parent directory,
/// with the same second/nanosecond precision as Sidecar evidence.
pub fn original_facts(
    root: &LibraryRoot,
    original: &RelativeOriginalPath,
) -> Result<SidecarFacts, ConfinementError> {
    let (parent, name) = parts(original);
    let directory = root.sidecar_directory(parent)?;
    let name = CString::new(name).map_err(|_| ConfinementError::InvalidPath)?;
    let file = confinement::sidecar_open(directory.as_raw_fd(), &name)
        .map_err(|_| ConfinementError::UnsafeOpen)?;
    let facts =
        confinement::sidecar_stat(file.as_raw_fd()).map_err(|_| ConfinementError::UnsafeOpen)?;
    if facts.st_mode & libc::S_IFMT != libc::S_IFREG {
        return Err(ConfinementError::UnsafeOpen);
    }
    Ok(SidecarFacts {
        size: facts.st_size.max(0) as u64,
        device: facts.st_dev,
        inode: facts.st_ino,
        modified_seconds: facts.st_mtime,
        modified_nanoseconds: facts.st_mtime_nsec as u32,
    })
}

pub fn publish(
    root: &LibraryRoot,
    original: &RelativeOriginalPath,
    evidence: &SidecarEvidence,
    lease: &ExclusiveSaveLease,
    document: &[u8],
) -> Result<PublishedSidecar, PublishError> {
    publish_inner(root, original, evidence, lease, document, || {})
}

fn publish_inner(
    root: &LibraryRoot,
    original: &RelativeOriginalPath,
    evidence: &SidecarEvidence,
    lease: &ExclusiveSaveLease,
    document: &[u8],
    after_rename: impl FnOnce(),
) -> Result<PublishedSidecar, PublishError> {
    if document.len() > MAX_BYTES {
        return Err(PublishError::ResourceLimit("Sidecar exceeds 16 MiB".into()));
    }
    if std::str::from_utf8(document).is_err() {
        return Err(PublishError::StorageUnavailable);
    }
    let (parent, name) = parts(original);
    let kind = classify_name(name).ok_or(PublishError::Conflict)?;
    let directory = root
        .sidecar_directory(parent)
        .map_err(|_| PublishError::StorageUnavailable)?;
    let observed = observe_in(directory.as_raw_fd(), original, kind)
        .map_err(|_| PublishError::StorageUnavailable)?;
    let target_name = match (evidence, observed) {
        (SidecarEvidence::Absent, SidecarObservation::Absent) => {
            format!("{}.xmp", pairing_stem(name))
        }
        (
            SidecarEvidence::Present { facts, sha256 },
            SidecarObservation::Eligible {
                location,
                facts: current_facts,
                sha256: current_sha256,
            },
        ) if *facts == current_facts && *sha256 == current_sha256 => {
            location.rsplit('/').next().unwrap().to_owned()
        }
        (_, SidecarObservation::ResourceLimit { .. }) => {
            return Err(PublishError::ResourceLimit("Sidecar exceeds 16 MiB".into()));
        }
        (_, SidecarObservation::Unreadable { .. }) => return Err(PublishError::StorageUnavailable),
        _ => return Err(PublishError::Conflict),
    };
    let target =
        CString::new(target_name.as_str()).map_err(|_| PublishError::StorageUnavailable)?;
    let temporary = lease.staging_name()?;
    let descriptor = confinement::sidecar_create(directory.as_raw_fd(), &temporary)
        .map_err(|_| PublishError::StorageUnavailable)?;
    let mut staged = File::from(descriptor);
    let before_rename = (|| {
        staged
            .write_all(document)
            .and_then(|()| staged.sync_all())
            .map_err(|_| PublishError::StorageUnavailable)?;
        confinement::sidecar_rename(
            directory.as_raw_fd(),
            &temporary,
            &target,
            matches!(evidence, SidecarEvidence::Absent),
        )
        .map_err(|error| {
            if error.raw_os_error() == Some(libc::EEXIST) {
                PublishError::Conflict
            } else {
                PublishError::StorageUnavailable
            }
        })
    })();
    if let Err(error) = before_rename {
        let _ = confinement::sidecar_unlink(directory.as_raw_fd(), &temporary);
        return Err(error);
    }
    after_rename();
    confinement::sidecar_sync(directory.as_raw_fd()).map_err(|_| PublishError::OutcomeUnknown)?;
    let published = read_candidate(directory.as_raw_fd(), &target_name)
        .map_err(|_| PublishError::OutcomeUnknown)?;
    if published.sha256 != format!("{:x}", Sha256::digest(document)) {
        return Err(PublishError::OutcomeUnknown);
    }
    Ok(published)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::symlink,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct TempTree(PathBuf);
    impl TempTree {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "slipstream-sidecar-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            fs::write(path.join("foo.nef"), b"original bytes").unwrap();
            Self(path)
        }
        fn root(&self) -> LibraryRoot {
            LibraryRoot::open(&self.0).unwrap()
        }
        fn write(&self, name: &str, bytes: &[u8]) {
            fs::write(self.0.join(name), bytes).unwrap();
        }
        fn observe(&self) -> SidecarObservation {
            observe(&self.root(), &original(), OriginalKind::Raw).unwrap()
        }
    }
    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn original() -> RelativeOriginalPath {
        RelativeOriginalPath::parse("foo.nef").unwrap()
    }
    fn lease() -> ExclusiveSaveLease {
        ExclusiveSaveLease::from_supervisor_token(SupervisorLeaseToken("supervisor-proof".into()))
    }
    fn evidence(observation: SidecarObservation) -> SidecarEvidence {
        match observation {
            SidecarObservation::Eligible { facts, sha256, .. } => {
                SidecarEvidence::Present { facts, sha256 }
            }
            SidecarObservation::Absent => SidecarEvidence::Absent,
            other => panic!("unexpected {other:?}"),
        }
    }
    #[test]
    fn sidecar_raw_eligible_and_observed_bytes_revision_checked() {
        let tree = TempTree::new();
        tree.write("foo.xmp", b"metadata");
        let observed = tree.observe();
        assert!(matches!(observed, SidecarObservation::Eligible { .. }));
        assert_eq!(
            read_observed(&tree.root(), &observed).unwrap(),
            ObservedBytes::Bytes(b"metadata".to_vec())
        );
        tree.write("foo.xmp", b"tampered");
        assert_eq!(
            read_observed(&tree.root(), &observed).unwrap(),
            ObservedBytes::Changed
        );
        fs::remove_file(tree.0.join("foo.xmp")).unwrap();
        assert_eq!(
            read_observed(&tree.root(), &observed).unwrap(),
            ObservedBytes::Changed
        );
    }
    #[test]
    fn sidecar_absent_never_confused_with_observed_disappearance() {
        let tree = TempTree::new();
        assert_eq!(tree.observe(), SidecarObservation::Absent);
        assert_eq!(
            read_observed(&tree.root(), &SidecarObservation::Absent).unwrap(),
            ObservedBytes::Absent
        );
    }
    #[test]
    fn sidecar_jpeg_raw_precedence_and_multiple_owners() {
        let tree = TempTree::new();
        tree.write("foo.jpg", b"jpeg");
        tree.write("foo.xmp", b"xmp");
        assert!(matches!(
            observe(
                &tree.root(),
                &RelativeOriginalPath::parse("foo.jpg").unwrap(),
                OriginalKind::Jpeg
            )
            .unwrap(),
            SidecarObservation::Ineligible { .. }
        ));
        tree.write("foo.cr2", b"raw");
        assert!(
            matches!(tree.observe(), SidecarObservation::Ambiguous { candidates } if candidates == vec!["foo.xmp"])
        );
    }
    #[test]
    fn sidecar_case_distinct_candidates_ambiguous() {
        let tree = TempTree::new();
        tree.write("foo.xmp", b"one");
        tree.write("foo.XMP", b"two");
        assert!(
            matches!(tree.observe(), SidecarObservation::Ambiguous { candidates } if candidates == vec!["foo.XMP", "foo.xmp"])
        );
    }
    #[test]
    fn sidecar_symlink_directory_and_non_utf8_invalid() {
        let tree = TempTree::new();
        symlink("foo.nef", tree.0.join("foo.xmp")).unwrap();
        assert!(matches!(tree.observe(), SidecarObservation::Invalid { .. }));
        fs::remove_file(tree.0.join("foo.xmp")).unwrap();
        fs::create_dir(tree.0.join("foo.xmp")).unwrap();
        assert!(matches!(tree.observe(), SidecarObservation::Invalid { .. }));
        fs::remove_dir(tree.0.join("foo.xmp")).unwrap();
        tree.write("foo.xmp", &[0xff]);
        assert!(matches!(tree.observe(), SidecarObservation::Invalid { .. }));
    }
    #[test]
    fn sidecar_oversize_resource_limit() {
        let tree = TempTree::new();
        File::create(tree.0.join("foo.xmp"))
            .unwrap()
            .set_len(MAX_BYTES as u64 + 1)
            .unwrap();
        assert!(matches!(
            tree.observe(),
            SidecarObservation::ResourceLimit { .. }
        ));
        assert!(matches!(
            publish(
                &tree.root(),
                &original(),
                &SidecarEvidence::Absent,
                &lease(),
                &vec![0; MAX_BYTES + 1]
            ),
            Err(PublishError::ResourceLimit(_))
        ));
    }
    #[test]
    fn sidecar_mtime_preserved_in_place_change_has_new_digest() {
        let tree = TempTree::new();
        tree.write("foo.xmp", b"before");
        let before = tree.observe();
        let file = File::open(tree.0.join("foo.xmp")).unwrap();
        let modified = file.metadata().unwrap().modified().unwrap();
        tree.write("foo.xmp", b"after!");
        file.set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let after = tree.observe();
        match (before, after) {
            (
                SidecarObservation::Eligible {
                    facts: a,
                    sha256: x,
                    ..
                },
                SidecarObservation::Eligible {
                    facts: b,
                    sha256: y,
                    ..
                },
            ) => {
                assert_eq!(a, b);
                assert_ne!(x, y);
            }
            _ => panic!("not eligible"),
        }
    }
    #[test]
    fn sidecar_publish_creation_and_racing_conflict() {
        let tree = TempTree::new();
        let absent = evidence(tree.observe());
        tree.write("foo.xmp", b"racing");
        assert_eq!(
            publish(&tree.root(), &original(), &absent, &lease(), b"new"),
            Err(PublishError::Conflict)
        );
        assert_eq!(fs::read(tree.0.join("foo.xmp")).unwrap(), b"racing");
        assert_eq!(fs::read_dir(&tree.0).unwrap().count(), 2);
        fs::remove_file(tree.0.join("foo.xmp")).unwrap();
        let published = publish(&tree.root(), &original(), &absent, &lease(), b"created").unwrap();
        assert_eq!(
            published.sha256,
            format!("{:x}", Sha256::digest(b"created"))
        );
    }
    #[test]
    fn sidecar_staging_name_is_lease_bound_and_exclusive() {
        let tree = TempTree::new();
        let lease = lease();
        let absent = evidence(tree.observe());
        let temporary = lease.staging_name().unwrap();
        let descriptor = confinement::sidecar_create(
            tree.root().sidecar_directory("").unwrap().as_raw_fd(),
            &temporary,
        )
        .unwrap();
        drop(descriptor);
        let path = tree.0.join(temporary.to_str().unwrap());
        fs::write(&path, b"unrelated content").unwrap();
        assert_eq!(
            publish(&tree.root(), &original(), &absent, &lease, b"new"),
            Err(PublishError::StorageUnavailable)
        );
        assert_eq!(fs::read(path).unwrap(), b"unrelated content");
    }

    #[test]
    fn sidecar_publish_update_leaves_original_unchanged() {
        let tree = TempTree::new();
        tree.write("foo.xmp", b"old");
        let before = Sha256::digest(fs::read(tree.0.join("foo.nef")).unwrap());
        let published = publish(
            &tree.root(),
            &original(),
            &evidence(tree.observe()),
            &lease(),
            b"new document",
        )
        .unwrap();
        assert_eq!(fs::read(tree.0.join("foo.xmp")).unwrap(), b"new document");
        assert_eq!(
            published.sha256,
            format!("{:x}", Sha256::digest(b"new document"))
        );
        assert_eq!(
            before,
            Sha256::digest(fs::read(tree.0.join("foo.nef")).unwrap())
        );
    }
    #[test]
    fn sidecar_publish_preserves_case_and_rejects_stale_evidence() {
        let tree = TempTree::new();
        tree.write("foo.XMP", b"old");
        let observed = evidence(tree.observe());
        publish(&tree.root(), &original(), &observed, &lease(), b"new").unwrap();
        assert_eq!(fs::read(tree.0.join("foo.XMP")).unwrap(), b"new");
        assert!(!tree.0.join("foo.xmp").exists());
        assert_eq!(
            publish(&tree.root(), &original(), &observed, &lease(), b"stale"),
            Err(PublishError::Conflict)
        );
        assert_eq!(fs::read(tree.0.join("foo.XMP")).unwrap(), b"new");
    }

    #[test]
    fn sidecar_publish_refuses_new_owners_and_case_variant_candidates() {
        let tree = TempTree::new();
        let absent = evidence(tree.observe());
        tree.write("foo.XMP", b"external");
        assert_eq!(
            publish(&tree.root(), &original(), &absent, &lease(), b"new"),
            Err(PublishError::Conflict)
        );
        let observed = evidence(tree.observe());
        tree.write("foo.cr2", b"second raw");
        assert_eq!(
            publish(&tree.root(), &original(), &observed, &lease(), b"new"),
            Err(PublishError::Conflict)
        );
        assert_eq!(fs::read(tree.0.join("foo.XMP")).unwrap(), b"external");
        assert!(!tree.0.join("foo.xmp").exists());
    }

    #[test]
    fn sidecar_observation_uses_retained_parent_and_regular_owners() {
        let tree = TempTree::new();
        fs::create_dir(tree.0.join("nested")).unwrap();
        fs::write(tree.0.join("nested/photo.jpg"), b"jpeg").unwrap();
        fs::write(tree.0.join("nested/photo.XMP"), b"retained").unwrap();
        fs::create_dir(tree.0.join("nested/photo.nef")).unwrap();
        symlink("photo.jpg", tree.0.join("nested/photo.cr2")).unwrap();
        let root = tree.root();
        let parent = root.sidecar_directory("nested").unwrap();
        fs::rename(tree.0.join("nested"), tree.0.join("moved")).unwrap();
        fs::create_dir(tree.0.join("nested")).unwrap();
        let path = RelativeOriginalPath::parse("nested/photo.jpg").unwrap();
        assert!(matches!(
            observe_in(parent.as_raw_fd(), &path, OriginalKind::Jpeg).unwrap(),
            SidecarObservation::Eligible { location, .. } if location == "nested/photo.XMP"
        ));
    }

    #[test]
    fn sidecar_post_rename_failure_keeps_target() {
        let tree = TempTree::new();
        let lease = lease();
        tree.write("foo.xmp", b"old");
        let result = publish_inner(
            &tree.root(),
            &original(),
            &evidence(tree.observe()),
            &lease,
            b"published",
            || tree.write("foo.xmp", b"tampered"),
        );
        assert_eq!(result, Err(PublishError::OutcomeUnknown));
        assert_eq!(fs::read(tree.0.join("foo.xmp")).unwrap(), b"tampered");
        assert_eq!(fs::read_dir(&tree.0).unwrap().count(), 2);
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::*;
    use std::{fs, os::unix::fs::symlink};

    #[test]
    fn sidecar_sole_jpeg_nested_publish_and_parent_symlink_refusal() {
        let base = std::env::temp_dir().join(format!(
            "slipstream-sidecar-boundary-{}",
            std::process::id()
        ));
        fs::create_dir(&base).unwrap();
        fs::create_dir(base.join("nested")).unwrap();
        fs::write(base.join("nested/foo.jpg"), b"original").unwrap();
        let root = LibraryRoot::open(&base).unwrap();
        let original = RelativeOriginalPath::parse("nested/foo.jpg").unwrap();
        assert_eq!(
            observe(&root, &original, OriginalKind::Jpeg).unwrap(),
            SidecarObservation::Absent
        );
        let lease = ExclusiveSaveLease::from_supervisor_token(SupervisorLeaseToken("proof".into()));
        publish(
            &root,
            &original,
            &SidecarEvidence::Absent,
            &lease,
            b"nested metadata",
        )
        .unwrap();
        assert!(
            matches!(observe(&root, &original, OriginalKind::Jpeg).unwrap(), SidecarObservation::Eligible { location, .. } if location == "nested/foo.xmp")
        );
        symlink("nested", base.join("link")).unwrap();
        let escaped = RelativeOriginalPath::parse("link/foo.jpg").unwrap();
        assert!(observe(&root, &escaped, OriginalKind::Jpeg).is_err());
        assert_eq!(
            publish(&root, &escaped, &SidecarEvidence::Absent, &lease, b"unsafe"),
            Err(PublishError::StorageUnavailable)
        );
        assert_eq!(
            fs::read(base.join("nested/foo.xmp")).unwrap(),
            b"nested metadata"
        );
        root.close();
        assert!(matches!(
            observe(&root, &original, OriginalKind::Jpeg),
            Err(ConfinementError::Closed)
        ));
        fs::remove_dir_all(base).unwrap();
    }
}
