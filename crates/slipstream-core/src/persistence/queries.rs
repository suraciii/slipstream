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
