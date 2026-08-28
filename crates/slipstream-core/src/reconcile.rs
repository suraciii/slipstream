use crate::{
    OriginalKind,
    domain::{DiscoveredOriginal, OriginalRecord, PhotoRecord, PreviewCandidate, PreviewState},
    identity::pairing_stem,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Clone, Debug, PartialEq)]
pub struct ReconciledPhoto {
    pub id: String,
    pub raw_id: Option<String>,
    pub jpeg_id: Option<String>,
    pub raw: Option<DiscoveredOriginal>,
    pub jpeg: Option<DiscoveredOriginal>,
    pub raw_present: bool,
    pub jpeg_present: bool,
    pub ambiguous: bool,
    pub sort_path: String,
    pub prior: Option<PhotoRecord>,
}

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
    let mut groups: BTreeMap<Vec<u8>, Vec<DiscoveredOriginal>> = BTreeMap::new();
    for original in discovered {
        let path = original.path.as_str();
        let (directory, name) = path.rsplit_once('/').unwrap_or(("", path));
        groups
            .entry(format!("{directory}\0{}", pairing_stem(name)).into_bytes())
            .or_default()
            .push(original.clone());
    }
    for group in groups.values_mut() {
        group.sort_by(|left, right| {
            left.path
                .as_str()
                .as_bytes()
                .cmp(right.path.as_str().as_bytes())
        });
    }

    let mut by_original = HashMap::<String, PhotoRecord>::new();
    for photo in existing {
        if let Some(id) = &photo.raw_original_id {
            by_original.insert(id.clone(), photo.clone());
        }
        if let Some(id) = &photo.jpeg_original_id {
            by_original.insert(id.clone(), photo.clone());
        }
    }

    let mut updates = Vec::new();
    let mut used = BTreeSet::new();
    for group in groups.values() {
        let raws: Vec<_> = group
            .iter()
            .filter(|item| item.kind == OriginalKind::Raw)
            .cloned()
            .collect();
        let jpegs: Vec<_> = group
            .iter()
            .filter(|item| item.kind == OriginalKind::Jpeg)
            .cloned()
            .collect();
        if raws.len() == 1 && jpegs.len() == 1 {
            let raw = raws.into_iter().next().unwrap();
            let jpeg = jpegs.into_iter().next().unwrap();
            let raw_id = original_id(&raw);
            let jpeg_id = original_id(&jpeg);
            let default_sort_path = raw.path.as_str().to_owned();
            let raw_prior = by_original.get(&raw_id);
            let jpeg_prior = by_original.get(&jpeg_id);
            let prior = match (raw_prior, jpeg_prior) {
                (Some(raw_prior), Some(jpeg_prior)) if raw_prior.id == jpeg_prior.id => {
                    Some(raw_prior.clone())
                }
                (Some(raw_prior), None) => Some(raw_prior.clone()),
                (None, Some(jpeg_prior)) => Some(jpeg_prior.clone()),
                // Do not silently merge two separate singleton Photos. The
                // existing rows remain unavailable until a product decision
                // defines how their durable state should be reconciled.
                (Some(raw_prior), Some(jpeg_prior)) if raw_prior.id != jpeg_prior.id => {
                    for prior in [raw_prior, jpeg_prior] {
                        used.insert(prior.id.clone());
                        updates.push(ReconciledPhoto {
                            id: prior.id.clone(),
                            raw_id: prior.raw_original_id.clone(),
                            jpeg_id: prior.jpeg_original_id.clone(),
                            raw: None,
                            jpeg: None,
                            raw_present: false,
                            jpeg_present: false,
                            ambiguous: prior.ambiguous,
                            sort_path: if prior.sort_path.is_empty() {
                                prior.id.clone()
                            } else {
                                prior.sort_path.clone()
                            },
                            prior: Some(prior.clone()),
                        });
                    }
                    None
                }
                (Some(_), Some(_)) => None,
                (None, None) => None,
            };
            let id = match prior.as_ref() {
                Some(photo) => photo.id.clone(),
                None => allocate_photo_id()?,
            };
            used.insert(id.clone());
            updates.push(ReconciledPhoto {
                id,
                raw_id: Some(raw_id),
                jpeg_id: Some(jpeg_id),
                raw: Some(raw),
                jpeg: Some(jpeg),
                raw_present: true,
                jpeg_present: true,
                ambiguous: false,
                sort_path: default_sort_path,
                prior,
            });
            continue;
        }

        let ambiguous = group.len() > 1;
        let prior_pairs = if ambiguous {
            let mut pairs = BTreeMap::new();
            for item in group {
                if let Some(photo) = by_original.get(&original_id(item))
                    && photo.raw_original_id.is_some()
                    && photo.jpeg_original_id.is_some()
                {
                    pairs.insert(photo.id.clone(), photo.clone());
                }
            }
            pairs.into_values().collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let mut pair_members = BTreeSet::new();
        for prior in prior_pairs {
            let raw_id = prior.raw_original_id.clone().unwrap();
            let jpeg_id = prior.jpeg_original_id.clone().unwrap();
            let raw = group
                .iter()
                .find(|item| original_id(item) == raw_id)
                .cloned();
            let jpeg = group
                .iter()
                .find(|item| original_id(item) == jpeg_id)
                .cloned();
            pair_members.insert(raw_id.clone());
            pair_members.insert(jpeg_id.clone());
            used.insert(prior.id.clone());
            updates.push(ReconciledPhoto {
                id: prior.id.clone(),
                raw_id: Some(raw_id),
                jpeg_id: Some(jpeg_id),
                raw_present: raw.is_some(),
                jpeg_present: jpeg.is_some(),
                raw,
                jpeg,
                ambiguous: true,
                sort_path: if prior.sort_path.is_empty() {
                    prior.id.clone()
                } else {
                    prior.sort_path.clone()
                },
                prior: Some(prior),
            });
        }

        for item in group {
            let item_id = original_id(item);
            if pair_members.contains(&item_id) {
                continue;
            }
            let prior = by_original.get(&item_id).cloned();
            if prior.as_ref().is_some_and(|photo| {
                photo.raw_original_id.is_some() && photo.jpeg_original_id.is_some()
            }) && !ambiguous
            {
                continue;
            }
            let id = match prior.as_ref() {
                Some(photo) => photo.id.clone(),
                None => allocate_photo_id()?,
            };
            used.insert(id.clone());
            updates.push(ReconciledPhoto {
                id,
                raw_id: (item.kind == OriginalKind::Raw).then(|| item_id.clone()),
                jpeg_id: (item.kind == OriginalKind::Jpeg).then(|| item_id.clone()),
                raw: (item.kind == OriginalKind::Raw).then(|| item.clone()),
                jpeg: (item.kind == OriginalKind::Jpeg).then(|| item.clone()),
                raw_present: item.kind == OriginalKind::Raw,
                jpeg_present: item.kind == OriginalKind::Jpeg,
                ambiguous,
                sort_path: item.path.as_str().to_owned(),
                prior,
            });
        }
    }

    for prior in existing {
        if used.contains(&prior.id)
            || prior.raw_original_id.is_none()
            || prior.jpeg_original_id.is_none()
        {
            continue;
        }
        let raw = discovered
            .iter()
            .find(|item| Some(original_id(item)) == prior.raw_original_id)
            .cloned();
        let jpeg = discovered
            .iter()
            .find(|item| Some(original_id(item)) == prior.jpeg_original_id)
            .cloned();
        {
            used.insert(prior.id.clone());
            updates.push(ReconciledPhoto {
                id: prior.id.clone(),
                raw_id: prior.raw_original_id.clone(),
                jpeg_id: prior.jpeg_original_id.clone(),
                raw_present: raw.is_some(),
                jpeg_present: jpeg.is_some(),
                raw,
                jpeg,
                ambiguous: prior.ambiguous,
                sort_path: if prior.sort_path.is_empty() {
                    prior.id.clone()
                } else {
                    prior.sort_path.clone()
                },
                prior: Some(prior.clone()),
            });
        }
    }

    updates.sort_by(|left, right| {
        left.sort_path
            .as_bytes()
            .cmp(right.sort_path.as_bytes())
            .then_with(|| left.id.as_bytes().cmp(right.id.as_bytes()))
    });
    Ok(updates)
}

