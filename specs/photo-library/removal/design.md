# Removal and Restore Design

[Removal and Restore](spec.md) owns observable behavior. [Photo Library Design](../design.md) owns Photo and Album state. [Rejected Cleanup](rejected-cleanup/design.md) owns the browser's reviewed Snapshot admission.

## Model and ownership

The persistence owner MUST be the sole writer of Photo removal state, Restore, removal markers, and operation receipts. A Photo row stores `removed_at_ms` and `removed_operation`; both MUST be null together or set together. The retained row avoids a second recovery record that could drift from Photo facts.

Each removal marker MUST be strictly greater than every marker the Library assigned before it. Markers MUST NOT be reused, even for removal and Restore within one clock millisecond. Restore MUST compare the exact observed marker before clearing it.

An explicit removal attempt has a caller-supplied operation ID and an ordered, distinct target set. Each target carries:

- Photo identity;
- expected Selection State, with `rejected` the only eligible value;
- expected decision version; and
- expected removal marker, `null` for a current Library Photo.

The process-local Photo decision version MUST change on every successful decision, removal, and Restore mutation. A restarted owner MUST start a new version epoch, making pre-restart evidence stale.

A Restore attempt has its own caller-supplied operation ID and ordered `{photoId, removedAtMs}` targets. A request MUST name either one operation or a bounded nonempty list of markers. A fresh select-all Restore captures Trash identities and markers, applies exclusions, and binds the result before mutation.

## Admission and receipts

Before admitting an explicit attempt, Slipstream MUST reject duplicate, empty, malformed, or over-limit input without changing any Photo. Input files MUST be locally validated for exact keys, evidence, distinct nonempty IDs, and the advertised maximum before network access.

The owner MUST inspect the receipt before checking current targets:

| Receipt                               | Required outcome                                                                                                                          |
| ------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| Known ID, identical intent            | Replay its historical outcome.                                                                                                            |
| Known ID, changed targets or evidence | Report a conflict.                                                                                                                        |
| New ID                                | Check each target's evidence and eligibility in the owner transaction. Valid siblings can commit alongside per-target no-effect outcomes. |

Each newly removed target receives a monotonic marker. The durable receipt MUST retain the reviewed sequence and immutable per-target outcomes, including the marker for each newly removed Photo. It MUST survive Restore, cleanup, reload, and restart without requiring the Photo row to survive.

A browser removal MUST commit its outcomes in one transaction. A persistence failure MUST leave the whole request unapplied and retryable. A failed or stale target MUST NOT change Photo facts.

## Undo and Restore

Operation Undo MUST restore in one transaction every Photo that the confirmed operation still owns and that remains eligible. A Photo already restored or cleaned up is not restored again. Restore and Trash Record Cleanup MUST serialize against the same removal.

A named Restore MUST compare each supplied marker against the current removal. A mismatch MUST refuse that Photo without changing it, including a Photo removed again after the listing.

The browser result MUST report `restored`, `changedElsewhere`, or `missing` for each named Photo.

The [CLI Reference](../../command-line/reference.md#reversible-photo-removal-and-restore) owns explicit-set result vocabulary.

A Restore response MUST report the remaining count for each touched removal operation, including zero when emptied. Operations not touched MUST be absent. This lets the browser withdraw or retain Undo based on the operation's actual remaining ownership.

## Publication

Confirmed removal and Restore MUST update the published Library and derived Original Folder index in the same publication critical section as the owning SQLite write. Readers serialized by that lock MUST see the whole state before or after the commit. Other readers MUST linearize at a whole published snapshot, replaced under its write lock.

The patch MUST complete before the response. A window from a pre-removal Browse Snapshot MUST expire instead of presenting the removed Photo or a page shorter than its named position. The browser MUST reopen the source against current state; rescans MUST carry persisted removal markers forward.

Machine-client query projections MUST move with the same commit. A retained query MUST keep its ordered identities, report a subsequently removed Photo as `{"id":"…","state":"missing"}` in its original position, and re-admit it after Restore without a rescan. Refused or rolled-back requests MUST NOT change the projection.

## Interface boundary

Browser and CLI routes MUST share the same persistence owner and publication patching while retaining their separate admission contracts. Browser routes MUST remain byte-compatible. [CLI Reference](../../command-line/reference.md) owns CLI syntax; the CLI MUST use normal Access Token and contract-header admission.

Outcome lookup MUST be read-only. A missing or uninspectable receipt is `outcome_unknown`, not proof that replacement mutation is safe. A lost response or historical deletion MUST NOT impose an indefinite Restore lock.
