use super::*;
use slipstream_core::metadata::sidecar::SidecarFacts;

fn facts() -> SidecarFacts {
    SidecarFacts {
        size: 32,
        modified_seconds: 1_000,
        modified_nanoseconds: 0,
        device: 1,
        inode: 2,
    }
}

fn present(location: &str, sha256: &str) -> MetadataSidecarEvidence {
    MetadataSidecarEvidence::Present {
        location: location.to_owned(),
        facts: crate::metadata_wire::MetadataFileFacts {
            device: 1,
            inode: 2,
            size: 32,
            modified_seconds: 1_000,
            modified_nanoseconds: 0,
        },
        sha256: sha256.to_owned(),
    }
}

fn orphan(sidecar_path: &str, observed_digest: Option<&str>) -> RetainedOrphan {
    RetainedOrphan {
        sidecar_path: sidecar_path.to_owned(),
        retired_photo_id: "retired".to_owned(),
        retired_original_path: "old.JPG".to_owned(),
        original_kind: "jpeg".to_owned(),
        retired_generation: 3,
        observed_size: Some(32),
        observed_mtime_ms: Some(1_000.0),
        observed_digest: observed_digest.map(str::to_owned),
    }
}

fn eligible() -> MetadataAssociation {
    MetadataAssociation {
        state: MetadataAssociationState::Eligible,
        candidates: vec!["one.xmp".to_owned()],
        reason: None,
    }
}

fn inspection_with(orphan: Option<RetainedOrphan>) -> Inspection {
    Inspection {
        original_location: "one.JPG".to_owned(),
        original: RelativeOriginalPath::parse("one.JPG").unwrap(),
        kind: OriginalKind::Jpeg,
        original_facts: facts(),
        observation: SidecarObservation::Eligible {
            location: "one.xmp".to_owned(),
            facts: facts(),
            sha256: "a".repeat(64),
        },
        bytes: ObservedBytes::Absent,
        association_generation: 1,
        library_rating: 0,
        removed: false,
        orphan,
    }
}

/// A retained orphan whose observed Sidecar still matches the current
/// observation blocks the association with the exact same state and
/// reason the Read and Save report.
#[test]
fn a_matching_retained_orphan_blocks_with_its_exact_state_and_reason() {
    let inspection = inspection_with(Some(orphan("one.xmp", Some(&"a".repeat(64)))));
    let evidence = present("one.xmp", &"a".repeat(64));
    let association = retained_orphan_association(&inspection, &evidence, eligible());
    assert_eq!(association.state, MetadataAssociationState::Unresolved);
    assert_eq!(association.reason.as_deref(), Some("retained-orphan"));
    assert_eq!(association.candidates, vec!["one.xmp"]);
}

/// Every other association survives unchanged: an orphan whose Sidecar
/// path differs, whose observed digest differs, whose digest is absent,
/// and an observation that is not a present matching Sidecar.
#[test]
fn only_a_matching_retained_orphan_changes_the_association() {
    let digest = "a".repeat(64);
    let other = "b".repeat(64);

    // A different Sidecar path never matches.
    let inspection = inspection_with(Some(orphan("other.xmp", Some(&digest))));
    let association =
        retained_orphan_association(&inspection, &present("one.xmp", &digest), eligible());
    assert_eq!(association, eligible());

    // A different observed digest never matches.
    let inspection = inspection_with(Some(orphan("one.xmp", Some(&other))));
    let association =
        retained_orphan_association(&inspection, &present("one.xmp", &digest), eligible());
    assert_eq!(association, eligible());

    // An orphan without an observed digest never matches.
    let inspection = inspection_with(Some(orphan("one.xmp", None)));
    let association =
        retained_orphan_association(&inspection, &present("one.xmp", &digest), eligible());
    assert_eq!(association, eligible());

    // No retained orphan never matches.
    let inspection = inspection_with(None);
    let association =
        retained_orphan_association(&inspection, &present("one.xmp", &digest), eligible());
    assert_eq!(association, eligible());

    // An absent observation never matches, even with a matching orphan.
    let inspection = inspection_with(Some(orphan("one.xmp", Some(&digest))));
    let association =
        retained_orphan_association(&inspection, &MetadataSidecarEvidence::Absent, eligible());
    assert_eq!(association, eligible());

    // A non-eligible observation stays non-eligible even when the
    // orphan matches: the blocked state is Unresolved with its reason,
    // never an ineligible or ambiguous association relabelled.
    let ambiguous = MetadataAssociation {
        state: MetadataAssociationState::Ambiguous,
        candidates: vec!["one.xmp".to_owned(), "two.xmp".to_owned()],
        reason: Some("Multiple candidate Sidecars".to_owned()),
    };
    let inspection = inspection_with(Some(orphan("one.xmp", Some(&digest))));
    let association =
        retained_orphan_association(&inspection, &present("one.xmp", &digest), ambiguous);
    assert_eq!(association.state, MetadataAssociationState::Unresolved);
    assert_eq!(association.reason.as_deref(), Some("retained-orphan"));
}
