//! Closed source-profile classifier for the first development workload.
//!
//! The approved list mirrors the qualified evidence of Issue #329. Only these
//! camera and container classes have a qualified decoder identity, camera
//! matrix, crop, black and white levels for the `development-tiff` workload.
//! The launcher validates the same closed list against its configured bundle;
//! the service uses this classifier to report per-Photo support and to select
//! the `profile_id` carried by an Export snapshot.

/// One qualified source class. `make`, `model` and `container` are compared
/// case-insensitively after trimming surrounding whitespace, because camera
/// metadata and file extensions vary in case between bodies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApprovedProfile {
    pub profile_id: &'static str,
    pub make: &'static str,
    pub model: &'static str,
    pub container: &'static str,
}

pub const APPROVED_PROFILES: &[ApprovedProfile] = &[
    ApprovedProfile {
        profile_id: "sony-ilce-7rm5-arw",
        make: "SONY",
        model: "ILCE-7RM5",
        container: "ARW",
    },
    ApprovedProfile {
        profile_id: "sony-ilce-7cm2-arw",
        make: "SONY",
        model: "ILCE-7CM2",
        container: "ARW",
    },
];

/// The qualified exposure range of this workload in thousandths of an EV. The
/// approved bundle supplies the same finite range to the launcher; a stored
/// value outside it is readable but not representable by the execution payload.
pub const APPROVED_EXPOSURE_MILLI_EV_MIN: i64 = 0;
pub const APPROVED_EXPOSURE_MILLI_EV_MAX: i64 = 1000;

/// The white-balance mode the qualified workload admits. It is not
/// adjustable, so the capability report publishes no executable range beside
/// it and the capability report's `whiteBalanceRanges` stays `null`.
pub const APPROVED_WHITE_BALANCE_MODE: &str = "as-shot";

pub fn approved_profile_ids() -> impl Iterator<Item = &'static str> {
    APPROVED_PROFILES.iter().map(|profile| profile.profile_id)
}

/// The RAW container class of one Original filename, or `None` when the name
/// has no extension. Only the final extension is significant.
pub fn container_of_filename(filename: &str) -> Option<String> {
    let (_, extension) = filename.rsplit_once('.')?;
    if extension.is_empty() {
        return None;
    }
    Some(extension.to_ascii_uppercase())
}

/// Classify one RAW source against the approved list. A source class without a
/// matching profile is unsupported and has no admitted plan.
pub fn classify(make: &str, model: &str, container: &str) -> Option<&'static ApprovedProfile> {
    APPROVED_PROFILES.iter().find(|profile| {
        same(profile.make, make) && same(profile.model, model) && same(profile.container, container)
    })
}

fn same(expected: &str, observed: &str) -> bool {
    let observed = observed.trim();
    !observed.is_empty() && expected.eq_ignore_ascii_case(observed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approved_bodies_classify_regardless_of_case_and_padding() {
        let profile = classify(" Sony ", "ilce-7rm5\t", "arw").expect("approved body");
        assert_eq!(profile.profile_id, "sony-ilce-7rm5-arw");

        let profile = classify("SONY", "ILCE-7CM2", "ARW").expect("approved body");
        assert_eq!(profile.profile_id, "sony-ilce-7cm2-arw");
    }

    #[test]
    fn unapproved_source_classes_have_no_profile() {
        assert!(classify("SONY", "ILCE-7M4", "ARW").is_none());
        assert!(classify("Canon", "Canon EOS R5", "CR3").is_none());
        assert!(classify("SONY", "ILCE-7RM5", "JPG").is_none());
        assert!(classify("", "ILCE-7RM5", "ARW").is_none());
        assert!(classify("SONY", "   ", "ARW").is_none());
    }

    #[test]
    fn container_comes_from_the_final_extension() {
        assert_eq!(
            container_of_filename("_R5_5063.ARW").as_deref(),
            Some("ARW")
        );
        assert_eq!(container_of_filename("shot.v2.arw").as_deref(), Some("ARW"));
        assert_eq!(container_of_filename("shot"), None);
        assert_eq!(container_of_filename("shot."), None);
    }
}
