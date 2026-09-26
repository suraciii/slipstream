# Library Management: Composable Removal and Restore

A Photographer's Agent may identify Photos by combining queries and judgments. It must be able to remove exactly those Photos into Trash and restore a mistaken removal without opening a browser or changing Album membership as a workaround. The application must enforce the same domain rules for explicit target sets and direct human actions.

## Capability boundary

Remove from Library and Restore are separate capabilities. The Agent owns querying, choosing targets, dividing work into bounded groups, and reporting the combined result. Slipstream owns identity, eligibility, effect scope, concurrency checks, and outcome recovery. No scenario-specific cleanup command or persistent workflow object is required.

[Remove Rejected Photos](rejected-photo-cleanup.md) owns the effects of reversible removal and the browser review flow. [Trash](library-management-trash.md) owns recovery and permanent-deletion semantics. This document owns explicit-set invocation and its outcomes for both clients. It does not relax rejected eligibility for removal or authorize permanent deletion.

## Explicit targets and limits

The caller must supply a nonempty set of distinct Photo identities and the current-state evidence obtained when reviewing those targets. Paths, filenames, an Album name, or a live query must not substitute for those identities in a mutation. Query results may supply the targets without a browser view or Browse Snapshot prerequisite.

Photo queries or a Photo-scoped read must provide all current-state evidence needed for removal. A removal result or Trash listing must provide the evidence needed for Restore. The caller must not synthesize evidence from a filename, infer it from Selection State alone, or read private storage. If a query result lacks required evidence, the caller must be able to obtain it through an ordinary Photo read.

The application must expose the maximum accepted set size. Malformed, duplicate, empty, or over-limit requests must fail before changing any Photo. It must not truncate a set, process its first page, or reinterpret an omitted set as all Photos.

A caller may compose multiple bounded operations. Each operation must retain its own effects and outcome; Slipstream must not promise one atomic action or global Undo across caller-composed batches. Later query matches must not join a previously submitted set. Changes to the query source must never substitute different targets.

## Remove from Library

Inputs are the explicit Photo set, observed decision and removal-state evidence, and an identity for this removal attempt that the caller can retain before submission. Eligibility must be checked when applying the action, not only when the query runs.

- A Photo must still be in the Library, not removed, and rejected under the observed state. A changed decision or intervening removal-and-Restore must refuse the stale intent even if the current state appears rejected again.
- An already removed Photo must be reported as such; this invocation must not claim that it removed the Photo or replace its existing removal identity.
- An unknown or permanently deleted Photo must be reported as unavailable.
- A pending permanent deletion must remain protected under the Trash contract.
- An unavailable Original does not prevent reversible Library removal. No file access or write is needed to claim that its Library state changed.

The result must identify each newly removed Photo and its observed removal identity, so the caller can later Restore that exact removal. Decisions, Ratings, Photo identity, Original Files, and retained Album facts follow the existing removal preservation rules. The explicit-set form must not select all rejected Photos in the source or require a temporary Album.

## Restore

Inputs are an explicit Photo set, each Photo's observed removal identity from a removal result or Trash listing, and an identity for this restore attempt.

The application must restore only the removal the caller observed. If a Photo was restored and removed again, the old intent must not restore the newer removal. If it is already active, report no change without claiming a new Restore. A permanently deleted Photo cannot be restored; pending verification must remain protected until the deletion outcome is settled.

Pending deletion or unresolved deletion verification takes precedence over an already-removed or other no-effect classification. Restore and permanent deletion of the same removal must not both succeed: once either has taken effect or deletion is unresolved, the competing action must report the current conflict rather than claim success.

Restore effects follow [Selection and Restore](library-management-trash.md#selection-and-restore), including surviving Album membership and unavailable Originals. Restore must not change a Photo to selected or undecided, recreate a deleted Album, or recreate file bytes.

## Outcomes and uncertainty

A valid set may have mixed outcomes. The result must account for every requested identity exactly once and distinguish:

- changed by this operation;
- already in the requested state, with no effect by this operation;
- changed since review or otherwise ineligible, with the reason;
- unavailable;
- failed with an actionable reason; and
- unresolved, requiring outcome verification.

Summary counts must agree with per-Photo outcomes. No-effect items must not increase applied counts or become owned by this operation's Undo. One item's refusal must not be described as rolling back confirmed effects on other items. Admission failure before processing must be distinguishable from an accepted action whose effects are uncertain.

The caller must be able to recover the outcome of the same attempt after a lost response or service restart. Retrying that attempt with the same intent must not reapply effects; reusing its identity with a different intent must be refused. A later Restore must not turn a historical successful removal into a fresh permission to remove the Photo again. Historical operation results and current Photo state must remain distinguishable.

A first attempt refused before admission must report that no effect occurred. An outcome lookup that cannot establish whether an attempt was admitted must report that uncertainty; absence of a result alone is not proof that retrying as a new operation is safe. Replay of a known attempt must recover its historical outcome before considering whether its old preconditions still hold. The application must not silently forget a previously accepted attempt and admit the same identity as new.

A timeout, malformed response, or missing outcome entry must be treated as uncertain. The caller must reconcile the original attempt before issuing replacement mutations for unresolved items. An outcome lookup must not itself change Photo state. Successful items remain inspectable while failed items can be reviewed for a new attempt.

## Discovery and composition

The CLI must expose removal, Restore, and their outcome inspection with discoverable prerequisites, input requirements, limits, and effect categories. Web and CLI must observe the same state after confirmation. Exact command grammar and wire representations belong in the authoritative CLI Reference; these requirements add no parallel syntax definition.

Permanent deletion remains a separate reviewed and confirmed capability. An authorization to mark, remove, or Restore must never imply authority to delete Originals. A permanent-deletion review composed after removal must select the authorized remaining Trash items explicitly, not all unrelated Trash contents.

## Acceptance examples

- Query rejected RAW Photos within a shoot period, exclude one, and remove the chosen identities in bounded groups. Other dates, JPEG siblings, excluded Photos, and unrelated rejected Photos stay unchanged. Web shows exactly the confirmed removals in Trash.
- Change one decision to selected after querying. Its removal is refused; valid siblings produce their own outcomes.
- Remove a Photo, Restore it, and remove it again. An older Restore intent cannot clear the second removal; retrying the first removal attempt does not create a third removal.
- Lose a response and restart the service. Recover that attempt's exact outcomes without repeating its effects or pretending current state is its historical result.
- Restore a named mistaken item, then separately review the remaining authorized Originals for permanent deletion. No browser automation, direct database access, or temporary Album is needed.
- Obtain every required precondition through public query, Photo-read, or Trash results. No synthesized version or private-storage read is needed.
- Race Restore with permanent deletion of the same removal. At most one takes effect; unresolved deletion blocks Restore until reconciliation.
- Submit duplicate or over-limit targets. The request changes nothing and explains the input or limit problem.
