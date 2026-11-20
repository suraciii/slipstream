# Folder Freshness Design

## Authority

[Folder Freshness](spec.md) owns pending Folder behavior and folder-scoped verification.

[Photo Library and Albums](../spec.md) owns Location Recovery.

[Library Browsing Design](../../library-browser/design.md) owns Published Library and File Location windows.

## Laws

1. A potential relocation candidate is never published as a new Photo before content verification.
2. A Pending Candidate never overwrites the last published facts of an Original.
3. A Folder Check verifies all candidates of a shape that can affect the selected Folder.
4. Only one Library Check or Folder Check owns the Library scanner.
5. A successful apply publishes one complete snapshot; scope leaves unrelated Folders unchanged.

## Candidate Classification

A candidate has one shape: `(kind, size)`.

The planner derives candidate shapes from every Content Fingerprint in the current recovery evidence set.

A new path is a candidate only when its shape matches an evidence shape or its discovery failed.

A changed path is a candidate when the existing planner already requires its fingerprint evidence.

Classification reads no candidate content.

## Budget

The automatic startup check uses `AUTOMATIC_VERIFICATION_BUDGET_BYTES`.

The budget applies to the sum of candidate sizes in one automatic check.

When the total exceeds the budget, every candidate in that check is deferred.

When the total is within budget, the check uses the existing full recovery planner.

An explicit full check and a Folder Check ignore the budget.

The initial budget is 256 MiB. It is a product policy constant, not a client option.

## Scope Expansion

A Folder Check selects candidates whose Location is the requested Folder or a descendant.

It collects the shapes of those candidates.

It also hashes every candidate outside the Folder with a selected shape, because those candidates can affect uniqueness.

Candidates with other shapes remain deferred.

An unreadable candidate outside the Folder blocks automatic recovery for its kind.

A relocation is blocked when a deferred readable candidate has the source fingerprint's shape.

This rule proves that an outside candidate cannot be the unique match before the planner commits a relocation.

## Planner Result

The recovery planner returns:

- successful fingerprints for hashed candidates;
- relocations;
- deferred candidate paths;
- Pending Folder summaries.

A failed hash target joins the deferred set and records its kind as failed evidence.

Pending Folder summaries group deferred paths by their parent Location.

A summary stores the Folder Location, pending file count, and known pending bytes.

The summaries replace the previous persisted set on every apply.

## Partial Application

The scanner removes deferred new paths from the discovered set.

For a deferred changed path, the scanner preserves the persisted Original's facts, availability, error state, and Capture fact.

The apply transaction receives the deferred paths.

It uses the persisted values for those paths instead of the discovered values.

Deferred discovery errors are removed from the scan error list.

The apply then reconciles all remaining files normally.

This makes safe additions visible while holding uncertain identities.

## Persistence

The state store owns one `pending_folders` table.

Each row stores one Folder Location, its pending file count, and its pending byte count.

Only pending Folders have rows.

The `ScanSnapshot` exposes the complete pending Folder set.

The apply transaction replaces the set atomically with the scan result.

## Folder Projection

`FolderIndex` merges pending Folder rows into the derived File Location tree.

A pending Folder is a known Folder even without a published Photo.

An ancestor of a pending Folder is also known.

Each Folder child reports:

- direct pending file count;
- whether any descendant Folder is pending.

A File Location window also reports the same facts for its parent.

A pending Folder marker appears only when pending state exists.

## Scan Lifecycle

`ScanOptions` carries the verification scope and the automatic budget decision.

- Automatic startup: no scope, budget enabled.
- Explicit full check: no scope, budget disabled.
- Folder Check: one Folder Location, budget disabled.

The Library scanner tracks the active options.

A second scan with different options reports busy.

The Application Scan Cycle tracks the active scope and serves waiters only for the same scope.

## Status

Loading Status reports the active Folder Check scope when one exists.

Folder Check phases and counts use the existing scanner progress.

The Folder strip shows the active status only while its Folder is the active scope.

No Library-wide pending warning exists.

## Protocol

`POST /api/scan` accepts an optional JSON object:

```json literal
{"folder": "2026/travel"}
```

An empty object or omitted body starts an explicit full check.

An invalid Folder Location is a request error.

`ScanStatusWire` adds an optional `scope` Folder Location.

`FileLocationsResponse` and `FolderChildWire` add `pendingFiles` and `hasPendingDescendants`.

## Folder Check Action

The selected Folder strip sends one `check-folder` intent.

The Application Owner sends the scoped request.

The strip disables the action while a different check runs.

A same-scope request joins the active cycle.

## Acceptance Boundaries

The implementation MUST cover:

- budget deferral leaves pending folders and no candidate Photo;
- safe additions still publish;
- a folder check proves uniqueness across same-shape candidates outside the folder;
- a failed candidate stays pending;
- pending markers appear only on affected folder rows;
- normal folders render no strip or marker.
