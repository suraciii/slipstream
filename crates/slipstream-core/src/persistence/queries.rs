//! Photo query, read, and projection: the SELECT shapes the Web and CLI surfaces
//! depend on.

use super::owner::{MutationVersions, PersistenceError};
use super::scan;
use super::scan::parse_selection_state;
use crate::{
    PhotoQuery, PhotoQueryCandidate, PhotoQueryError, PhotoQueryOrder, PhotoQueryProjection,
    PhotoQuerySource, PhotoRead, PreviewState, SelectionState,
};
use rusqlite::{Connection, OptionalExtension, params};

pub(super) fn read_photo(
    connection: &Connection,
    versions: &MutationVersions,
    photo_id: &str,
) -> Result<Option<PhotoRead>, PersistenceError> {
    let row = connection
        .query_row(
            "SELECT p.id,o.relative_path,o.kind,o.available,p.selection_state,p.rating,
                    p.removed_at_ms,o.capture_metadata_state,o.capture_order_key,o.capture_time_field,
                    o.capture_offset_minutes,o.capture_source_revision,p.preview_state,
                    p.preview_source_revision,p.preview_width,p.preview_height,
                    EXISTS(SELECT 1 FROM edit_recipes e WHERE e.photo_id=p.id)
             FROM photos p JOIN original_files o ON o.id=p.original_id WHERE p.id=?",
            [photo_id],
            |row| {
                let path = row.get::<_, String>(1)?;
                let kind = scan::parse_kind(&row.get::<_, String>(2)?)?;
                let preview_state = scan::parse_preview_state(&row.get::<_, String>(12)?)?;
                let preview_source_revision: Option<String> = row.get(13)?;
                let preview_width = scan::parse_dimension(row.get(14)?)?;
                let preview_height = scan::parse_dimension(row.get(15)?)?;
                let ready = preview_state == PreviewState::Ready;
                Ok(PhotoRead {
                    decision_version: versions.photo(photo_id),
                    id: row.get(0)?,
                    filename: path.rsplit('/').next().unwrap_or(&path).to_owned(),
                    original_kind: kind,
                    original_available: row.get::<_, i64>(3)? != 0,
                    selection_state: parse_selection_state(&row.get::<_, String>(4)?)?,
                    removed_at_ms: row.get(6)?,
                    rating: row
                        .get::<_, i64>(5)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    capture: scan::parse_capture_fact(
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                        row.get(11)?,
                    )?,
                    preview_state,
                    preview_source: ready.then(|| kind.preview_source()),
                    preview_source_revision: ready.then_some(preview_source_revision).flatten(),
                    preview_width: ready.then_some(preview_width).flatten(),
                    preview_height: ready.then_some(preview_height).flatten(),
                    has_saved_edits: row.get::<_, i64>(16)? != 0,
                })
            },
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    Ok(row)
}

pub(super) fn read_projected_photo(
    connection: &Connection,
    versions: &MutationVersions,
    projection: &PhotoQueryProjection,
    photo_id: &str,
) -> Result<Option<PhotoRead>, PersistenceError> {
    let Some(candidate) = projection.get(photo_id) else {
        return Ok(None);
    };
    let decisions = connection
        .query_row(
            "SELECT p.selection_state,p.rating,p.removed_at_ms,
                    EXISTS(SELECT 1 FROM edit_recipes e WHERE e.photo_id=p.id)
             FROM photos p WHERE p.id=?",
            [photo_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, i64>(3)? != 0,
                ))
            },
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    let Some((selection_state, rating, removed_at_ms, has_saved_edits)) = decisions else {
        return Ok(None);
    };
    let selection_state =
        parse_selection_state(&selection_state).map_err(|_| PersistenceError::Storage)?;
    let rating = rating.try_into().map_err(|_| PersistenceError::Storage)?;
    let ready = candidate.preview_state == PreviewState::Ready;
    Ok(Some(PhotoRead {
        id: candidate.photo_id.clone(),
        filename: candidate
            .relative_path
            .rsplit('/')
            .next()
            .unwrap_or(&candidate.relative_path)
            .to_owned(),
        original_kind: candidate.original_kind,
        original_available: candidate.original_available,
        selection_state,
        removed_at_ms,
        rating,
        decision_version: versions.photo(photo_id),
        capture: candidate.capture.clone(),
        preview_state: candidate.preview_state,
        preview_source: ready.then(|| candidate.original_kind.preview_source()),
        preview_source_revision: ready
            .then(|| candidate.preview_source_revision.clone())
            .flatten(),
        preview_width: ready.then_some(candidate.preview_width).flatten(),
        preview_height: ready.then_some(candidate.preview_height).flatten(),
        has_saved_edits,
    }))
}

