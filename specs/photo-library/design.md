# Physical File Locations and Virtual Albums

Slipstream MUST expose the Photographer's existing directory organization without implying that ordinary product actions manage Original Files.

It MUST also provide a virtual grouping model whose membership can cross directories.

The two axes MAY share display names but MUST NOT share ownership, identity, lifecycle, or mutation semantics.

[Photo Library and Albums](spec.md) owns product behavior. [Library Identity](identity.md) owns stable identity and Library Folder expansion. [Capture-Time Ordering](ordering.md) owns source ordering and its metadata authority.

The filesystem owns Original Files and physical directories; SQLite owns virtual organization and user state.

Folder navigation and source loading MUST remain bounded for Libraries containing tens of thousands of Photos and directories.

Foreground source and current-Photo work MUST take priority over cancelable Folder, thumbnail, and Preview background work.

## Model

### Original Folder

[Original Folders](../../CONTEXT.md#library) are derived from parent components of known Original Locations.

An Original Folder owns no identity or user state.

The Library Folder always participates as the root File Location and is presented with the product label `Library Folder`, not its absolute server path.

Other directories participate only when represented by at least one known Original Location.

A remembered unavailable Original keeps its last known Location and therefore keeps its Folder projection.

Other empty directories and directories containing no supported known Original do not become product objects.

An Original Folder path is a mutable Location, not identity.

The root uses one explicit empty relative Folder Location.

A descendant uses normalized valid UTF-8 components relative to the Library Folder.

It never contains an absolute host path, `.` or `..`, an empty descendant component, or a NUL byte.

### Photo Folder Projection

A Photo projects to one Original Folder through its own Original Location.

Independent Photos that share a directory or base name project separately, so each Photo is counted once.

A Folder source contains every Photo whose projected Folder equals the selected Folder or has it as a component-aware ancestor.

Prefix text alone is insufficient: `a` is not an ancestor of `ab`.

### Album

An Album is a Photographer-owned virtual group with:

- one opaque stable ID;
- one case-insensitively unique name in the flat Album list: uniqueness folds ASCII letter case exactly as SQLite's NOCASE collation does, and scripts without letter case (such as Chinese) are unaffected;
- creation order;
- explicitly positioned Photo memberships; and
- optional saved Photo position.

An Album MAY be empty.

One Photo MAY belong to multiple Albums.

Album membership refers to stable Photo IDs and does not own Selection State, Rating, Preview facts, Original Locations, or Original Files.

Deleting an Album deletes its memberships and saved position.

It does not delete or modify a Photo, Original File, Original Folder, Selection State, Rating, or Preview fact.

### Library Browser Source

The Library Browser opens one of three source kinds:

- `All Photos`: every Photo in Published Library Capture Time order;
- `Original Folder`: one recursive Folder subtree filtered from that same order; or
- `Album`: one persisted explicit membership order.

`All Photos` is a system source, not an Album.

Original Folder and `All Photos` positions remain browser-local.

Album saved position remains durable.

An open source copies one immutable ordered Photo-ID sequence into the existing hidden Browse Snapshot boundary.

A later publication MAY change File Location navigation and newly opened sources, but it cannot insert, remove, or reorder an existing Browse Snapshot.

## Semantics

### File Location Navigation

File Location navigation reads one Published Library generation.

It returns bounded windows of direct child Folders for one parent Folder.

Each item exposes only:

- relative Folder Location;
- display name;
- recursive Photo count; and
- whether known descendant Folders exist.

A response also reports its parent, requested range, total direct-child count, and one opaque publication value.

The first request MAY omit publication and binds to the current Published Library.

Every retained later window and Folder-source open supplies that exact value.

The server enforces a small maximum window.

No Overview or File Location route returns every Folder or every Photo in a Folder subtree.

Direct-child Folder ordering uses relative Folder component bytes after UTF-8 validation.

Folder count counts Photos, not Original Files, and counts each Photo once.

When a rescan replaces the current publication, a request carrying the old value fails as expired.

The browser discards the old File Location tree and reloads one current publication instead of combining pages from different generations.

The server need not retain an old complete Folder tree or create a second durable snapshot.

Opening an Original Folder immediately creates a stable Browse Snapshot from the validated current publication.

### Folder Source Opening

The client submits one validated relative Original Folder Location, including the explicit empty value for the Library Folder root, together with the publication value that produced the navigation item.

The server rejects an expired publication and an absolute, malformed, unknown, or non-directory projection.

It does not reinterpret a stale Folder against a newer publication or open an arbitrary filesystem path supplied by the client.

The server filters the Published Library's ordered Photos by component-aware Folder ancestry and stores only their Photo IDs in the existing bounded-lifecycle Browse Snapshot.

Window traversal, Preview hydration, expiration, reconnect, cancellation, and current-fact refresh use the existing browsing contracts.

A completed rescan MAY add or remove Folder nodes and MAY change newly opened Folder membership.

An already open Folder source keeps its copied ID order.

A rescan restores a moved Original File only when one persisted Content Fingerprint identifies exactly one unclaimed same-kind candidate. Otherwise the prior Photo remains unavailable at its remembered Folder, and a file discovered at a new Location receives a new identity.

### Location Recovery Application

Automatic Location Recovery runs inside the scan application, before any new Original or Photo is allocated.

Manual Location Recovery reuses the same rules from an explicit Photographer-approved batch.

A recovery batch runs in one `BEGIN IMMEDIATE` transaction.

Before writing, it revalidates every remembered Original File, its kind, and its destination Location against current state.

It assigns affected Original associations through temporary unique Location values, so known associations MAY exchange Locations without violating the unique Location constraint; it never moves the filesystem files.

A batch is all-or-nothing: a stale, colliding, or occupied destination without a permitted Retire and Bind rejects the whole batch with per-mapping reasons and no partial association.

A destination Location MAY already belong to a Photo discovered by an earlier scan.

Retire and Bind is allowed only when that destination Photo is otherwise unreferenced: it has default decisions, no Album membership, no Trash removal, and no retained workspace state, Edit Recipe, or referenced Processing Artifact.

The eligibility check reads the authoritative workspace and processing records at proposal and commit, not only a scan-time summary.

The transaction removes exactly that record before binding the recovered Original.

It MUST NOT delete a filesystem file.

When the destination Photo has independent user state, a retained development reference, or a removal, the transaction preserves both records and reports the conflict.

A moving record MUST be an active Photo of the Library.

Location Recovery never restores a Photo from Trash and never reverses Trash Record Cleanup; a removed record keeps its own restore contract.

A file discovered after cleanup first follows normal Location Recovery for other retained identities; only an unclaimed file receives a new identity.

Neither case recovers the cleaned-up Photo's identity or state.

A relocation resets the affected Original's derived Capture Time facts to `pending` and invalidates location-derived Preview facts.

Selection State, Rating, Album membership and order, and saved Album positions remain unchanged.

### Reviewed Location Recovery Access

Web and CLI share one review-and-confirm protocol over the same rules.

A Photographer MAY review a correspondence directly or delegate preparation and application of explicitly authorized mappings to an external Agent.

Neither client owns eligibility, evidence, or outcome rules of its own, and delegated use never requires browser automation or the server database.

Unavailable Photos are inspected through bounded reviews.

Opening a review evaluates the current set of active unavailable Photos once and retains the ordered Original IDs with their Photo IDs.

Later pages resolve current facts for the retained identities, so a recovered, newly unavailable, or removed Photo cannot silently shift another item out of its reviewed position.

A review reports its evaluated total, evaluation time, and continuation.

A continuation expires after the advertised idle period and then fails explicitly instead of returning a different set.

A proposal is read-only.

It evaluates one correspondence per unavailable Original: a remembered Location under one Folder prefix mapped onto another prefix with the same suffix, or one remembered Original File mapped onto one Location.

Every evaluated mapping carries an opaque reviewed mapping identity.

The reviewed mapping identity is a deterministic function of:

- Original File and Photo identity;
- remembered and proposed Locations;
- kind and content evidence;
- observed destination revision;
- the destination Photo that Retire and Bind MAY replace; and
- any reason that blocks the mapping.

Equal facts produce an equal identity, so no durable proposal record is required: a reviewed mapping stays confirmable across a service restart and loses validity when any reviewed fact changes.

Confirmation names the reviewed mapping identity of every submitted mapping.

Apply recomputes each mapping from current facts and refuses the complete submitted batch when a recomputed identity differs from the submitted one.

This single rule binds confirmation to the reviewed Original, Location, content evidence, and destination revision, and it prevents a retired permission from transferring to a different Photo that later occupies the same Location.

When an unavailable Original has no persisted fingerprint, its historical content cannot be verified against the destination. Confirmation MUST explicitly acknowledge that limitation.

The reviewed destination revision is its observed state. A destination that changes after review MUST refuse the mapping instead of inheriting that acknowledgement.

A missing, unreadable, wrong-kind, or content-mismatched destination MUST NOT be accepted as an unverified match.

Retire and Bind requires one explicit choice for the identified destination Photo.

The request names that Photo, and apply refuses an absent or different occupant even when the mapping identity matches.

A destination Photo with independent decisions, Album membership, saved Edit Recipe, retained workspace state, referenced Processing Artifacts, or a Trash removal is never disposable, and its conflict is reported instead of resolved.

A Folder-prefix proposal reports its complete evaluated scope.

A scope larger than the advertised mapping bound is refused before any continuation is issued instead of being truncated to one page or one apply operation.

Apply accepts only the explicit confirmed mappings of one request and commits them atomically; separate requests are separate commits, and a later refusal never rolls back an earlier committed batch.

A lost apply response leaves the outcome unknown.

The caller inspects the same Photo identities and compares current Location and availability; current state is not evidence of which attempt changed it.

New work requires a fresh proposal and confirmation, and no client replays an unconfirmed request.

### Fingerprint Persistence

[Content Fingerprints](../../CONTEXT.md#library) are revision-bound recovery evidence, not unique keys. [Photo Library Identity](identity.md#content-fingerprint) owns persistence and enrollment.

### Album Mutations

Album create, rename, delete, membership, reorder, and saved-position mutations run through the existing single SQLite owner and admitted `BEGIN IMMEDIATE` boundary.

New members append in supplied order.

Adding an existing member is an idempotent product action and MUST NOT create a duplicate row or position.

Removing one member compacts later positions while preserving relative order.

Only explicit reorder changes existing relative positions.

A bounded batch membership result MUST identify `addedPhotoIds` and `alreadyMemberPhotoIds` in request order.

The browser MAY retain `addedPhotoIds` as a page-level compensation record.

`POST /api/albums/{albumId}/members/batch-remove` accepts only that record.

It removes listed members still present and reports `removedPhotoIds` and `alreadyAbsentPhotoIds`.

Compensation MUST be idempotent for retries and MUST NOT remove an ID reported as already a member.

It is not part of global Selection State Undo.

The browser MUST expire the compensation record on source change, a new membership operation, target Album deletion, page teardown, page reload, or application teardown.

Removing the saved Photo updates the Album's saved position through the removal-compaction rule.

The browser MAY update presentation after confirmation.

It MUST NOT abort an
admitted Album or Photo-state persistence operation solely because the current
source or Photo changes.

UI continuation still belongs to the generation that
initiated it and cannot overwrite a newer current source or error.

A late
compensation result is applied only while its page-level record and Album
source remain current.

### Folder-to-Album Add

The Folder source owns the input set for an explicit bulk membership action.

The browser supplies the validated relative Folder Location and the exact
Published Library value used to open that source.

The server resolves the
Folder recursively from that publication and appends the resulting ordered
Photo IDs in one SQLite transaction.

The browser never materializes or sends
the complete Folder membership list.

The operation is bounded at 100,000 Photos.

It validates the Album, every
resolved Photo identity, and the publication before committing any membership.

Existing members are skipped without changing their positions.

The operation
returns matched, added, and already-member counts.

An expired publication,
unknown Folder, unknown Album, or limit violation fails without changing Album
membership.

A successful operation does not bind the Album to the Folder;
later scans do not add future Photos automatically.

### Empty Albums

An empty Album remains a valid source with total count zero and position zero.

The Web application MUST NOT disable it.

Opening it creates an ordinary empty Browse Snapshot and displays a usable empty Grid with Album management controls.

### Persistence Migration

SQLite schema version 6 stores exactly one `original_id` (`TEXT NOT NULL UNIQUE`) on each Photo row and adds `original_fingerprints` keyed by Original File ID. It replaces the v5 paired-Photo columns and retired Preview Source names.

The admitted v5-to-v6 migration runs in one `BEGIN IMMEDIATE` transaction. [Identity and expansion](identity.md#persistence-and-cache-semantics) owns its preservation, Preview, enrollment, and rollback rules.

Legacy `Photo Set` names remain only in immutable v2-v4 compatibility fixtures and the v4-to-v5 migration reader.

Legacy paired-Photo values and the old Preview Source names remain only in immutable v5 fixtures and the v5-to-v6 migration reader.

Active Rust, protocol, Web, tests, and documentation use Album language and independent-Photo language.

Legacy HTTP routes and browse-source values return `404` or validation failure rather than becoming indefinite aliases.

### Presentation

Wide and narrow navigation both use separate `Folders` and `Albums` sections. `Folders` is the UI label for the File Location source defined by this document.

A same-name Folder and Album remain distinguishable by section and current-source labeling.

Folder sources identify that they include subfolders.

Album deletion confirmation identifies that Photos and Original Files remain unchanged.

The first management surface supports Album create, rename, delete, current-Photo add, current-Album removal, adding the current recursive Folder to an Album, and the Grid's multi-selection for batch Selection State decisions and one batch Album addition. Drag-and-drop, visual bulk reorder, Album covers, sharing, Album Groups, and Smart Albums are not required.

## Failure Behavior

A malformed or unknown Original Folder source fails without opening a different source or exposing an absolute path.

A failed Folder window retains already loaded sibling navigation and identifies its failed range.

An expired File Location publication replaces the retained tree with one current publication instead of mixing generations.

An Album mutation conflict or persistence failure leaves Original Files and unrelated state unchanged.

The Web retains a recoverable source and identifies the failed action.

It MUST NOT report a mutation as complete before confirmation.

A schema migration fails closed before service admission when the source shape is not canonical, a sidecar exists, a transaction fails, or the resulting current-schema shape is not exact.

The pre-migration database remains recoverable from the required verified backup.
