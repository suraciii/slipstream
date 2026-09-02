# Physical File Locations and Virtual Albums

Slipstream must expose the Photographer's existing directory organization without implying that ordinary product actions manage Original Files. It must also provide a virtual grouping model whose membership can cross directories. The two axes may share display names but must not share ownership, identity, lifecycle, or mutation semantics.

## Design Drivers

- The filesystem owns Original Files and their physical directories.
- SQLite owns virtual organization and user state.
- Original Files remain descriptor-confined and read-only.
- One Photo may represent a RAW Original and matching JPEG Original in one directory.
- A Photo may belong to multiple virtual groups without copying bytes.
- A Library may contain tens of thousands of Photos and directories.
- No browser route may materialize a complete Library, complete Album membership, or complete Folder tree without a bound.
- Ordinary rescan does not infer arbitrary moves or silently transfer state.
- The current need does not justify Folder persistence, Album hierarchy, rule-defined membership, or general asset management.

## Model

### Original Folder

An Original Folder is the admitted Library Folder itself or one of its real descendant filesystem directories. Descendant Folders are derived from the parent components of known Original Locations. The Library Folder owns confinement and discovery; an Original Folder owns no identity or user state.

The Library Folder always participates as the root File Location and is presented with the product label `Library Folder`, not its absolute server path. Other directories participate only when represented by at least one known Original Location. A remembered unavailable Original keeps its last known Location and therefore keeps its Folder projection. Other empty directories and directories containing no supported known Original do not become product objects.

An Original Folder path is a mutable Location, not identity. The root uses one explicit empty relative Folder Location. A descendant uses normalized valid UTF-8 components relative to the Library Folder. It never contains an absolute host path, `.` or `..`, an empty descendant component, or a NUL byte.

### Photo Folder Projection

A Photo projects to one Original Folder through its ordering Original Location:

1. the RAW Original Location when RAW exists;
2. otherwise the JPEG Original Location.

The RAW/JPEG pairing contract already requires an unambiguous pair to share one directory. The ordering rule nevertheless gives Folder projection one explicit owner and prevents a paired Photo from being counted twice.

A Folder source contains every Photo whose projected Folder equals the selected Folder or has it as a component-aware ancestor. Prefix text alone is insufficient: `a` is not an ancestor of `ab`.

### Album

An Album is a Photographer-owned virtual group with:

- one opaque stable ID;
- one case-insensitively unique name in the flat Album list: uniqueness folds ASCII letter case exactly as SQLite's NOCASE collation does, and scripts without letter case (such as Chinese) are unaffected;
- creation order;
- explicitly positioned Photo memberships; and
- optional saved Photo position.

An Album may be empty. One Photo may belong to multiple Albums. Album membership refers to stable Photo IDs and does not own Selection State, Rating, Preview facts, Original Locations, or Original Files.

Deleting an Album deletes its memberships and saved position. It does not delete or modify a Photo, Original File, Original Folder, Selection State, Rating, or Preview fact.

### Library Browser Source

The Library Browser opens one of three source kinds:

- `All Photos`: every Photo in Published Library Capture Time order;
- `Original Folder`: one recursive Folder subtree filtered from that same order; or
- `Album`: one persisted explicit membership order.

`All Photos` is a system source, not an Album. Original Folder and `All Photos` positions remain browser-local. Album saved position remains durable.

An open source copies one immutable ordered Photo-ID sequence into the existing hidden Browse Snapshot boundary. A later publication may change File Location navigation and newly opened sources, but it cannot insert, remove, or reorder an existing Browse Snapshot.

## Semantics

### File Location Navigation

File Location navigation reads one Published Library generation. It returns bounded windows of direct child Folders for one parent Folder. Each item exposes only:

- relative Folder Location;
- display name;
- recursive Photo count; and
- whether known descendant Folders exist.

