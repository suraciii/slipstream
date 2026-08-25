# Photo Library and Photo Sets

A Photographer already owns files and directory organization. Slipstream must add selection and grouping without requiring an import copy or proprietary file layout.

## Opening a Photo Library

Slipstream must index an existing configured directory. The directory may contain nested directories, RAW files, JPEG files, and unrelated files.

Indexing must not move, rename, rewrite, or delete an Original File. Unsupported and unrelated files must remain unchanged and must not appear as Photos.

The first product may require the server operator to configure the directory at startup. Selecting an arbitrary server filesystem directory from the browser is not required.

If the configured path does not exist, is not a directory, or cannot be read, Slipstream must identify the failure and must not present a partially indexed directory as current.

## Photos

Slipstream presents one logical Photo for one reviewable capture.

A RAW Original and JPEG Original must form one Photo when all of these conditions hold:

- they are in the same directory;
- their file names differ only by a recognized RAW or JPEG extension;
- exactly one matching RAW Original and one matching JPEG Original exist.

A RAW or JPEG without a match forms its own Photo.

When matching is ambiguous, Slipstream must keep the files as separate Photos. It must not guess from capture time or visual similarity in the first product.

The JPEG Original is not a disposable derivative. It remains an Original File and must not be modified.

## Stable Identity

Slipstream must retain Photo Set membership, Selection State, and Rating across an ordinary rescan when an Original File remains at the same relative path with the same file size and modification time.

Moving or renaming Original Files outside Slipstream may create a new Photo and leave the prior Photo unavailable. Automatic move detection is not part of the first product.

An unavailable Photo must retain its recorded state until the Photographer removes it from Slipstream. Slipstream must not silently transfer that state to a different file.

## Photo Sets

The Photographer may create, rename, and delete a Photo Set.

A Photo Set contains ordered references to Photos. One Photo may belong to multiple Photo Sets. Deleting a Photo Set must not delete or modify a Photo or Original File.

The Photographer may add or remove Photos from a Photo Set. A Photo's Selection State and Rating belong to the Photo, not to one membership. The same decision therefore appears in every Photo Set that contains the Photo.

Directory structure and Photo Sets are separate. Indexing a directory must not automatically create a permanent Photo Set for every directory.

## Rescanning

The Photographer must be able to request a rescan. Slipstream may also scan at startup.

A rescan must:

- add newly discovered supported files;
- refresh a changed file's Preview state;
- mark missing files unavailable;
- preserve unaffected Photo Sets and decisions;
- never remove a decision only because a file is temporarily unavailable.

Continuous filesystem watching is not required initially.

## Failure Behavior

A failure to inspect one file must identify that file and allow other valid Photos to remain available. Slipstream must not claim that the failed Photo has a trustworthy Preview.

A database or indexing failure must not change Original Files.

If a previously paired RAW or JPEG changes so that the pair becomes ambiguous, Slipstream must preserve existing records and identify the ambiguity. Automatic state splitting or merging is not required in the first product.

## Examples

The following files form two Photos:

```text literal
shoot/DSCF0001.RAF
shoot/DSCF0001.JPG
shoot/DSCF0002.RAF
```

`DSCF0001.RAF` and `DSCF0001.JPG` form one Photo. `DSCF0002.RAF` forms another Photo.

Deleting a Photo Set named `Portfolio candidates` removes the group only. It does not delete its Photos or files.
