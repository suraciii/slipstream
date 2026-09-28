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
    persistence::{DiscoveredFingerprint, ScanRecoveryPlan, ScanRelocationSource},
};
use sha2::{Digest as _, Sha256};
use std::collections::{HashMap, HashSet};

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
///
/// `excluded_original_ids` names Originals whose Photo was confirmed
/// permanently deleted. They are not relocation candidates: their bytes were
/// deleted, and a file that later occupies their reviewed Location is a new
/// Original instead of a recovered identity.
pub fn plan_recovery(
    root: &crate::LibraryRoot,
    native_work: &NativeWorkBudget,
    discovered: &[DiscoveredOriginal],
    previous: &ScanSnapshot,
    persisted_fingerprints: &[crate::OriginalFingerprint],
    excluded_original_ids: &std::collections::HashSet<String>,
    progress: &mut RecoveryProgress,
    report: &dyn Fn(u64, u64),
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
    report(progress.hashed, progress.hash_total);
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
        report(progress.hashed, progress.hash_total);
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
        if file.kind == original.kind && fingerprint.size == file.size {
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
pub struct RecoveryRecord {
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
    /// Current Original availability. The unavailable survey reports only
    /// records that are currently unavailable; the identity read of one
    /// retained review reports the observed value.
    pub available: bool,
    /// Whether the Photographer moved this Photo into Trash.
    pub removed: bool,
}

/// Counts the unavailable Originals one Folder-prefix review would evaluate.
/// The count reads no content, so an oversized scope is refused before any
/// destination is opened.
pub fn count_prefix_scope(
    survey: &RecoverySurvey,
    old_prefix: &str,
) -> Result<usize, crate::domain::PathError> {
    parse_location_prefix(old_prefix)?;
    Ok(survey
        .unavailable
        .iter()
        .filter(|record| under_prefix(&record.relative_path, old_prefix))
        .count())
}

/// Everything the manual planner needs from one consistent persisted read.
pub struct RecoverySurvey {
    pub unavailable: Vec<RecoveryRecord>,
    /// Photo IDs that keep independent user state: Album membership, saved
    /// edit recipes, or an Export record. An explicit retire never replaces
    /// one of these Photos.
    pub referenced_photo_ids: std::collections::HashSet<String>,
}

/// The occupying record an explicit retire-and-bind may replace.
#[derive(Clone, Debug)]
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
    /// occupant has independent user state, a Trash removal, or an unsupported
    /// state, and both records are preserved.
    Occupied { retire: Option<RetireSummary> },
    /// Another mapping in the same reviewed set already targets this Location.
    Colliding,
}

impl ManualOutcome {
    /// The closed code shared by the review wire shape and the reviewed
    /// mapping identity.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Matched => "matched",
            Self::ContentMismatch => "content-mismatch",
            Self::Missing => "missing",
            Self::KindMismatch => "kind-mismatch",
            Self::Unreadable => "unreadable",
            Self::Occupied { .. } => "occupied",
            Self::Colliding => "colliding",
        }
    }
}

/// Why one evaluated mapping cannot be applied. A mapping without a reason is
/// the only one a caller may confirm.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MappingBlock {
    /// Another mapping of the same reviewed set already targets this Location.
    Colliding,
    /// The destination digest differs from the persisted fingerprint.
    ContentMismatch,
    /// The occupying Photo has independent user state or retained references.
    DestinationInUse,
    /// The occupying Photo is in Trash; its own Restore contract applies.
    DestinationRemoved,
    /// The destination filename carries an unsupported kind for this Original.
    KindMismatch,
    /// No file exists at the destination.
    Missing,
    /// The destination cannot be read, so content cannot be judged.
    Unreadable,
}

