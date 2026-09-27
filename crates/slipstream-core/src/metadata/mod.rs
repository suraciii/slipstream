//! Bounded standard metadata reading and checked Sidecar saving.
//!
//! `xmp` owns the lossless XMP document model. `embedded` owns Original
//! embedded source extraction (EXIF capture facts, embedded XMP, IPTC IIM).

pub mod embedded;
pub mod sidecar;
pub mod xmp;
