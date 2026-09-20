use crate::{
    OriginalKind,
    domain::{DiscoveredOriginal, OriginalRecord, PhotoRecord, PreviewSource},
};
use std::collections::HashMap;

/// One Photo as reconciled by a scan: its stable identity, its single
/// Original, and whether that Original was discovered by this scan. A
/// remembered unavailable Photo keeps its identity and prior decisions with
/// `original: None`.
#[derive(Clone, Debug, PartialEq)]
pub struct ReconciledPhoto {
    pub id: String,
    pub original_id: String,
    pub original: Option<DiscoveredOriginal>,
    pub present: bool,
    pub sort_path: String,
    pub prior: Option<PhotoRecord>,
}

/// Reconciles one complete scan against the persisted Photo records. Every
/// discovered Original joins the Photo that already owns its Original File
/// identity, or receives a newly allocated Photo identity. Persisted Photos
/// whose Original was not discovered remain as remembered unavailable records
/// with their prior decisions.
pub fn reconcile<E>(
    discovered: &[DiscoveredOriginal],
    existing: &[PhotoRecord],
    original_ids: &HashMap<String, String>,
    mut allocate_photo_id: impl FnMut() -> Result<String, E>,
) -> Result<Vec<ReconciledPhoto>, E> {
    let original_id = |original: &DiscoveredOriginal| {
        original_ids
            .get(original.path.as_str())
            .expect("every discovered Original has an assigned identity")
            .clone()
    };
    let mut photo_by_original = HashMap::<String, &PhotoRecord>::with_capacity(existing.len());
    for photo in existing {
        photo_by_original.insert(photo.original_id.clone(), photo);
    }

    let mut updates = Vec::new();
    let mut used = std::collections::BTreeSet::new();
    for original in discovered {
        let original_id = original_id(original);
        let prior = photo_by_original.get(&original_id).copied().cloned();
        let id = match prior {
            Some(ref photo) => photo.id.clone(),
            None => allocate_photo_id()?,
        };
        used.insert(id.clone());
        updates.push(ReconciledPhoto {
            id,
            original_id,
            original: Some(original.clone()),
            present: true,
            sort_path: original.path.as_str().to_owned(),
            prior,
        });
    }

    for prior in existing {
        if used.contains(&prior.id) {
            continue;
        }
        used.insert(prior.id.clone());
        updates.push(ReconciledPhoto {
            id: prior.id.clone(),
            original_id: prior.original_id.clone(),
            original: None,
            present: false,
            sort_path: if prior.sort_path.is_empty() {
                prior.id.clone()
            } else {
                prior.sort_path.clone()
            },
            prior: Some(prior.clone()),
        });
    }

    updates.sort_by(|left, right| {
        left.sort_path
            .as_bytes()
            .cmp(right.sort_path.as_bytes())
            .then_with(|| left.id.as_bytes().cmp(right.id.as_bytes()))
    });
    Ok(updates)
}

/// The Preview Source the Photo's own Original supplies for this scan, when
/// the Original was discovered without an inspection error.
pub fn selected_source(photo: &ReconciledPhoto) -> Option<(&DiscoveredOriginal, PreviewSource)> {
    let original = photo.original.as_ref()?;
    if original.error_category.is_some() {
        return None;
    }
    Some((original, original.kind.preview_source()))
}

