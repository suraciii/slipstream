//! Host-global instance claim ownership shared by processing executors.

use crate::{
    backend::secure_directory,
    protocol::{Config, ErrorCode, REQUEST_BYTES},
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

/// The durable identity a claim records. Completeness comes from the root's registry.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Claim {
    pub version: u8,
    pub root: String,
}

/// An exclusive claim. A new claim is a lease until the caller disarms it.
pub(crate) struct InstanceClaim {
    lease: Option<PathBuf>,
    file: Option<File>,
}

impl InstanceClaim {
    /// Keep the claim for the executor's lifetime after durable initialization.
    pub(crate) fn take(mut self) -> File {
        self.file.take().expect("claim descriptor")
    }
}

impl Drop for InstanceClaim {
    fn drop(&mut self) {
        if let (Some(path), Some(_)) = (self.lease.take(), self.file.as_ref()) {
            let _ = fs::remove_file(path);
        }
    }
}

/// Acquire an exact-root claim. An existing claim without a registry is quarantined.
/// A fresh claim is removed after a post-lock write failure; a failed pre-lock
/// acquisition leaves the claim to the winner of the race.
pub(crate) fn hold_claim(
    path: &Path,
    namespace: &Path,
    root: &str,
) -> Result<InstanceClaim, ErrorCode> {
    let (mut file, fresh) = match OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => (file, true),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (
            OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
                .open(path)
                .map_err(|_| ErrorCode::Uncertain)?,
            false,
        ),
        Err(_) => return Err(ErrorCode::Uncertain),
    };
    let metadata = file.metadata().map_err(|_| ErrorCode::Uncertain)?;
    // The production launcher runs as root; focused tests use the invoking UID.
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.nlink() != 1
        || metadata.mode() & 0o077 != 0
        || metadata.len() > REQUEST_BYTES as u64
    {
        return Err(ErrorCode::Uncertain);
    }
    // SAFETY: this exact open inode is retained for the owner's entire lifetime.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(ErrorCode::Busy);
    }
    if fresh {
        let write = (|| {
            let bytes = serde_json::to_vec(&Claim {
                version: 1,
                root: root.to_owned(),
            })
            .map_err(|_| ErrorCode::Uncertain)?;
            if bytes.len() > REQUEST_BYTES {
                return Err(ErrorCode::Uncertain);
            }
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|_| ErrorCode::Uncertain)?;
            File::open(namespace)
                .and_then(|file| file.sync_all())
                .map_err(|_| ErrorCode::Uncertain)?;
            Ok(())
        })();
        if let Err(error) = write {
            // Only the flock holder may remove this freshly created inode.
            let _ = fs::remove_file(path);
            return Err(error);
        }
    } else {
        let mut bytes = Vec::new();
        (&mut file)
            .take(REQUEST_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ErrorCode::Uncertain)?;
        if bytes.len() > REQUEST_BYTES {
            return Err(ErrorCode::Uncertain);
        }
        let claim: Claim = serde_json::from_slice(&bytes).map_err(|_| ErrorCode::Uncertain)?;
        if claim.version != 1
            || claim.root != root
            || !Path::new(root)
                .join("registry.json")
                .try_exists()
                .map_err(|_| ErrorCode::Uncertain)?
        {
            return Err(ErrorCode::Uncertain);
        }
    }
    Ok(InstanceClaim {
        lease: fresh.then(|| path.to_owned()),
        file: Some(file),
    })
}

