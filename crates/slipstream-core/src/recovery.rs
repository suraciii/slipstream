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
    hash_targets.sort_by(|left, right| {
        left.path
            .as_str()
            .as_bytes()
            .cmp(right.path.as_str().as_bytes())
    });
    hash_targets.dedup_by(|left, right| left.path.as_str() == right.path.as_str());

    progress.hash_total = hash_targets.len() as u64;
    let mut hashed: HashMap<String, HashedFile> = HashMap::new();
    let mut failed_kinds: Vec<crate::OriginalKind> = Vec::new();
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
                failed_kinds.push(target.kind);
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
        if originals.len() != 1 || files.len() != 1 || failed_kinds.contains(&originals[0].kind) {
            // Ambiguous or incomplete evidence: preserve records untouched.
            // An unreadable candidate of the same kind could hold the same
            // content, so a failed hash must not become proof of uniqueness.
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
    let relocating_ids: std::collections::HashSet<String> = relocations.values().cloned().collect();
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
            && discovered.iter().any(|item| {
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

/// One unavailable Photo listed by the bounded recovery review entry.
/// Remembers the folder, filename, format, and retained decisions so the
/// Photographer can judge a mapping without the file.
pub struct UnavailablePhotoRecord {
    pub original_id: String,
    pub photo_id: String,
    pub relative_path: String,
    pub kind: OriginalKind,
    pub rating: u8,
    pub selection_state: crate::SelectionState,
    /// Persisted digest when the Original was enrolled before it went
    /// missing. Manual mappings for fingerprinted Originals are verified
    /// against the candidate's content; the rest cannot be verified and
    /// require explicit confirmation.
    pub fingerprint: Option<String>,
    pub album_count: u64,
}

/// Everything the manual planner needs from one consistent persisted read.
pub struct RecoverySurvey {
    pub unavailable: Vec<UnavailablePhotoRecord>,
    /// Photo IDs referenced by any Album membership.
    pub referenced_photo_ids: std::collections::HashSet<String>,
}

/// The occupying record an explicit retire-and-bind may replace.
pub struct RetireSummary {
    pub photo_id: String,
    pub original_id: String,
    pub location: String,
}

/// What one proposed mapping found.
pub enum ManualOutcome {
    /// A readable candidate exists with the expected kind.
    Matched,
    /// The candidate's digest differs from the persisted fingerprint.
    ContentMismatch,
    /// No file exists at the destination.
    Missing,
    /// The destination file exists but carries an unsupported kind for this
    /// Original.
    KindMismatch,
    /// The destination cannot be read, so content cannot be judged.
    Unreadable,
    /// Another persisted Original owns the destination. `retire` describes
    /// the record an explicit retire-and-bind may replace; `None` means the
    /// occupant has independent user state and both records are preserved.
    Occupied { retire: Option<RetireSummary> },
    /// Another mapping in the same proposal already targets this Location.
    Colliding,
}

/// One inspectable proposed mapping for an unavailable Original.
pub struct ManualProposal {
    pub original_id: String,
    pub photo_id: String,
    pub from_location: String,
    pub to_location: String,
    pub kind: OriginalKind,
    pub outcome: ManualOutcome,
    /// True when a persisted fingerprint matched the candidate digest.
    /// False means historical content could not be verified and the
    /// confirmation must say so.
    pub verified: bool,
}

/// One confirmed mapping the persistence owner revalidates and commits.
/// `facts` were observed through a confined descriptor outside the write
/// transaction; the transaction trusts them and the next scan re-checks the
/// revision.
#[derive(Clone)]
pub struct RequestedRelocation {
    pub original_id: String,
    pub to_location: String,
    pub facts: crate::OriginalFacts,
    /// Explicitly retire an otherwise unreferenced default-state occupying
    /// Photo and bind its Location to the relocated Original. No filesystem
    /// file is deleted.
    pub retire_destination: bool,
}

/// Committed result of one manual relocation batch.
pub struct AppliedRelocations {
    pub relocated_photos: u64,
    pub unavailable_photos: u64,
}

/// Validates one server-relative Library Location prefix. The empty prefix
/// addresses the Library root. Component rules match
/// [`crate::RelativeOriginalPath`]; a trailing separator is not accepted.
pub fn parse_location_prefix(value: &str) -> Result<(), crate::domain::PathError> {
    if value.is_empty() {
        return Ok(());
    }
    if value.starts_with('/')
        || value.ends_with('/')
        || value.contains('\0')
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(crate::domain::PathError);
    }
    Ok(())
}

/// True when `location` names something strictly inside `prefix`. The empty
/// prefix contains every Location.
fn under_prefix(location: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        return true;
    }
    location.len() > prefix.len() + 1
        && location.starts_with(prefix)
        && location.as_bytes()[prefix.len()] == b'/'
}

/// Joins a destination prefix with one suffix that begins with `/`. An empty
/// prefix addresses the Library root.
fn join_destination(prefix: &str, suffix: &str) -> String {
    debug_assert!(suffix.starts_with('/'));
    if prefix.is_empty() {
        suffix[1..].to_owned()
    } else {
        format!("{prefix}{suffix}")
    }
}

/// Evaluates one candidate destination for one unavailable record.
fn evaluate_candidate(
    root: &crate::LibraryRoot,
    native_work: &NativeWorkBudget,
    record: &UnavailablePhotoRecord,
    occupant: Option<&OriginalRecord>,
    occupant_photo: Option<&crate::PhotoRecord>,
    referenced: &std::collections::HashSet<String>,
    to_location: &str,
) -> ManualOutcome {
    // Kind comes from the destination filename, exactly as the scanner
    // classifies it.
    let filename = to_location.rsplit('/').next().unwrap_or(to_location);
    let destination_kind = crate::identity::classify_name(filename);
    if destination_kind != Some(record.kind) {
        return ManualOutcome::KindMismatch;
    }
    let path = match crate::RelativeOriginalPath::parse(to_location.to_owned()) {
        Ok(path) => path,
        Err(_) => return ManualOutcome::Missing,
    };
    let capability = match root.original(path) {
        Ok(capability) => capability,
        Err(_) => return ManualOutcome::Missing,
    };
    // A persisted fingerprint is verified against the destination content;
    // a match falls through so the outcome below reports the destination
    // state, and any mismatch or read failure refuses the mapping.
    if let Some(expected) = record.fingerprint.as_deref() {
        let permit = native_work.acquire();
        let digest = capability.digest_file();
        drop(permit);
        match digest {
            Ok(checked) if checked.digest == expected => {}
            Ok(_) => return ManualOutcome::ContentMismatch,
            Err(_) => return ManualOutcome::Unreadable,
        }
    }
    match occupant {
        // A destination owned by the record itself is an in-place restore;
        // there is no separate occupant to retire or conflict with.
        Some(owner) if owner.id != record.original_id => {
            let retire = match occupant_photo {
                Some(photo)
                    if photo.rating == 0
                        && photo.selection_state == crate::SelectionState::Undecided
                        && !referenced.contains(&photo.id) =>
                {
                    Some(RetireSummary {
                        photo_id: photo.id.clone(),
                        original_id: owner.id.clone(),
                        location: to_location.to_owned(),
                    })
                }
                _ => None,
            };
            ManualOutcome::Occupied { retire }
        }
        _ => ManualOutcome::Matched,
    }
}

/// Proposes one batch of relocations for unavailable Originals whose
/// remembered Locations sit under `old_prefix`, mapped onto `new_prefix`
/// with the same relative suffix. Filesystem evidence is gathered through
/// confined read-only descriptors; nothing is written.
pub fn plan_manual_relocations(
    root: &crate::LibraryRoot,
    native_work: &NativeWorkBudget,
    survey: &RecoverySurvey,
    snapshot: &ScanSnapshot,
    old_prefix: &str,
    new_prefix: &str,
) -> Result<Vec<ManualProposal>, crate::domain::PathError> {
    parse_location_prefix(old_prefix)?;
    parse_location_prefix(new_prefix)?;

    let originals_by_path: HashMap<&str, &OriginalRecord> = snapshot
        .originals
        .iter()
        .map(|original| (original.relative_path.as_str(), original))
        .collect();
    let photos_by_original: HashMap<&str, &crate::PhotoRecord> = snapshot
        .photos
        .iter()
        .map(|photo| (photo.original_id.as_str(), photo))
        .collect();

    let mut seen_destinations: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut proposals = Vec::new();
    for record in &survey.unavailable {
        if !under_prefix(&record.relative_path, old_prefix) {
            continue;
        }
        let suffix = &record.relative_path[old_prefix.len()..];
        let to_location = join_destination(new_prefix, suffix);
        let (occupant, occupant_photo) = match originals_by_path.get(to_location.as_str()) {
            Some(owner) => {
                let photo = photos_by_original.get(owner.id.as_str()).copied();
                (Some(*owner), photo)
            }
            None => (None, None),
        };
        let outcome = if !seen_destinations.insert(to_location.clone()) {
            ManualOutcome::Colliding
        } else {
            evaluate_candidate(
                root,
                native_work,
                record,
                occupant,
                occupant_photo,
                &survey.referenced_photo_ids,
                &to_location,
            )
        };
        // Verification is exactly "the destination content matched the
        // persisted fingerprint": every outcome below proved the digest,
        // and rows without a fingerprint stay unverified.
        let verified = record.fingerprint.is_some()
            && (matches!(outcome, ManualOutcome::Matched)
                || matches!(outcome, ManualOutcome::Occupied { .. }));
        proposals.push(ManualProposal {
            original_id: record.original_id.clone(),
            photo_id: record.photo_id.clone(),
            from_location: record.relative_path.clone(),
            to_location,
            kind: record.kind,
            outcome,
            verified,
        });
    }
    Ok(proposals)
}

/// Proposes one mapping for a single unavailable Original, for renamed or
/// split files that a folder-prefix batch cannot express.
pub fn plan_single_relocation(
    root: &crate::LibraryRoot,
    native_work: &NativeWorkBudget,
    survey: &RecoverySurvey,
    snapshot: &ScanSnapshot,
    original_id: &str,
    to_location: &str,
) -> Result<ManualProposal, crate::domain::PathError> {
    crate::RelativeOriginalPath::parse(to_location.to_owned())?;
    let Some(record) = survey
        .unavailable
        .iter()
        .find(|record| record.original_id == original_id)
    else {
        return Err(crate::domain::PathError);
    };
    let occupant = snapshot
        .originals
        .iter()
        .find(|original| original.relative_path.as_str() == to_location);
    let occupant_photo = occupant.and_then(|owner| {
        snapshot
            .photos
            .iter()
            .find(|photo| photo.original_id == owner.id)
    });
    let outcome = evaluate_candidate(
        root,
        native_work,
        record,
        occupant,
        occupant_photo,
        &survey.referenced_photo_ids,
        to_location,
    );
    let verified = record.fingerprint.is_some()
        && (matches!(outcome, ManualOutcome::Matched)
            || matches!(outcome, ManualOutcome::Occupied { .. }));
    Ok(ManualProposal {
        original_id: record.original_id.clone(),
        photo_id: record.photo_id.clone(),
        from_location: record.relative_path.clone(),
        to_location: to_location.to_owned(),
        kind: record.kind,
        outcome,
        verified,
    })
}
