# Photo Library Identity and Expansion

Slipstream initially configured one shoot directory as the Library Folder. A Photographer now needs to expand that Library to an ancestor directory without losing the Photos, Selection State, Ratings, Albums, or saved Album positions already recorded below the original Folder.

A filesystem location tells Slipstream where to find an Original File. It must not become the identity of the Original File or Photo. At the same time, Slipstream must not silently reinterpret one state database against an unrelated directory.

## Design Drivers

- Original Files remain read-only and may be irreplaceable.
- Selection State, Rating, Album membership, membership order, and saved Album positions belong to existing Photos.
- One state store owns one Photo Library and one configured Library Folder.
- The current need is expansion to an ancestor directory, not arbitrary file relocation or multiple storage roots.
- Ordinary rescans restore a moved Original File only from exact content-fingerprint evidence.
- A failed expansion must leave the current Library and its state recoverable.

## Model

### Photo Library

One state store owns one Photo Library. The state store is already the Library boundary; a second Library UUID adds no value while one store cannot contain multiple Libraries.

### Library Folder

The Library Folder is the configured filesystem directory whose supported descendant files belong to the Photo Library. It owns read-only containment and discovery scope. Its absolute path is an admitted storage binding, not the identity of the Photo Library.

The admitted binding remains fail-closed during ordinary startup. A different configured Folder must not silently reinterpret existing Original Locations.

### Original File and Original Location

An Original File has one stable persisted identity. Its ID is opaque and remains unchanged after first discovery. The state store assigns every new ID independently of Original Location and rejects a collision before insertion.

An Original Location is the relative directory and filename used to find that Original File beneath the current Library Folder. The Location may change through a supported Library expansion or through Location Recovery proved by a persisted Content Fingerprint. Ordinary rescan must not infer relocation from names, metadata, or similarity.

File size, modification time, device, and inode are revision or admission facts, not Original File identity.

### Content Fingerprint

A Content Fingerprint is a persisted SHA-256 digest of one Original File's complete content together with the size and modification time observed while hashing. It is revision-bound: a digest produced for one revision must not be treated as current after a size or modification-time change.

A Content Fingerprint is recovery evidence, not identity. Two independent Originals can share a digest, so the digest is not a unique database key and must not replace Original File or Photo identity.

Hashing reads the Original through the existing confined read-only descriptor and verifies stability before and after the read, so changed bytes cannot produce a trustworthy current digest. Enrollment is bounded, resumable background work: it yields to foreground Preview requests and pauses during scans. The state store persists one fingerprint row per Original File, keyed by Original File ID.

### Photo

A Photo has one stable persisted identity and refers to exactly one independently managed Original File: a RAW Original or a JPEG Original. Its ID remains unchanged when a supported Library expansion or Location Recovery changes its Original Location. The state store assigns a new Photo ID independently of Location when reconciliation cannot preserve an existing Photo.

Selection State, Rating, Album membership, membership order, and saved Album positions continue to refer to the stable Photo.

## Library Expansion

Slipstream supports one explicit expansion: replace the current Library Folder with one of its ancestor directories.

For example:

```text literal
current Library Folder: /photos/26-spring
new Library Folder:     /photos
old Folder prefix:      26-spring
```

The operation must run while the service is stopped and after a verified state backup. It must:

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

Prefixing every old Location keeps every remembered Original Location coherent within the former Folder. The subsequent scan discovers supported files in sibling directories as new Original Files and Photos. It rebuilds derived File Location navigation and does not create Albums from directories.

An unavailable remembered Original File receives the same deterministic prefix as available Original Files. The operation proves the old Folder itself, rather than guessing individual moves, so temporary file unavailability does not discard its state.

## Failure Behavior

Expansion must fail before changing SQLite when:

- the proposed Folder is not an ancestor of the current Folder;
- the current Folder cannot be opened as the derived confined descendant;
- the two Folder descriptors do not identify the same directory;
- a prefixed Location is invalid, unsupported, duplicated, or collides with another persisted Location;
- state, schema, sidecar, transaction, or backup admission fails; or
- the proposed Folder cannot be scanned within configured resource limits.

A pre-commit failure leaves the existing binding and state unchanged. If the required post-commit scan has a root-level failure, the service must remain unready. The operator may retry after correcting that failure or restore the verified pre-expansion backup and prior Library Folder. Per-file Capture Time or Preview failures follow normal scan behavior and do not roll back valid sibling Photos. Original Files remain unchanged.

Ordinary startup with a mismatched Folder remains a hard failure. Ordinary rescan marks a missing Original File unavailable unless one persisted Content Fingerprint identifies exactly one unclaimed same-kind candidate; otherwise it must discover a file at another Location as new and must not transfer state by filename, Capture Time, camera ID, inode, or visual similarity.

## Persistence and Cache Semantics

Persisted Original File and Photo IDs are opaque after creation. Canonical v3 path-derived IDs remain valid opaque values, but deterministic v3 identity vectors are migration inputs only. Every ID assigned under this contract must come from a state-store-unique, Location-independent allocator.

SQLite schema version 4 introduced the stable identity fence. A v3-to-v4 migration preserves every row and existing ID in one admitted `BEGIN IMMEDIATE` transaction. Canonical writable state is now schema version 6. Its v4-to-v5 Album terminology migration and its v5-to-v6 independent-Photo and fingerprint migration preserve that identity fence and every user-owned value. Older binaries must reject newer state.

