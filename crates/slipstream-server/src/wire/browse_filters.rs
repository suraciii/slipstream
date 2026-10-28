// The defined Photo facts one Browse Snapshot may be filtered by, besides
// Selection State, and the validation and matching rules they share with the
// CLI photo query contract.
//
// One fact has exactly one meaning across Web, CLI, and server: Rating bounds
// are inclusive integers from 0 through 5 with `0` meaning No Rating, `kind`
// names the Photo's own Original File, `available` describes the remembered
// Original File's availability and never Preview readiness, and Capture Time
// bounds are camera-local with an inclusive lower and exclusive upper bound.
// This module reuses the CLI's validated boundary type and the same
// scan-owned candidate projection, so no second definition or query service
// exists for these facts.
use slipstream_core::CaptureTimeBound;

/// The defined Photo facts one Browse Snapshot may be filtered by, besides
/// Selection State. Every field is optional: an omitted field applies no
/// condition, and different fields combine with AND.
///
/// Rating `0` is a real bound meaning No Rating, not a missing value. `kind`
/// describes the Photo's own Original File, so a RAW Photo and a
/// same-basename JPEG Photo stay independent Photos. `available` describes
/// whether the remembered Original File is available, never whether its
/// Preview is ready. Capture Time uses the CLI's camera-local boundary shape
/// with an inclusive lower bound and an exclusive upper bound, and a Photo
/// without a Capture Time never matches a supplied bound.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BrowseFilterConditions {
    pub rating_minimum: Option<u8>,
    pub rating_maximum: Option<u8>,
    pub kind: Option<slipstream_core::OriginalKind>,
    pub available: Option<bool>,
    pub captured_from: Option<CaptureTimeBound>,
    pub captured_before: Option<CaptureTimeBound>,
}

impl BrowseFilterConditions {
    /// True when no fact condition is supplied, so the Snapshot is an
    /// unfiltered view of the source order.
    pub(crate) fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// True when one Photo's own Original facts satisfy every supplied
    /// condition. The kind, availability, and Capture Time facts come from the
    /// Published Library's scan-owned projection, so Preview readiness is
    /// never consulted; `rating` is the Photo's current decision.
    pub(crate) fn matches(
        &self,
        candidate: &slipstream_core::PhotoQueryCandidate,
        rating: u8,
    ) -> bool {
        self.rating_minimum.is_none_or(|minimum| rating >= minimum)
            && self.rating_maximum.is_none_or(|maximum| rating <= maximum)
            && self.kind.is_none_or(|kind| candidate.original_kind == kind)
            && self
                .available
                .is_none_or(|available| candidate.original_available == available)
            && self.captured_from.as_ref().is_none_or(|from| {
                candidate
                    .capture_order_key()
                    .is_some_and(|value| value >= from.as_str())
            })
            && self.captured_before.as_ref().is_none_or(|before| {
                candidate
                    .capture_order_key()
                    .is_some_and(|value| value < before.as_str())
            })
    }
}

