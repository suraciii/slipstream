# Photo Library and Photo Sets

A Photographer already owns files and directory organization. Slipstream must add selection and grouping without requiring an import copy or proprietary file layout.

## Opening a Photo Library

The server operator configures one Library Folder. Slipstream must index supported files below that Folder, including files in nested directories. Unsupported and unrelated files must remain unchanged and must not appear as Photos.

The Library Folder defines discovery and read-only containment. Its filesystem location must not define the identity of an existing Original File or Photo.

Indexing must not move, rename, rewrite, or delete an Original File. Selecting an arbitrary server filesystem directory from the browser is not required.

If the configured Library Folder does not exist, is not a directory, or cannot be read, Slipstream must identify the failure and must not present a partially indexed Folder as current.

## Photos

Slipstream presents one logical Photo for one photograph.

A RAW Original and JPEG Original must form one Photo when all of these conditions hold:

- they are in the same directory;
- their file names differ only by a recognized RAW or JPEG extension;
- exactly one matching RAW Original and one matching JPEG Original exist.

A RAW or JPEG without a match forms its own Photo.

When matching is ambiguous, Slipstream must keep the files as separate Photos. It must not guess from capture time or visual similarity in the first product.

The JPEG Original is not a disposable derivative. It remains an Original File and must not be modified.

## Stable Identity

An Original File and Photo must keep their persisted identities after first discovery. Their current Original Locations must not be their identities.

Slipstream must retain Photo Set membership, Selection State, and Rating across an ordinary rescan when an Original File remains at the same Location with the same file size and modification time.

Moving or renaming Original Files outside Slipstream may create a new Photo and leave the prior Photo unavailable. Ordinary rescan must not guess a move or silently transfer state by filename, Capture Time, camera metadata, inode, content similarity, or another heuristic.

An unavailable Photo must retain its recorded state until the Photographer removes it from Slipstream. Slipstream must not silently transfer that state to a different file.

## Expanding a Photo Library

The server operator may expand the Photo Library by replacing its current Library Folder with an ancestor directory that contains it. This is a controlled Library operation, not automatic move detection.

Before changing state, Slipstream must prove that the current Library Folder is the same directory found beneath the proposed Folder. It must not guess individual file moves.

A successful expansion must:

- preserve every existing Original File and Photo identity;
- preserve Selection State, Rating, Photo Set membership and order, and saved Photo Set positions;
- preserve remembered unavailable Photos;
- discover supported files outside the former Folder as new Photos;
- leave Photo Set membership unchanged unless the Photographer changes it; and
- leave every Original File unchanged.

Slipstream may invalidate and rebuild Capture Time inspection facts, Preview facts, and cached derivatives when their revisions include an old Location. These are derived state and must not replace or reset Selection State, Rating, Photo Sets, membership order, or saved Photo Set positions.

Expansion requires a stopped Library and a verified backup. If Slipstream cannot prove the ancestor relationship or preserve every remembered Original Location without conflict, it must reject the expansion without changing the current Library.

The first product does not support arbitrary per-file relinking, automatic move detection, multiple Library Folders, or moving the Library to an unrelated directory.

For example, expanding `/photos/26-spring` to `/photos` keeps `26-spring/a.ARW` as the same Original File and discovers supported files in sibling directories. Changing `/photos/26-spring` to unrelated `/archive` is not a Library Expansion.

## Capture Time

Capture Time is optional camera metadata. Slipstream uses it to order `All Photos` in the Library Browser. Capture Time must not determine pairing or change a Photo Set's membership order.

Slipstream must inspect each available Original independently. It must use the first valid base field in this order:

- EXIF `DateTimeOriginal`;
- EXIF `DateTimeDigitized`.

For the selected base field, Slipstream must use its matching `SubSecTimeOriginal` or `SubSecTimeDigitized` value when valid. It must retain a valid matching `OffsetTimeOriginal` or `OffsetTimeDigitized` value as a metadata fact.

Capture ordering uses the camera-local date and time. Slipstream must not convert known offsets to UTC or invent an offset when one is missing. This keeps files with and without timezone metadata in one stable camera-local sequence.

A missing or malformed subsecond value must contribute zero. Slipstream must normalize valid subseconds to nine decimal digits; digits beyond the first nine do not affect ordering. A missing or malformed offset remains unknown and does not invalidate an otherwise valid Capture Time.

Slipstream must not use EXIF `DateTime`, GPS time, filesystem modification time, a filename, Preview metadata, XMP, or another Original as a guessed fallback.

For a RAW/JPEG pair, a valid RAW Capture Time is authoritative. A valid matching JPEG Capture Time is used only when RAW has no valid Capture Time. If both Originals have valid values that differ, Slipstream must retain the disagreement and use RAW for ordering. If both have valid timezone offsets that differ, that is also a disagreement. A known offset on one Original and an unknown offset on the other is not a disagreement.

Missing, invalid, or failed capture metadata must not make an otherwise readable Photo unavailable. A Photo without an authoritative Capture Time remains browsable and sorts in the missing-time partition.

When an Original becomes unavailable, Slipstream must retain its last successfully inspected Capture Time for ordering. When that Original returns with changed file revision facts, Slipstream must replace the retained fact with the result of inspecting the current bytes.

## Photo Sets

The Photographer may create, rename, and delete a Photo Set.

A Photo Set contains explicitly ordered references to Photos. Its membership positions are authoritative whenever the Photographer browses that Photo Set. Capture metadata and rescans must not silently change those positions.

One Photo may belong to multiple Photo Sets. New members append in the order supplied by the add operation. Only an explicit reorder operation may change the order of existing members. Deleting a Photo Set must not delete or modify a Photo or Original File.

The Photographer may add or remove Photos from a Photo Set. A Photo's Selection State and Rating belong to the Photo, not to one membership. The same decision therefore appears in every Photo Set that contains the Photo.

Directory structure and Photo Sets are separate. Indexing a directory must not automatically create a permanent Photo Set for every directory.

## Rescanning

The Photographer must be able to request a rescan. Slipstream may also scan at startup.

A rescan must:

- add newly discovered supported files below the current Library Folder;
- refresh a changed file's Preview state;
- mark missing files unavailable;
- inspect Capture Time for newly discovered or changed available Originals;
- reuse persisted Capture Time facts for unchanged Originals;
- retain the last successfully inspected Capture Time for a remembered unavailable Original;
- publish one completed Library snapshot without exposing partial reordering while the rescan runs;
- leave explicit Photo Set membership positions unchanged;
- preserve unaffected Photo Sets and decisions;
- never remove a decision only because a file is temporarily unavailable.

Continuous filesystem watching is not required initially.

## Failure Behavior

A failure to inspect one file must identify that file and allow other valid Photos to remain available. Slipstream must not claim that the failed Photo has a trustworthy Preview.

A database or indexing failure must not change Original Files.

If a previously paired RAW or JPEG changes so that the pair becomes ambiguous, Slipstream must preserve existing records and identify the ambiguity. Automatic state splitting or merging is not required in the first product.

A malformed or unavailable capture metadata value must affect only that Original's capture fact. Slipstream must continue indexing valid sibling Photos. It must not use filesystem modification time or another guessed value to hide the failure.

## Examples

The following files form two Photos:

```text literal
shoot/DSCF0001.RAF
shoot/DSCF0001.JPG
shoot/DSCF0002.RAF
```

`DSCF0001.RAF` and `DSCF0001.JPG` form one Photo. `DSCF0002.RAF` forms another Photo.

Deleting a Photo Set named `Portfolio candidates` removes the group only. It does not delete its Photos or files.
