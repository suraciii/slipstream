# Slipstream Context

## Library

**Photo Library**:
The Original Files known to Slipstream together with their Photos, Albums, and selection state. One state store owns one Photo Library.

**Library Folder**:
The configured filesystem directory whose supported descendant files belong to the Photo Library. The Folder defines discovery and read-only containment, not Photo or Original identity.
_Avoid_: Library Root, source root

**Photo**:
One photograph presented for browsing and selection. A Photo may contain a RAW Original and its matching JPEG Original, and remains the same Photo when a supported Library expansion changes their Locations.

**Capture Time**:
The optional camera-recorded local date and time used to order Photos in the Photo Library. Capture Time does not come from filesystem modification time and does not determine Album membership order.

**Original File**:
A RAW or JPEG file owned by the Photographer and known to Slipstream under one stable identity. Slipstream must not modify it, and a supported Library expansion must not create a new identity for it.

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
The rule that links one same-directory, same-basename XMP Sidecar to one Photo through its Original Locations. An unambiguous RAW/JPEG pair shares one association; the sidecar does not create a Photo or change its Original identity.
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
The JPEG shown in Grid View or Photo View. It comes from a matching JPEG Original or the RAW Original's largest usable embedded JPEG.

**Preview Source**:
The content used for a Preview: `matching-jpeg` or `embedded-raw-jpeg`.

**Detail Review**:
Magnified Preview inspection for focus, motion, or expression. It is a Preview Zoom state in Photo View, not a separate mode. Its detail is limited by the Preview resolution.