fn valid_folder_location(location: &str) -> bool {
    !location.starts_with('/')
        && !location.ends_with('/')
        && !location.contains('\0')
        && !location
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
}

fn candidate_in_folder(candidate: &PhotoQueryCandidate, location: &str) -> bool {
    location.is_empty()
        || candidate
            .relative_path
            .strip_prefix(location)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn candidate_matches(
    query: &PhotoQuery,
    candidate: &PhotoQueryCandidate,
    selection_state: SelectionState,
    rating: u8,
) -> bool {
    query
        .selection_state
        .is_none_or(|expected| selection_state == expected)
        && query.rating_minimum.is_none_or(|minimum| rating >= minimum)
        && query.rating_maximum.is_none_or(|maximum| rating <= maximum)
        && query
            .original_kind
            .is_none_or(|kind| candidate.original_kind == kind)
        && query
            .original_available
            .is_none_or(|available| candidate.original_available == available)
        && query.captured_from.as_ref().is_none_or(|from| {
            candidate
                .capture_order_key()
                .is_some_and(|value| value >= from.as_str())
        })
        && query.captured_before.as_ref().is_none_or(|before| {
            candidate
                .capture_order_key()
                .is_some_and(|value| value < before.as_str())
        })
}

fn push_query_match(
    ids: &mut Vec<String>,
    photo_id: &str,
    maximum_results: usize,
) -> Result<(), PhotoQueryError> {
    ids.push(photo_id.to_owned());
    if ids.len() > maximum_results {
        Err(PhotoQueryError::ResultLimitExceeded {
            limit: maximum_results,
        })
    } else {
        Ok(())
    }
}

pub(super) fn create_photo_query(
    connection: &Connection,
    query: PhotoQuery,
    projection: &PhotoQueryProjection,
    maximum_results: usize,
) -> Result<Vec<String>, PhotoQueryError> {
    if maximum_results == 0
        || maximum_results == usize::MAX
        || query.rating_minimum.is_some_and(|value| value > 5)
        || query.rating_maximum.is_some_and(|value| value > 5)
        || query
            .rating_minimum
            .zip(query.rating_maximum)
            .is_some_and(|(minimum, maximum)| minimum > maximum)
        || query
            .captured_from
            .as_ref()
            .zip(query.captured_before.as_ref())
            .is_some_and(|(from, before)| from.as_str() >= before.as_str())
        || (query.order == PhotoQueryOrder::AlbumOrder
            && !matches!(query.source, PhotoQuerySource::Album(_)))
    {
        return Err(PhotoQueryError::Invalid);
    }

    if let PhotoQuerySource::Folder(location) = &query.source {
        if !location.is_empty() && !valid_folder_location(location) {
            return Err(PhotoQueryError::Invalid);
        }
        // The Library Folder root always exists, even while no Photo is
        // present to prove it; every other Folder needs a member.
        if !location.is_empty()
            && !projection
                .ascending()
                .iter()
                .any(|candidate| candidate_in_folder(candidate, location))
        {
            return Err(PhotoQueryError::SourceNotFound);
        }
    }

    let album_id = match &query.source {
        PhotoQuerySource::Album(album_id) if !album_id.is_empty() => {
            let exists = connection
                .query_row("SELECT 1 FROM albums WHERE id=?", [album_id], |_| Ok(()))
                .optional()
                .map_err(|_| PhotoQueryError::Storage)?;
            if exists.is_none() {
                return Err(PhotoQueryError::SourceNotFound);
            }
            Some(album_id.as_str())
        }
        PhotoQuerySource::Album(_) => return Err(PhotoQueryError::Invalid),
        _ => None,
    };

    let mut ids = Vec::with_capacity(maximum_results.min(64).saturating_add(1));
    if query.order == PhotoQueryOrder::AlbumOrder {
        let album_id = album_id.expect("Album order was validated with an Album source");
        let mut statement = connection
            .prepare(
                "SELECT m.photo_id,p.selection_state,p.rating
                 FROM album_members m JOIN photos p ON p.id=m.photo_id
                 WHERE m.album_id=? AND p.removed_at_ms IS NULL ORDER BY m.position",
            )
            .map_err(|_| PhotoQueryError::Storage)?;
        let mut rows = statement
            .query([album_id])
            .map_err(|_| PhotoQueryError::Storage)?;
        while let Some(row) = rows.next().map_err(|_| PhotoQueryError::Storage)? {
            let photo_id = row
                .get::<_, String>(0)
                .map_err(|_| PhotoQueryError::Storage)?;
            let Some(candidate) = projection.get(&photo_id) else {
                continue;
            };
            let selection = parse_selection_state(
                &row.get::<_, String>(1)
                    .map_err(|_| PhotoQueryError::Storage)?,
            )
            .map_err(|_| PhotoQueryError::Storage)?;
            let rating = row
                .get::<_, i64>(2)
                .ok()
                .and_then(|value| value.try_into().ok())
                .ok_or(PhotoQueryError::Storage)?;
            if candidate_matches(&query, candidate, selection, rating) {
                push_query_match(&mut ids, &photo_id, maximum_results)?;
            }
        }
        return Ok(ids);
    }

    let mut current = connection
        .prepare(if album_id.is_some() {
            "SELECT p.selection_state,p.rating FROM photos p
             JOIN album_members m ON m.photo_id=p.id
             WHERE p.id=?1 AND m.album_id=?2 AND p.removed_at_ms IS NULL"
        } else {
            "SELECT selection_state,rating FROM photos WHERE id=?1 AND removed_at_ms IS NULL"
        })
        .map_err(|_| PhotoQueryError::Storage)?;
    let candidates: Box<dyn Iterator<Item = &PhotoQueryCandidate>> = match query.order {
        PhotoQueryOrder::CaptureTimeAscending => Box::new(projection.ascending().iter()),
        PhotoQueryOrder::CaptureTimeDescending => Box::new(projection.descending()),
        PhotoQueryOrder::AlbumOrder => unreachable!(),
    };
    for candidate in candidates {
        if let PhotoQuerySource::Folder(location) = &query.source
            && !candidate_in_folder(candidate, location)
        {
            continue;
        }
        let facts = if let Some(album_id) = album_id {
            current
                .query_row(params![candidate.photo_id, album_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .optional()
        } else {
            current
                .query_row([&candidate.photo_id], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .optional()
        }
        .map_err(|_| PhotoQueryError::Storage)?;
        let Some((selection, rating)) = facts else {
            continue;
        };
        let selection = parse_selection_state(&selection).map_err(|_| PhotoQueryError::Storage)?;
        let rating = rating.try_into().map_err(|_| PhotoQueryError::Storage)?;
        if candidate_matches(&query, candidate, selection, rating) {
            push_query_match(&mut ids, &candidate.photo_id, maximum_results)?;
        }
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use crate::persistence::Persistence;
    use crate::persistence::test_support::*;
    use crate::{
        AlbumMutation, CaptureFact, CaptureMetadataState, CaptureTimeBound, CaptureTimeField,
        OriginalKind, PhotoQuery, PhotoQueryError, PhotoQueryOrder, PhotoQuerySource,
        PhotoStateField, PhotoStateMutation, PhotoStateValue, SelectionState,
    };
    use rusqlite::Connection;
    use rusqlite::params;
    use serde::Deserialize;
    use std::{collections::HashMap, sync::Arc};

    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct CaptureOrderVector {
        name: String,
        raw_path: Option<String>,
        raw_order_key: Option<String>,
        jpeg_path: Option<String>,
        jpeg_order_key: Option<String>,
        order_key: Option<String>,
        #[serde(default)]
        expected_paths: Vec<String>,
        expected_photo_ids: Option<Vec<String>>,
    }

    fn capture_order_vectors() -> Vec<CaptureOrderVector> {
        serde_json::from_str(include_str!(
            "../../../../compatibility/metadata/capture-order.json"
        ))
        .unwrap()
    }

    #[tokio::test]
    async fn the_library_folder_root_resolves_while_no_photo_is_projected() {
        let (_base, library, state, name, _path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(Vec::new(), Vec::new())
            .await
            .unwrap();
        let projection = query_projection(&snapshot);
        // The Library Folder root is the Library itself, so it stays a valid
        // source while no Photo is present to prove it; a named Folder still
        // needs a member.
        let ids = persistence
            .create_photo_query_receiver(
                PhotoQuery {
                    source: PhotoQuerySource::Folder(String::new()),
                    selection_state: None,
                    rating_minimum: None,
                    rating_maximum: None,
                    original_kind: None,
                    original_available: None,
                    captured_from: None,
                    captured_before: None,
                    order: PhotoQueryOrder::CaptureTimeAscending,
                },
                Arc::clone(&projection),
                10,
            )
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert!(ids.is_empty());
        assert!(matches!(
            persistence
                .create_photo_query_receiver(
                    PhotoQuery {
                        source: PhotoQuerySource::Folder("shoot".to_owned()),
                        selection_state: None,
                        rating_minimum: None,
                        rating_maximum: None,
                        original_kind: None,
                        original_available: None,
                        captured_from: None,
                        captured_before: None,
                        order: PhotoQueryOrder::CaptureTimeAscending,
                    },
                    Arc::clone(&projection),
                    10,
                )
                .unwrap()
                .await
                .unwrap(),
            Err(PhotoQueryError::SourceNotFound)
        ));
    }

    #[tokio::test]
    async fn bounded_photo_queries_fix_ordered_membership_and_read_current_facts() {
        let (_base, library, state, name, _path) = fixture();
        let mut early = discovered("shoot/early.JPG", OriginalKind::Jpeg, 1, 1.0);
        early.capture = CaptureFact {
            state: CaptureMetadataState::Known,
            order_key: Some("2026-01-01T09:00:00.000000000".to_owned()),
            field: Some(CaptureTimeField::DateTimeOriginal),
            offset_minutes: Some(90),
            source_revision: Some("early-revision".to_owned()),
        };
        let mut late = discovered("shoot/nested/late.RAF", OriginalKind::Raw, 2, 2.0);
        late.capture = CaptureFact {
            state: CaptureMetadataState::Known,
            order_key: Some("2026-01-01T10:00:00.000000000".to_owned()),
            field: Some(CaptureTimeField::DateTimeOriginal),
            offset_minutes: None,
            source_revision: Some("late-revision".to_owned()),
        };
        let missing_time = discovered("other/missing.JPG", OriginalKind::Jpeg, 3, 3.0);
        let upper = discovered("Shoot/upper.JPG", OriginalKind::Jpeg, 4, 4.0);
        let short = discovered("a/one.JPG", OriginalKind::Jpeg, 5, 5.0);
        let sibling = discovered("ab/two.JPG", OriginalKind::Jpeg, 6, 6.0);
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![late, missing_time, early, upper, short, sibling],
                Vec::new(),
            )
            .await
            .unwrap();
        let projection = query_projection(&snapshot);
        let by_path = snapshot
            .photos
            .iter()
            .map(|photo| (photo.sort_path.as_str(), photo.id.clone()))
            .collect::<HashMap<_, _>>();
        let early_id = by_path["shoot/early.JPG"].clone();
        let late_id = by_path["shoot/nested/late.RAF"].clone();
        let album_id = persistence
            .mutate_album(AlbumMutation::Create {
                name: "Order".to_owned(),
            })
            .await
            .unwrap()
            .album_id;
        persistence
            .mutate_album(AlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: vec![late_id.clone(), early_id.clone()],
            })
            .await
            .unwrap();
        let album_order = persistence
            .create_photo_query_receiver(
                PhotoQuery {
                    source: PhotoQuerySource::Album(album_id),
                    selection_state: None,
                    rating_minimum: None,
                    rating_maximum: None,
                    original_kind: None,
                    original_available: None,
                    captured_from: None,
                    captured_before: None,
                    order: PhotoQueryOrder::AlbumOrder,
                },
                Arc::clone(&projection),
                10,
            )
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(album_order, vec![late_id.clone(), early_id.clone()]);

        persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: early_id.clone(),
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Selected),
                expected_current: None,
                album_id: None,
            })
            .await
            .unwrap();
        persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: early_id.clone(),
                field: PhotoStateField::Rating,
                value: PhotoStateValue::Rating(4),
                expected_current: None,
                album_id: None,
            })
            .await
            .unwrap();

        let query = PhotoQuery {
            source: PhotoQuerySource::Folder("shoot".to_owned()),
            selection_state: Some(SelectionState::Selected),
            rating_minimum: Some(4),
            rating_maximum: Some(5),
            original_kind: Some(OriginalKind::Jpeg),
            original_available: Some(true),
            captured_from: Some(CaptureTimeBound::parse("2026-01-01T08:00:00").unwrap()),
            captured_before: Some(CaptureTimeBound::parse("2026-01-01T10:00:00").unwrap()),
            order: PhotoQueryOrder::CaptureTimeAscending,
        };
        let ids = persistence
            .create_photo_query_receiver(query, Arc::clone(&projection), 10)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(ids, vec![early_id.clone()]);
        let narrow = persistence
            .create_photo_query_receiver(
                PhotoQuery {
                    source: PhotoQuerySource::AllPhotos,
                    selection_state: Some(SelectionState::Selected),
                    rating_minimum: Some(4),
                    rating_maximum: None,
                    original_kind: None,
                    original_available: None,
                    captured_from: None,
                    captured_before: None,
                    order: PhotoQueryOrder::CaptureTimeAscending,
                },
                Arc::clone(&projection),
                1,
            )
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(narrow, vec![early_id.clone()]);
        for (folder, expected_path) in [("Shoot", "Shoot/upper.JPG"), ("a", "a/one.JPG")] {
            let ids = persistence
                .create_photo_query_receiver(
                    PhotoQuery {
                        source: PhotoQuerySource::Folder(folder.to_owned()),
                        selection_state: None,
                        rating_minimum: None,
                        rating_maximum: None,
                        original_kind: None,
                        original_available: None,
                        captured_from: None,
                        captured_before: None,
                        order: PhotoQueryOrder::CaptureTimeAscending,
                    },
                    Arc::clone(&projection),
                    10,
                )
                .unwrap()
                .await
                .unwrap()
                .unwrap();
            assert_eq!(ids, vec![by_path[expected_path].clone()]);
        }
        assert!(matches!(
            persistence
                .create_photo_query_receiver(
                    PhotoQuery {
                        source: PhotoQuerySource::Folder("SHOOT".to_owned()),
                        selection_state: None,
                        rating_minimum: None,
                        rating_maximum: None,
                        original_kind: None,
                        original_available: None,
                        captured_from: None,
                        captured_before: None,
                        order: PhotoQueryOrder::CaptureTimeAscending,
                    },
                    Arc::clone(&projection),
                    10,
                )
                .unwrap()
                .await
                .unwrap(),
            Err(PhotoQueryError::SourceNotFound)
        ));
        // Query membership is fixed, while a later owner read returns current
        // facts even when the Photo no longer matches the creation filter.
        persistence
            .mutate_photo_state(PhotoStateMutation {
                photo_id: early_id.clone(),
                field: PhotoStateField::SelectionState,
                value: PhotoStateValue::Selection(SelectionState::Rejected),
                expected_current: None,
                album_id: None,
            })
            .await
            .unwrap();
        assert!(matches!(
            persistence
                .create_photo_query_receiver(
                    PhotoQuery {
                        source: PhotoQuerySource::Folder("missing".to_owned()),
                        selection_state: None,
                        rating_minimum: None,
                        rating_maximum: None,
                        original_kind: None,
                        original_available: None,
                        captured_from: None,
                        captured_before: None,
                        order: PhotoQueryOrder::CaptureTimeAscending,
                    },
                    Arc::clone(&projection),
                    10,
                )
                .unwrap()
                .await
                .unwrap(),
            Err(PhotoQueryError::SourceNotFound)
        ));
        assert!(matches!(
            persistence
                .create_photo_query_receiver(
                    PhotoQuery {
                        source: PhotoQuerySource::AllPhotos,
                        selection_state: None,
                        rating_minimum: None,
                        rating_maximum: None,
                        original_kind: None,
                        original_available: None,
                        captured_from: None,
                        captured_before: None,
                        order: PhotoQueryOrder::CaptureTimeDescending,
                    },
                    Arc::clone(&projection),
                    2,
                )
                .unwrap()
                .await
                .unwrap(),
            Err(PhotoQueryError::ResultLimitExceeded { limit: 2 })
        ));

        let current = persistence
            .photos_by_id_receiver(
                vec![late_id, early_id.clone(), "removed".to_owned()],
                Arc::clone(&projection),
            )
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(current.len(), 3);
        assert_eq!(current[1].as_ref().unwrap().filename, "early.JPG");
        assert_eq!(current[1].as_ref().unwrap().rating, 4);
        assert_eq!(
            current[1].as_ref().unwrap().selection_state,
            SelectionState::Rejected
        );
        assert_eq!(
            current[1].as_ref().unwrap().capture.offset_minutes,
            Some(90)
        );
        assert!(current[2].is_none());
    }

    #[test]
    fn capture_time_bounds_reject_offsets_and_invalid_calendar_values() {
        assert!(CaptureTimeBound::parse("2026-02-28T23:59:59").is_ok());
        assert!(CaptureTimeBound::parse("2024-02-29T00:00:00").is_ok());
        for invalid in [
            "0000-01-01T00:00:00",
            "2026-02-29T00:00:00",
            "2026-01-01T00:00:00Z",
            "2026-01-01T00:00:00+01:00",
            "2026-13-01T00:00:00",
        ] {
            assert!(CaptureTimeBound::parse(invalid).is_err(), "{invalid}");
        }
    }

    #[tokio::test]
    async fn capture_order_orders_each_photo_by_its_own_original_and_retains_unavailable_facts() {
        let (_base, library, state, name, _path) = fixture();
        let vectors = capture_order_vectors();
        let disagreement = vectors
            .iter()
            .find(|vector| vector.name == "independent-photos-order-by-their-own-capture-time")
            .unwrap();
        let missing_partition = vectors
            .iter()
            .find(|vector| vector.name == "missing-capture-time-is-a-final-path-partition")
            .unwrap();
        let known = |key: &str, revision: &str| CaptureFact {
            state: CaptureMetadataState::Known,
            order_key: Some(key.to_owned()),
            field: Some(CaptureTimeField::DateTimeOriginal),
            offset_minutes: None,
            source_revision: Some(revision.to_owned()),
        };
        let mut raw = discovered(
            disagreement.raw_path.as_deref().unwrap(),
            OriginalKind::Raw,
            1,
            1.0,
        );
        raw.capture = known(disagreement.raw_order_key.as_deref().unwrap(), "raw");
        let mut paired_jpeg = discovered(
            disagreement.jpeg_path.as_deref().unwrap(),
            OriginalKind::Jpeg,
            1,
            1.0,
        );
        paired_jpeg.capture = known(disagreement.jpeg_order_key.as_deref().unwrap(), "jpeg");
        let mut middle = discovered("middle.JPG", OriginalKind::Jpeg, 1, 1.0);
        middle.capture = known("2026-01-01T10:30:00.000000000", "middle");
        let mut z = discovered("z.JPG", OriginalKind::Jpeg, 1, 1.0);
        z.capture = known("2026-01-01T12:00:00.000000000", "z");
        let mut a = discovered("a.JPG", OriginalKind::Jpeg, 1, 1.0);
        a.capture = known("2026-01-01T12:00:00.000000000", "a");
        let missing = discovered("missing.JPG", OriginalKind::Jpeg, 1, 1.0);
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let first = persistence
            .apply_scan(
                vec![raw.clone(), paired_jpeg, middle, z, a, missing],
                Vec::new(),
            )
            .await
            .unwrap();
        assert_eq!(
            first
                .photos
                .iter()
                .map(|photo| photo.sort_path.as_str())
                .collect::<Vec<_>>(),
            disagreement
                .expected_paths
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
        assert_eq!(missing_partition.expected_paths, ["missing.JPG"]);
        let unavailable = persistence
            .apply_scan(Vec::new(), Vec::new())
            .await
            .unwrap();
        assert_eq!(
            unavailable
                .originals
                .iter()
                .find(|original| original.relative_path.as_str() == "pair.ARW")
                .unwrap()
                .capture,
            raw.capture
        );
        raw.facts.size = 2;
        raw.capture = CaptureFact {
            state: CaptureMetadataState::Missing,
            order_key: None,
            field: None,
            offset_minutes: None,
            source_revision: Some("raw-replaced".to_owned()),
        };
        let replacement_fact = raw.capture.clone();
        let replaced = persistence.apply_scan(vec![raw], Vec::new()).await.unwrap();
        assert_eq!(
            replaced
                .originals
                .iter()
                .find(|original| original.relative_path.as_str() == "pair.ARW")
                .unwrap()
                .capture,
            replacement_fact
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn equal_capture_and_path_ties_use_photo_id_bytes() {
        let (_base, library, state, name, path) = fixture();
        let vectors = capture_order_vectors();
        let tie = vectors
            .iter()
            .find(|vector| vector.name == "equal-time-ties-use-path-then-photo-id-bytes")
            .unwrap();
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v3.sql"),
        );
        let connection = Connection::open(&path).unwrap();
        for (original_id, path) in [
            ("original-a", "source-a.JPG"),
            ("original-z", "source-z.JPG"),
        ] {
            connection.execute(
                "INSERT INTO original_files(id,relative_path,kind,size,mtime_ms,available,error_category,error_message,capture_metadata_state,capture_order_key,capture_time_field,capture_offset_minutes,capture_source_revision) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
                params![original_id, path, "jpeg", 1_i64, 1.0_f64, 1_i64, Option::<String>::None, Option::<String>::None, "known", tie.order_key.as_deref().unwrap(), "date-time-original", Option::<i64>::None, "revision"],
            ).unwrap();
        }
        for (photo_id, original_id) in [("z-photo", "original-z"), ("a-photo", "original-a")] {
            connection.execute(
                "INSERT INTO photos(id,jpeg_original_id,ambiguous,available,preview_state,sort_path,selection_state,rating) VALUES(?,?,?,?,?,?,?,?)",
                params![photo_id, original_id, 0_i64, 1_i64, "inspection-pending", "same.JPG", "undecided", 0_i64],
            ).unwrap();
        }
        drop(connection);
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        assert_eq!(
            persistence
                .snapshot()
                .await
                .unwrap()
                .photos
                .iter()
                .map(|photo| photo.id.as_str())
                .collect::<Vec<_>>(),
            tie.expected_photo_ids
                .as_ref()
                .unwrap()
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
        persistence.shutdown().unwrap();
    }
}
