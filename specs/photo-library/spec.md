# Photo Library and Albums

Slipstream MUST add selection and grouping without requiring an import copy or proprietary file layout.

## Opening a Photo Library

The server operator configures one Library Folder.

Slipstream MUST index supported files below that Folder, including files in nested directories.

Unsupported and unrelated files MUST remain unchanged and MUST NOT appear as Photos.

The Library Folder defines discovery and read-only containment.

Its filesystem location MUST NOT define the identity of an existing Original File or Photo.

An Original Folder is the Library Folder itself or a real descendant directory represented by one or more known Original Locations.

Slipstream MUST present Original Folders as read-only File Locations.

The Library Folder remains the root File Location even when the Library is empty.

Slipstream MUST NOT present any other empty directory or directory containing no known supported Original as a product object.

Indexing MUST NOT move, rename, rewrite, or delete an Original File or Original Folder.

Selecting an arbitrary server filesystem directory from the browser is not required.

If the configured Library Folder does not exist, is not a directory, or cannot be read, Slipstream MUST identify the failure and MUST NOT present a partially indexed Folder as current.

Library admission MUST fail closed for a root binding, schema, sidecar, or confinement admission failure.

## Photos

Slipstream presents one Photo for one independently managed supported Original File.

RAW and JPEG Originals are independent Photos even when they share a directory and base name.

Naming alone MUST NOT share Selection State, Rating, Album membership, or Preview.

The first product supports RAW and JPEG Originals.

It does not add TIFF support.

A JPEG Original is not a disposable derivative.

It remains an Original File and MUST NOT be modified.

## Stable Identity

An Original File and Photo MUST keep their persisted identities after first discovery.

Their current Original Locations MUST NOT be their identities.

Slipstream MUST retain Album membership, Selection State, Rating, and saved Album positions across an ordinary rescan when an Original File remains recoverable.

A moved or renamed Original File MUST keep its identity when a scan can prove one exact content match under the Location Recovery rules below.

Ordinary rescan MUST NOT guess a move or silently transfer state by filename, Capture Time, camera metadata, inode, content similarity, or another heuristic.

An unavailable Photo MUST retain its recorded state until the Photographer removes it from Slipstream.

Slipstream MUST NOT silently transfer that state to a different file.

## Content Fingerprints

