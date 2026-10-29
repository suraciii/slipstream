# Photo Library Identity and Expansion

A filesystem location identifies where to find an Original File, not its identity.

The admitted state store MUST NOT silently reinterpret its Library against an unrelated directory.

## Model

### Photo Library

One state store owns one [Photo Library](../../CONTEXT.md#library).

Its [Library Folder](../../CONTEXT.md#library) owns read-only containment and discovery scope; its absolute path is an admitted storage binding, not Library identity.

The admitted binding remains fail-closed during ordinary startup.

A different configured Folder MUST NOT silently reinterpret existing Original Locations.

### Original File and Original Location

An [Original File](../../CONTEXT.md#library) has an opaque persisted ID that remains unchanged after first discovery.

The state store MUST assign every new ID independently of Original Location and reject collisions before insertion.

An [Original Location](../../CONTEXT.md#library) MAY change through Library Expansion or Location Recovery.

Ordinary rescan MUST NOT infer relocation from names, metadata, or similarity.

File size, modification time, device, and inode are revision or admission facts, not Original File identity.

### Content Fingerprint

A [Content Fingerprint](../../CONTEXT.md#library) is revision-bound: a digest produced for one revision MUST NOT be treated as current after a size or modification-time change.

It is not a unique database key and MUST NOT replace Original File or Photo identity.

The state store persists one fingerprint row per Original File, keyed by Original File ID.

Enrollment runs after structural migration and every scan without holding a migration or scan transaction open.

Hashing reads through a confined read-only descriptor and verifies stability before and after the read.

Enrollment is bounded, resumable background work that yields to foreground Preview requests and pauses during scans.

### Photo

A [Photo](../../CONTEXT.md#library) has one stable persisted ID and refers to exactly one independently managed RAW or JPEG Original.

Its ID remains unchanged when Library Expansion or Location Recovery changes its Original Location.

The state store assigns a new Photo ID independently of Location when reconciliation cannot preserve an existing Photo.

Selection State, Rating, Album membership, membership order, and saved Album positions continue to refer to the stable Photo.

## Library Expansion

Slipstream supports one explicit expansion: replace the current Library Folder with one of its ancestor directories.

For example:

```text literal
current Library Folder: /photos/26-spring
new Library Folder:     /photos
old Folder prefix:      26-spring
```

The operation MUST run while the service is stopped and after a verified state backup.

It MUST:

1. open the current and proposed Library Folders read-only;
2. resolve the current Folder as one confined descendant of the proposed Folder;
3. prove that both descriptors identify the same directory;
4. derive one non-empty relative prefix from the proposed Folder to the current Folder;
5. preflight a complete confined traversal of the proposed Folder within scan limits;
6. prove that prefixing every persisted Original Location is valid and collision-free;
7. begin one admitted `BEGIN IMMEDIATE` transaction;
8. update the Folder binding, every persisted Location, and every location-derived ordering value;
9. invalidate every rebuildable fact whose identity includes an old Location;
10. validate that every Original File ID, Photo ID, user decision, Album membership position, and saved Album position remains unchanged; and
11. commit the transaction, then complete a normal scan before reporting readiness.

Prefixing every old Location keeps every remembered Original Location coherent within the former Folder.

The subsequent scan discovers supported files in sibling directories as new Original Files and Photos.

It rebuilds derived File Location navigation and does not create Albums from directories.

An unavailable remembered Original File receives the same deterministic prefix as available Original Files.

The operation proves the old Folder itself, rather than guessing individual moves, so temporary file unavailability does not discard its state.

## Failure Behavior

Expansion MUST fail before changing SQLite when:

- the proposed Folder is not an ancestor of the current Folder;
- the current Folder cannot be opened as the derived confined descendant;
- the two Folder descriptors do not identify the same directory;
- a prefixed Location is invalid, unsupported, duplicated, or collides with another persisted Location;
- state, schema, sidecar, transaction, or backup admission fails; or
- the proposed Folder cannot be scanned within configured resource limits.

A pre-commit failure leaves the existing binding and state unchanged.

If the required post-commit scan has a root-level failure, the service MUST remain unready.

The operator MAY retry after correcting that failure or restore the verified pre-expansion backup and prior Library Folder. Per-file Capture Time or Preview failures follow normal scan behavior and do not roll back valid sibling Photos.

Original Files remain unchanged.

Ordinary startup with a mismatched Folder remains a hard failure.

Ordinary rescan marks a missing Original File unavailable unless one persisted Content Fingerprint identifies exactly one unclaimed same-kind candidate. Otherwise it MUST discover a file at another Location as new.

Rescan MUST NOT transfer state using filename, Capture Time, camera ID, inode, or visual similarity.

## Persistence and Cache Semantics

Persisted Original File and Photo IDs are opaque after creation.

Canonical v3 path-derived IDs remain valid opaque values, but deterministic v3 identity vectors are migration inputs only.

Every new ID MUST come from a state-store-unique, Location-independent allocator, so a newly discovered sibling cannot collide with a preserved legacy ID.

The v3-to-v4 and v4-to-v5 migrations MUST preserve every existing Original File ID, Photo ID, row, and user-owned state.

Older binaries MUST reject newer state.

The v4-to-v6 migration sequence establishes the independent-Photo and Album baseline at schema version 6; later migrations extend that state to writable schema version 15.

The v5-to-v6 migration MUST split each legacy paired Photo RAW-first.

The RAW Original keeps the Photo ID and every user state reference, even when unavailable.

The JPEG Original keeps its Original File ID and receives a new independent Photo ID with default decisions and no inherited Album memberships.

Singleton Photos and remembered unavailable records keep their IDs and state.

The migration MUST preserve Album IDs, names and creation order, the admitted Folder binding, membership positions, saved Photos, Selection State, Rating, and Capture Time facts.

A Preview survives only when its legacy source matches the kept kind and its revision matches current facts; a sibling-JPEG-derived Preview for a kept RAW is invalidated.

The migration runs as one atomic schema change with fail-closed admission, and fingerprint enrollment starts only after that transaction commits.

Expansion is admitted only against canonical schema versions 7 through 15 after a verified backup.

The offline command migrates an admitted older version to schema 15 before its one transaction changes the admitted Folder binding, persisted Locations, location-derived ordering values, and rebuildable derived facts.

It MUST NOT change user-owned state.

Capture inspection facts, Preview/cache records, and File Location navigation whose inputs include the old Location MAY be reset or invalidated in that transaction.

They are derived state and MAY be rebuilt from the same read-only Original File.

Selection State, Rating, Albums, membership order, and saved Album positions MUST NOT be reset.

Rollback across the v3-to-v4 identity migration stops the newer process, restores the verified pre-migration v3 backup, restores the prior Library Folder configuration, and starts the compatible v3 image.

Rollback across the v4-to-v5 Album migration restores its verified v4 backup and compatible v4 image.

Rollback across the v5-to-v6 independent-Photo migration restores its verified pre-upgrade v5 backup and compatible v5 image.

There is no in-place down migration.

Rollback of an expansion on the current schema restores the verified pre-expansion backup and prior Library Folder.

## Deployment Boundary

The Docker bind source and container-visible path remain deployment facts. Read-only Original Folder navigation is derived from Published Library Locations according to [Physical File Locations and Virtual Albums](design.md); this design still does not require multiple roots, a Folder hierarchy in SQLite, or a new deployment-path abstraction.

Compose MUST continue to mount the admitted Library Folder read-only and production acceptance MUST verify the exact source, target, and mode.

A later need to relocate the same logical Folder across hosts or mount points requires a separate decision.

It MUST NOT be smuggled into Library expansion.
