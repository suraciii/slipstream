# Photo Library Identity and Expansion

Slipstream initially configured one shoot directory as the Library Folder. A Photographer now needs to expand that Library to an ancestor directory without losing the Photos and review state already recorded below the original Folder.

A filesystem location tells Slipstream where to find an Original File. It must not become the identity of the Original File or Photo. At the same time, Slipstream must not silently reinterpret one state database against an unrelated directory.

## Design Drivers

- Original Files remain read-only and may be irreplaceable.
- Selection State, Rating, Photo Set membership, membership order, and progress belong to existing Photos.
- One state store owns one Photo Library and one configured Library Folder.
- The current need is expansion to an ancestor directory, not arbitrary file relocation or multiple storage roots.
- Ordinary rescans must not guess that two paths refer to the same Original File.
- A failed expansion must leave the current Library and its state recoverable.

## Model

### Photo Library

One state store owns one Photo Library. The state store is already the Library boundary; a second Library UUID adds no value while one store cannot contain multiple Libraries.

### Library Folder

The Library Folder is the configured filesystem directory whose supported descendant files belong to the Photo Library. It owns read-only containment and discovery scope. Its absolute path is an admitted storage binding, not the identity of the Photo Library.

The admitted binding remains fail-closed during ordinary startup. A different configured Folder must not silently reinterpret existing Original Locations.

### Original File and Original Location

An Original File has one stable persisted identity. Its ID is opaque and remains unchanged after first discovery. The state store assigns every new ID independently of Original Location and rejects a collision before insertion.

An Original Location is the relative directory and filename used to find that Original File beneath the current Library Folder. The Location may change only through a supported Library expansion. Ordinary rescan does not infer relocation.

File size, modification time, device, and inode are revision or admission facts, not Original File identity.

### Photo

A Photo has one stable persisted identity and refers to one RAW Original File, one JPEG Original File, or an unambiguous pair. Its ID remains unchanged when a supported Library expansion changes its Original Locations. The state store assigns a new Photo ID independently of those Locations when reconciliation cannot preserve an existing Photo.

Selection State, Rating, Photo Set membership, membership order, and progress continue to refer to the stable Photo.

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
10. validate that every Original File ID, Photo ID, user decision, Photo Set membership position, and saved progress remains unchanged; and
11. commit the transaction, then complete a normal scan before reporting readiness.

Prefixing every old Location preserves same-directory RAW/JPEG pairing within the former Folder. The subsequent scan discovers supported files in sibling directories as new Original Files and Photos. It does not create Photo Sets from directories.

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

Ordinary startup with a mismatched Folder remains a hard failure. Ordinary rescan continues to mark a missing Original File unavailable and may discover a file at another Location as new; it must not transfer state by filename, Capture Time, camera ID, inode, or visual similarity.

## Persistence and Cache Semantics

Persisted Original File and Photo IDs are opaque after creation. Canonical v3 path-derived IDs remain valid opaque values, but deterministic v3 identity vectors are migration inputs only. Every ID assigned under this contract must come from a state-store-unique, Location-independent allocator.

SQLite schema version 4 is the writable identity fence for this contract. The exact canonical v4 shape may retain the existing tables, but it must set `PRAGMA user_version=4`. A v3-to-v4 migration preserves every row and existing ID in one admitted `BEGIN IMMEDIATE` transaction before any Library Expansion. A v3 binary must reject v4 as newer state.

Expansion is admitted only against canonical v4 state and a verified pre-expansion v4 backup. Its one transaction changes the admitted Folder binding, persisted Locations, location-derived ordering values, and rebuildable derived facts. It must not change user-owned state.

Capture inspection facts and Preview/cache records whose source revision includes the old Location may be reset or invalidated in that transaction. They are derived state and may be rebuilt from the same read-only Original File. Selection State, Rating, Photo Sets, membership order, and progress must not be reset.

Rollback across the identity migration stops the v4 process, restores the verified pre-migration v3 backup, restores the prior Library Folder configuration, and starts the compatible v3 image. There is no in-place down migration. Rollback of an expansion while remaining on v4 restores the verified pre-expansion v4 backup and prior Library Folder.

## Deployment Boundary

The Docker bind source and container-visible path remain deployment facts. This design does not require multiple roots, a Folder hierarchy in SQLite, or a new deployment-path abstraction. Compose must continue to mount the admitted Library Folder read-only and production acceptance must verify the exact source, target, and mode.

A later need to relocate the same logical Folder across hosts or mount points requires a separate decision. It must not be smuggled into Library expansion.

## Options

### Selected: Stable Persisted IDs and Explicit Ancestor Expansion

This keeps the current one-Library model, preserves user state, retains fail-closed storage admission, and adds only the operation required by the real Library layout.

### Rejected: Remove the Folder Binding and Rescan

The same file has a different relative Location after expansion. Current path-derived IDs would change, old Photos would become unavailable, and duplicate new Photos would appear. This fails open and makes state loss look like discovery.

### Rejected: Create a New State Database and Copy Decisions

An external copy would need to reproduce identity, pairing, membership, progress, Preview, and failure semantics outside the owning persistence boundary. It turns one domain operation into an ad hoc migration and makes rollback harder to audit.

### Rejected: Continue Assigning New IDs from Original Location

After expansion, a newly discovered sibling may occupy a Location that existed before prefixing. A path-derived allocator could collide with the preserved legacy ID. New IDs therefore require a Location-independent, store-unique allocator.

### Rejected: General Relink and Automatic Move Detection

Matching arbitrary moves by filename, metadata, inode, partial hash, or image similarity introduces ambiguous ownership decisions. The current requirement has one deterministic ancestor prefix and does not justify a generic relink engine.

### Rejected: Volume, Folder, and Multi-Root Asset Model

A Lightroom-style hierarchy can support many storage layouts, but Slipstream currently owns one Photographer, one Library, and one Folder. Volume records, multiple roots, content-addressable assets, and Folder management add a digital-asset-management model without a current product need.

### Rejected: Camera-Embedded ID as Original File Identity

Camera identifiers are optional, vendor-specific, and not reliably unique or stable across all supported files. They cannot own user state.

## Verification

Verification must prove:

- a v3-to-v4 migration preserves every existing Original File ID, Photo ID, row, and user-owned state and is rejected by a v3 binary;
- an ancestor expansion preserves every existing Original File ID and Photo ID;
- all old Locations receive exactly one prefix and still resolve beneath the new Folder;
- a newly discovered sibling whose Location equals one legacy pre-expansion Location receives distinct Original File and Photo IDs;
- Selection State, Rating, Photo Set membership/order, and progress are byte-for-byte equivalent projections before and after expansion;
- an unavailable Original File retains its state and prefixed Location;
- sibling directories add new Photos without changing old Photo Set positions;
- Capture Time order for future Library Review includes the expanded Library while active Sessions remain unaffected because expansion requires stopped service;
- derived Preview/cache facts rebuild without modifying Original Files;
- non-ancestor, descriptor mismatch, Location collision, ID collision, sidecar, schema, and transaction failures leave the old state unchanged;
- a root-level post-commit scan failure exposes no ready service and supports retry or verified restore;
- rollback restores the correct verified v3 or v4 backup and prior Library Folder; and
- representative Original File hashes remain unchanged.