A response also reports its parent, requested range, total direct-child count, and one opaque publication value. The first request may omit publication and binds to the current Published Library. Every retained later window and Folder-source open supplies that exact value. The server enforces a small maximum window. No Overview or File Location route returns every Folder or every Photo in a Folder subtree.

Direct-child Folder ordering uses relative Folder component bytes after UTF-8 validation. Folder count counts Photos, not Original Files, and counts a paired RAW/JPEG Photo once.

When a rescan replaces the current publication, a request carrying the old value fails as expired. The browser discards the old File Location tree and reloads one current publication instead of combining pages from different generations. The server need not retain an old complete Folder tree or create a second durable snapshot. Opening an Original Folder immediately creates a stable Browse Snapshot from the validated current publication.

### Folder Source Opening

The client submits one validated relative Original Folder Location, including the explicit empty value for the Library Folder root, together with the publication value that produced the navigation item. The server rejects an expired publication and an absolute, malformed, unknown, or non-directory projection. It does not reinterpret a stale Folder against a newer publication or open an arbitrary filesystem path supplied by the client.

The server filters the Published Library's ordered Photos by component-aware Folder ancestry and stores only their Photo IDs in the existing bounded-lifecycle Browse Snapshot. Window traversal, Preview hydration, expiration, reconnect, cancellation, and current-fact refresh use the existing browsing contracts.

A completed rescan may add or remove Folder nodes and may change newly opened Folder membership. An already open Folder source keeps its copied ID order. Ordinary external moves retain existing identity behavior: the prior Photo may remain unavailable at its remembered Folder and a Photo discovered at a new Location may receive a new identity.

### Album Mutations

Album create, rename, delete, membership, reorder, and saved-position mutations run through the existing single SQLite owner and admitted `BEGIN IMMEDIATE` boundary.

New members append in supplied order. Adding an existing member is an idempotent product action and must not create a duplicate row or position. Removing one member compacts later positions while preserving relative order. Only explicit reorder changes existing relative positions.

The browser may update presentation after confirmation. It must not abort an admitted Album or Photo-state persistence operation solely because the current source or Photo changes. UI continuation still belongs to the generation that initiated it and cannot overwrite a newer current source or error.

### Empty Albums

An empty Album remains a valid source with total count zero and position zero. The Web application must not disable it. Opening it creates an ordinary empty Browse Snapshot and displays a usable empty Grid with Album management controls.

### Persistence Migration

Canonical writable state uses Album language. SQLite schema version 5 uses `albums`, `album_members`, and `album_progress`, with `album_id` foreign-key columns and the `album_members_photo` reverse-membership index. It replaces active `photo_sets`, `photo_set_members`, `review_progress`, and Photo-Set-named columns.

The admitted v4-to-v5 migration runs in one `BEGIN IMMEDIATE` transaction and preserves:

- every Album ID, name, and creation order;
- every Photo ID and Original File ID;
- every membership and position;
- every saved Photo;
- Selection State and Rating;
- Preview and Capture Time facts; and
- the admitted Library Folder binding.

A v4 binary rejects v5 as newer state. Rollback restores the verified pre-migration v4 backup and compatible image. There is no in-place down migration.

Legacy `Photo Set` names remain only in immutable v2-v4 compatibility fixtures and the v4-to-v5 migration reader. Active Rust, protocol, Web, tests, and documentation use Album language. Legacy HTTP routes and browse-source values return `404` or validation failure rather than becoming indefinite aliases.

### Presentation

Wide and narrow navigation both use separate `File Locations` and `Albums` sections. A same-name Folder and Album remain distinguishable by section and current-source labeling. Folder sources identify that they include subfolders. Album deletion confirmation identifies that Photos and Original Files remain unchanged.

The first management surface supports Album create, rename, delete, current-Photo add, and current-Album removal. Grid multi-select, drag-and-drop, visual bulk reorder, Album covers, sharing, Album Groups, and Smart Albums are not required.

## Failure Behavior

