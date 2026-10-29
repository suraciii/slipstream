# Removal and Restore

[Photo Library and Albums](../spec.md) owns Photo identity and Album membership. This contract owns reversible removal and Restore for browser and explicit-set actions. [Rejected Cleanup](rejected-cleanup/spec.md) owns the browser review; [Trash](../trash/spec.md) owns inspection and record cleanup.

## Capability boundary

The Agent MAY query, choose targets, divide work into bounded groups, and report their combined results. Slipstream MUST enforce identity, eligibility, effect scope, concurrency checks, and outcome recovery. A scenario-specific cleanup command or persistent workflow object is not required.

Removal and Restore MUST NOT authorize Trash Record Cleanup. A cleanup review composed after removal MUST explicitly select the authorized remaining Trash items, not unrelated Trash contents.

## Explicit targets and limits

An explicit request MUST supply nonempty distinct Photo identities and current-state evidence obtained when reviewing those targets. Paths, filenames, Album names, and live queries MUST NOT substitute for mutation identities. Query results MAY supply targets without a browser view or Browse Snapshot.

Photo queries or Photo-scoped reads MUST provide the evidence needed for removal. A removal result or Trash listing MUST provide the evidence needed for Restore. If a query lacks evidence, an ordinary Photo read MUST provide it; callers MUST NOT synthesize evidence or read private storage.

The application MUST expose the maximum accepted set size. Malformed, duplicate, empty, or over-limit requests MUST fail before changing a Photo. It MUST NOT truncate a set, process only its first page, or interpret an omitted set as all Photos.

Fresh select-all Restore authorizes a current Trash capture, less caller-named exclusions, rather than an earlier listing. Slipstream MUST capture identities and removal markers before mutation and bind the remaining set to the attempt identity. Later arrivals and newer markers MUST NOT join it; retry MUST NOT recapture it.

To act on an earlier inspected selection, the caller MUST supply its explicit identities and observed markers. Exclusions MUST NOT belong to the attempt or be inferred from a display filter or unchecked-cell list. Fresh select-all Restore does not authorize Trash Record Cleanup from a live query.

A caller MAY compose multiple bounded operations. Each operation MUST retain its own effects and outcome; Slipstream MUST NOT promise one atomic action or global Undo across batches. Later query matches or changes to the query source MUST NOT substitute different targets.

## Remove from Library
An explicit removal request names its reviewed Photos, observed decision and removal-state evidence, and an attempt identity the caller can retain before submission. Slipstream MUST check eligibility when applying either removal form, not only when the query runs.

- A Photo MUST still be in the Library, not removed, and rejected. For explicit-set removal, a changed observed decision or intervening removal and Restore MUST refuse stale intent, even if the Photo is rejected again.
- An already removed Photo MUST be reported as such; the invocation MUST NOT claim or replace its existing removal identity.
- An unknown or cleaned-up Photo MUST be reported as unavailable.
- An unavailable Original MUST NOT prevent removal. Removal requires no file access or write to establish its Library effect.

An explicit-set result MUST identify newly removed Photos and their removal identities for later Restore.

Removal MUST retain:

- Photo identity and Original Location;
- Rating and Selection State;
- Album membership and position; and
- saved-position eligibility.

Removal MUST NOT rename, move, overwrite, or delete an Original File.

Removed Photos MUST leave all normal Library sources and their counts, including:

- Album summaries and member listings;
- Folder counts;
- the CLI Photo query; and
- the unavailable-Original recovery survey.

Their retained records MUST remain addressable by identity and available in Trash until restored or cleaned up. Album memberships remain intact.

An explicit-set action MUST NOT select all rejected Photos in the source or require a temporary Album. RAW and same-basename JPEG Photos remain independent targets.

## Restore

A Restore request names an attempt identity and either explicit Photos with their observed removal markers or a fresh select-all capture with named exclusions. Slipstream MUST restore only the removal named by that evidence. An older intent MUST NOT clear a newer removal.

An already active Photo MUST be reported as unchanged, not newly restored. A cleaned-up record MUST be reported as unavailable and MUST NOT be recreated from a historical outcome. Restore and Trash Record Cleanup of the same removal MUST serialize, so at most one takes effect.

Restore MUST preserve retained facts and membership positions in surviving Albums. It MUST NOT erase newer decisions, change Selection State to picked or unflagged, or recreate a deleted Album.

An unavailable Original remains unavailable. Restore MUST NOT rescan, rename, move, rewrite, or recreate file bytes.

## Outcomes and uncertainty

An accepted explicit set MAY have mixed outcomes. Its result MUST account for every requested identity exactly once and distinguish:

- changed by this operation;
- already in the requested state, with no effect by this operation;
- changed since review or otherwise ineligible, with the reason;
- unavailable;
- failed with an actionable reason; and
- unresolved, requiring outcome verification.

Summary counts MUST agree with per-Photo outcomes. No-effect items MUST NOT increase applied counts or become owned by the operation's Undo. A refusal of one item MUST NOT be described as rolling back confirmed effects on others.

Admission failure before processing MUST be distinguishable from an accepted action with uncertain effects. A first attempt refused before admission MUST report that no effect occurred.

The caller MUST be able to recover the same attempt's outcome after a lost response or service restart. Retrying identical intent MUST NOT reapply effects; reuse of an identity for different intent MUST be refused. Receipts MUST remain replayable after later Restore or record cleanup, and historical outcomes MUST remain distinguishable from current Photo state.

A known attempt MUST replay its historical outcome before reconsidering old preconditions. The application MUST NOT forget an accepted attempt and admit its identity as new. Missing outcome evidence MUST NOT be treated as proof that a replacement operation is safe.

A timeout, malformed response, or missing outcome entry MUST be treated as uncertain. The caller MUST reconcile the original attempt before replacement mutations for unresolved items. Outcome lookup MUST NOT change Photo state; successful items remain inspectable and failed items can be reviewed for a new attempt.

## Discovery and composition

The CLI MUST expose removal, Restore, and outcome inspection with discoverable prerequisites, inputs, limits, and effect categories. Web and CLI MUST observe the same confirmed state. The [CLI Reference](../../command-line/reference.md) owns command grammar and wire representations.