pub(crate) fn claim_instance(config: &Config) -> Result<InstanceClaim, ErrorCode> {
    let namespace = Path::new("/var/lib/slipstream-processing/instances");
    for path in [Path::new("/var/lib/slipstream-processing"), namespace] {
        if !path.try_exists().map_err(|_| ErrorCode::Uncertain)? {
            fs::DirBuilder::new()
                .mode(0o700)
                .create(path)
                .map_err(|_| ErrorCode::Uncertain)?;
            File::open(path.parent().ok_or(ErrorCode::Uncertain)?)
                .and_then(|directory| directory.sync_all())
                .map_err(|_| ErrorCode::Uncertain)?;
        }
        secure_directory(path, 0)?;
    }
    hold_claim(
        &namespace.join(format!("{}.claim", config.instance)),
        namespace,
        &config.root,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::DirBuilderExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_CLAIM_DIR: AtomicU64 = AtomicU64::new(0);
    /// A scratch claim namespace outside the fixed host path, so the claim
    /// semantics run under any CI UID.
    fn claim_dir(tag: &str) -> std::path::PathBuf {
        let dir: std::path::PathBuf = std::env::temp_dir().join(format!(
            "slipstream-claim-{tag}-{}-{}",
            std::process::id(),
            NEXT_CLAIM_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::DirBuilder::new()
            .mode(0o700)
            .recursive(true)
            .create(&dir)
            .unwrap();
        dir
    }

    /// Claims hold an exclusive descriptor, so assertions compare outcomes.
    fn claim_error(claim: Result<InstanceClaim, ErrorCode>) -> ErrorCode {
        claim.err().unwrap()
    }

    fn foreign_claim_bytes(root: &str) -> Vec<u8> {
        serde_json::to_vec(&Claim {
            version: 2,
            root: root.to_owned(),
        })
        .unwrap()
    }

    fn write_claim_file(path: &Path, bytes: &[u8], mode: u32) {
        use std::os::unix::fs::OpenOptionsExt;
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode)
            .open(path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
    }

    #[test]
    fn a_claim_without_an_initialized_root_stays_quarantined_until_the_registry_exists() {
        let dir = claim_dir("quarantine");
        let path = dir.join("instance.claim");
        let root = dir.display().to_string();
        // Residue of a crash between claiming and the durable journal: a
        // claim file with no live holder and no registry. The lease of a
        // graceful failed start removes itself, so this state is written by
        // hand the way the dead process left it.
        write_claim_file(
            &path,
            &serde_json::to_vec(&Claim {
                version: 1,
                root: root.clone(),
            })
            .unwrap(),
            0o600,
        );
        assert!(!dir.join("registry.json").try_exists().unwrap());
        // The claim alone proves exclusivity, not completeness, so the
        // registry-less state is never adopted.
        assert_eq!(
            claim_error(hold_claim(&path, &dir, &root)),
            ErrorCode::Uncertain
        );
        // Once the root is initialized, the same claim is adoptable as a
        // non-lease that a failure here can never remove.
        fs::File::create(dir.join("registry.json")).unwrap();
        let adopted = hold_claim(&path, &dir, &root).unwrap();
        // Adoption keeps the recorded identity; it never rewrites the claim.
        let claim: Claim = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(claim.version, 1);
        assert_eq!(claim.root, root);
        drop(adopted);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_held_claim_stays_busy() {
        let dir = claim_dir("busy");
        let path = dir.join("instance.claim");
        let held = hold_claim(&path, &dir, "/var/lib/slipstream-processing/a").unwrap();
        // Losing the create-to-flock race is a Busy refusal that leaves the
        // file to the live holder; it is never removed by the loser.
        assert_eq!(
            claim_error(hold_claim(&path, &dir, "/var/lib/slipstream-processing/a")),
            ErrorCode::Busy
        );
        assert!(path.try_exists().unwrap());
        drop(held);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn foreign_root_and_foreign_version_claims_stay_refused() {
        let dir = claim_dir("foreign");
        let path = dir.join("instance.claim");
        // A claim written for a different root is never adoptable.
        write_claim_file(
            &path,
            &serde_json::to_vec(&Claim {
                version: 1,
                root: "/var/lib/slipstream-processing/other".to_owned(),
            })
            .unwrap(),
            0o600,
        );
        assert_eq!(
            claim_error(hold_claim(&path, &dir, "/var/lib/slipstream-processing/a")),
            ErrorCode::Uncertain
        );
        fs::remove_file(&path).unwrap();
        write_claim_file(
            &path,
            &foreign_claim_bytes("/var/lib/slipstream-processing/a"),
            0o600,
        );
        assert_eq!(
            claim_error(hold_claim(&path, &dir, "/var/lib/slipstream-processing/a")),
            ErrorCode::Uncertain
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn nonconforming_claim_files_stay_refused() {
        let dir = claim_dir("nonconforming");
        let bytes = serde_json::to_vec(&Claim {
            version: 1,
            root: "/var/lib/slipstream-processing/a".to_owned(),
        })
        .unwrap();
        // A group-readable claim file fails the private-mode check.
        let path = dir.join("loose.claim");
        write_claim_file(&path, &bytes, 0o644);
        assert_eq!(
            claim_error(hold_claim(&path, &dir, "/var/lib/slipstream-processing/a")),
            ErrorCode::Uncertain
        );
        // A symlinked claim path fails the no-follow check.
        let target = dir.join("target.claim");
        write_claim_file(&target, &bytes, 0o600);
        let link = dir.join("link.claim");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert_eq!(
            claim_error(hold_claim(&link, &dir, "/var/lib/slipstream-processing/a")),
            ErrorCode::Uncertain
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
