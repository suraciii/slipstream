//! Location recovery planning for one complete scan.

use crate::{
    NativeWorkBudget, OriginalKind, OriginalRecord, PendingFolder, ScanSnapshot,
    domain::DiscoveredOriginal,
    persistence::{DiscoveredFingerprint, ScanRecoveryPlan, ScanRelocationSource},
    recovery::{RecoveryOptions, RecoveryPlanInput, RecoveryProgress, RecoveryReporter},
};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Minimum number of fact-changed files that triggers hashing for
/// location-swap detection even when nothing is missing.
const SWAP_DETECTION_THRESHOLD: usize = 2;

/// Plans relocations and fresh fingerprints for one scan.
///
/// `persisted_fingerprints` must contain every stored fingerprint for the
/// Originals whose identity could participate: the missing Originals and the
/// owners of fact-changed Locations.
///
/// `excluded_original_ids` names Originals whose Photo was confirmed
/// permanently deleted. They are not relocation candidates: their bytes were
/// deleted, and a file that later occupies their reviewed Location is a new
/// Original instead of a recovered identity.
///
/// Photos the Photographer moved into Trash are excluded as well, through
/// [`ScanSnapshot`]'s removed markers: their Originals keep the Trash Restore
/// contract and never follow a scan-discovered copy.
pub fn plan_recovery(
    root: &crate::LibraryRoot,
    native_work: &NativeWorkBudget,
    discovered: &[DiscoveredOriginal],
    previous: &ScanSnapshot,
    persisted_fingerprints: &[crate::OriginalFingerprint],
    excluded_original_ids: &std::collections::HashSet<String>,
    reporter: &mut dyn RecoveryReporter,
) -> ScanRecoveryPlan {
    let options = RecoveryOptions::full();
    plan_recovery_with_options(
        RecoveryPlanInput {
            root,
            native_work,
            discovered,
            previous,
            persisted_fingerprints,
            excluded_original_ids,
            options: &options,
        },
        reporter,
    )
}

