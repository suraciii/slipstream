# Photo Library and Albums

A Photographer already owns files and directory organization. Slipstream must add selection and grouping without requiring an import copy or proprietary file layout.

## Opening a Photo Library

The server operator configures one Library Folder. Slipstream must index supported files below that Folder, including files in nested directories. Unsupported and unrelated files must remain unchanged and must not appear as Photos.

The Library Folder defines discovery and read-only containment. Its filesystem location must not define the identity of an existing Original File or Photo.

An Original Folder is the Library Folder itself or a real descendant directory represented by one or more known Original Locations. Slipstream must present Original Folders as read-only File Locations. The Library Folder remains the root File Location even when the Library is empty. Slipstream must not present any other empty directory or directory containing no known supported Original as a product object.

Indexing must not move, rename, rewrite, or delete an Original File or Original Folder. Selecting an arbitrary server filesystem directory from the browser is not required.

If the configured Library Folder does not exist, is not a directory, or cannot be read, Slipstream must identify the failure and must not present a partially indexed Folder as current.

## Photos

Slipstream presents one Photo for one independently managed supported Original File.

RAW and JPEG Originals are independent Photos even when they share a directory and base name. Naming alone must not share Selection State, Rating, Album membership, or Preview.

The first product supports RAW and JPEG Originals. It does not add TIFF support.

A JPEG Original is not a disposable derivative. It remains an Original File and must not be modified.

## Stable Identity

An Original File and Photo must keep their persisted identities after first discovery. Their current Original Locations must not be their identities.

Slipstream must retain Album membership, Selection State, Rating, and saved Album positions across an ordinary rescan when an Original File remains recoverable.

A moved or renamed Original File must keep its identity when a scan can prove one exact content match under the Location Recovery rules below. Ordinary rescan must not guess a move or silently transfer state by filename, Capture Time, camera metadata, inode, content similarity, or another heuristic.

An unavailable Photo must retain its recorded state until the Photographer removes it from Slipstream. Slipstream must not silently transfer that state to a different file.

## Content Fingerprints

Slipstream persists a content fingerprint for every Original File it can read: the SHA-256 digest of the complete content together with the size and modification time observed while hashing.

A fingerprint is exact-content evidence, not identity. Independent Originals may share one fingerprint, and changed content must not keep an old fingerprint.

Slipstream must establish fingerprints automatically in the background. Enrollment must:

- read Originals through the existing confined read-only access;
- keep memory independent of file size and cancel between chunks;
- run one bounded worker that yields to foreground Preview work;
- pause while a scan runs;
- resume incomplete enrollment after restart; and
- verify the source revision before and after hashing.

Slipstream must report truthful enrollment progress. It must not promise instant readiness: the first enrollment requires approximately one complete read of every Original File without a fingerprint.

Slipstream must not fingerprint an Original File larger than 4 GiB. Such a file keeps no automatic recovery evidence and remains eligible for manual recovery only.

Automatic recovery applies only to fingerprints completed before an Original became unavailable. Slipstream must not infer historical fingerprints for Originals that were already missing.

## Location Recovery

Slipstream must recover a moved Original File when a scan can prove one exact match. Automatic recovery applies when all of these conditions hold:

- a remembered Original File is missing at its Original Location;
- exactly one candidate Original of the same supported kind exists below the current Library Folder with the exact same content and no other owner; and
- a persisted fingerprint proves that content.

A recovered Original File keeps its Original File ID and Photo ID. Its Photo keeps Selection State, Rating, Album membership and order, and saved Album positions.

Slipstream must resolve recovery before it allocates and publishes new Photos. It must not publish temporary duplicates and later merge them. When known files exchange Locations in one scan, Slipstream must reconcile the provable content permutation.

If the old Original File remains and an equal copy appears, Slipstream must discover an independent Photo at the new Location and must not transfer state. If multiple equal candidates or multiple owners exist, Slipstream must preserve the unresolved records and must not choose by enumeration order, basename, Capture Time, camera metadata, inode, or visual similarity. The Photographer resolves that group through manual recovery.