/// A prior Preview result may be preserved only when the same Original, at
/// the same observed revision and Location, still produced it. Any Location
/// or content change resets inspection.
pub fn preview_should_preserve(
    prior: &PhotoRecord,
    selected: Option<(&DiscoveredOriginal, PreviewSource)>,
    previous_originals: &HashMap<String, OriginalRecord>,
) -> bool {
    let Some((selected_original, _source)) = selected else {
        return false;
    };
    if prior.preview_state == crate::PreviewState::InspectionPending {
        return false;
    }
    if selected_original.error_category.is_some() {
        return false;
    }

    let Some(revision) = prior.preview_source_revision.as_deref() else {
        return false;
    };
    let mut parts = revision.split('\0');
    let Some(previous_path) = parts.next() else {
        return false;
    };
    let Some(previous_size) = parts.next().and_then(|value| value.parse::<u64>().ok()) else {
        return false;
    };
    let Some(previous_mtime) = parts.next().and_then(|value| value.parse::<f64>().ok()) else {
        return false;
    };
    if parts.next().is_some() {
        return false;
    }
    if selected_original.path.as_str() != previous_path
        || selected_original.facts.size != previous_size
        || selected_original.facts.mtime_ms != previous_mtime
    {
        return false;
    }

    previous_originals.get(previous_path).is_some_and(|previous| {
        previous.kind
            == match selected_original.kind {
                OriginalKind::Raw => OriginalKind::Raw,
                OriginalKind::Jpeg => OriginalKind::Jpeg,
            }
            && previous.available
            && previous.error_category.is_none()
            && previous.facts.size == previous_size
            && previous.facts.mtime_ms == previous_mtime
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        PreviewState,
        domain::{OriginalErrorCategory, OriginalFacts, SelectionState},
    };

    fn original(path: &str, kind: OriginalKind) -> DiscoveredOriginal {
        DiscoveredOriginal {
            path: crate::RelativeOriginalPath::parse(path).unwrap(),
            kind,
            facts: OriginalFacts {
                size: 10,
                mtime_ms: 1_000.0,
                device: 1,
                inode: 1,
            },
            error_category: None,
            error_message: None,
            capture: crate::CaptureFact::pending(),
        }
    }

    fn photo(id: &str, original_id: &str) -> PhotoRecord {
        PhotoRecord {
            id: id.to_owned(),
            original_id: original_id.to_owned(),
            available: true,
            preview_state: PreviewState::InspectionPending,
            preview_source_revision: None,
            preview_width: None,
            preview_height: None,
            cache_revision: None,
            sort_path: String::new(),
            selection_state: SelectionState::Undecided,
            rating: 0,
        }
    }

    fn ids(paths: &[(&str, &str)]) -> HashMap<String, String> {
        paths
            .iter()
            .map(|(path, id)| (path.to_owned(), id.to_owned()))
            .collect()
    }

    #[test]
    fn keeps_photo_identity_for_the_same_original_location() {
        let discovered = [original("shoot/a.jpg", OriginalKind::Jpeg)];
        let existing = [photo("p1", "o1")];
        let mapping = ids(&[("shoot/a.jpg", "o1")]);
        let reconciled = reconcile(&discovered, &existing, &mapping, || {
            Ok::<(), ()>("p-new".to_owned())
        })
        .unwrap();
        assert_eq!(reconciled.len(), 1);
        assert_eq!(reconciled[0].id, "p1");
        assert_eq!(reconciled[0].original_id, "o1");
        assert!(reconciled[0].present);
    }

    #[test]
    fn moves_identity_with_a_relocated_original() {
        // The recovery planner maps the new Location to the persisted
        // Original identity before reconciliation runs.
        let discovered = [original("moved/a.jpg", OriginalKind::Jpeg)];
        let existing = [photo("p1", "o1")];
        let mapping = ids(&[("moved/a.jpg", "o1")]);
        let reconciled = reconcile(&discovered, &existing, &mapping, || {
            Ok::<(), ()>("p-new".to_owned())
        })
        .unwrap();
        assert_eq!(reconciled[0].id, "p1");
        assert_eq!(reconciled[0].sort_path, "moved/a.jpg");
    }

    #[test]
    fn same_basename_raw_and_jpeg_stay_independent_photos() {
        let discovered = [
            original("shoot/a.raw", OriginalKind::Raw),
            original("shoot/a.jpg", OriginalKind::Jpeg),
        ];
        let mapping = ids(&[("shoot/a.raw", "o-raw"), ("shoot/a.jpg", "o-jpeg")]);
        let mut allocated = 0;
        let reconciled = reconcile(&discovered, &[], &mapping, || {
            allocated += 1;
            Ok::<(), ()>(format!("p{allocated}"))
        })
        .unwrap();
        assert_eq!(reconciled.len(), 2);
        assert_ne!(reconciled[0].id, reconciled[1].id);
        assert_ne!(reconciled[0].original_id, reconciled[1].original_id);
    }

    #[test]
    fn remembers_unavailable_photo_with_its_decisions() {
        let existing = [photo("p1", "o1")];
        let reconciled = reconcile(&[], &existing, &HashMap::new(), || {
            Ok::<(), ()>("p-new".to_owned())
        })
        .unwrap();
        assert_eq!(reconciled.len(), 1);
        assert!(!reconciled[0].present);
        assert!(reconciled[0].original.is_none());
        assert_eq!(reconciled[0].id, "p1");
    }

    #[test]
    fn selected_source_follows_the_original_kind() {
        let raw = ReconciledPhoto {
            id: "p1".to_owned(),
            original_id: "o1".to_owned(),
            original: Some(original("shoot/a.raw", OriginalKind::Raw)),
            present: true,
            sort_path: "shoot/a.raw".to_owned(),
            prior: None,
        };
        assert_eq!(
            selected_source(&raw).map(|(_, source)| source),
            Some(PreviewSource::RawEmbeddedJpeg)
        );
        let jpeg = ReconciledPhoto {
            id: "p2".to_owned(),
            original_id: "o2".to_owned(),
            original: Some(original("shoot/a.jpg", OriginalKind::Jpeg)),
            present: true,
            sort_path: "shoot/a.jpg".to_owned(),
            prior: None,
        };
        assert_eq!(
            selected_source(&jpeg).map(|(_, source)| source),
            Some(PreviewSource::JpegOriginal)
        );
    }

    #[test]
    fn preview_preserves_only_matching_location_size_and_mtime() {
        let discovered = original("shoot/a.jpg", OriginalKind::Jpeg);
        let photo = ReconciledPhoto {
            id: "p1".to_owned(),
            original_id: "o1".to_owned(),
            original: Some(discovered.clone()),
            present: true,
            sort_path: "shoot/a.jpg".to_owned(),
            prior: None,
        };
        let selected = selected_source(&photo);
        let mut prior = photo("p1", "o1");
        prior.preview_state = PreviewState::Ready;
        let revision = crate::source_revision("shoot/a.jpg", 10, 1_000.0).unwrap();
        prior.preview_source_revision = Some(revision);
        let mut previous = std::collections::HashMap::new();
        previous.insert(
            "shoot/a.jpg".to_owned(),
            OriginalRecord {
                id: "o1".to_owned(),
                relative_path: crate::RelativeOriginalPath::parse("shoot/a.jpg").unwrap(),
                kind: OriginalKind::Jpeg,
                facts: discovered.facts,
                available: true,
                error_category: None,
                error_message: None,
                capture: crate::CaptureFact::pending(),
            },
        );
        assert!(preview_should_preserve(&prior, selected, &previous));
        let moved = ReconciledPhoto {
            sort_path: "moved/a.jpg".to_owned(),
            original: Some(original("moved/a.jpg", OriginalKind::Jpeg)),
            ..photo.clone()
        };
        assert!(!preview_should_preserve(
            &prior,
            selected_source(&moved),
            &previous
        ));
    }

    #[test]
    fn unreadable_original_has_no_selected_source() {
        let mut discovered = original("shoot/a.jpg", OriginalKind::Jpeg);
        discovered.error_category = Some(OriginalErrorCategory::Unreadable);
        let photo = ReconciledPhoto {
            id: "p1".to_owned(),
            original_id: "o1".to_owned(),
            original: Some(discovered),
            present: true,
            sort_path: "shoot/a.jpg".to_owned(),
            prior: None,
        };
        assert!(selected_source(&photo).is_none());
    }
}
