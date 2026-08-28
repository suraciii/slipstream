# Slipstream Context

## Library

**Photo Library**:
The Original Files known to Slipstream together with their Photos, Photo Sets, and selection state. One state store owns one Photo Library.

**Library Folder**:
The configured filesystem directory whose supported descendant files belong to the Photo Library. The Folder defines discovery and read-only containment, not Photo or Original identity.
_Avoid_: Library Root, source root

**Photo**:
One photograph presented for browsing and selection. A Photo may contain a RAW Original and its matching JPEG Original, and remains the same Photo when a supported Library expansion changes their Locations.

**Capture Time**:
The optional camera-recorded local date and time used to order Photos in the Photo Library. Capture Time does not come from filesystem modification time and does not determine Photo Set membership order.

**Original File**:
A RAW or JPEG file owned by the Photographer and known to Slipstream under one stable identity. Slipstream must not modify it, and a supported Library expansion must not create a new identity for it.

**Original Location**:
The relative directory and filename used to find an Original File beneath the current Library Folder. A Location is not Original File identity.
_Avoid_: Original path, file identity

**Library Expansion**:
A controlled replacement of the current Library Folder with an ancestor directory while preserving existing Original File and Photo identities.
_Avoid_: Rebase, relink, root migration

**Photo Set**:
A Photographer-defined group of Photos. One Photo may belong to multiple Photo Sets.

## Browsing and Selection

**Library Browser**:
The primary interface for viewing Photos from the Photo Library or one Photo Set. It provides a progressively loaded Grid View and a focused Photo View.

**Grid View**:
The progressively loaded thumbnail view of the current Photo Library or Photo Set source.

**Photo View**:
The focused view of one Photo with Preview, navigation, Selection State, Rating, and Detail Review controls.

**Selection State**:
The keep decision for a Photo: `undecided`, `selected`, or `rejected`.

**Rating**:
An optional zero-to-five-star assessment. Rating is separate from Selection State.

## Preview

**Preview**:
The JPEG shown in Grid View or Photo View. It comes from a matching JPEG Original or the RAW Original's largest usable embedded JPEG.

**Preview Source**:
The content used for a Preview: `matching-jpeg` or `embedded-raw-jpeg`.

**Detail Review**:
Magnified Preview inspection for focus, motion, or expression. Its detail is limited by the Preview resolution.