Changed content plus a changed Location is not guaranteed to recover automatically. Incomplete traversal, inaccessible storage, uncertain candidate revisions, and unreadable candidates must not be treated as proof of absence or uniqueness.

Recovery must search only supported descendants of the admitted Library Folder. Moving the configured Library Folder itself remains a Library Expansion.

## Manual Recovery

Every unavailable Original File must remain listed with its remembered Location, filename, kind, Rating, Selection State, Album count, and whether a fingerprint exists.

The Photographer must be able to review unavailable Originals and propose new Locations through one bounded `Review unavailable originals` entry from the scan result or an affected Photo.

Slipstream must support two proposal forms:

- a batch mapping from one old Folder prefix to one new Folder prefix that keeps each relative suffix; and
- a single mapping from one remembered Original File to one new Location for a renamed or split file.

Every proposed mapping must be inspectable before it is applied, and proposal must never write. A proposal for an Original File without a persisted fingerprint must state that the old content cannot be verified and must require explicit confirmation. A fingerprinted Original File must be verified against its persisted fingerprint at proposal and at commit.

Slipstream must apply a batch atomically with revalidation at commit. It must reject the whole batch with per-mapping reasons when any mapping is stale, colliding, or occupied without a permitted retire, and it must not apply a partial association.

A destination Location may already belong to a Photo discovered by an earlier scan. Slipstream must not silently merge the two Photos, and default Rating or Selection State must not be proof that the destination Photo is disposable. Slipstream may offer an explicit retire-and-bind action only when the destination Photo is otherwise unreferenced with no non-default decisions and no Album membership, and it must show which record will be retired. If the destination Photo has independent user state, Slipstream must preserve both Photos and report the conflict. Recovery must not delete any filesystem file.

Recovery must use server-relative Library Locations. It must not require a client file upload or expose an arbitrary server path. Recovery must not import or write an XMP Sidecar and must preserve the Photo's Rating in Slipstream.

## Expanding a Photo Library

The server operator may expand the Photo Library by replacing its current Library Folder with an ancestor directory that contains it. This is a controlled Library operation, not Location Recovery.

Before changing state, Slipstream must prove that the current Library Folder is the same directory found beneath the proposed Folder. It must not resolve individual file moves as part of an expansion.

A successful expansion must:

- preserve every existing Original File and Photo identity;
- preserve Selection State, Rating, Album membership and order, and saved Album positions;
- preserve remembered unavailable Photos;
- discover supported files outside the former Folder as new Photos;
- leave Album membership unchanged unless the Photographer changes it; and
- leave every Original File unchanged.

Slipstream may invalidate and rebuild Capture Time inspection facts, Preview facts, cached derivatives, and derived Original Folder navigation when their inputs include an old Location. These are derived state and must not replace or reset Selection State, Rating, Albums, membership order, or saved Album positions.

Expansion requires a stopped Library and a verified backup. If Slipstream cannot prove the ancestor relationship or preserve every remembered Original Location without conflict, it must reject the expansion without changing the current Library.

The first product does not support a continuous filesystem watcher, global disk search, multiple Library Folders, or moving the Library to an unrelated directory.

For example, expanding `/photos/26-spring` to `/photos` keeps `26-spring/a.ARW` as the same Original File and discovers supported files in sibling directories. Changing `/photos/26-spring` to unrelated `/archive` is not a Library Expansion.

## Capture Time

Capture Time is optional camera metadata. Slipstream uses it to order `All Photos` and Original Folder sources in the Library Browser. Capture Time must not change an Album's membership order.

Slipstream must inspect each available Original independently. It must use the first valid base field in this order:

- EXIF `DateTimeOriginal`;
- EXIF `DateTimeDigitized`.

For the selected base field, Slipstream must use its matching `SubSecTimeOriginal` or `SubSecTimeDigitized` value when valid. It must retain a valid matching `OffsetTimeOriginal` or `OffsetTimeDigitized` value as a metadata fact.