/// Validates one Browse open request's Rating bounds, Original kind,
/// availability, and Capture Time bounds. Every value is checked against its
/// own fact before any Snapshot exists: an out-of-range Rating, an inverted
/// Rating range, an unknown `kind` value, and a malformed or reversed Capture
/// Time range are refused there.
pub fn valid_browse_filters(
    rating_minimum: Option<u8>,
    rating_maximum: Option<u8>,
    kind: Option<&str>,
    available: Option<bool>,
    captured_from: Option<&str>,
    captured_before: Option<&str>,
) -> Result<BrowseFilterConditions, ()> {
    let rating_minimum = match rating_minimum {
        Some(rating) if rating <= 5 => Some(rating),
        Some(_) => return Err(()),
        None => None,
    };
    let rating_maximum = match rating_maximum {
        Some(rating) if rating <= 5 => Some(rating),
        Some(_) => return Err(()),
        None => None,
    };
    if rating_minimum
        .zip(rating_maximum)
        .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        return Err(());
    }
    let kind = match kind {
        None => None,
        Some("raw") => Some(slipstream_core::OriginalKind::Raw),
        Some("jpeg") => Some(slipstream_core::OriginalKind::Jpeg),
        Some(_) => return Err(()),
    };
    let captured_from = match captured_from {
        Some(value) => Some(CaptureTimeBound::parse(value).map_err(|_| ())?),
        None => None,
    };
    let captured_before = match captured_before {
        Some(value) => Some(CaptureTimeBound::parse(value).map_err(|_| ())?),
        None => None,
    };
    if captured_from
        .as_ref()
        .zip(captured_before.as_ref())
        .is_some_and(|(from, before)| from.as_str() >= before.as_str())
    {
        return Err(());
    }
    Ok(BrowseFilterConditions {
        rating_minimum,
        rating_maximum,
        kind,
        available,
        captured_from,
        captured_before,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(
        kind: slipstream_core::OriginalKind,
        available: bool,
        capture: &str,
    ) -> slipstream_core::PhotoQueryCandidate {
        slipstream_core::PhotoQueryCandidate {
            photo_id: "photo".to_owned(),
            relative_path: "a.jpg".to_owned(),
            sort_path: "a.jpg".to_owned(),
            original_kind: kind,
            original_available: available,
            capture: slipstream_core::CaptureFact {
                state: slipstream_core::CaptureMetadataState::Known,
                order_key: Some(capture.to_owned()),
                field: None,
                offset_minutes: None,
                source_revision: None,
                identity: slipstream_core::CameraIdentity::default(),
            },
            preview_state: slipstream_core::PreviewState::InspectionPending,
            preview_source_revision: None,
            preview_width: None,
            preview_height: None,
        }
    }

    #[test]
    fn rating_bounds_are_inclusive_and_zero_is_a_real_bound() {
        let conditions =
            valid_browse_filters(Some(0), None, None, None, None, None).expect("valid");
        assert!(conditions.matches(
            &candidate(
                slipstream_core::OriginalKind::Jpeg,
                true,
                "2026-01-01T00:00:00"
            ),
            0
        ));
        let four = valid_browse_filters(Some(4), None, None, None, None, None).expect("valid");
        assert!(!four.matches(
            &candidate(
                slipstream_core::OriginalKind::Jpeg,
                true,
                "2026-01-01T00:00:00"
            ),
            3
        ));
        assert!(four.matches(
            &candidate(
                slipstream_core::OriginalKind::Jpeg,
                true,
                "2026-01-01T00:00:00"
            ),
            4
        ));
        let five = valid_browse_filters(None, Some(5), None, None, None, None).expect("valid");
        assert!(five.matches(
            &candidate(
                slipstream_core::OriginalKind::Jpeg,
                true,
                "2026-01-01T00:00:00"
            ),
            5
        ));
    }

    #[test]
    fn availability_names_the_original_not_preview_readiness() {
        let unavailable =
            valid_browse_filters(None, None, None, Some(false), None, None).expect("valid");
        assert!(unavailable.matches(
            &candidate(
                slipstream_core::OriginalKind::Jpeg,
                false,
                "2026-01-01T00:00:00"
            ),
            0
        ));
        assert!(!unavailable.matches(
            &candidate(
                slipstream_core::OriginalKind::Jpeg,
                true,
                "2026-01-01T00:00:00"
            ),
            0
        ));
    }

    #[test]
    fn capture_time_bounds_are_inclusive_lower_and_exclusive_upper() {
        let conditions = valid_browse_filters(
            None,
            None,
            None,
            None,
            Some("2026-01-01T00:00:00"),
            Some("2026-02-01T00:00:00"),
        )
        .expect("valid");
        let in_range = candidate(
            slipstream_core::OriginalKind::Jpeg,
            true,
            "2026-01-31T23:59:59",
        );
        assert!(conditions.matches(&in_range, 0));
        // The lower bound includes its own second; the upper bound excludes it.
        assert!(conditions.matches(
            &candidate(
                slipstream_core::OriginalKind::Jpeg,
                true,
                "2026-01-01T00:00:00"
            ),
            0
        ));
        assert!(!conditions.matches(
            &candidate(
                slipstream_core::OriginalKind::Jpeg,
                true,
                "2026-02-01T00:00:00"
            ),
            0
        ));
    }

    #[test]
    fn a_photo_without_capture_time_never_matches_a_supplied_bound() {
        let conditions =
            valid_browse_filters(None, None, None, None, Some("2026-01-01T00:00:00"), None)
                .expect("valid");
        let missing = slipstream_core::PhotoQueryCandidate {
            capture: slipstream_core::CaptureFact {
                state: slipstream_core::CaptureMetadataState::Missing,
                order_key: None,
                field: None,
                offset_minutes: None,
                source_revision: None,
                identity: slipstream_core::CameraIdentity::default(),
            },
            ..candidate(
                slipstream_core::OriginalKind::Jpeg,
                true,
                "2026-05-01T00:00:00",
            )
        };
        assert!(!conditions.matches(&missing, 0));
    }

    #[test]
    fn kind_keeps_raw_and_jpeg_photos_independent() {
        let raw = valid_browse_filters(None, None, Some("raw"), None, None, None).expect("valid");
        assert!(raw.matches(
            &candidate(
                slipstream_core::OriginalKind::Raw,
                true,
                "2026-01-01T00:00:00"
            ),
            0
        ));
        assert!(!raw.matches(
            &candidate(
                slipstream_core::OriginalKind::Jpeg,
                true,
                "2026-01-01T00:00:00"
            ),
            0
        ));
    }

    #[test]
    fn out_of_range_inverted_malformed_and_reversed_values_are_refused() {
        assert!(valid_browse_filters(Some(6), None, None, None, None, None).is_err());
        assert!(valid_browse_filters(None, Some(6), None, None, None, None).is_err());
        assert!(valid_browse_filters(Some(4), Some(2), None, None, None, None).is_err());
        assert!(valid_browse_filters(None, None, Some("tiff"), None, None, None).is_err());
        assert!(valid_browse_filters(None, None, None, None, Some("2026-01-01"), None).is_err());
        assert!(
            valid_browse_filters(None, None, None, None, None, Some("2026-13-01T00:00:00"))
                .is_err()
        );
        assert!(
            valid_browse_filters(
                None,
                None,
                None,
                None,
                Some("2026-02-01T00:00:00"),
                Some("2026-01-01T00:00:00")
            )
            .is_err()
        );
        // Equal bounds are reversed: the upper bound is exclusive.
        assert!(
            valid_browse_filters(
                None,
                None,
                None,
                None,
                Some("2026-01-01T00:00:00"),
                Some("2026-01-01T00:00:00")
            )
            .is_err()
        );
    }

    #[test]
    fn an_unfiltered_condition_set_is_the_default() {
        assert!(
            valid_browse_filters(None, None, None, None, None, None)
                .expect("valid")
                .is_default()
        );
        assert!(
            !valid_browse_filters(Some(0), None, None, None, None, None)
                .expect("valid")
                .is_default()
        );
    }
}
