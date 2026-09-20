# Slipstream Context

## Library

**Photo Library**:
The Original Files known to Slipstream together with their Photos, Albums, and selection state. One state store owns one Photo Library.

**Library Folder**:
The configured filesystem directory whose supported descendant files belong to the Photo Library. The Folder defines discovery and read-only containment, not Photo or Original identity.
_Avoid_: Library Root, source root

**Photo**:
One independently managed supported Original File presented for browsing and selection. RAW and same-basename JPEG are independent Photos; their naming must not share decisions, Album membership, or Preview. A Photo keeps its identity when a supported operation changes its Original Location.
_Avoid_: image pair, RAW/JPEG group

**Content Fingerprint**:
A persisted SHA-256 digest of one Original File's complete content together with the size and modification time observed while hashing. A Content Fingerprint is exact-content evidence for Location Recovery, not identity; independent Originals may share one.
_Avoid_: checksum identity, hash as ID

**Location Recovery**:
The automatic or manual restoration of a remembered Original Location for an unavailable Original File when exact content evidence identifies one unambiguous candidate. Recovery never modifies or deletes an Original File.
_Avoid_: relink, repair, reimport

**Retire and Bind**:
An explicit recovery action that binds an unavailable Original File to an occupied destination Location. It is available only when the destination Photo is otherwise unreferenced with default decisions and no Album membership. It shows the record that will be retired and must not delete a filesystem file.
_Avoid_: replace, merge, overwrite

**Capture Time**:
The optional camera-recorded local date and time used to order Photos in the Photo Library. Capture Time comes from the Photo's own Original File and does not come from filesystem modification time. It does not determine Album membership order.

**Original File**:
A RAW or JPEG file owned by the Photographer and known to Slipstream under one stable identity. Slipstream must not modify it, and a supported Library expansion or Location Recovery must not create a new identity for it.

**Original Location**:
The relative directory and filename used to find an Original File beneath the current Library Folder. A Location is not Original File identity.
_Avoid_: Original path, file identity

**Original Folder**:
The Library Folder or one of its descendant filesystem directories, used to physically organize Original Files by their Original Locations. Slipstream presents it read-only. An Original Folder is not an Album and does not own Photo state.
_Avoid_: Album folder, virtual folder

**Library Expansion**:
A controlled replacement of the current Library Folder with an ancestor directory while preserving existing Original File and Photo identities.
_Avoid_: Rebase, relink, root migration

**Album**:
A Photographer-defined, explicitly ordered virtual group of Photos. One Photo may belong to multiple Albums. Album membership does not change an Original File or Original Location.
_Avoid_: Photo Set, Collection, Favorites

## Metadata

**XMP Sidecar**:
A Photographer-owned external XMP file associated with a Photo and stored separately from its Original Files. It carries supported metadata for exchange with other photo applications and does not define Photo identity or modify an Original File.
_Avoid_: sidecar when the kind is unclear, XMP Original, XMP Photo

**Sidecar Association**:
The rule that links one same-directory, same-basename XMP Sidecar to one Photo through its Original Location. A sole RAW Original owns the association; a sole JPEG Original owns it only when no RAW shares the basename. Multiple eligible owners remain ambiguous and have no writable association. A Location or ownership change invalidates an existing synchronization baseline. The sidecar does not create a Photo or change its Original identity.
_Avoid_: sidecar identity, filename identity

**Sidecar Metadata**:
The supported Photo metadata carried by an XMP Sidecar. In the current product boundary, Rating may have a Sidecar Metadata representation; Selection State, Album membership, and Photo identity do not.
_Avoid_: all metadata, EXIF state

**Metadata Synchronization**:
Reconciliation between a Photo's supported facts in Slipstream and its Sidecar Metadata through explicit read or save actions.
_Avoid_: automatic import, automatic merge

**Metadata Conflict**:
A condition in which Slipstream and external software have changed the same Sidecar Metadata independently. Both values remain available until the Photographer chooses which value to keep.
_Avoid_: sync error, overwrite conflict

## Browsing and Selection

**Library Browser**:
The primary interface for viewing Photos from `All Photos`, one Original Folder, or one Album. It provides a progressively loaded Grid View and a focused Photo View.

**Grid View**:
The progressively loaded thumbnail view of the current `All Photos`, Original Folder, or Album source.

**Photo View**:
The focused view of one Photo with Preview, zoom, navigation, Selection State, Rating, and Album membership controls.

**Selection State**:
The keep decision for a Photo: `undecided`, `selected`, or `rejected`. Selection State is independent of Rating and is not inferred from Sidecar Metadata in the current product boundary.

**Rating**:
An optional zero-to-five-star assessment owned by a Photo. Rating is separate from Selection State and may have a Sidecar Metadata representation.

## Preview

**Preview**:
The JPEG shown in Grid View or Photo View. A JPEG Photo's Preview comes from its own content; a RAW Photo's Preview comes from its own largest usable embedded JPEG. A sibling JPEG must not substitute for a RAW Preview.

**Preview Source**:
The content used for a Preview: `jpeg-original` or `raw-embedded-jpeg`. The legacy names `matching-jpeg` and `embedded-raw-jpeg` existed only in schema version 5, before independent Photos.

**Detail Review**:
Magnified Preview inspection for focus, motion, or expression. It is a Preview Zoom state in Photo View, not a separate mode. Its detail is limited by the Preview resolution.
