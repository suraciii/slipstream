//! Location recovery planning for one complete scan.
//!
//! The planner runs in the scanner thread between Capture Time inspection
//! and state-store application. It reads Original Files only through confined
//! descriptors, computes complete-content SHA-256 digests where evidence is
//! needed, and emits relocation decisions that the persistence transaction
//! revalidates. It never writes user state and never modifies Originals.

use crate::{
    NativeWorkBudget, OriginalKind, OriginalRecord, ScanSnapshot,
    domain::DiscoveredOriginal,
    persistence::{DiscoveredFingerprint, ScanRecoveryPlan},
};
use sha2::{Digest as _, Sha256};
use std::collections::HashMap;

/// Minimum number of fact-changed files that triggers hashing for
/// location-swap detection even when nothing is missing.
const SWAP_DETECTION_THRESHOLD: usize = 2;

/// Progress counters the planner reports for the recovering phase.
#[derive(Default)]
pub struct RecoveryProgress {
    pub hashed: u64,
    pub hash_total: u64,
    pub failed_hashes: u64,
}

/// Plans relocations and fresh fingerprints for one scan.
///
/// `persisted_fingerprints` must contain every stored fingerprint for the
/// Originals whose identity could participate: the missing Originals and the
/// owners of fact-changed Locations.
pub fn plan_recovery(
    root: &crate::LibraryRoot,
    native_work: &NativeWorkBudget,
    discovered: &[DiscoveredOriginal],
    previous: &ScanSnapshot,
    persisted_fingerprints: &[crate::OriginalFingerprint],
    progress: &mut RecoveryProgress,
) -> ScanRecoveryPlan {
    let mut discovered_by_path = HashMap::with_capacity(discovered.len());
    for original in discovered {
        discovered_by_path.insert(original.path.as_str().to_owned(), original);
    }

    let mut fingerprints = HashMap::with_capacity(persisted_fingerprints.len());
    for fingerprint in persisted_fingerprints {
        fingerprints.insert(fingerprint.original_id.clone(), fingerprint);
    }

    // Classify every persisted Original against the complete discovery.
    let mut unchanged_paths = std::collections::HashSet::new();
    let mut changed_originals = Vec::new();
    let mut missing_originals = Vec::new();
    for original in &previous.originals {
        match discovered_by_path.get(original.relative_path.as_str()) {
            Some(discovered) => {
                if discovered.facts.size == original.facts.size
                    && discovered.facts.mtime_ms == original.facts.mtime_ms
                {
                    unchanged_paths.insert(original.relative_path.as_str().to_owned());
                } else {
                    changed_originals.push(original);
                }
            }
            None => missing_originals.push(original),
        }
    }

    let missing_with_evidence = missing_originals
        .iter()
        .filter(|original| fingerprints.contains_key(&original.id))
        .count();

    // Hash only where evidence can change an identity decision: candidate
    // Locations for missing Originals, and fact-changed files for swap
    // detection. Everything else is enrolled by the background worker.
    let mut hash_targets: Vec<&DiscoveredOriginal> = Vec::new();
    let need_new_paths = missing_with_evidence > 0;
    let need_changed =
        missing_with_evidence > 0 || changed_originals.len() >= SWAP_DETECTION_THRESHOLD;
    if need_new_paths {
        for original in discovered {
            if !previous
                .originals
                .iter()
                .any(|persisted| persisted.relative_path.as_str() == original.path.as_str())
            {
                hash_targets.push(original);
            }
        }
    }
    if need_changed {
        for original in &changed_originals {
            if let Some(discovered) = discovered_by_path.get(original.relative_path.as_str()) {
                hash_targets.push(discovered);
            }
        }
    }
    hash_targets.sort_by(|left, right| left.path.as_str().as_bytes().cmp(right.path.as_str().as_bytes()));
    hash_targets.dedup_by(|left, right| left.path.as_str() == right.path.as_str());

    progress.hash_total = hash_targets.len() as u64;
    let mut hashed: HashMap<String, HashedFile> = HashMap::new();
    for target in hash_targets {
        let permit = native_work.acquire();
        let result = root
            .original(target.path.clone())
            .and_then(|capability| capability.digest_file());
        drop(permit);
        match result {
            Ok(checked) => {
                hashed.insert(
                    target.path.as_str().to_owned(),
                    HashedFile {
                        digest: checked.digest,
                        kind: target.kind,
                        size: checked.facts.size,
                        owner_keeps_bytes: false,
                    },
                );
            }
            Err(_) => {
                progress.failed_hashes += 1;
            }
        }
        progress.hashed += 1;
    }

    // A fact-changed file whose digest still equals its owner's fingerprint
    // is a re-save of the same bytes: the owner keeps it as a revision, and
    // the file must not become another Original's target.
    for original in &changed_originals {
        if let Some(fingerprint) = fingerprints.get(&original.id)
            && let Some(file) = hashed.get_mut(original.relative_path.as_str())
            && file.digest == fingerprint.digest
        {
            file.owner_keeps_bytes = true;
        }
    }

    // An Original is eligible for relocation when its bytes are not at its
    // remembered Location: it is missing, or its Location now provably
    // carries other bytes. An unchanged Location or a failed hash keeps the
    // owner in place as a revision.
    let mut eligible: Vec<&OriginalRecord> = Vec::new();
    for original in &previous.originals {
        let eligible_now = match discovered_by_path.get(original.relative_path.as_str()) {
            None => true,
            Some(_) => {
                !unchanged_paths.contains(original.relative_path.as_str())
                    && hashed
                        .get(original.relative_path.as_str())
                        .is_some_and(|file| !file.owner_keeps_bytes)
            }
        };
        if eligible_now {
            eligible.push(original);
        }
    }

    // Target files: hashed Locations whose owner (if any) does not keep its
    // own bytes there.
    let mut relocations = HashMap::new();
    let mut by_digest: HashMap<String, (Vec<&OriginalRecord>, Vec<String>)> = HashMap::new();
    for original in &eligible {
        if let Some(fingerprint) = fingerprints.get(&original.id) {
            by_digest
                .entry(fingerprint.digest.clone())
                .or_default()
                .0
                .push(original);
        }
    }
    for (path, file) in &hashed {
        if file.owner_keeps_bytes {
            continue;
        }
        by_digest
            .entry(file.digest.clone())
            .or_default()
            .1
            .push(path.clone());
    }
    for (_digest, originals, files) in by_digest
        .iter()
        .map(|(digest, (originals, files))| (digest, originals, files))
    {
        if originals.len() != 1 || files.len() != 1 {
            // Ambiguous or incomplete evidence: preserve records untouched.
            continue;
        }
        let original = originals[0];
        let path = &files[0];
        let fingerprint = &fingerprints[&original.id];
        let file = &hashed[path];
        if file.kind == original.kind && fingerprint.size == file.size {
            relocations.insert(path.clone(), original.id.clone());
        }
    }

    // Global consistency: a relocation may land on a Location owned by
    // another Original only when that owner also relocates in this plan.
    let relocating_ids: std::collections::HashSet<String> =
        relocations.values().cloned().collect();
    let owner_by_path: HashMap<&str, &OriginalRecord> = previous
        .originals
        .iter()
        .map(|original| (original.relative_path.as_str(), original))
        .collect();
    let blocked_targets: Vec<String> = relocations
        .keys()
        .filter(|path| match owner_by_path.get(path.as_str()) {
            Some(owner) => !relocating_ids.contains(&owner.id),
            None => false,
        })
        .cloned()
        .collect();
    for path in blocked_targets {
        relocations.remove(&path);
    }

    ScanRecoveryPlan {
        fingerprints: hashed
            .into_iter()
            .map(|(path, file)| DiscoveredFingerprint {
                path,
                digest: file.digest,
            })
            .collect(),
        relocations,
    }
}

struct HashedFile {
    digest: String,
    kind: OriginalKind,
    size: u64,
    owner_keeps_bytes: bool,
}

/// Computes the digest of in-memory bytes. Used by tests and mirrors the
/// streaming reader's algorithm.
pub fn digest_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The persisted Originals whose fingerprints a scan needs: every missing
/// Original plus every owner of a fact-changed Location.
pub fn evidence_original_ids(
    discovered: &[DiscoveredOriginal],
    previous: &ScanSnapshot,
) -> Vec<String> {
    let discovered_by_path: std::collections::HashSet<&str> = discovered
        .iter()
        .map(|original| original.path.as_str())
        .collect();
    let mut ids = Vec::new();
    for original in &previous.originals {
        let present = discovered_by_path.contains(original.relative_path.as_str());
        let changed = present
            && discovered
                .iter()
                .any(|item| {
                    item.path.as_str() == original.relative_path.as_str()
                        && (item.facts.size != original.facts.size
                            || item.facts.mtime_ms != original.facts.mtime_ms)
                });
        if !present || changed {
            ids.push(original.id.clone());
        }
    }
    ids
}