A malformed or unknown Original Folder source fails without opening a different source or exposing an absolute path. A failed Folder window retains already loaded sibling navigation and identifies its failed range. An expired File Location publication replaces the retained tree with one current publication instead of mixing generations.

A root-level scan or publication failure cannot replace the prior Published Library. The prior publication and its File Locations remain authoritative under the existing background-rescan contract. A per-request Folder derivation failure returns an error without mutating that publication or Album state.

An Album mutation conflict or persistence failure leaves Original Files and unrelated state unchanged. The Web retains a recoverable source and identifies the failed action. It must not report a mutation as complete before confirmation.

Schema migration fails closed before service admission when the v4 shape is not canonical, a sidecar exists, a transaction fails, or the resulting v5 shape is not exact. The pre-migration database remains recoverable from the required verified backup.

## Options

### Selected: Album as the Canonical Virtual Group

`Album` is familiar product language and directly expresses virtual photo grouping. It avoids implying a favorite state and works for one Photo belonging to multiple groups.

### Rejected: Retain Photo Set Internally and Rename Only the UI

This leaves one concept with different product, protocol, persistence, and code names. It amplifies every future change and conflicts with the repository rule against preserving obsolete paths without a required compatibility contract.

### Rejected: Collection or Favorites

`Collection` is specialized Lightroom Classic language, while `Favorites` implies a preference state. Both conflict with Slipstream's Selection State and Rating language.

### Selected: Derive Original Folders from Published Original Locations

The directory hierarchy is filesystem-owned and rebuildable. Deriving it preserves one source of truth, includes remembered unavailable Locations, and avoids migration or synchronization rules for a second Folder model.

### Rejected: Persist Folder Entities and Membership

Persistent Folder IDs, parent rows, membership rows, rename reconciliation, and empty-directory lifecycle would duplicate filesystem facts and introduce an asset-management model without a current requirement.

### Selected: Recursive Folder Sources

A Photographer opening a shoot directory normally expects Photos in nested camera or date directories. One fixed recursive rule is visible and predictable.

### Rejected: Direct Children Only or a Persistent Include-Subfolders Toggle

Direct-only sources make common project directories appear empty. A toggle adds hidden state that changes counts, order, navigation, and resume behavior without solving a demonstrated workflow.

### Selected: Bounded Direct-Child Folder Windows

This keeps transfer and retained navigation proportional to the visible tree while preserving ordinary hierarchical exploration.

### Rejected: Complete Folder Tree in Library Overview

Folder count grows with Library layout. Returning the complete tree would make startup transfer and browser memory Library-size dependent and would recreate the unbounded protocol shape already retired for Photos.

### Rejected: Automatic Folder-to-Album Synchronization

A synchronized object has ambiguous ownership when the filesystem changes and makes Album removal appear to fight rescan. Explicit Album membership keeps physical and virtual organization independent.

## Verification

Verification must prove:

- exact v4-to-v5 migration and rollback preservation;
- absence of active `Photo Set` names outside legacy migration inputs;
- Album identity, unique names, empty state, order, saved position, idempotent add, removal compaction, and deletion safety;
- empty-Library root, root-level Photos, nested, Unicode, same-prefix, paired, JPEG-only, ambiguous, and remembered unavailable Folder projection;
- bounded direct-child Folder windows, single-publication pagination, expiration refresh, and rejection of malformed or absolute Locations;
- no complete Folder tree or complete Folder membership route;
- Folder source filtering by component-aware ancestry in Capture Time order;
- stable open Folder and Album Browse Snapshots through rescan;
- same-name File Location and Album presentation on wide and narrow viewports;
- bounded browser-retained facts and DOM for a generated Library with at least 40,000 Photos and a large Folder hierarchy;
- foreground source and current-Photo priority while Folder, thumbnail, and Preview work remains cancelable background work; and
- unchanged representative Original File hashes before and after migration, browsing, Album mutations, rescan, backup restore, and rollback rehearsal.
