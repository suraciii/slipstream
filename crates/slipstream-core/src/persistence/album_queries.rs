//! Albums: reads, browse targets, and the bounded Album query projection. The
//! write error surface and mutation normalization live in `album_write`.

use super::codecs::parse_selection_state;
use super::owner::{MutationVersions, PersistenceError};
use crate::{
    AlbumBrowseMember, AlbumBrowseTarget, AlbumMember, AlbumQueryFilter, AlbumRecord, AlbumSummary,
    PhotoAlbumMembership, PhotoQueryError,
};
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, params_from_iter};

pub(super) fn list_albums(connection: &Connection) -> Result<Vec<AlbumRecord>, PersistenceError> {
    let albums = connection
        .prepare("SELECT id,name FROM albums ORDER BY created_at,id")
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    let mut result = Vec::with_capacity(albums.len());
    for (id, name) in albums {
        let members = connection
            .prepare(
                "SELECT m.photo_id,m.position,p.available,p.selection_state,p.rating
                 FROM album_members m JOIN photos p ON p.id=m.photo_id
                 WHERE m.album_id=? AND p.removed_at_ms IS NULL ORDER BY m.position",
            )
            .map_err(|_| PersistenceError::Storage)?
            .query_map([id.as_str()], |row| {
                Ok(AlbumMember {
                    photo_id: row.get(0)?,
                    position: row
                        .get::<_, i64>(1)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    available: row.get::<_, i64>(2)? != 0,
                    selection_state: parse_selection_state(&row.get::<_, String>(3)?)?,
                    rating: row
                        .get::<_, i64>(4)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                })
            })
            .map_err(|_| PersistenceError::Storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| PersistenceError::Storage)?;
        let last_reviewed_photo_id = connection
            .query_row(
                "SELECT photo_id FROM album_progress WHERE album_id=?",
                [id.as_str()],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| PersistenceError::Storage)?;
        result.push(AlbumRecord {
            id,
            name,
            last_reviewed_photo_id,
            members,
        });
    }
    Ok(result)
}

/// The row shape both album summary reads project: identity, name, the live
/// photo count, and whether a saved progress position exists.
type AlbumSummaryRow = (String, String, i64, i64);

fn album_summary_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AlbumSummaryRow> {
    Ok((
        row.get::<_, String>(0)?,
        row.get::<_, String>(1)?,
        row.get::<_, i64>(2)?,
        row.get::<_, i64>(3)?,
    ))
}

/// Projects one summary row against the mutation versions. An out-of-range
/// count maps to the same storage error both readers use.
fn album_summary(
    versions: &MutationVersions,
    (id, name, photo_count, has_saved_position): AlbumSummaryRow,
) -> Result<AlbumSummary, PersistenceError> {
    Ok(AlbumSummary {
        album_version: versions.album(&id),
        id,
        name,
        photo_count: photo_count
            .try_into()
            .map_err(|_| PersistenceError::Storage)?,
        has_saved_position: has_saved_position != 0,
    })
}

pub(super) fn list_album_summaries(
    connection: &Connection,
    versions: &MutationVersions,
) -> Result<Vec<AlbumSummary>, PersistenceError> {
    let rows = connection
        .prepare(
            "SELECT a.id, a.name,
                    (SELECT count(*) FROM album_members m
                       JOIN photos p ON p.id = m.photo_id
                      WHERE m.album_id = a.id AND p.removed_at_ms IS NULL),
                    EXISTS(SELECT 1 FROM album_progress p WHERE p.album_id = a.id)
             FROM albums a ORDER BY a.created_at, a.id",
        )
        .map_err(|_| PersistenceError::Storage)?
        .query_map([], album_summary_row)
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    rows.into_iter()
        .map(|row| album_summary(versions, row))
        .collect()
}

