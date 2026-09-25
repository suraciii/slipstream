# Rejected Photo Cleanup Design

Rejected Photo cleanup must handle a result larger than one Grid window while preserving the existing server authority for Selection State and the repository rule that Original Files are read-only. The design must also make a failed or concurrent operation inspectable instead of turning uncertainty into deletion.

## Design Drivers

- The rejected result can exceed the existing 100-Photo ordinary multi-selection limit.
- A Browse Snapshot gives the browser a stable ordered set, but Photo facts can change while it is open.
- Recovery must survive reload and restart; an in-memory undo record is not sufficient.
- Moving a Photo out of normal sources must not alter the Original File or its identity.
- XMP currently represents Rating only. Selection State must not be silently encoded as a rating or keyword.

## Model

The Recovery Area is an application-owned durable collection of cleanup records. A record references one stable Photo identity and retains the Photo's pre-cleanup state needed for restore: Selection State, Rating, Album membership, Album positions, Original Location, and availability facts. The record also identifies the cleanup operation that created it and whether the Photo is currently recoverable.

The Original File remains owned by the Photographer and stays at its Original Location. The Recovery Area changes Library visibility and application ownership state; it does not represent a filesystem directory and must not be described as moving the file.

A Cleanup Selection is an ephemeral set bound to one Browse Snapshot whose filter is `rejected`. It contains Photo IDs, the expected `rejected` state for each ID, and the snapshot identity. It is invalid after source reopen, filter or order change, snapshot expiry, or publication replacement.

## Semantics

Cleanup admission compares every requested Photo's current state with its expected `rejected` state in one transaction. Matching Photos receive Recovery Area records and become unavailable to normal sources. The transaction retains all state needed for restore. A changed or missing Photo receives an outcome and is not moved. The operation is atomic per Photo and returns one non-overlapping outcome for each requested ID.

The operation is separate from the existing bounded Selection State batch. The existing batch remains capped at 100 Photos and continues to own Select, Reject, and Add to Album. Cleanup owns the larger filtered-result selection and its Recovery Area lifecycle.

Undo is a compare-and-set restore of the Photos admitted by one cleanup operation. It must not overwrite a newer restore or other state change. The durable Recovery Area restore uses the same per-record compare-and-set rule and reports records that changed elsewhere. A cleanup result may therefore be partially restored without claiming that every record was restored.

Normal source snapshots exclude Photos currently in the Recovery Area. Recovery Area browsing is a later surface; this capability only requires a durable restore action and the immediate result's Undo. Restoring a record re-admits the same Photo identity and retained facts without rescanning or rewriting the Original File.

Cleanup and restore do not write XMP. Rating remains independent from Selection State. Sidecar synchronization, if added later, must define its own conflict and ownership rules.

## Options

### Selected: Snapshot-bound cleanup transaction

The browser selects the complete rejected result from one stable snapshot and submits Photo IDs with expected states. The server revalidates those states and creates durable recovery records in one transaction, returning per-Photo outcomes.

This supports large rejected sets, preserves the existing snapshot contract, and makes concurrent changes visible. It keeps the dangerous action behind a dedicated confirmation and does not require a new general-purpose tag or delete abstraction.

### Rejected: Reuse ordinary Grid multi-selection

The existing multi-selection is intentionally capped at 100 Photos and only includes loaded positions. Reusing it would either make large cleanup impossible or weaken a bound that protects every existing batch endpoint. It also exposes a destructive action in a surface designed for decisions and Album membership.

### Rejected: Delete Original Files immediately

Immediate filesystem deletion is irreversible, violates the Original File safety boundary, and gives a concurrent state change no recovery path. A Recovery Area preserves the user's ability to inspect and restore while keeping the file untouched.

## Verification

The implementation must prove that:

- a rejected snapshot can represent a result larger than one Grid window;
- a cleanup request is rejected when it is not bound to a rejected snapshot;
- every requested Photo yields exactly one `moved`, `changedElsewhere`, or `missing` outcome;
- changed or missing Photos are never moved, and an Original File's bytes and Location remain unchanged;
- a cleanup result and restart preserve Recovery Area records and restore the same Photo identity and retained state; and
- malformed, incomplete, stale, and transport-failed responses do not update visible state or claim success.