The v5-to-v6 migration splits each legacy paired Photo RAW-first. The RAW Original keeps the Photo ID and every user state reference, even when it is unavailable. The JPEG Original keeps its Original File ID and receives a new independent Photo ID with default decisions and no inherited Album memberships. Singleton Photos and remembered unavailable records keep their IDs and state. A Preview survives only when its legacy source matches the kept kind and its revision matches current facts; a sibling-JPEG-derived Preview for a kept RAW is invalidated. The migration runs as one atomic schema change with fail-closed admission, and fingerprint enrollment starts only after that transaction commits.

Expansion is admitted only against canonical v6 state and a verified pre-expansion v6 backup. Its one transaction changes the admitted Folder binding, persisted Locations, location-derived ordering values, and rebuildable derived facts. It must not change user-owned state.

Capture inspection facts, Preview/cache records, and File Location navigation whose inputs include the old Location may be reset or invalidated in that transaction. They are derived state and may be rebuilt from the same read-only Original File. Selection State, Rating, Albums, membership order, and saved Album positions must not be reset.

Rollback across the v3-to-v4 identity migration stops the newer process, restores the verified pre-migration v3 backup, restores the prior Library Folder configuration, and starts the compatible v3 image. Rollback across the v4-to-v5 Album migration restores its verified v4 backup and compatible v4 image. Rollback across the v5-to-v6 independent-Photo migration restores its verified pre-upgrade v5 backup and compatible v5 image. There is no in-place down migration. Rollback of an expansion while remaining on v6 restores the verified pre-expansion v6 backup and prior Library Folder.

## Deployment Boundary

The Docker bind source and container-visible path remain deployment facts. Read-only Original Folder navigation is derived from Published Library Locations according to [Physical File Locations and Virtual Albums](photo-organization.md); this design still does not require multiple roots, a Folder hierarchy in SQLite, or a new deployment-path abstraction. Compose must continue to mount the admitted Library Folder read-only and production acceptance must verify the exact source, target, and mode.

A later need to relocate the same logical Folder across hosts or mount points requires a separate decision. It must not be smuggled into Library expansion.

## Options

### Selected: Stable Persisted IDs and Explicit Ancestor Expansion

This keeps the current one-Library model, preserves user state, retains fail-closed storage admission, and adds only the operation required by the real Library layout.

### Rejected: Remove the Folder Binding and Rescan

The same file has a different relative Location after expansion. Current path-derived IDs would change, old Photos would become unavailable, and duplicate new Photos would appear. This fails open and makes state loss look like discovery.

### Rejected: Create a New State Database and Copy Decisions

An external copy would need to reproduce identity, recovery, membership, saved-position, Preview, and failure semantics outside the owning persistence boundary. It turns one domain operation into an ad hoc migration and makes rollback harder to audit.

### Rejected: Continue Assigning New IDs from Original Location

After expansion, a newly discovered sibling may occupy a Location that existed before prefixing. A path-derived allocator could collide with the preserved legacy ID. New IDs therefore require a Location-independent, store-unique allocator.

### Selected: Content-Fingerprint Location Recovery

A persisted full-content digest is the only first-product evidence strong enough to restore a moved Original File. Recovery requires one globally unambiguous, unclaimed, same-kind candidate with exact content. Every other case keeps both records untouched and defers to the Photographer. This restores identity for reliable moves without guessing or merging user state.

### Rejected: Filename, Metadata, Inode, or Similarity Relink

These signals occur in ordinary camera workflows but do not prove that two files are the same Original. Using them would transfer user state to a copy or a different photograph. A partial hash has the same failure with less collision resistance.

### Rejected: General Offline Relink Engine

An operator-driven relink surface would duplicate the bounded manual recovery contract outside the state store and would permit associations that no durable evidence supports.

### Rejected: Volume, Folder, and Multi-Root Asset Model

A Lightroom-style hierarchy can support many storage layouts, but Slipstream currently owns one Photographer, one Library, and one Folder. Volume records, multiple roots, content-addressable assets, and Folder management add a digital-asset-management model without a current product need.

### Rejected: Camera-Embedded ID as Original File Identity

Camera identifiers are optional, vendor-specific, and not reliably unique or stable across all supported files. They cannot own user state.

## Verification

Verification must prove:

- a v3-to-v4 migration preserves every existing Original File ID, Photo ID, row, and user-owned state and is rejected by a v3 binary;
- the v4-to-v5 Album migration preserves the same identity and user-owned state and is rejected by a v4 binary;
- the v5-to-v6 migration splits paired Photos RAW-first, preserves every RAW Photo ID and user state reference, gives each JPEG a new Photo ID with default decisions, preserves remembered unavailable records, and is rejected by a v5 binary;
- automatic recovery restores exactly one unambiguous unclaimed same-kind candidate, refuses equal copies and ambiguous groups, and never transfers state to a duplicate;
- an ancestor expansion preserves every existing Original File ID and Photo ID;
- all old Locations receive exactly one prefix and still resolve beneath the new Folder;
- a newly discovered sibling whose Location equals one legacy pre-expansion Location receives distinct Original File and Photo IDs;
- Selection State, Rating, Album membership/order, and saved Album positions are byte-for-byte equivalent projections before and after expansion;
- an unavailable Original File retains its state and prefixed Location;
- content fingerprints are revision-bound, resumable, and never inferred for Originals that were already missing;
- sibling directories add new Photos and File Locations without changing old Album positions;
- Capture Time order for a newly opened `All Photos` source includes the expanded Library while no open Browse Snapshot can exist because expansion requires stopped service;
- derived Preview/cache facts rebuild without modifying Original Files;
- non-ancestor, descriptor mismatch, Location collision, ID collision, sidecar, schema, and transaction failures leave the old state unchanged;
- a root-level post-commit scan failure exposes no ready service and supports retry or verified restore;
- rollback restores the correct verified v3, v4, v5, or v6 backup and prior Library Folder; and
- representative Original File hashes remain unchanged.
