//! Stable Review Preview capability derived from delivered pixel dimensions.

/// The long-edge target of a Review Preview, in pixels.
pub const REVIEW_LONG_EDGE: u32 = 2560;

/// The stable capability of a delivered Preview, independent of its display.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreviewCapability {
    NormalReview,
    DetailLimited,
}

impl PreviewCapability {
    /// Compare the delivered derivative, including thumbnails, against the
    /// Review target. Callers represent unavailable dimensions separately.
    pub fn from_dimensions(width: u32, height: u32) -> Self {
        if width.max(height) < REVIEW_LONG_EDGE {
            Self::DetailLimited
        } else {
            Self::NormalReview
        }
    }

    /// Preserve the existing boolean representation at wire boundaries.
    pub fn is_detail_limited(self) -> bool {
        self == Self::DetailLimited
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_depends_on_the_long_edge_against_the_review_target() {
        for (width, height, expected) in [
            (1, 1, PreviewCapability::DetailLimited),
            (512, 341, PreviewCapability::DetailLimited),
            (2559, 1700, PreviewCapability::DetailLimited),
            (1700, 2559, PreviewCapability::DetailLimited),
            (2560, 1700, PreviewCapability::NormalReview),
            (1700, 2560, PreviewCapability::NormalReview),
            (2561, 1700, PreviewCapability::NormalReview),
            (1700, 2561, PreviewCapability::NormalReview),
            (u32::MAX, 1, PreviewCapability::NormalReview),
        ] {
            let capability = PreviewCapability::from_dimensions(width, height);
            assert_eq!(capability, expected, "{width}x{height}");
            assert_eq!(
                capability.is_detail_limited(),
                expected == PreviewCapability::DetailLimited
            );
        }
    }
}