pub(super) fn read_album(
    connection: &Connection,
    versions: &MutationVersions,
    album_id: &str,
) -> Result<Option<AlbumSummary>, PersistenceError> {
    let row = connection
        .query_row(
            "SELECT a.id, a.name,
                    (SELECT count(*) FROM album_members m
                       JOIN photos p ON p.id = m.photo_id
                      WHERE m.album_id = a.id AND p.removed_at_ms IS NULL),
                    EXISTS(SELECT 1 FROM album_progress p WHERE p.album_id = a.id)
             FROM albums a WHERE a.id=?",
            [album_id],
            album_summary_row,
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    row.map(|row| album_summary(versions, row)).transpose()
}

pub(super) fn create_album_query(
    connection: &Connection,
    filter: AlbumQueryFilter,
    maximum_results: usize,
) -> Result<Vec<String>, PhotoQueryError> {
    if maximum_results == 0 || maximum_results == usize::MAX {
        return Err(PhotoQueryError::Invalid);
    }
    let sql_limit = i64::try_from(maximum_results + 1).map_err(|_| PhotoQueryError::Invalid)?;
    let (sql, parameters): (&str, Vec<Value>) = match filter {
        AlbumQueryFilter::All => (
            "SELECT a.id FROM albums a ORDER BY a.created_at,a.id LIMIT ?",
            vec![sql_limit.into()],
        ),
        AlbumQueryFilter::ExactName(name) if !name.is_empty() => (
            "SELECT a.id FROM albums a WHERE a.name=? COLLATE NOCASE ORDER BY a.created_at,a.id LIMIT ?",
            vec![name.into(), sql_limit.into()],
        ),
        AlbumQueryFilter::ContainsPhoto(photo_id) if !photo_id.is_empty() => (
            "SELECT a.id FROM albums a JOIN album_members m ON m.album_id=a.id WHERE m.photo_id=? ORDER BY a.created_at,a.id LIMIT ?",
            vec![photo_id.into(), sql_limit.into()],
        ),
        _ => return Err(PhotoQueryError::Invalid),
    };
    let mut statement = connection
        .prepare(sql)
        .map_err(|_| PhotoQueryError::Storage)?;
    let ids = statement
        .query_map(params_from_iter(parameters), |row| row.get::<_, String>(0))
        .map_err(|_| PhotoQueryError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PhotoQueryError::Storage)?;
    if ids.len() > maximum_results {
        Err(PhotoQueryError::ResultLimitExceeded {
            limit: maximum_results,
        })
    } else {
        Ok(ids)
    }
}
pub(super) fn photo_albums(
    connection: &Connection,
    photo_id: &str,
) -> Result<Option<Vec<PhotoAlbumMembership>>, PersistenceError> {
    let exists = connection
        .query_row("SELECT 1 FROM photos WHERE id=?", [photo_id], |_| Ok(()))
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    if exists.is_none() {
        return Ok(None);
    }
    connection
        .prepare(
            "SELECT a.id, a.name
             FROM album_members m JOIN albums a ON a.id = m.album_id
             WHERE m.photo_id = ? ORDER BY a.created_at, a.id",
        )
        .map_err(|_| PersistenceError::Storage)?
        .query_map([photo_id], |row| {
            Ok(PhotoAlbumMembership {
                album_id: row.get(0)?,
                album_name: row.get(1)?,
            })
        })
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
        .map_err(|_| PersistenceError::Storage)
}

pub(super) fn album_browse_target(
    connection: &Connection,
    album_id: &str,
) -> Result<Option<AlbumBrowseTarget>, PersistenceError> {
    let exists = connection
        .query_row("SELECT 1 FROM albums WHERE id=?", [album_id], |_| Ok(()))
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    if exists.is_none() {
        return Ok(None);
    }
    let members = connection
        .prepare(
            "SELECT m.photo_id, p.available
             FROM album_members m JOIN photos p ON p.id=m.photo_id
             WHERE m.album_id=? AND p.removed_at_ms IS NULL ORDER BY m.position",
        )
        .map_err(|_| PersistenceError::Storage)?
        .query_map([album_id], |row| {
            Ok(AlbumBrowseMember {
                photo_id: row.get(0)?,
                available: row.get::<_, i64>(1)? != 0,
            })
        })
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    let saved_photo_id = connection
        .query_row(
            "SELECT photo_id FROM album_progress WHERE album_id=?",
            [album_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    Ok(Some(AlbumBrowseTarget {
        members,
        saved_photo_id,
    }))
}