Capture ordering uses the camera-local date and time. Slipstream must not convert known offsets to UTC or invent an offset when one is missing. This keeps files with and without timezone metadata in one stable camera-local sequence.

A missing or malformed subsecond value must contribute zero. Slipstream must normalize valid subseconds to nine decimal digits; digits beyond the first nine do not affect ordering. A missing or malformed offset remains unknown and does not invalidate an otherwise valid Capture Time.

Slipstream must not use EXIF `DateTime`, GPS time, filesystem modification time, a filename, Preview metadata, or XMP as a guessed fallback.

Capture Time comes from the Photo's own Original File. A relocated Original File re-derives its capture facts from its current bytes; a value retained from the old Location must not remain authoritative.

Missing, invalid, or failed capture metadata must not make an otherwise readable Photo unavailable. A Photo without an authoritative Capture Time remains browsable and sorts in the missing-time partition.

When an Original becomes unavailable, Slipstream must retain its last successfully inspected Capture Time for ordering. When that Original returns with changed file revision facts, Slipstream must replace the retained fact with the result of inspecting the current bytes.

## Physical and Virtual Organization

Original Folders and Albums are separate organization axes.

An Original Folder answers where Original Files are known to exist. Its membership is derived from Original Locations and changes only when a completed scan publishes added, removed, or changed Locations. Remembered unavailable Originals remain projected at their last known Locations. An Original Folder does not own Photos, Selection State, Rating, or saved position.

For Folder browsing, a Photo belongs to the parent directory of its own Original Location. Independent Photos that share a base name therefore appear once each.

Selecting an Original Folder must include Photos in that Folder and every descendant Folder. It must include remembered unavailable Photos at their last known Original Locations. The interface must identify this recursive behavior instead of implying that only direct children are shown.

An Album answers how the Photographer wants to use or organize Photos. The Photographer may create, rename, and delete an Album. An Album may be empty and must remain openable and manageable while empty.

An Album contains explicitly ordered references to Photos. Its membership positions are authoritative whenever the Photographer browses that Album. Capture metadata, Original Folder changes, and rescans must not silently change those positions.

One Photo may belong to multiple Albums. New members append in the order supplied by the add operation. Only an explicit reorder operation may change the order of existing members. Deleting an Album must not delete or modify a Photo, Original Location, Original Folder, or Original File.

The Photographer may add or remove Photos from an Album. A Photo's Selection State and Rating belong to the Photo, not to one Album membership. The same decision therefore appears in every Album that contains the Photo.

Indexing a directory must not automatically create an Album. A Folder and an Album may have the same display name, but the interface must keep File Locations and Albums visibly separate. The first product does not provide Album Groups, Smart Albums, synchronized Folder-backed Albums, folder mutation, or automatic merging of conflicting user state.

## Rescanning

The Photographer must be able to request a rescan. Slipstream may also scan at startup.

A rescan must:

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

A failure to inspect one file must identify that file and allow other valid Photos to remain available. Slipstream must not claim that the failed Photo has a trustworthy Preview.

A database or indexing failure must not change Original Files.

If a scan cannot prove one exact candidate for a missing Original File, Slipstream must keep the affected Photo unavailable and must not transfer its state to another file.

A malformed or unavailable capture metadata value must affect only that Original's capture fact. Slipstream must continue indexing valid sibling Photos. It must not use filesystem modification time or another guessed value to hide the failure.

## Examples

The following files form three Photos:

```text literal
shoot/DSCF0001.RAF
shoot/DSCF0001.JPG
shoot/DSCF0002.RAF
```

`DSCF0001.RAF`, `DSCF0001.JPG`, and `DSCF0002.RAF` are independent Photos with their own decisions, Albums, and Previews. `DSCF0001.RAF` and `DSCF0001.JPG` share only a base name.

The filesystem contains `RAW/26春节`, and the Photographer also creates an Album named `26春节`. The File Location changes when a completed rescan observes changed Original Locations. The Album changes only through explicit membership operations.

Deleting an Album named `Portfolio candidates` removes the virtual group only. It does not delete its Photos, Original Locations, Original Folders, or files.