impl MappingBlock {
    /// The closed code shared by the review wire shape and the reviewed
    /// mapping identity.
    pub fn code(self) -> &'static str {
        match self {
            Self::Colliding => "colliding",
            Self::ContentMismatch => "content-mismatch",
            Self::DestinationInUse => "destination-in-use",
            Self::DestinationRemoved => "destination-removed",
            Self::KindMismatch => "kind-mismatch",
            Self::Missing => "missing",
            Self::Unreadable => "unreadable",
        }
    }
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
    /// Why this mapping cannot be applied, or `None` when it can.
    pub blocked: Option<MappingBlock>,
    /// Opague reviewed identity binding a later confirmation to every
    /// reviewed fact of this mapping. The service recomputes it and refuses a
    /// mismatch instead of applying an unreviewed correspondence.
    pub mapping_id: String,
    /// Observed facts of the destination file, or `None` when no readable
    /// file occupies it. The persistence transaction stores these facts and
    /// the next scan re-checks the revision.
    pub destination_facts: Option<crate::OriginalFacts>,
}

/// One confirmed mapping the persistence owner revalidates and commits.
/// `facts` were observed through a confined descriptor outside the write
/// transaction; the transaction trusts them and the next scan re-checks the
/// revision.
#[derive(Clone)]
pub struct RequestedRelocation {
    pub original_id: String,
    /// The remembered Location this confirmation reviewed. A changed
    /// Location refuses the mapping instead of relocating an unreviewed one.
    pub from_location: String,
    pub to_location: String,
    /// The reviewed mapping identity. The persistence owner carries this
    /// binding through the write boundary; callers must not synthesize it.
    pub mapping_id: String,
    /// The persisted digest that made the reviewed destination verifiable.
    /// The persistence owner rechecks it while the commit transaction is
    /// still open, so a replacement with the same filesystem facts cannot
    /// inherit the reviewed confirmation.
    pub fingerprint: Option<String>,
    pub facts: crate::OriginalFacts,
    /// The exact occupying Photo an explicit retire-and-bind replaces. A
    /// different occupant refuses the mapping.
    pub retire_photo_id: Option<String>,
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

/// Everything one destination observation found.
struct CandidateEvaluation {
    outcome: ManualOutcome,
    /// Observed facts of the destination file, or `None` when no readable file
    /// occupies it.
    destination_facts: Option<crate::OriginalFacts>,
    /// True when a persisted fingerprint matched the destination digest.
    verified: bool,
}

/// The canonical revision token of one observed destination. `absent` means
/// no readable file occupies the Location.
fn revision_token(facts: Option<crate::OriginalFacts>) -> String {
    match facts {
        None => "absent".to_owned(),
        Some(facts) => format!(
            "{}:{}:{}:{}",
            facts.size, facts.mtime_ms, facts.device, facts.inode
        ),
    }
}

/// The opaque identity of one reviewed mapping. It is a deterministic
/// function of every reviewed fact — including the persisted fingerprint
/// that supplied content evidence, the observed destination revision, the
/// destination Photo an explicit retire may replace, and the blocking reason
/// — so equal facts produce an equal identity and no durable proposal record
/// is required.
fn mapping_identity(
    record: &RecoveryRecord,
    to_location: &str,
    evidence: Option<&str>,
    destination_facts: Option<crate::OriginalFacts>,
    retire_photo_id: Option<&str>,
    blocked: Option<MappingBlock>,
    outcome: &ManualOutcome,
) -> String {
    let parts = [
        "slipstream-recovery-mapping-v1".to_owned(),
        record.original_id.clone(),
        record.relative_path.clone(),
        to_location.to_owned(),
        match record.kind {
            OriginalKind::Raw => "raw".to_owned(),
            OriginalKind::Jpeg => "jpeg".to_owned(),
        },
        record.photo_id.clone(),
        evidence.unwrap_or("unverified").to_owned(),
        revision_token(destination_facts),
        retire_photo_id.unwrap_or("-").to_owned(),
        blocked.map_or("-".to_owned(), |block| block.code().to_owned()),
        outcome.code().to_owned(),
    ];
    digest_bytes(parts.join("\0").as_bytes())
}

/// Evaluates one candidate destination for one unavailable record.
///
/// `relocating` names the Originals that move in the same reviewed or
/// submitted set, so a destination owned by one of them is vacated instead of
/// occupied. `claimed_destinations` names Locations already targeted by an
/// earlier mapping of that set.
fn evaluate_candidate(
    root: &crate::LibraryRoot,
    native_work: &NativeWorkBudget,
    record: &RecoveryRecord,
    occupant: Option<&OriginalRecord>,
    occupant_photo: Option<&crate::PhotoRecord>,
    referenced: &HashSet<String>,
    to_location: &str,
) -> CandidateEvaluation {
    let absent = |outcome: ManualOutcome| CandidateEvaluation {
        outcome,
        destination_facts: None,
        verified: false,
    };
    // Kind comes from the destination filename, exactly as the scanner
    // classifies it.
    let filename = to_location.rsplit('/').next().unwrap_or(to_location);
    let destination_kind = crate::identity::classify_name(filename);
    if destination_kind != Some(record.kind) {
        return absent(ManualOutcome::KindMismatch);
    }
    let path = match crate::RelativeOriginalPath::parse(to_location.to_owned()) {
        Ok(path) => path,
        Err(_) => return absent(ManualOutcome::Missing),
    };
    let capability = match root.original(path) {
        Ok(capability) => capability,
        Err(_) => return absent(ManualOutcome::Missing),
    };
    let mut verified = false;
    let destination_facts = if let Some(expected) = record.fingerprint.as_deref() {
        // A persisted fingerprint is verified against the destination
        // content; a match falls through so the outcome below reports the
        // destination state, and any mismatch or read failure refuses the
        // mapping. The optional-digest path keeps an absent destination a
        // missing candidate instead of an unreadable one; only a real read
        // failure of an existing entry is unreadable.
        let permit = native_work.acquire();
        let digest = capability.digest_file_if_present();
        drop(permit);
        match digest {
            Ok(Some(checked)) if checked.digest == expected => {
                verified = true;
                Some(checked.facts)
            }
            Ok(Some(_)) => return absent(ManualOutcome::ContentMismatch),
            Ok(None) => return absent(ManualOutcome::Missing),
            Err(_) => return absent(ManualOutcome::Unreadable),
        }
    } else {
        // Without a fingerprint nothing proves the destination holds the
        // remembered content, so the proposal must at least establish that
        // the Location really holds a readable Original. An absent or
        // unreadable candidate is neither a match nor a retireable occupant,
        // whatever the persisted state remembers about that Location.
        match capability.facts_if_present() {
            Ok(Some(facts)) => Some(facts),
            Ok(None) => return absent(ManualOutcome::Missing),
            Err(_) => return absent(ManualOutcome::Unreadable),
        }
    };
    match occupant {
        // A destination owned by the record itself is an in-place restore;
        // there is no separate occupant to retire or conflict with.
        Some(owner) if owner.id != record.original_id => {
            let retire = match occupant_photo {
                Some(photo)
                    if !photo.removed
                        && photo.rating == 0
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
    .into_evaluation(destination_facts, verified)
}

impl ManualOutcome {
    fn into_evaluation(
        self,
        destination_facts: Option<crate::OriginalFacts>,
        verified: bool,
    ) -> CandidateEvaluation {
        CandidateEvaluation {
            outcome: self,
            destination_facts,
            verified,
        }
    }
}

/// Builds one reviewable proposal from one evaluated destination.
fn build_proposal(
    record: &RecoveryRecord,
    to_location: &str,
    colliding: bool,
    evaluation: CandidateEvaluation,
    occupant_removed: bool,
) -> ManualProposal {
    let outcome = if colliding {
        ManualOutcome::Colliding
    } else {
        evaluation.outcome
    };
    let blocked = match &outcome {
        ManualOutcome::Matched => None,
        ManualOutcome::Occupied { retire: Some(_) } => None,
        ManualOutcome::Occupied { retire: None } => Some(if occupant_removed {
            MappingBlock::DestinationRemoved
        } else {
            MappingBlock::DestinationInUse
        }),
        ManualOutcome::ContentMismatch => Some(MappingBlock::ContentMismatch),
        ManualOutcome::Missing => Some(MappingBlock::Missing),
        ManualOutcome::KindMismatch => Some(MappingBlock::KindMismatch),
        ManualOutcome::Unreadable => Some(MappingBlock::Unreadable),
        ManualOutcome::Colliding => Some(MappingBlock::Colliding),
    };
    let retire_photo_id = match &outcome {
        ManualOutcome::Occupied {
            retire: Some(retire),
        } => Some(retire.photo_id.as_str()),
        _ => None,
    };
    let verified = evaluation.verified
        && matches!(
            outcome,
            ManualOutcome::Matched | ManualOutcome::Occupied { .. }
        );
    let mapping_id = mapping_identity(
        record,
        to_location,
        record.fingerprint.as_deref(),
        evaluation.destination_facts,
        retire_photo_id,
        blocked,
        &outcome,
    );
    ManualProposal {
        original_id: record.original_id.clone(),
        photo_id: record.photo_id.clone(),
        from_location: record.relative_path.clone(),
        to_location: to_location.to_owned(),
        kind: record.kind,
        outcome,
        verified,
        blocked,
        mapping_id,
        destination_facts: evaluation.destination_facts,
    }
}

/// One mapping of a reviewed or submitted set. `relocating` names the
/// Originals that move in the same set, so their destinations are vacated
/// instead of occupied. `claimed_destinations` names Locations already
/// targeted by an earlier mapping of the same set.
pub struct RelocationSet<'a> {
    /// The Originals that move in this set, so their destinations are
    /// vacated instead of occupied.
    pub relocating: &'a HashSet<String>,
    /// The Locations already targeted by an earlier mapping of this set.
    pub claimed_destinations: &'a HashSet<String>,
}

/// Evaluates one mapping for one unavailable Original against current facts.
/// A mapping whose reviewed identity no longer matches this evaluation must be
/// refused instead of applied.
pub fn evaluate_relocation(
    root: &crate::LibraryRoot,
    native_work: &NativeWorkBudget,
    survey: &RecoverySurvey,
    snapshot: &ScanSnapshot,
    original_id: &str,
    to_location: &str,
    set: RelocationSet<'_>,
) -> Result<ManualProposal, crate::domain::PathError> {
    let Some(record) = survey
        .unavailable
        .iter()
        .find(|record| record.original_id == original_id)
    else {
        return Err(crate::domain::PathError);
    };
    Ok(evaluate_record(
        root,
        native_work,
        survey,
        snapshot,
        record,
        to_location,
        set,
    ))
}

/// Evaluates one record of a surveyed set, including the occupancy context of
/// the other mappings in that set.
fn evaluate_record(
    root: &crate::LibraryRoot,
    native_work: &NativeWorkBudget,
    survey: &RecoverySurvey,
    snapshot: &ScanSnapshot,
    record: &RecoveryRecord,
    to_location: &str,
    set: RelocationSet<'_>,
) -> ManualProposal {
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
    // An occupant that moves in the same set vacates the destination; it is
    // neither retired nor a conflict.
    let vacating = occupant.is_some_and(|owner| {
        owner.id != record.original_id && set.relocating.contains(owner.id.as_str())
    });
    let evaluation = evaluate_candidate(
        root,
        native_work,
        record,
        if vacating { None } else { occupant },
        occupant_photo,
        &survey.referenced_photo_ids,
        to_location,
    );
    build_proposal(
        record,
        to_location,
        set.claimed_destinations.contains(to_location),
        evaluation,
        occupant_photo.is_some_and(|photo| photo.removed),
    )
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

    let mut reviewed = Vec::new();
    let mut relocating = HashSet::new();
    for record in &survey.unavailable {
        if !under_prefix(&record.relative_path, old_prefix) {
            continue;
        }
        let suffix = &record.relative_path[old_prefix.len()..];
        reviewed.push((record, join_destination(new_prefix, suffix)));
        relocating.insert(record.original_id.clone());
    }
    let mut claimed_destinations = HashSet::new();
    let mut proposals = Vec::with_capacity(reviewed.len());
    for (record, to_location) in reviewed {
        let proposal = evaluate_record(
            root,
            native_work,
            survey,
            snapshot,
            record,
            &to_location,
            RelocationSet {
                relocating: &relocating,
                claimed_destinations: &claimed_destinations,
            },
        );
        claimed_destinations.insert(to_location);
        proposals.push(proposal);
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
    evaluate_relocation(
        root,
        native_work,
        survey,
        snapshot,
        original_id,
        to_location,
        RelocationSet {
            relocating: &HashSet::new(),
            claimed_destinations: &HashSet::new(),
        },
    )
}