Slipstream MUST persist [Content Fingerprints](../../CONTEXT.md#library) as revision-bound recovery evidence.

Independent Originals MAY share one fingerprint, and changed content MUST NOT keep an old fingerprint.

Slipstream MUST establish fingerprints automatically in the background.

Enrollment MUST:

- read Originals through the existing confined read-only access;
- keep memory independent of file size and cancel between chunks;
- run one bounded worker that yields to foreground Preview work;
- pause while a scan runs;
- resume incomplete enrollment after restart; and
- verify the source revision before and after hashing.

Slipstream MUST report truthful enrollment progress.

It MUST NOT promise instant readiness: the first enrollment requires approximately one complete read of every Original File without a fingerprint.

Slipstream MUST NOT fingerprint an Original File larger than 4 GiB.

Such a file keeps no automatic recovery evidence and remains eligible for manual recovery only.

Automatic recovery applies only to fingerprints completed before an Original became unavailable.

Slipstream MUST NOT infer historical fingerprints for Originals that were already missing.

## Location Recovery

Slipstream MUST recover a moved Original File when a scan can prove one exact match.

Automatic recovery applies when all of these conditions hold:

- a remembered Original File is missing at its Original Location;
- exactly one candidate Original of the same supported kind exists below the current Library Folder with the exact same content and no other owner; and
- a persisted fingerprint proves that content.

A recovered Original File keeps its Original File ID and Photo ID.

Its Photo keeps Selection State, Rating, Album membership and order, and saved Album positions.

Slipstream MUST resolve recovery before it allocates and publishes new Photos.

It MUST NOT publish temporary duplicates and later merge them.

When known files exchange Locations in one scan, Slipstream MUST reconcile the provable content permutation.

If the old Original File remains and an equal copy appears, Slipstream MUST discover an independent Photo at the new Location and MUST NOT transfer state.

When the remembered Location instead provably carries different bytes and exactly one exact copy of the remembered content exists elsewhere, the identity follows the copy.

If multiple equal candidates or multiple owners exist, Slipstream MUST preserve the unresolved records and MUST NOT choose by enumeration order, basename, Capture Time, camera metadata, inode, or visual similarity.

The Photographer resolves that group through manual recovery.

Changed content plus a changed Location is not guaranteed to recover automatically.

Incomplete traversal, inaccessible storage, uncertain candidate revisions, and unreadable candidates MUST NOT be treated as proof of absence or uniqueness.

Location Recovery searches only supported descendants of the admitted Library Folder.

A file in an external directory, operating-system recycle area, backup, or unrelated Library Folder is not a recovery candidate.

If the Photographer restores such a file under the Library Folder outside Slipstream, a later Library Check MAY discover it and evaluate it under the normal recovery rules.

Slipstream MUST NOT copy, move, rename, upload, overwrite, or delete that file.

Moving the configured Library Folder itself remains a Library Expansion.

A fingerprint match, a proposed mapping, or an occupied candidate is not a recovery.

Scan status and apply results MUST report only associations that were actually committed.

Slipstream MUST NOT create a persistent recovery-history or recovery-dashboard surface; current scan status, the bounded review, and the current apply result are sufficient.

The recovery review MUST NOT classify a matching fingerprint as an automatic recovery or as a duplicate merge.

A candidate already owned by another Photo is an occupied or independently owned mapping that remains unresolved unless the Photographer explicitly confirms a permitted Retire and Bind.

Multiple unavailable Photos sharing one fingerprint MUST remain separate records until the Photographer selects an explicit mapping; Slipstream MUST NOT choose by order, filename, Capture Time, or visual similarity.

## Manual Recovery

Every unavailable Original File belonging to an active Photo MUST remain listed with its remembered Location, filename, kind, Rating, Selection State, Album count, and whether a fingerprint exists.

Here active means the Photo is not Removed and its Original is currently unavailable.

Removed Photos remain in [Trash](trash/spec.md); Location Recovery MUST NOT restore them or reverse Trash Record Cleanup.

A file discovered after cleanup follows normal recovery for other retained identities first; only when none qualifies does discovery assign a new identity.

Neither case recovers the cleaned-up identity or its discarded state.

The Photographer MUST be able to review unavailable Originals and propose new Locations through one bounded `Review unavailable Originals` entry from the scan result or an affected Photo.

Slipstream MUST support two proposal forms:

- a batch mapping from one old Folder prefix to one new Folder prefix that keeps each relative suffix; and
- a single mapping from one remembered Original File to one new Location for a renamed or split file.

The Web recovery surface MUST present these mapping actions before its progressive list of unavailable Originals.

The list and mapping results MUST scroll without displacing the task controls or Close action.

Each listed Original MUST retain its remembered facts and offer a direct way to choose it for a single mapping.

Photo links MUST use a recognizable filename or Location instead of an opaque identity.

A proposal MUST remain a separate read-only step before explicit Apply.

Every proposed mapping MUST be inspectable before it is applied, and proposal MUST NOT write.

A proposal for an Original File without a persisted fingerprint MUST state that the old content cannot be verified and MUST require explicit confirmation.

A fingerprinted Original File MUST be verified against its persisted fingerprint at proposal and at commit.

The user-facing review presents each proposal as an old Location and a new Location, with one plain-language evidence state: content confirmed, content not confirmed, or destination occupied.

It MUST NOT require the Photographer to understand mapping identities, revision facts, or internal reason codes.

Slipstream MUST apply a batch atomically with revalidation at commit.

It MUST reject the whole batch with per-mapping reasons when any mapping is stale, colliding, or occupied without a permitted Retire and Bind, and it MUST NOT apply a partial association.

A destination Location MAY already belong to a Photo discovered by an earlier scan.

Slipstream MUST NOT silently merge the two Photos, and default Rating or Selection State MUST NOT be proof that the destination Photo is disposable.

Slipstream MAY offer an explicit Retire and Bind action only when the destination Photo is otherwise unreferenced: it has default decisions, no Album membership, no Trash removal, no retained workspace state, no Edit Recipe, and no referenced Processing Artifact.

The eligibility check MUST use the authoritative workspace and processing records, not only a scan-time summary.

It MUST show which record will be retired.

If the destination Photo has independent user state or a retained development reference, Slipstream MUST preserve both Photos and report the conflict.

Recovery MUST NOT delete any filesystem file.

Recovery MUST use server-relative Library Locations.

It MUST NOT require a client file upload or expose an arbitrary server path.

Recovery MUST NOT import or write an XMP Sidecar and MUST preserve the Photo's Rating in Slipstream.

### Human and Agent Access

The Web and CLI MUST support the same inspection, proposal, confirmation, and
application capabilities.

A Photographer MAY review the correspondence directly
or ask an external Agent to prepare and apply explicitly authorized mappings.

Delegated recovery MUST NOT require browser automation, access to the server's
database, or a client copy of an Original File.

Unavailable-Photo inspection MUST provide stable Photo and Original File
identities, the remembered facts above, the current availability, and an
addressable Photo destination.

Results MUST be paged within advertised finite
limits and identify their evaluated scope, total, and continuation.

Continuation
MUST keep the evaluated membership; current facts MAY change.

A recovered or
removed item MUST NOT silently shift another item into its result position.

Expired continuation MUST fail explicitly.

An empty result means that no active
Photo in that evaluated scope needs recovery, not that missing file bytes have
been restored.

The caller MUST also be able to inspect an identified Photo's current Original
association and Location after it leaves the unavailable results.

Location
Recovery MUST leave the Photo's identity addressable for result inspection.

### Review Scope and Confirmation

Both proposal forms MUST return the affected Photo and Original File identities,
remembered and proposed Locations, content-verification status, and any reason
the mapping cannot be applied.

An occupied destination MUST identify the Photo
that would be retired and whether retirement is permitted. Missing, unreadable,
wrong-kind, content-mismatched, colliding, and independently owned destinations
MUST remain distinguishable.

The absence of a fingerprint MUST NOT be presented
as a content match.

Folder-prefix proposals MUST report the complete matching scope and support
bounded inspection.

A result MUST NOT silently substitute a first page or
newest subset for the complete proposal.

A later matching Photo MUST NOT join
an already reviewed set.

The caller chooses explicit mappings from that set;
applying a prefix or a live query MUST NOT choose additional targets.

Confirmation MUST refer to the exact correspondence reviewed, including the
remembered Original, proposed destination revision, content-verification
status, and any destination Photo to retire.

Confirming recovery without a
fingerprint MUST explicitly acknowledge that the old content cannot be verified.

Retire and Bind requires a separate explicit choice for that identified
destination Photo.

Neither permission follows from a general request to fix
unavailable Photos, a matching filename, or default Photo decisions.

Before committing, Slipstream MUST revalidate the reviewed facts and all
eligibility rules.

A changed Original Location, a changed destination revision
or owner, a newly removed Photo, or a destination that gained independent user
state MUST reject the affected batch.

The caller MUST inspect a fresh proposal
before confirming again.

A retirement permission MUST NOT transfer to a
different Photo that later occupies the same Location.

One apply operation MUST contain a nonempty explicit set within the advertised
mapping limit.

Invalid request-wide input MUST be refused before mutation.

If any mapping fails revalidation, none in that batch MAY change.

The result
MUST identify the refused mappings and explain that valid siblings were not
applied.

Multiple bounded operations are separate commits; earlier successful
batches are not rolled back when a later batch fails.

No interface MAY promise
one atomic operation over an arbitrarily large prefix result.

### Results and Continuation

A successful result MUST identify each recovered Photo and Original File, its
previous and current Location, and any explicitly retired destination record.

Applied and refused counts MUST describe that submitted batch.

Any remaining
active unavailable count MUST be identified separately; it counts only Photos
that are not Removed and whose Original is unavailable.

The same Photo
MUST keep its Selection State, Rating, Album membership and order, and saved
Album positions.

Other Photos MUST remain unchanged except for the explicitly
permitted retirement.

Recovery changes an association; it MUST NOT move, rename, overwrite, upload, or
delete file bytes.

A successful mapping is not proof that missing bytes were
recreated.

The [Preview](previews/spec.md) MUST belong to the recovered Original;
cached content from the old Location MUST NOT masquerade as current evidence.

Existing [Edit Recipe and Export source rules](../photo-development/spec.md) still
apply.

Location Recovery MUST NOT silently rebind an Edit Recipe or recapture
an existing Export's source or settings.

A changed Original Location MUST invalidate earlier metadata read evidence
under [Sidecar Association](metadata/spec.md#sources-and-association).

The caller MUST inspect metadata again before Save.

Recovery MUST NOT relocate
the old XMP Sidecar, copy its values to a new one, or apply its external Rating
to the Library Rating.

If a response is lost after application MAY have started, the caller MUST
receive an unknown outcome and retain the reviewed mappings.

It MUST inspect
the same Photo identities before deciding what remains.

Current Location and
availability establish current state, not which attempt changed it.

Clients
MUST NOT silently retry application, retire a newly observed occupant, or
describe a current-state match as proof of a historical success.

New work
requires a fresh proposal and confirmation for the remaining explicit set.

## Expanding a Photo Library

The server operator MAY expand the Photo Library by replacing its current Library Folder with an ancestor directory that contains it.

This is a controlled Library operation, not Location Recovery.

Before changing state, Slipstream MUST prove that the current Library Folder is the same directory found beneath the proposed Folder.

It MUST NOT resolve individual file moves as part of an expansion.

A successful expansion MUST:

- preserve every existing Original File and Photo identity;
- preserve Selection State, Rating, Album membership and order, and saved Album positions;
- preserve remembered unavailable Photos;
- discover supported files outside the former Folder as new Photos;
- leave Album membership unchanged unless the Photographer changes it; and
- leave every Original File unchanged.

Slipstream MAY invalidate and rebuild Capture Time inspection facts, Preview facts, cached derivatives, and derived Original Folder navigation when their inputs include an old Location.

These are derived state and MUST NOT replace or reset Selection State, Rating, Albums, membership order, or saved Album positions.

Expansion requires a stopped Library and a verified backup.

If Slipstream cannot prove the ancestor relationship or preserve every remembered Original Location without conflict, it MUST reject the expansion without changing the current Library.

The first product does not support a continuous filesystem watcher, global disk search, multiple Library Folders, or moving the Library to an unrelated directory.

For example, expanding `/photos/26-spring` to `/photos` keeps `26-spring/a.ARW` as the same Original File and discovers supported files in sibling directories.

Changing `/photos/26-spring` to unrelated `/archive` is not a Library Expansion.

## Capture Time

Slipstream MUST use each Photo's own optional Capture Time to order `All Photos` and Original Folder sources, without changing Album membership order. [Capture-Time Library Ordering](ordering.md) owns field precedence, normalization, fallback exclusions, and publication semantics.

Missing, invalid, or failed capture metadata MUST NOT make an otherwise readable Photo unavailable.

A Photo without an authoritative Capture Time remains browsable in the missing-time partition.

An unavailable Original retains its last successfully inspected Capture Time.

When it returns with changed revision facts, Slipstream MUST replace that fact with the result of inspecting its current bytes.

## Physical and Virtual Organization

Original Folders and Albums are separate organization axes.

An Original Folder answers where Original Files are known to exist.

Its membership is derived from Original Locations and changes only when a completed scan publishes added, removed, or changed Locations.

Remembered unavailable Originals remain projected at their last known Locations.

An Original Folder does not own Photos, Selection State, Rating, or saved position.

For Folder browsing, a Photo belongs to the parent directory of its own Original Location.

Independent Photos that share a base name therefore appear once each.

Selecting an Original Folder MUST include Photos in that Folder and every descendant Folder.

It MUST include remembered unavailable Photos at their last known Original Locations.

The interface MUST identify this recursive behavior instead of implying that only direct children are shown.

An Album answers how the Photographer wants to use or organize Photos.

The Photographer MAY create, rename, and delete an Album.

An Album MAY be empty and MUST remain openable and manageable while empty.

An Album contains explicitly ordered references to Photos.

Its membership positions are authoritative whenever the Photographer browses that Album.

Capture metadata, Original Folder changes, and rescans MUST NOT silently change those positions.

One Photo MAY belong to multiple Albums.

New members append in the order supplied by the add operation.

Only an explicit reorder operation MAY change the order of existing members.

Deleting an Album MUST NOT delete or modify a Photo, Original Location, Original Folder, or Original File.

The Photographer MAY add or remove Photos from an Album.

A Photo's Selection State and Rating belong to the Photo, not to one Album membership.

The same decision therefore appears in every Album that contains the Photo.

Indexing a directory MUST NOT automatically create an Album.

A Folder and an Album MAY have the same display name, but the interface MUST keep File Locations and Albums visibly separate.

The first product does not provide Album Groups, Smart Albums, synchronized Folder-backed Albums, folder mutation, or automatic merging of conflicting user state.

## Rescanning

The Photographer MUST be able to request a rescan.

Slipstream MAY also scan at startup.

A rescan MUST:

- resolve provable moved Originals before allocating and publishing new Photos;
- add newly discovered supported files below the current Library Folder;
- refresh a changed file's Preview state and content fingerprint;
- drop a stale fingerprint whose observed size or modification time no longer matches;
- mark missing files unavailable;
- inspect Capture Time for newly discovered, changed, or relocated available Originals;
- reuse persisted Capture Time facts for unchanged Originals;
- retain the last successfully inspected Capture Time for a remembered unavailable Original;
- publish one completed Library snapshot without exposing partial reordering while the rescan runs;
- refresh derived File Locations only from that completed publication;
- leave explicit Album membership positions unchanged;
- preserve unaffected Albums and decisions;
- never remove a decision only because a file is temporarily unavailable.

Continuous filesystem watching is not required initially.

## Failure Behavior

A failure to inspect one file MUST be recorded for that file and identify it. It MUST NOT roll back successfully indexed files or prevent other valid Photos from remaining available.

Slipstream MUST NOT claim that the failed Photo has a trustworthy Preview.

A database or indexing failure MUST NOT change Original Files.

If a scan cannot prove one exact candidate for a missing Original File, Slipstream MUST keep the affected Photo unavailable and MUST NOT transfer its state to another file.

A malformed or unavailable capture metadata value MUST affect only that Original's capture fact.

Slipstream MUST continue indexing valid sibling Photos.

It MUST NOT use filesystem modification time or another guessed value to hide the failure.

## Editing References During Recovery

[Photo Development](../photo-development/spec.md#original-availability-and-recovery) defines how saved editing intent and retained Exports affect recovery and Retire and Bind eligibility.

They MUST NOT be discarded as unused scan state.
