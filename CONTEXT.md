# Slipstream Context

## Library

**Photo Library**:
The Original Files indexed by Slipstream together with their organization and selection state.

**Photo**:
One photograph presented for review. A Photo may contain a RAW Original and its matching JPEG Original.

**Original File**:
A RAW or JPEG file owned by the Photographer. Slipstream must not modify it.

**Photo Set**:
A Photographer-defined group of Photos. One Photo may belong to multiple Photo Sets.

## Selection

**Selection State**:
The keep decision for a Photo: `undecided`, `selected`, or `rejected`.

**Rating**:
An optional zero-to-five-star assessment. Rating is separate from Selection State.

**Review Session**:
The ordered review of Photos from one Photo Set or filtered Photo Library.

## Preview

**Preview**:
The JPEG shown for review. It comes from a matching JPEG Original or the RAW Original's largest usable embedded JPEG.

**Preview Source**:
The content used for a Preview: `matching-jpeg` or `embedded-raw-jpeg`.

**Detail Review**:
Magnified Preview inspection for focus, motion, or expression. Its detail is limited by the Preview resolution.