/// Plans recovery with an explicit verification scope and read budget.
pub fn plan_recovery_with_options(
    input: RecoveryPlanInput<'_>,
    reporter: &mut dyn RecoveryReporter,
) -> ScanRecoveryPlan {
    let RecoveryPlanInput {
        root,
        native_work,
        discovered,
        previous,
        persisted_fingerprints,
        excluded_original_ids,
        options,
    } = input;
    let mut progress = RecoveryProgress::default();
    let mut discovered_by_path = HashMap::with_capacity(discovered.len());
    for original in discovered {
        discovered_by_path.insert(original.path.as_str().to_owned(), original);
    }

    let mut fingerprints = HashMap::with_capacity(persisted_fingerprints.len());
    for fingerprint in persisted_fingerprints {
        fingerprints.insert(fingerprint.original_id.clone(), fingerprint);
    }

    // Classify every persisted Original against the complete discovery. The
    // persisted-path index keeps the whole classification linear; a scan of
    // a large Library must not rescan every persisted Original per file.
    let previous_paths: HashSet<&str> = previous
        .originals
        .iter()
        .map(|original| original.relative_path.as_str())
        .collect();
    let previous_pending_changed: HashSet<&str> = previous
        .pending_changed_paths
        .iter()
        .map(String::as_str)
        .collect();
    let mut unchanged_paths = std::collections::HashSet::new();
    let mut changed_originals = Vec::new();
    let mut missing_originals = Vec::new();
    for original in &previous.originals {
        if excluded_original_ids.contains(&original.id) {
            continue;
        }
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

    // A candidate can relocate only when its kind and size equal the
    // remembered content's kind and size. Every persisted evidence
    // fingerprint is therefore a possible relocation source shape.
    let recovery_shapes: HashSet<(OriginalKind, u64)> = previous
        .originals
        .iter()
        .filter_map(|original| {
            fingerprints
                .get(&original.id)
                .map(|fingerprint| (original.kind, fingerprint.size))
        })
        .collect();

    // Hash only where evidence can change an identity decision: new
    // Locations with an evidence shape, and fact-changed files for swap
    // detection. Everything else is enrolled by the background worker.
    let mut hash_targets: Vec<&DiscoveredOriginal> = Vec::new();
    let has_recovery_shapes = !recovery_shapes.is_empty();
    let need_changed =
        missing_with_evidence > 0 || changed_originals.len() >= SWAP_DETECTION_THRESHOLD;
    if has_recovery_shapes || !previous_pending_changed.is_empty() {
        for original in discovered {
            let path = original.path.as_str();
            let was_pending =
                previous_pending_changed.contains(path) && !unchanged_paths.contains(path);
            let is_new_candidate = has_recovery_shapes
                && !previous_paths.contains(path)
                && (original.error_category.is_some()
                    || recovery_shapes.contains(&(original.kind, original.facts.size)));
            if was_pending || is_new_candidate {
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

    let candidate_bytes = hash_targets.iter().fold(0_u64, |total, target| {
        total.saturating_add(target.facts.size)
    });
    let mut deferred_paths: HashSet<String> = HashSet::new();
    let mut blocked_shapes: HashSet<(OriginalKind, u64)> = HashSet::new();
    let mut blocked_kinds: Vec<OriginalKind> = Vec::new();
    let mut targets_to_hash: Vec<&DiscoveredOriginal> = Vec::new();
    if let Some(scope) = options.verification_scope.as_deref() {
        let scope_shapes: HashSet<(OriginalKind, u64)> = hash_targets
            .iter()
            .filter(|target| {
                target.error_category.is_none() && folder_contains(target.path.as_str(), scope)
            })
            .map(|target| (target.kind, target.facts.size))
            .collect();
        for target in &hash_targets {
            let under_scope = folder_contains(target.path.as_str(), scope);
            let should_hash = if target.error_category.is_some() {
                under_scope
            } else {
                under_scope || scope_shapes.contains(&(target.kind, target.facts.size))
            };
            if should_hash {
                targets_to_hash.push(target);
                continue;
            }
            deferred_paths.insert(target.path.as_str().to_owned());
            if target.error_category.is_some() {
                blocked_kinds.push(target.kind);
            } else {
                blocked_shapes.insert((target.kind, target.facts.size));
            }
        }
    } else if options
        .automatic_verification_budget
        .is_some_and(|budget| candidate_bytes > budget)
    {
        for target in &hash_targets {
            deferred_paths.insert(target.path.as_str().to_owned());
            if target.error_category.is_some() {
                blocked_kinds.push(target.kind);
            } else {
                blocked_shapes.insert((target.kind, target.facts.size));
            }
        }
    } else {
        targets_to_hash = hash_targets;
    }

    progress.hash_total = targets_to_hash.len() as u64;
    reporter.recovery_progress(progress);
    let mut hashed: HashMap<String, HashedFile> = HashMap::new();
    let mut failed_kinds: Vec<crate::OriginalKind> = blocked_kinds;
    for target in targets_to_hash {
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
                deferred_paths.insert(target.path.as_str().to_owned());
            }
        }
        progress.hashed += 1;
        reporter.recovery_progress(progress);
    }
    reporter.recovery_progress(progress);

    let mut pending_by_folder: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    for path in &deferred_paths {
        let Some(original) = discovered_by_path.get(path.as_str()) else {
            continue;
        };
        let entry = pending_by_folder
            .entry(parent_folder(path.as_str()))
            .or_insert((0, 0));
        entry.0 += 1;
        if original.error_category.is_none() {
            entry.1 = entry.1.saturating_add(original.facts.size);
        }
    }
    let pending_folders = pending_by_folder
        .into_iter()
        .map(
            |(relative_path, (pending_files, pending_bytes))| PendingFolder {
                relative_path,
                pending_files,
                pending_bytes,
            },
        )
        .collect::<Vec<_>>();
    // New candidates remain derivable from the missing evidence. Only a
    // changed Location needs its old facts retained across a scan that no
    // longer classifies it from the current evidence.
    let mut pending_changed_paths = deferred_paths
        .iter()
        .filter(|path| previous_paths.contains(path.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    pending_changed_paths.sort();

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

    // A Photo the Photographer moved into Trash keeps its own Restore
    // contract: its Original is never a relocation candidate, whatever the
    // scan observes. The removed marker is Photo state, so it is read from
    // the snapshot's Photos instead of inferred from availability.
    let mut removed_original_ids = HashSet::with_capacity(previous.photos.len());
    for photo in &previous.photos {
        if photo.removed {
            removed_original_ids.insert(photo.original_id.as_str());
        }
    }

    // An Original is eligible for relocation when its bytes are not at its
    // remembered Location: it is missing, or its Location now provably
    // carries other bytes. An unchanged Location or a failed hash keeps the
    // owner in place as a revision.
    let mut eligible: Vec<&OriginalRecord> = Vec::new();
    for original in &previous.originals {
        if removed_original_ids.contains(original.id.as_str()) {
            continue;
        }
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
    let mut relocations: HashMap<String, String> = HashMap::new();
    let mut by_digest: HashMap<String, (Vec<&OriginalRecord>, Vec<String>)> = HashMap::new();
    let mut relocation_sources: HashMap<String, ScanRelocationSource> = HashMap::new();
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
        if file.kind == original.kind
            && fingerprint.size == file.size
            && !blocked_shapes.contains(&(original.kind, fingerprint.size))
        {
            relocations.insert(path.clone(), original.id.clone());
            relocation_sources.insert(
                original.id.clone(),
                ScanRelocationSource {
                    relative_path: original.relative_path.as_str().to_owned(),
                    facts: original.facts,
                    available: original.available,
                },
            );
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
        if let Some(original_id) = relocations.remove(&path) {
            relocation_sources.remove(&original_id);
        }
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
        relocation_sources,
        deferred_paths,
        pending_folders,
        pending_changed_paths,
    }
}

/// The relative parent Folder of one Original Location.
fn parent_folder(path: &str) -> String {
    path.rfind('/')
        .map_or_else(String::new, |position| path[..position].to_owned())
}

/// Whether one Original Location sits inside the selected Folder subtree.
fn folder_contains(path: &str, folder: &str) -> bool {
    let parent = parent_folder(path);
    if folder.is_empty() {
        return true;
    }
    parent == folder || parent.starts_with(&format!("{folder}/"))
}

struct HashedFile {
    digest: String,
    kind: OriginalKind,
    size: u64,
    owner_keeps_bytes: bool,
}