pub fn selected_source(photo: &ReconciledPhoto) -> Option<(&DiscoveredOriginal, PreviewCandidate)> {
    if photo
        .jpeg
        .as_ref()
        .is_some_and(|original| original.error_category.is_none())
    {
        return photo
            .jpeg
            .as_ref()
            .map(|original| (original, PreviewCandidate::MatchingJpeg));
    }
    if photo
        .raw
        .as_ref()
        .is_some_and(|original| original.error_category.is_none())
    {
        return photo
            .raw
            .as_ref()
            .map(|original| (original, PreviewCandidate::EmbeddedRawJpeg));
    }
    None
}

pub fn preview_should_preserve(
    prior: &PhotoRecord,
    photo: &ReconciledPhoto,
    selected: Option<(&DiscoveredOriginal, PreviewCandidate)>,
    previous_originals: &HashMap<String, OriginalRecord>,
) -> bool {
    let Some((selected_original, current_candidate)) = selected else {
        return false;
    };
    if prior.preview_state == PreviewState::InspectionPending
        || prior.preview_candidate != Some(current_candidate)
    {
        return false;
    }

    // The candidate is the source that was selected for inspection. The source
    // that actually produced the persisted preview may only differ when a
    // matching JPEG failed and the embedded RAW JPEG was used as fallback.
    let Some(actual_source) = prior.preview_source else {
        return false;
    };
    if actual_source != current_candidate
        && (current_candidate != PreviewCandidate::MatchingJpeg
            || actual_source != PreviewCandidate::EmbeddedRawJpeg)
    {
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

    let current_actual = match actual_source {
        PreviewCandidate::MatchingJpeg => photo.jpeg.as_ref(),
        PreviewCandidate::EmbeddedRawJpeg => photo.raw.as_ref(),
    };
    let Some(current_actual) = current_actual else {
        return false;
    };
    if current_actual.error_category.is_some()
        || current_actual.path.as_str() != previous_path
        || current_actual.facts.size != previous_size
        || current_actual.facts.mtime_ms != previous_mtime
    {
        return false;
    }

    previous_originals
        .get(previous_path)
        .is_some_and(|previous| {
            previous.kind
                == match actual_source {
                    PreviewCandidate::MatchingJpeg => OriginalKind::Jpeg,
                    PreviewCandidate::EmbeddedRawJpeg => OriginalKind::Raw,
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
        domain::{OriginalErrorCategory, OriginalFacts, SelectionState},
        identity::original_id,
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

    fn reconcile_test(
        discovered: &[DiscoveredOriginal],
        existing: &[PhotoRecord],
    ) -> Vec<ReconciledPhoto> {
        let original_ids = discovered
            .iter()
            .map(|original| {
                (
                    original.path.as_str().to_owned(),
                    original_id(original.path.as_str()),
                )
            })
            .collect();
        let mut next = 0;
        reconcile(discovered, existing, &original_ids, || {
            let id = format!("new-photo-{next}");
            next += 1;
            Ok::<_, ()>(id)
        })
        .unwrap()
    }

    fn photo(id: &str, raw: Option<&str>, jpeg: Option<&str>, ambiguous: bool) -> PhotoRecord {
        PhotoRecord {
            id: id.to_owned(),
            raw_original_id: raw.map(str::to_owned),
            jpeg_original_id: jpeg.map(str::to_owned),
            ambiguous,
            available: true,
            preview_state: PreviewState::InspectionPending,
            preview_candidate: None,
            preview_source: None,
            preview_source_revision: None,
            preview_width: None,
            preview_height: None,
            cache_revision: None,
            sort_path: "one.ARW".to_owned(),
            selection_state: SelectionState::Selected,
            rating: 4,
        }
    }

    fn record(original: &DiscoveredOriginal) -> OriginalRecord {
        OriginalRecord {
            id: original_id(original.path.as_str()),
            relative_path: original.path.clone(),
            kind: original.kind,
            facts: original.facts,
            available: original.error_category.is_none(),
            error_category: original.error_category,
            error_message: original.error_message.clone(),
            capture: original.capture.clone(),
        }
    }

    fn with_preview(
        mut prior: PhotoRecord,
        candidate: PreviewCandidate,
        source: PreviewCandidate,
        revision: String,
    ) -> PhotoRecord {
        prior.preview_state = PreviewState::Ready;
        prior.preview_candidate = Some(candidate);
        prior.preview_source = Some(source);
        prior.preview_source_revision = Some(revision);
        prior
    }

    #[test]
    fn pairing_reuses_singleton_id_in_either_direction() {
        for (first, second) in [
            (
                original("one.ARW", OriginalKind::Raw),
                original("one.JPG", OriginalKind::Jpeg),
            ),
            (
                original("one.JPG", OriginalKind::Jpeg),
                original("one.ARW", OriginalKind::Raw),
            ),
        ] {
            let first_id = original_id(first.path.as_str());
            let prior = photo(
                "stable",
                (first.kind == OriginalKind::Raw).then_some(first_id.as_str()),
                (first.kind == OriginalKind::Jpeg).then_some(first_id.as_str()),
                false,
            );
            let result = reconcile_test(&[first, second], &[prior]);
            assert_eq!(result.len(), 1);
            assert_eq!(result[0].id, "stable");
            assert!(result[0].raw_present && result[0].jpeg_present);
            assert!(!result[0].ambiguous);
        }
    }

    #[test]
    fn paired_photo_retains_id_and_missing_reference() {
        let raw = original("one.ARW", OriginalKind::Raw);
        let jpeg = original("one.JPG", OriginalKind::Jpeg);
        let raw_id = original_id(raw.path.as_str());
        let jpeg_id = original_id(jpeg.path.as_str());
        let prior = photo("paired", Some(&raw_id), Some(&jpeg_id), false);
        let result = reconcile_test(std::slice::from_ref(&raw), std::slice::from_ref(&prior));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "paired");
        assert_eq!(result[0].raw_id.as_deref(), Some(raw_id.as_str()));
        assert_eq!(result[0].jpeg_id.as_deref(), Some(jpeg_id.as_str()));
        assert!(result[0].raw_present && !result[0].jpeg_present);
        let restored = reconcile_test(&[raw, jpeg], &[prior]);
        assert_eq!(restored[0].id, "paired");
        assert!(restored[0].raw_present && restored[0].jpeg_present);
    }

    #[test]
    fn ambiguity_preserves_prior_pair_and_adds_conflicting_member() {
        let raw = original("one.ARW", OriginalKind::Raw);
        let jpeg = original("one.JPG", OriginalKind::Jpeg);
        let conflict = original("one.RAF", OriginalKind::Raw);
        let raw_id = original_id(raw.path.as_str());
        let jpeg_id = original_id(jpeg.path.as_str());
        let prior = photo("paired", Some(&raw_id), Some(&jpeg_id), false);
        let result = reconcile_test(&[raw, jpeg, conflict], &[prior]);
        assert_eq!(result.len(), 2);
        let retained = result.iter().find(|item| item.id == "paired").unwrap();
        assert!(retained.ambiguous);
        assert!(retained.raw_present && retained.jpeg_present);
        let separate = result.iter().find(|item| item.id != "paired").unwrap();
        let conflict_id = original_id("one.RAF");
        assert_eq!(separate.raw_id.as_deref(), Some(conflict_id.as_str()));
    }

    #[test]
    fn preview_preservation_requires_actual_source_and_allows_only_jpeg_to_raw_fallback() {
        let raw = original("one.ARW", OriginalKind::Raw);
        let jpeg = original("one.JPG", OriginalKind::Jpeg);
        let raw_revision = crate::source_revision("one.ARW", 10, 1_000.0).unwrap();
        let jpeg_revision = crate::source_revision("one.JPG", 10, 1_000.0).unwrap();
        let reconciled = ReconciledPhoto {
            id: "id".to_owned(),
            raw_id: Some(original_id("one.ARW")),
            jpeg_id: Some(original_id("one.JPG")),
            raw: Some(raw.clone()),
            jpeg: Some(jpeg.clone()),
            raw_present: true,
            jpeg_present: true,
            ambiguous: false,
            sort_path: "one.ARW".to_owned(),
            prior: None,
        };
        let previous = [
            (raw.path.as_str().to_owned(), record(&raw)),
            (jpeg.path.as_str().to_owned(), record(&jpeg)),
        ]
        .into_iter()
        .collect();
        let fallback = with_preview(
            photo("id", None, None, false),
            PreviewCandidate::MatchingJpeg,
            PreviewCandidate::EmbeddedRawJpeg,
            raw_revision,
        );
        assert!(preview_should_preserve(
            &fallback,
            &reconciled,
            Some((&jpeg, PreviewCandidate::MatchingJpeg)),
            &previous,
        ));
        let inverse = with_preview(
            photo("id", None, None, false),
            PreviewCandidate::EmbeddedRawJpeg,
            PreviewCandidate::MatchingJpeg,
            jpeg_revision,
        );
        assert!(!preview_should_preserve(
            &inverse,
            &reconciled,
            Some((&raw, PreviewCandidate::EmbeddedRawJpeg)),
            &previous,
        ));
        let missing_revision = with_preview(
            photo("id", None, None, false),
            PreviewCandidate::MatchingJpeg,
            PreviewCandidate::EmbeddedRawJpeg,
            String::new(),
        );
        assert!(!preview_should_preserve(
            &missing_revision,
            &reconciled,
            Some((&jpeg, PreviewCandidate::MatchingJpeg)),
            &previous,
        ));
    }

    #[test]
    fn selected_source_prefers_usable_matching_jpeg_and_falls_back_to_raw() {
        let mut jpeg = original("one.JPG", OriginalKind::Jpeg);
        let raw = original("one.ARW", OriginalKind::Raw);
        let photo = ReconciledPhoto {
            id: "id".to_owned(),
            raw_id: Some(original_id("one.ARW")),
            jpeg_id: Some(original_id("one.JPG")),
            raw: Some(raw.clone()),
            jpeg: Some(jpeg.clone()),
            raw_present: true,
            jpeg_present: true,
            ambiguous: false,
            sort_path: "one.ARW".to_owned(),
            prior: None,
        };
        assert_eq!(
            selected_source(&photo).unwrap().1,
            PreviewCandidate::MatchingJpeg
        );
        jpeg.error_category = Some(OriginalErrorCategory::Unreadable);
        let fallback = ReconciledPhoto {
            jpeg: Some(jpeg),
            ..photo
        };
        assert_eq!(
            selected_source(&fallback).unwrap().1,
            PreviewCandidate::EmbeddedRawJpeg
        );
    }

    #[test]
    fn distinct_singletons_are_not_silently_merged_when_pairing_appears() {
        let raw = original("one.ARW", OriginalKind::Raw);
        let jpeg = original("one.JPG", OriginalKind::Jpeg);
        let raw_id = original_id(raw.path.as_str());
        let jpeg_id = original_id(jpeg.path.as_str());
        let result = reconcile_test(
            &[raw, jpeg],
            &[
                photo("raw-singleton", Some(&raw_id), None, false),
                photo("jpeg-singleton", None, Some(&jpeg_id), false),
            ],
        );
        assert_eq!(result.len(), 3);
        let paired = result
            .iter()
            .find(|item| item.prior.is_none() && item.raw_present && item.jpeg_present)
            .unwrap();
        assert!(paired.prior.is_none());
        assert!(paired.raw_present && paired.jpeg_present);
        assert!(result.iter().any(|item| item.id == "raw-singleton"));
        assert!(result.iter().any(|item| item.id == "jpeg-singleton"));
    }

    #[test]
    fn deterministic_order_uses_utf8_bytes_then_id() {
        let values = [
            original("z.JPG", OriginalKind::Jpeg),
            original("a.JPG", OriginalKind::Jpeg),
            original("春节.JPG", OriginalKind::Jpeg),
        ];
        let result = reconcile_test(&values, &[]);
        let paths: Vec<_> = result.iter().map(|item| item.sort_path.as_str()).collect();
        assert_eq!(paths, ["a.JPG", "z.JPG", "春节.JPG"]);
    }
}
