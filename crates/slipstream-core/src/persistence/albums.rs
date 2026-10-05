//! Albums: reads, browse targets, versioned writes, membership, and the album
//! write error surface.

use super::admission::{DatabaseName, StateDirectory};
use super::mutation::{mutation_error_from_sqlite, mutation_transaction};
use super::owner::{MutationError, MutationVersions, PersistenceError, unix_millis, uuid_v4};
use super::scan::parse_selection_state;
use crate::{
    ALBUM_MEMBERSHIP_BATCH_MAX, AlbumBrowseMember, AlbumBrowseTarget, AlbumCreationResult,
    AlbumMember, AlbumMembershipMutation, AlbumMembershipResult, AlbumMutation,
    AlbumMutationResult, AlbumQueryFilter, AlbumRecord, AlbumSummary, CheckedAlbumMutation,
    CheckedAlbumMutationResult, MAXIMUM_FOLDER_ALBUM_PHOTOS, PhotoAlbumMembership, PhotoQueryError,
};
use rusqlite::types::Value;
use rusqlite::{Connection, OptionalExtension, Transaction, params, params_from_iter};
use std::collections::HashSet;
use std::fmt;

/// Detailed refusal or storage outcome for checked Album creation and changes.
/// Identity-bearing variants let the HTTP owner map the domain result without
/// parsing display text or issuing a racy follow-up read.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AlbumWriteError {
    Invalid,
    AlbumNotFound {
        album_id: String,
    },
    PhotoNotFound {
        photo_id: String,
    },
    VersionConflict {
        album_id: String,
        current_version: String,
    },
    NameConflict {
        name: String,
        album_id: String,
    },
    MembershipConflict {
        album_id: String,
        current_version: String,
    },
    LimitExceeded {
        limit: usize,
        actual: usize,
    },
    Persistence,
    Saturated,
    Closed,
}

impl fmt::Display for AlbumWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid => formatter.write_str("Album write request is not valid"),
            Self::AlbumNotFound { .. } => formatter.write_str("Album was not found"),
            Self::PhotoNotFound { .. } => formatter.write_str("Photo was not found"),
            Self::VersionConflict { .. } => {
                formatter.write_str("Album version conflicts with current state")
            }
            Self::NameConflict { .. } => formatter.write_str("Album name already exists"),
            Self::MembershipConflict { .. } => {
                formatter.write_str("Album order does not match current membership")
            }
            Self::LimitExceeded { .. } => formatter.write_str("Album write exceeds its limit"),
            Self::Persistence => formatter.write_str("Album write could not be persisted"),
            Self::Saturated => formatter.write_str("SQLite persistence queue is saturated"),
            Self::Closed => formatter.write_str("SQLite persistence is closed"),
        }
    }
}

impl std::error::Error for AlbumWriteError {}

pub(super) fn album_write_error_from_persistence(error: PersistenceError) -> AlbumWriteError {
    match error {
        PersistenceError::Saturated => AlbumWriteError::Saturated,
        PersistenceError::Closed => AlbumWriteError::Closed,
        _ => AlbumWriteError::Persistence,
    }
}

fn album_write_error_from_mutation(error: MutationError) -> AlbumWriteError {
    match error {
        MutationError::Saturated => AlbumWriteError::Saturated,
        MutationError::Closed => AlbumWriteError::Closed,
        _ => AlbumWriteError::Persistence,
    }
}

pub(super) fn normalize_album_mutation(
    mutation: AlbumMutation,
) -> Result<AlbumMutation, MutationError> {
    let trim_name = |name: String| {
        let name = name.trim().to_owned();
        if name.is_empty() || name.chars().count() > 120 {
            Err(MutationError::Conflict)
        } else {
            Ok(name)
        }
    };
    match mutation {
        AlbumMutation::Create { name } => Ok(AlbumMutation::Create {
            name: trim_name(name)?,
        }),
        AlbumMutation::Rename { album_id, name } => Ok(AlbumMutation::Rename {
            album_id,
            name: trim_name(name)?,
        }),
        AlbumMutation::AddMembers {
            album_id,
            photo_ids,
        } => {
            if photo_ids.len() > 100
                || photo_ids
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != photo_ids.len()
            {
                return Err(MutationError::Conflict);
            }
            Ok(AlbumMutation::AddMembers {
                album_id,
                photo_ids,
            })
        }
        AlbumMutation::AddFolderMembers {
            album_id,
            photo_ids,
        } => {
            if photo_ids.len() > MAXIMUM_FOLDER_ALBUM_PHOTOS
                || photo_ids
                    .iter()
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != photo_ids.len()
            {
                return Err(MutationError::Conflict);
            }
            Ok(AlbumMutation::AddFolderMembers {
                album_id,
                photo_ids,
            })
        }
        AlbumMutation::Delete { .. }
        | AlbumMutation::RemoveMember { .. }
        | AlbumMutation::Reorder { .. }
        | AlbumMutation::SetProgress { .. } => Ok(mutation),
    }
}

pub(super) fn normalize_album_membership_mutation(
    mutation: AlbumMembershipMutation,
) -> Result<AlbumMembershipMutation, MutationError> {
    let (album_id, photo_ids, kind) = match mutation {
        AlbumMembershipMutation::Add {
            album_id,
            photo_ids,
        } => (album_id, photo_ids, 0_u8),
        AlbumMembershipMutation::RemoveAdded {
            album_id,
            photo_ids,
        } => (album_id, photo_ids, 1_u8),
    };
    if album_id.trim().is_empty()
        || photo_ids.is_empty()
        || photo_ids.len() > ALBUM_MEMBERSHIP_BATCH_MAX
        || photo_ids.iter().any(|photo_id| photo_id.trim().is_empty())
        || photo_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != photo_ids.len()
    {
        return Err(MutationError::Invalid);
    }
    Ok(if kind == 0 {
        AlbumMembershipMutation::Add {
            album_id,
            photo_ids,
        }
    } else {
        AlbumMembershipMutation::RemoveAdded {
            album_id,
            photo_ids,
        }
    })
}

pub(super) fn normalize_album_name(name: String) -> Result<String, AlbumWriteError> {
    let name = name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 120 {
        Err(AlbumWriteError::Invalid)
    } else {
        Ok(name)
    }
}

pub(super) fn normalize_checked_album_mutation(
    mutation: CheckedAlbumMutation,
) -> Result<CheckedAlbumMutation, AlbumWriteError> {
    let validate_target = |album_id: &str, expected_version: &str| {
        if album_id.trim().is_empty() || expected_version.is_empty() {
            Err(AlbumWriteError::Invalid)
        } else {
            Ok(())
        }
    };
    let validate_photo_ids = |photo_ids: &[String]| {
        if photo_ids.len() > ALBUM_MEMBERSHIP_BATCH_MAX {
            return Err(AlbumWriteError::LimitExceeded {
                limit: ALBUM_MEMBERSHIP_BATCH_MAX,
                actual: photo_ids.len(),
            });
        }
        if photo_ids.is_empty()
            || photo_ids.iter().any(|photo_id| photo_id.trim().is_empty())
            || photo_ids
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != photo_ids.len()
        {
            Err(AlbumWriteError::Invalid)
        } else {
            Ok(())
        }
    };
    match mutation {
        CheckedAlbumMutation::Rename {
            album_id,
            name,
            expected_version,
        } => {
            validate_target(&album_id, &expected_version)?;
            Ok(CheckedAlbumMutation::Rename {
                album_id,
                name: normalize_album_name(name)?,
                expected_version,
            })
        }
        CheckedAlbumMutation::Delete {
            album_id,
            expected_version,
        } => {
            validate_target(&album_id, &expected_version)?;
            Ok(CheckedAlbumMutation::Delete {
                album_id,
                expected_version,
            })
        }
        CheckedAlbumMutation::AddMembers {
            album_id,
            photo_ids,
            expected_version,
        } => {
            validate_target(&album_id, &expected_version)?;
            validate_photo_ids(&photo_ids)?;
            Ok(CheckedAlbumMutation::AddMembers {
                album_id,
                photo_ids,
                expected_version,
            })
        }
        CheckedAlbumMutation::RemoveMembers {
            album_id,
            photo_ids,
            expected_version,
        } => {
            validate_target(&album_id, &expected_version)?;
            validate_photo_ids(&photo_ids)?;
            Ok(CheckedAlbumMutation::RemoveMembers {
                album_id,
                photo_ids,
                expected_version,
            })
        }
        CheckedAlbumMutation::Reorder {
            album_id,
            photo_ids,
            expected_version,
        } => {
            validate_target(&album_id, &expected_version)?;
            validate_photo_ids(&photo_ids)?;
            Ok(CheckedAlbumMutation::Reorder {
                album_id,
                photo_ids,
                expected_version,
            })
        }
    }
}

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
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(|_| PersistenceError::Storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| PersistenceError::Storage)?;
    rows.into_iter()
        .map(|(id, name, photo_count, has_saved_position)| {
            Ok(AlbumSummary {
                album_version: versions.album(&id),
                id,
                name,
                photo_count: photo_count
                    .try_into()
                    .map_err(|_| PersistenceError::Storage)?,
                has_saved_position: has_saved_position != 0,
            })
        })
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
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    row.map(|(id, name, photo_count, has_saved_position)| {
        Ok(AlbumSummary {
            album_version: versions.album(&id),
            id,
            name,
            photo_count: photo_count
                .try_into()
                .map_err(|_| PersistenceError::Storage)?,
            has_saved_position: has_saved_position != 0,
        })
    })
    .transpose()
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

#[derive(Eq, PartialEq)]
struct AlbumVersionState {
    pub(super) name: String,
    pub(super) ordered_photo_ids: Vec<String>,
    pub(super) saved_photo_id: Option<String>,
}

fn album_version_state(
    connection: &Connection,
    album_id: &str,
) -> Result<Option<AlbumVersionState>, PersistenceError> {
    let name = connection
        .query_row("SELECT name FROM albums WHERE id=?", [album_id], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|_| PersistenceError::Storage)?;
    let Some(name) = name else {
        return Ok(None);
    };
    let ordered_photo_ids = connection
        .prepare("SELECT photo_id FROM album_members WHERE album_id=? ORDER BY position")
        .map_err(|_| PersistenceError::Storage)?
        .query_map([album_id], |row| row.get(0))
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
    Ok(Some(AlbumVersionState {
        name,
        ordered_photo_ids,
        saved_photo_id,
    }))
}

pub(super) struct AlbumVersionPlan {
    pub(super) album_id: String,
    pub(super) advance: bool,
    pub(super) deleted: bool,
}

pub(super) fn album_version_plan(
    connection: &Connection,
    mutation: &AlbumMutation,
) -> Result<AlbumVersionPlan, MutationError> {
    let (album_id, advance, deleted) = match mutation {
        // A new opaque Album ID begins at counter zero in this process epoch.
        AlbumMutation::Create { .. } => {
            return Ok(AlbumVersionPlan {
                album_id: String::new(),
                advance: false,
                deleted: false,
            });
        }
        AlbumMutation::Rename { album_id, name } => {
            let before = album_version_state(connection, album_id)
                .map_err(|_| MutationError::Persistence)?;
            (
                album_id,
                before.is_some_and(|before| before.name != *name),
                false,
            )
        }
        AlbumMutation::Delete { album_id } => (album_id, false, true),
        AlbumMutation::AddMembers {
            album_id,
            photo_ids,
        }
        | AlbumMutation::AddFolderMembers {
            album_id,
            photo_ids,
        } => {
            let before = album_version_state(connection, album_id)
                .map_err(|_| MutationError::Persistence)?;
            let advance = before.is_some_and(|before| {
                photo_ids
                    .iter()
                    .any(|photo_id| !before.ordered_photo_ids.contains(photo_id))
            });
            (album_id, advance, false)
        }
        AlbumMutation::RemoveMember { album_id, .. } => (album_id, true, false),
        AlbumMutation::Reorder {
            album_id,
            photo_ids,
        } => {
            let before = album_version_state(connection, album_id)
                .map_err(|_| MutationError::Persistence)?;
            (
                album_id,
                before.is_some_and(|before| before.ordered_photo_ids != *photo_ids),
                false,
            )
        }
        AlbumMutation::SetProgress { album_id, .. } => (album_id, false, false),
    };
    Ok(AlbumVersionPlan {
        album_id: album_id.clone(),
        advance,
        deleted,
    })
}

fn conflicting_album_id(
    connection: &Connection,
    name: &str,
    except_album_id: Option<&str>,
) -> Result<Option<String>, AlbumWriteError> {
    let existing = connection
        .query_row(
            "SELECT id FROM albums WHERE name=? COLLATE NOCASE",
            [name],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| AlbumWriteError::Persistence)?;
    Ok(existing.filter(|album_id| Some(album_id.as_str()) != except_album_id))
}

fn require_checked_photos(
    connection: &Connection,
    photo_ids: &[String],
) -> Result<(), AlbumWriteError> {
    for photo_id in photo_ids {
        let exists = connection
            .query_row("SELECT 1 FROM photos WHERE id=?", [photo_id], |_| Ok(()))
            .optional()
            .map_err(|_| AlbumWriteError::Persistence)?;
        if exists.is_none() {
            return Err(AlbumWriteError::PhotoNotFound {
                photo_id: photo_id.clone(),
            });
        }
    }
    Ok(())
}

pub(super) fn create_album_checked(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    versions: &MutationVersions,
    name: String,
) -> Result<AlbumCreationResult, AlbumWriteError> {
    if let Some(album_id) = conflicting_album_id(connection, &name, None)? {
        return Err(AlbumWriteError::NameConflict { name, album_id });
    }
    let result = mutate_album(
        state,
        database_name,
        connection,
        AlbumMutation::Create { name: name.clone() },
    )
    .map_err(album_write_error_from_mutation)?;
    Ok(AlbumCreationResult {
        album: AlbumSummary {
            album_version: versions.album(&result.album_id),
            id: result.album_id,
            name,
            photo_count: 0,
            has_saved_position: false,
        },
    })
}

pub(super) fn mutate_album_checked(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    versions: &mut MutationVersions,
    mutation: CheckedAlbumMutation,
) -> Result<CheckedAlbumMutationResult, AlbumWriteError> {
    let identity = match &mutation {
        CheckedAlbumMutation::Rename {
            album_id,
            expected_version,
            ..
        }
        | CheckedAlbumMutation::Delete {
            album_id,
            expected_version,
        }
        | CheckedAlbumMutation::AddMembers {
            album_id,
            expected_version,
            ..
        }
        | CheckedAlbumMutation::RemoveMembers {
            album_id,
            expected_version,
            ..
        }
        | CheckedAlbumMutation::Reorder {
            album_id,
            expected_version,
            ..
        } => (album_id.clone(), expected_version.clone()),
    };
    let (album_id, expected_version) = (&identity.0, &identity.1);
    let Some(mut summary) =
        read_album(connection, versions, album_id).map_err(|_| AlbumWriteError::Persistence)?
    else {
        return Err(AlbumWriteError::AlbumNotFound {
            album_id: album_id.clone(),
        });
    };
    // The guard is deliberately checked before classifying an otherwise
    // idempotent request. A changed-away-and-back Album still conflicts.
    if summary.album_version != *expected_version {
        return Err(AlbumWriteError::VersionConflict {
            album_id: album_id.clone(),
            current_version: summary.album_version,
        });
    }
    let current = album_version_state(connection, album_id)
        .map_err(|_| AlbumWriteError::Persistence)?
        .ok_or_else(|| AlbumWriteError::AlbumNotFound {
            album_id: album_id.clone(),
        })?;

    match mutation {
        CheckedAlbumMutation::Rename { name, .. } => {
            if let Some(conflicting_id) = conflicting_album_id(connection, &name, Some(album_id))? {
                return Err(AlbumWriteError::NameConflict {
                    name,
                    album_id: conflicting_id,
                });
            }
            let renamed = current.name != name;
            if renamed && !versions.can_advance_album(album_id) {
                return Err(AlbumWriteError::Persistence);
            }
            mutate_album(
                state,
                database_name,
                connection,
                AlbumMutation::Rename {
                    album_id: album_id.clone(),
                    name: name.clone(),
                },
            )
            .map_err(album_write_error_from_mutation)?;
            if renamed {
                versions
                    .advance_album(album_id)
                    .map_err(album_write_error_from_mutation)?;
            }
            summary.name = name;
            summary.album_version = versions.album(album_id);
            Ok(CheckedAlbumMutationResult::Renamed {
                album: summary,
                renamed,
            })
        }
        CheckedAlbumMutation::Delete { .. } => {
            mutate_album(
                state,
                database_name,
                connection,
                AlbumMutation::Delete {
                    album_id: album_id.clone(),
                },
            )
            .map_err(album_write_error_from_mutation)?;
            versions.album.remove(album_id);
            Ok(CheckedAlbumMutationResult::Deleted {
                album_id: album_id.clone(),
            })
        }
        CheckedAlbumMutation::AddMembers { photo_ids, .. } => {
            require_checked_photos(connection, &photo_ids)?;
            let existing = current
                .ordered_photo_ids
                .iter()
                .map(String::as_str)
                .collect::<HashSet<_>>();
            let added_photo_ids = photo_ids
                .iter()
                .filter(|photo_id| !existing.contains(photo_id.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            let already_member_photo_ids = photo_ids
                .iter()
                .filter(|photo_id| existing.contains(photo_id.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            if !added_photo_ids.is_empty() && !versions.can_advance_album(album_id) {
                return Err(AlbumWriteError::Persistence);
            }
            mutate_album_membership(
                state,
                database_name,
                connection,
                AlbumMembershipMutation::Add {
                    album_id: album_id.clone(),
                    photo_ids,
                },
            )
            .map_err(album_write_error_from_mutation)?;
            if !added_photo_ids.is_empty() {
                versions
                    .advance_album(album_id)
                    .map_err(album_write_error_from_mutation)?;
            }
            summary = read_album(connection, versions, album_id)
                .map_err(|_| AlbumWriteError::Persistence)?
                .ok_or_else(|| AlbumWriteError::AlbumNotFound {
                    album_id: album_id.clone(),
                })?;
            Ok(CheckedAlbumMutationResult::Added {
                album: summary,
                added_photo_ids,
                already_member_photo_ids,
            })
        }
        CheckedAlbumMutation::RemoveMembers { photo_ids, .. } => {
            require_checked_photos(connection, &photo_ids)?;
            let existing = current
                .ordered_photo_ids
                .iter()
                .map(String::as_str)
                .collect::<HashSet<_>>();
            let removed_photo_ids = photo_ids
                .iter()
                .filter(|photo_id| existing.contains(photo_id.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            let already_absent_photo_ids = photo_ids
                .iter()
                .filter(|photo_id| !existing.contains(photo_id.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            if !removed_photo_ids.is_empty() && !versions.can_advance_album(album_id) {
                return Err(AlbumWriteError::Persistence);
            }
            mutate_album_membership(
                state,
                database_name,
                connection,
                AlbumMembershipMutation::RemoveAdded {
                    album_id: album_id.clone(),
                    photo_ids,
                },
            )
            .map_err(album_write_error_from_mutation)?;
            if !removed_photo_ids.is_empty() {
                versions
                    .advance_album(album_id)
                    .map_err(album_write_error_from_mutation)?;
            }
            let saved_photo_id = current
                .saved_photo_id
                .filter(|saved| !removed_photo_ids.iter().any(|removed| removed == saved));
            summary = read_album(connection, versions, album_id)
                .map_err(|_| AlbumWriteError::Persistence)?
                .ok_or_else(|| AlbumWriteError::AlbumNotFound {
                    album_id: album_id.clone(),
                })?;
            Ok(CheckedAlbumMutationResult::Removed {
                album: summary,
                removed_photo_ids,
                already_absent_photo_ids,
                saved_photo_id,
            })
        }
        CheckedAlbumMutation::Reorder { photo_ids, .. } => {
            require_checked_photos(connection, &photo_ids)?;
            if current.ordered_photo_ids.len() > ALBUM_MEMBERSHIP_BATCH_MAX {
                return Err(AlbumWriteError::LimitExceeded {
                    limit: ALBUM_MEMBERSHIP_BATCH_MAX,
                    actual: current.ordered_photo_ids.len(),
                });
            }
            let current_ids = current
                .ordered_photo_ids
                .iter()
                .map(String::as_str)
                .collect::<HashSet<_>>();
            let requested_ids = photo_ids.iter().map(String::as_str).collect::<HashSet<_>>();
            if current_ids != requested_ids {
                return Err(AlbumWriteError::MembershipConflict {
                    album_id: album_id.clone(),
                    current_version: summary.album_version,
                });
            }
            let reordered = current.ordered_photo_ids != photo_ids;
            if reordered && !versions.can_advance_album(album_id) {
                return Err(AlbumWriteError::Persistence);
            }
            mutate_album(
                state,
                database_name,
                connection,
                AlbumMutation::Reorder {
                    album_id: album_id.clone(),
                    photo_ids: photo_ids.clone(),
                },
            )
            .map_err(album_write_error_from_mutation)?;
            if reordered {
                versions
                    .advance_album(album_id)
                    .map_err(album_write_error_from_mutation)?;
            }
            summary.album_version = versions.album(album_id);
            Ok(CheckedAlbumMutationResult::Reordered {
                album: summary,
                ordered_photo_ids: photo_ids,
                reordered,
            })
        }
    }
}

fn require_album(transaction: &Transaction<'_>, album_id: &str) -> Result<(), MutationError> {
    transaction
        .query_row("SELECT 1 FROM albums WHERE id=?", [album_id], |_| Ok(()))
        .optional()
        .map_err(mutation_error_from_sqlite)?
        .ok_or(MutationError::NotFound)
}

pub(super) fn mutate_album(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    mutation: AlbumMutation,
) -> Result<AlbumMutationResult, MutationError> {
    mutation_transaction(state, database_name, connection, |transaction| {
        let (album_id, added_count, already_member_count) = match mutation {
            AlbumMutation::Create { name } => {
                let id = uuid_v4()?;
                transaction
                    .execute(
                        "INSERT INTO albums(id,name,created_at) VALUES(?,?,?)",
                        params![id, name, unix_millis()],
                    )
                    .map_err(mutation_error_from_sqlite)?;
                (id, 0, 0)
            }
            AlbumMutation::Rename { album_id, name } => {
                require_album(transaction, &album_id)?;
                transaction
                    .execute(
                        "UPDATE albums SET name=? WHERE id=?",
                        params![name, album_id],
                    )
                    .map_err(mutation_error_from_sqlite)?;
                (album_id, 0, 0)
            }
            AlbumMutation::Delete { album_id } => {
                require_album(transaction, &album_id)?;
                transaction
                    .execute("DELETE FROM albums WHERE id=?", [&album_id])
                    .map_err(mutation_error_from_sqlite)?;
                (album_id, 0, 0)
            }
            AlbumMutation::AddMembers {
                album_id,
                photo_ids,
            } => {
                require_album(transaction, &album_id)?;
                for photo_id in &photo_ids {
                    transaction
                        .query_row("SELECT 1 FROM photos WHERE id=?", [photo_id], |_| Ok(()))
                        .optional()
                        .map_err(mutation_error_from_sqlite)?
                        .ok_or(MutationError::NotFound)?;
                }
                let mut position: i64 = transaction
                    .query_row(
                        "SELECT COALESCE(MAX(position)+1,0) FROM album_members WHERE album_id=?",
                        [&album_id],
                        |row| row.get(0),
                    )
                    .map_err(mutation_error_from_sqlite)?;
                let mut added_count = 0;
                let mut already_member_count = 0;
                for photo_id in photo_ids {
                    let already_member = transaction
                        .query_row(
                            "SELECT 1 FROM album_members WHERE album_id=? AND photo_id=?",
                            params![album_id, photo_id],
                            |_| Ok(()),
                        )
                        .optional()
                        .map_err(mutation_error_from_sqlite)?;
                    if already_member.is_some() {
                        // Adding an existing member is idempotent: the
                        // persisted position is kept and no row is added.
                        already_member_count += 1;
                        continue;
                    }
                    transaction
                        .execute(
                            "INSERT INTO album_members(album_id,photo_id,position) VALUES(?,?,?)",
                            params![album_id, photo_id, position],
                        )
                        .map_err(mutation_error_from_sqlite)?;
                    position = position.checked_add(1).ok_or(MutationError::Conflict)?;
                    added_count += 1;
                }
                (album_id, added_count, already_member_count)
            }
            AlbumMutation::AddFolderMembers {
                album_id,
                photo_ids,
            } => {
                require_album(transaction, &album_id)?;
                for photo_id in &photo_ids {
                    transaction
                        .query_row("SELECT 1 FROM photos WHERE id=?", [photo_id], |_| Ok(()))
                        .optional()
                        .map_err(mutation_error_from_sqlite)?
                        .ok_or(MutationError::NotFound)?;
                }
                let mut position: i64 = transaction
                    .query_row(
                        "SELECT COALESCE(MAX(position)+1,0) FROM album_members WHERE album_id=?",
                        [&album_id],
                        |row| row.get(0),
                    )
                    .map_err(mutation_error_from_sqlite)?;
                let mut added_count = 0;
                let mut already_member_count = 0;
                for photo_id in photo_ids {
                    let already_member = transaction
                        .query_row(
                            "SELECT 1 FROM album_members WHERE album_id=? AND photo_id=?",
                            params![album_id, photo_id],
                            |_| Ok(()),
                        )
                        .optional()
                        .map_err(mutation_error_from_sqlite)?;
                    if already_member.is_some() {
                        already_member_count += 1;
                        continue;
                    }
                    transaction
                        .execute(
                            "INSERT INTO album_members(album_id,photo_id,position) VALUES(?,?,?)",
                            params![album_id, photo_id, position],
                        )
                        .map_err(mutation_error_from_sqlite)?;
                    position = position.checked_add(1).ok_or(MutationError::Conflict)?;
                    added_count += 1;
                }
                (album_id, added_count, already_member_count)
            }
            AlbumMutation::RemoveMember { album_id, photo_id } => {
                let position: i64 = transaction
                    .query_row(
                        "SELECT position FROM album_members WHERE album_id=? AND photo_id=?",
                        params![album_id, photo_id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(mutation_error_from_sqlite)?
                    .ok_or(MutationError::NotFound)?;
                transaction
                    .execute(
                        "DELETE FROM album_members WHERE album_id=? AND photo_id=?",
                        params![album_id, photo_id],
                    )
                    .map_err(mutation_error_from_sqlite)?;
                transaction
                    .execute(
                        "UPDATE album_members SET position=position-1 WHERE album_id=? AND position>?",
                        params![album_id, position],
                    )
                    .map_err(mutation_error_from_sqlite)?;
                (album_id, 0, 0)
            }
            AlbumMutation::Reorder {
                album_id,
                photo_ids,
            } => {
                require_album(transaction, &album_id)?;
                let current = transaction
                    .prepare(
                        "SELECT photo_id,position FROM album_members WHERE album_id=? ORDER BY position",
                    )
                    .map_err(mutation_error_from_sqlite)?
                    .query_map([&album_id], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                    })
                    .map_err(mutation_error_from_sqlite)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(mutation_error_from_sqlite)?;
                let current_ids = current
                    .iter()
                    .map(|(photo_id, _)| photo_id.clone())
                    .collect::<std::collections::BTreeSet<_>>();
                let requested_ids = photo_ids
                    .iter()
                    .cloned()
                    .collect::<std::collections::BTreeSet<_>>();
                if current
                    .iter()
                    .enumerate()
                    .any(|(index, (_, position))| *position != index as i64)
                    || current_ids != requested_ids
                    || requested_ids.len() != photo_ids.len()
                {
                    return Err(MutationError::Conflict);
                }
                // SQLite's schema requires nonnegative positions. After validating the
                // dense 0..n-1 invariant, n is strictly above every current and final
                // position, so the temporary range [n, 2n) cannot collide with either.
                let temporary_offset =
                    i64::try_from(current.len()).map_err(|_| MutationError::Conflict)?;
                if temporary_offset.checked_mul(2).is_none() {
                    return Err(MutationError::Conflict);
                }
                transaction
                    .execute(
                        "UPDATE album_members SET position=position+? WHERE album_id=?",
                        params![temporary_offset, album_id],
                    )
                    .map_err(mutation_error_from_sqlite)?;
                for (position, photo_id) in photo_ids.iter().enumerate() {
                    transaction
                        .execute(
                            "UPDATE album_members SET position=? WHERE album_id=? AND photo_id=?",
                            params![position as i64, album_id, photo_id],
                        )
                        .map_err(mutation_error_from_sqlite)?;
                }
                (album_id, 0, 0)
            }
            AlbumMutation::SetProgress { album_id, photo_id } => {
                transaction
                    .query_row(
                        "SELECT 1 FROM album_members WHERE album_id=? AND photo_id=?",
                        params![album_id, photo_id],
                        |_| Ok(()),
                    )
                    .optional()
                    .map_err(mutation_error_from_sqlite)?
                    .ok_or(MutationError::NotFound)?;
                transaction
                    .execute(
                        "INSERT INTO album_progress(album_id,photo_id) VALUES(?,?)
                         ON CONFLICT(album_id) DO UPDATE SET photo_id=excluded.photo_id",
                        params![album_id, photo_id],
                    )
                    .map_err(mutation_error_from_sqlite)?;
                (album_id, 0, 0)
            }
        };
        Ok(AlbumMutationResult {
            album_id,
            added_count,
            already_member_count,
        })
    })
}

/// Applies one bounded identity-bearing membership operation. The add result
/// distinguishes newly inserted IDs from existing members; the compensation
/// result distinguishes removed IDs from members already absent. Both result
/// lists preserve request order so the browser can retain an exact, bounded
/// record without reconstructing membership from aggregate counts.
pub(super) fn mutate_album_membership(
    state: &StateDirectory,
    database_name: &DatabaseName,
    connection: &mut Connection,
    mutation: AlbumMembershipMutation,
) -> Result<AlbumMembershipResult, MutationError> {
    mutation_transaction(state, database_name, connection, |transaction| {
        let (album_id, photo_ids) = match &mutation {
            AlbumMembershipMutation::Add {
                album_id,
                photo_ids,
            }
            | AlbumMembershipMutation::RemoveAdded {
                album_id,
                photo_ids,
            } => (album_id, photo_ids),
        };
        require_album(transaction, album_id)?;
        // Resolve every identity before the first membership write. An
        // unknown Photo is a malformed compensation record, not an implicit
        // already-absent result, and therefore cannot produce a partial batch.
        for photo_id in photo_ids {
            transaction
                .query_row("SELECT 1 FROM photos WHERE id=?", [photo_id], |_| Ok(()))
                .optional()
                .map_err(mutation_error_from_sqlite)?
                .ok_or(MutationError::NotFound)?;
        }

        match mutation {
            AlbumMembershipMutation::Add {
                album_id,
                photo_ids,
            } => {
                let mut position: i64 = transaction
                    .query_row(
                        "SELECT COALESCE(MAX(position)+1,0) FROM album_members WHERE album_id=?",
                        [&album_id],
                        |row| row.get(0),
                    )
                    .map_err(mutation_error_from_sqlite)?;
                let mut added_photo_ids = Vec::new();
                let mut already_member_photo_ids = Vec::new();
                for photo_id in photo_ids {
                    let already_member = transaction
                        .query_row(
                            "SELECT 1 FROM album_members WHERE album_id=? AND photo_id=?",
                            params![album_id, photo_id],
                            |_| Ok(()),
                        )
                        .optional()
                        .map_err(mutation_error_from_sqlite)?;
                    if already_member.is_some() {
                        already_member_photo_ids.push(photo_id);
                        continue;
                    }
                    transaction
                        .execute(
                            "INSERT INTO album_members(album_id,photo_id,position) VALUES(?,?,?)",
                            params![album_id, photo_id, position],
                        )
                        .map_err(mutation_error_from_sqlite)?;
                    position = position.checked_add(1).ok_or(MutationError::Conflict)?;
                    added_photo_ids.push(photo_id);
                }
                Ok(AlbumMembershipResult {
                    album_id,
                    added_photo_ids,
                    already_member_photo_ids,
                    removed_photo_ids: Vec::new(),
                    already_absent_photo_ids: Vec::new(),
                })
            }
            AlbumMembershipMutation::RemoveAdded {
                album_id,
                photo_ids,
            } => {
                let mut removed_photo_ids = Vec::new();
                let mut already_absent_photo_ids = Vec::new();
                for photo_id in photo_ids {
                    let position = transaction
                        .query_row(
                            "SELECT position FROM album_members WHERE album_id=? AND photo_id=?",
                            params![album_id, photo_id],
                            |row| row.get::<_, i64>(0),
                        )
                        .optional()
                        .map_err(mutation_error_from_sqlite)?;
                    let Some(position) = position else {
                        already_absent_photo_ids.push(photo_id);
                        continue;
                    };
                    transaction
                        .execute(
                            "DELETE FROM album_members WHERE album_id=? AND photo_id=?",
                            params![album_id, photo_id],
                        )
                        .map_err(mutation_error_from_sqlite)?;
                    transaction
                        .execute(
                            "UPDATE album_members SET position=position-1 WHERE album_id=? AND position>?",
                            params![album_id, position],
                        )
                        .map_err(mutation_error_from_sqlite)?;
                    removed_photo_ids.push(photo_id);
                }
                Ok(AlbumMembershipResult {
                    album_id,
                    added_photo_ids: Vec::new(),
                    already_member_photo_ids: Vec::new(),
                    removed_photo_ids,
                    already_absent_photo_ids,
                })
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use crate::persistence::admission::StateDirectory;
    use crate::persistence::albums;
    use crate::persistence::test_support::*;
    use crate::persistence::{MutationError, Persistence};
    use crate::{
        ALBUM_MEMBERSHIP_BATCH_MAX, AlbumMembershipMutation, AlbumMutation, CheckedAlbumMutation,
        CheckedAlbumMutationResult, OriginalKind, PhotoStateBatchItem, PhotoStateBatchMutation,
        SelectionState,
    };
    use rusqlite::Connection;
    use rusqlite::params;
    use std::fs;

    #[tokio::test]
    async fn every_existing_album_writer_participates_in_version_invalidation() {
        let (_base, library, state, name, _path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![
                    discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0),
                    discovered("two.JPG", OriginalKind::Jpeg, 2, 2.0),
                    discovered("three.JPG", OriginalKind::Jpeg, 3, 3.0),
                ],
                Vec::new(),
            )
            .await
            .unwrap();
        let photo_ids = snapshot
            .photos
            .iter()
            .map(|photo| photo.id.clone())
            .collect::<Vec<_>>();
        let album_id = persistence
            .mutate_album(AlbumMutation::Create {
                name: "Writers".to_owned(),
            })
            .await
            .unwrap()
            .album_id;
        let version = || async {
            persistence
                .album_receiver(&album_id)
                .unwrap()
                .await
                .unwrap()
                .unwrap()
                .unwrap()
                .album_version
        };
        let mut prior = version().await;

        persistence
            .mutate_album(AlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: vec![photo_ids[0].clone()],
            })
            .await
            .unwrap();
        let after_add = version().await;
        assert_ne!(after_add, prior);
        prior = after_add;
        persistence
            .mutate_album(AlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: vec![photo_ids[0].clone()],
            })
            .await
            .unwrap();
        assert_eq!(version().await, prior);

        persistence
            .mutate_album(AlbumMutation::AddFolderMembers {
                album_id: album_id.clone(),
                photo_ids: vec![photo_ids[1].clone()],
            })
            .await
            .unwrap();
        let after_folder_add = version().await;
        assert_ne!(after_folder_add, prior);
        prior = after_folder_add;
        persistence
            .mutate_album(AlbumMutation::Reorder {
                album_id: album_id.clone(),
                photo_ids: vec![photo_ids[1].clone(), photo_ids[0].clone()],
            })
            .await
            .unwrap();
        let after_reorder = version().await;
        assert_ne!(after_reorder, prior);
        prior = after_reorder;
        persistence
            .mutate_album(AlbumMutation::RemoveMember {
                album_id: album_id.clone(),
                photo_id: photo_ids[0].clone(),
            })
            .await
            .unwrap();
        let after_remove = version().await;
        assert_ne!(after_remove, prior);
        prior = after_remove;

        persistence
            .mutate_album_membership(AlbumMembershipMutation::Add {
                album_id: album_id.clone(),
                photo_ids: vec![photo_ids[2].clone()],
            })
            .await
            .unwrap();
        let after_membership_add = version().await;
        assert_ne!(after_membership_add, prior);
        prior = after_membership_add;
        persistence
            .mutate_album_membership(AlbumMembershipMutation::RemoveAdded {
                album_id: album_id.clone(),
                photo_ids: vec![photo_ids[2].clone()],
            })
            .await
            .unwrap();
        let after_compensation = version().await;
        assert_ne!(after_compensation, prior);
        prior = after_compensation;
        persistence
            .mutate_album(AlbumMutation::SetProgress {
                album_id: album_id.clone(),
                photo_id: photo_ids[1].clone(),
            })
            .await
            .unwrap();
        assert_eq!(version().await, prior);

        let photo_version = persistence
            .photo_receiver(&photo_ids[0])
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .decision_version;
        persistence
            .mutate_photo_state_batch_receiver(PhotoStateBatchMutation {
                photos: vec![PhotoStateBatchItem {
                    photo_id: photo_ids[0].clone(),
                    expected_current: SelectionState::Unflagged,
                }],
                value: SelectionState::Picked,
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let after_batch = persistence
            .photo_receiver(&photo_ids[0])
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .decision_version;
        assert_ne!(after_batch, photo_version);
    }

    #[tokio::test]
    async fn album_summaries_and_browse_target_avoid_member_materialization() {
        let (_base, library, state, name, path) = fixture();
        let originals = vec![
            discovered("one.ARW", OriginalKind::Raw, 3, 1000.0),
            discovered("two.ARW", OriginalKind::Raw, 4, 1000.0),
        ];
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v5.sql"),
        );
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence.apply_scan(originals, Vec::new()).await.unwrap();
        let photo_one = snapshot.photos[0].id.clone();
        let photo_two = snapshot.photos[1].id.clone();
        let created = persistence
            .mutate_album(AlbumMutation::Create {
                name: "Bounded".to_owned(),
            })
            .await
            .unwrap();
        let album_id = created.album_id;
        // Empty summary before any member exists.
        let summaries = persistence
            .list_album_summaries_receiver()
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].id, album_id);
        assert_eq!(summaries[0].photo_count, 0);
        assert!(!summaries[0].has_saved_position);
        // Browse target for the empty album exists with no members.
        let target = persistence
            .album_browse_target_receiver(&album_id)
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(target.as_ref().unwrap().members.len(), 0);
        assert_eq!(target.as_ref().unwrap().saved_photo_id, None);
        persistence
            .mutate_album(AlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: vec![photo_one.clone(), photo_two.clone()],
            })
            .await
            .unwrap();
        persistence
            .mutate_album(AlbumMutation::SetProgress {
                album_id: album_id.clone(),
                photo_id: photo_two.clone(),
            })
            .await
            .unwrap();
        let summaries = persistence
            .list_album_summaries_receiver()
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(summaries[0].photo_count, 2);
        assert!(summaries[0].has_saved_position);
        let target = persistence
            .album_browse_target_receiver(&album_id)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(target.members.len(), 2);
        assert_eq!(target.members[0].photo_id, photo_one);
        assert!(target.members[0].available);
        assert_eq!(target.members[1].photo_id, photo_two);
        assert_eq!(target.saved_photo_id.as_deref(), Some(photo_two.as_str()));
        // Unknown albums report no browse target.
        assert!(
            persistence
                .album_browse_target_receiver("00000000-0000-4000-8000-00000000dead")
                .unwrap()
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn checked_album_changes_are_guarded_atomic_and_identity_bearing() {
        let (_base, library, state, name, path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![
                    discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0),
                    discovered("two.JPG", OriginalKind::Jpeg, 2, 2.0),
                    discovered("three.JPG", OriginalKind::Jpeg, 3, 3.0),
                ],
                Vec::new(),
            )
            .await
            .unwrap();
        let ids = photo_ids(&snapshot);
        let created = persistence
            .create_album_checked("  Picks  ".to_owned())
            .await
            .unwrap();
        let album_id = created.album.id.clone();
        assert_eq!(created.album.name, "Picks");
        assert_eq!(created.album.photo_count, 0);
        assert!(!created.album.has_saved_position);
        assert_eq!(
            persistence.create_album_checked("pIcKs".to_owned()).await,
            Err(albums::AlbumWriteError::NameConflict {
                name: "pIcKs".to_owned(),
                album_id: album_id.clone(),
            })
        );

        let initial_version = created.album.album_version;
        assert_eq!(
            persistence
                .mutate_album_checked(CheckedAlbumMutation::AddMembers {
                    album_id: album_id.clone(),
                    photo_ids: (0..=ALBUM_MEMBERSHIP_BATCH_MAX)
                        .map(|index| format!("photo-{index}"))
                        .collect(),
                    expected_version: initial_version.clone(),
                })
                .await,
            Err(albums::AlbumWriteError::LimitExceeded {
                limit: ALBUM_MEMBERSHIP_BATCH_MAX,
                actual: ALBUM_MEMBERSHIP_BATCH_MAX + 1,
            })
        );
        let unchanged = persistence
            .mutate_album_checked(CheckedAlbumMutation::Rename {
                album_id: album_id.clone(),
                name: "Picks".to_owned(),
                expected_version: initial_version.clone(),
            })
            .await
            .unwrap();
        let CheckedAlbumMutationResult::Renamed { album, renamed } = unchanged else {
            panic!("expected rename result");
        };
        assert!(!renamed);
        assert_eq!(album.album_version, initial_version);

        assert_eq!(
            persistence
                .mutate_album_checked(CheckedAlbumMutation::AddMembers {
                    album_id: album_id.clone(),
                    photo_ids: vec![ids[0].clone(), "missing".to_owned()],
                    expected_version: initial_version.clone(),
                })
                .await,
            Err(albums::AlbumWriteError::PhotoNotFound {
                photo_id: "missing".to_owned(),
            })
        );
        let after_refusal = persistence
            .album_receiver(&album_id)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(after_refusal.album_version, initial_version);
        assert_eq!(after_refusal.photo_count, 0);

        let added = persistence
            .mutate_album_checked(CheckedAlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: vec![ids[0].clone(), ids[1].clone()],
                expected_version: initial_version.clone(),
            })
            .await
            .unwrap();
        let CheckedAlbumMutationResult::Added {
            album,
            added_photo_ids,
            already_member_photo_ids,
        } = added
        else {
            panic!("expected add result");
        };
        assert_eq!(added_photo_ids, vec![ids[0].clone(), ids[1].clone()]);
        assert!(already_member_photo_ids.is_empty());
        assert_eq!(album.photo_count, 2);
        let added_version = album.album_version;
        assert_ne!(added_version, initial_version);

        assert_eq!(
            persistence
                .mutate_album_checked(CheckedAlbumMutation::AddMembers {
                    album_id: album_id.clone(),
                    photo_ids: vec![ids[0].clone()],
                    expected_version: initial_version,
                })
                .await,
            Err(albums::AlbumWriteError::VersionConflict {
                album_id: album_id.clone(),
                current_version: added_version.clone(),
            })
        );
        persistence
            .mutate_album(AlbumMutation::SetProgress {
                album_id: album_id.clone(),
                photo_id: ids[1].clone(),
            })
            .await
            .unwrap();

        let removed = persistence
            .mutate_album_checked(CheckedAlbumMutation::RemoveMembers {
                album_id: album_id.clone(),
                photo_ids: vec![ids[1].clone(), ids[2].clone()],
                expected_version: added_version,
            })
            .await
            .unwrap();
        let CheckedAlbumMutationResult::Removed {
            album,
            removed_photo_ids,
            already_absent_photo_ids,
            saved_photo_id,
        } = removed
        else {
            panic!("expected remove result");
        };
        assert_eq!(removed_photo_ids, vec![ids[1].clone()]);
        assert_eq!(already_absent_photo_ids, vec![ids[2].clone()]);
        assert_eq!(saved_photo_id, None);
        assert!(!album.has_saved_position);
        let removed_version = album.album_version;

        let added = persistence
            .mutate_album_checked(CheckedAlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: vec![ids[2].clone(), ids[0].clone()],
                expected_version: removed_version,
            })
            .await
            .unwrap();
        let CheckedAlbumMutationResult::Added {
            album,
            added_photo_ids,
            already_member_photo_ids,
        } = added
        else {
            panic!("expected add result");
        };
        assert_eq!(added_photo_ids, vec![ids[2].clone()]);
        assert_eq!(already_member_photo_ids, vec![ids[0].clone()]);
        let reordered = persistence
            .mutate_album_checked(CheckedAlbumMutation::Reorder {
                album_id: album_id.clone(),
                photo_ids: vec![ids[2].clone(), ids[0].clone()],
                expected_version: album.album_version,
            })
            .await
            .unwrap();
        let CheckedAlbumMutationResult::Reordered {
            album,
            ordered_photo_ids,
            reordered,
        } = reordered
        else {
            panic!("expected reorder result");
        };
        assert!(reordered);
        assert_eq!(ordered_photo_ids, vec![ids[2].clone(), ids[0].clone()]);
        let reordered_version = album.album_version;
        assert_eq!(
            persistence
                .mutate_album_checked(CheckedAlbumMutation::Reorder {
                    album_id: album_id.clone(),
                    photo_ids: vec![ids[0].clone()],
                    expected_version: reordered_version.clone(),
                })
                .await,
            Err(albums::AlbumWriteError::MembershipConflict {
                album_id: album_id.clone(),
                current_version: reordered_version.clone(),
            })
        );

        let renamed = persistence
            .mutate_album_checked(CheckedAlbumMutation::Rename {
                album_id: album_id.clone(),
                name: "Final".to_owned(),
                expected_version: reordered_version,
            })
            .await
            .unwrap();
        let CheckedAlbumMutationResult::Renamed { album, renamed } = renamed else {
            panic!("expected rename result");
        };
        assert!(renamed);
        assert_eq!(album.name, "Final");
        let observed_final_version = album.album_version;
        persistence
            .mutate_album(AlbumMutation::Rename {
                album_id: album_id.clone(),
                name: "Away".to_owned(),
            })
            .await
            .unwrap();
        persistence
            .mutate_album(AlbumMutation::Rename {
                album_id: album_id.clone(),
                name: "Final".to_owned(),
            })
            .await
            .unwrap();
        let final_version = persistence
            .album_receiver(&album_id)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .album_version;
        assert_eq!(
            persistence
                .mutate_album_checked(CheckedAlbumMutation::Rename {
                    album_id: album_id.clone(),
                    name: "Final".to_owned(),
                    expected_version: observed_final_version,
                })
                .await,
            Err(albums::AlbumWriteError::VersionConflict {
                album_id: album_id.clone(),
                current_version: final_version.clone(),
            })
        );

        let delete_target = persistence
            .create_album_checked("Delete me".to_owned())
            .await
            .unwrap()
            .album;
        assert_eq!(
            persistence
                .mutate_album_checked(CheckedAlbumMutation::Delete {
                    album_id: delete_target.id.clone(),
                    expected_version: delete_target.album_version,
                })
                .await
                .unwrap(),
            CheckedAlbumMutationResult::Deleted {
                album_id: delete_target.id.clone(),
            }
        );
        assert!(
            persistence
                .album_receiver(&delete_target.id)
                .unwrap()
                .await
                .unwrap()
                .unwrap()
                .is_none()
        );

        let sidecar = path.with_file_name("library.sqlite-wal");
        fs::write(&sidecar, b"blocked").unwrap();
        assert_eq!(
            persistence
                .mutate_album_checked(CheckedAlbumMutation::Rename {
                    album_id: album_id.clone(),
                    name: "Blocked".to_owned(),
                    expected_version: final_version.clone(),
                })
                .await,
            Err(albums::AlbumWriteError::Persistence)
        );
        assert_eq!(
            persistence
                .album_receiver(&album_id)
                .unwrap()
                .await
                .unwrap()
                .unwrap()
                .unwrap()
                .album_version,
            final_version
        );
    }

    #[tokio::test]
    async fn checked_reorder_refuses_a_large_album_without_mutation() {
        let (_base, library, state, name, _path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let originals = (0..=ALBUM_MEMBERSHIP_BATCH_MAX)
            .map(|index| {
                discovered(
                    &format!("photo-{index:03}.JPG"),
                    OriginalKind::Jpeg,
                    index as u64 + 1,
                    index as f64 + 1.0,
                )
            })
            .collect();
        let snapshot = persistence.apply_scan(originals, Vec::new()).await.unwrap();
        let ids = photo_ids(&snapshot);
        let album = persistence
            .create_album_checked("Large".to_owned())
            .await
            .unwrap()
            .album;
        persistence
            .mutate_album(AlbumMutation::AddFolderMembers {
                album_id: album.id.clone(),
                photo_ids: ids.clone(),
            })
            .await
            .unwrap();
        let current = persistence
            .album_receiver(&album.id)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            persistence
                .mutate_album_checked(CheckedAlbumMutation::Reorder {
                    album_id: album.id.clone(),
                    photo_ids: ids[..ALBUM_MEMBERSHIP_BATCH_MAX].to_vec(),
                    expected_version: current.album_version.clone(),
                })
                .await,
            Err(albums::AlbumWriteError::LimitExceeded {
                limit: ALBUM_MEMBERSHIP_BATCH_MAX,
                actual: ALBUM_MEMBERSHIP_BATCH_MAX + 1,
            })
        );
        let after = persistence
            .album_receiver(&album.id)
            .unwrap()
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(after.album_version, current.album_version);
        assert_eq!(after.photo_count, ids.len());
        assert_eq!(
            persistence.list_albums().await.unwrap()[0]
                .members
                .iter()
                .map(|member| member.photo_id.clone())
                .collect::<Vec<_>>(),
            ids
        );
    }

    #[tokio::test]
    async fn adding_an_existing_album_member_is_idempotent() {
        let (_base, library, state, name, path) = fixture();
        let originals = vec![
            discovered("one.ARW", OriginalKind::Raw, 3, 1000.0),
            discovered("two.ARW", OriginalKind::Raw, 4, 1000.0),
            discovered("three.ARW", OriginalKind::Raw, 5, 1000.0),
        ];
        seed(
            &path,
            include_str!("../../../../compatibility/sqlite/schema-v5.sql"),
        );
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence.apply_scan(originals, Vec::new()).await.unwrap();
        let photo_one = snapshot.photos[0].id.clone();
        let photo_two = snapshot.photos[1].id.clone();
        let photo_three = snapshot.photos[2].id.clone();
        let created = persistence
            .mutate_album(AlbumMutation::Create {
                name: "Idempotent".to_owned(),
            })
            .await
            .unwrap();
        let album_id = created.album_id;
        persistence
            .mutate_album(AlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: vec![photo_one.clone(), photo_two.clone()],
            })
            .await
            .unwrap();
        // Re-adding a persisted member succeeds and keeps its position.
        persistence
            .mutate_album(AlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: vec![photo_two.clone()],
            })
            .await
            .unwrap();
        let album = &persistence.list_albums().await.unwrap()[0];
        assert_eq!(album.members.len(), 2);
        assert_eq!(album.members[0].photo_id, photo_one);
        assert_eq!(album.members[0].position, 0);
        assert_eq!(album.members[1].photo_id, photo_two);
        assert_eq!(album.members[1].position, 1);
        // A mixed request appends only the new member after existing ones.
        persistence
            .mutate_album(AlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: vec![photo_two, photo_three.clone()],
            })
            .await
            .unwrap();
        let album = &persistence.list_albums().await.unwrap()[0];
        assert_eq!(album.members.len(), 3);
        assert_eq!(album.members[2].photo_id, photo_three);
        assert_eq!(album.members[2].position, 2);
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn membership_batch_reports_identities_and_compensates_idempotently() {
        let (_base, library, state, name, _path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![
                    discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0),
                    discovered("two.JPG", OriginalKind::Jpeg, 2, 2.0),
                    discovered("three.JPG", OriginalKind::Jpeg, 3, 3.0),
                ],
                Vec::new(),
            )
            .await
            .unwrap();
        let ids = photo_ids(&snapshot);
        let album_id = persistence
            .mutate_album(AlbumMutation::Create {
                name: "Compensation".to_owned(),
            })
            .await
            .unwrap()
            .album_id;
        persistence
            .mutate_album(AlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: vec![ids[0].clone(), ids[1].clone()],
            })
            .await
            .unwrap();
        persistence
            .mutate_album(AlbumMutation::SetProgress {
                album_id: album_id.clone(),
                photo_id: ids[1].clone(),
            })
            .await
            .unwrap();

        let add = persistence
            .mutate_album_membership(AlbumMembershipMutation::Add {
                album_id: album_id.clone(),
                photo_ids: vec![ids[1].clone(), ids[2].clone()],
            })
            .await
            .unwrap();
        assert_eq!(add.added_photo_ids, vec![ids[2].clone()]);
        assert_eq!(add.already_member_photo_ids, vec![ids[1].clone()]);
        let album = &persistence.list_albums().await.unwrap()[0];
        assert_eq!(
            album
                .members
                .iter()
                .map(|member| member.photo_id.clone())
                .collect::<Vec<_>>(),
            vec![ids[0].clone(), ids[1].clone(), ids[2].clone()]
        );
        assert_eq!(album.members[0].position, 0);
        assert_eq!(album.members[1].position, 1);
        assert_eq!(album.members[2].position, 2);
        assert_eq!(
            album.last_reviewed_photo_id.as_deref(),
            Some(ids[1].as_str())
        );

        let removed = persistence
            .mutate_album_membership(AlbumMembershipMutation::RemoveAdded {
                album_id: album_id.clone(),
                photo_ids: vec![ids[1].clone()],
            })
            .await
            .unwrap();
        assert_eq!(removed.removed_photo_ids, vec![ids[1].clone()]);
        assert!(removed.already_absent_photo_ids.is_empty());
        let album = &persistence.list_albums().await.unwrap()[0];
        assert_eq!(
            album
                .members
                .iter()
                .map(|member| member.photo_id.clone())
                .collect::<Vec<_>>(),
            vec![ids[0].clone(), ids[2].clone()]
        );
        assert_eq!(album.members[0].position, 0);
        assert_eq!(album.members[1].position, 1);
        assert_eq!(album.last_reviewed_photo_id, None);

        let repeated = persistence
            .mutate_album_membership(AlbumMembershipMutation::RemoveAdded {
                album_id: album_id.clone(),
                photo_ids: vec![ids[1].clone()],
            })
            .await
            .unwrap();
        assert!(repeated.removed_photo_ids.is_empty());
        assert_eq!(repeated.already_absent_photo_ids, vec![ids[1].clone()]);
        let removed_tail = persistence
            .mutate_album_membership(AlbumMembershipMutation::RemoveAdded {
                album_id: album_id.clone(),
                photo_ids: vec![ids[2].clone()],
            })
            .await
            .unwrap();
        assert_eq!(removed_tail.removed_photo_ids, vec![ids[2].clone()]);
        assert!(removed_tail.already_absent_photo_ids.is_empty());
        let album = &persistence.list_albums().await.unwrap()[0];
        assert_eq!(album.members.len(), 1);
        assert_eq!(album.last_reviewed_photo_id, None);
        assert_eq!(
            persistence
                .mutate_album_membership(AlbumMembershipMutation::RemoveAdded {
                    album_id: album_id.clone(),
                    photo_ids: vec!["00000000-0000-4000-8000-00000000dead".to_owned()],
                })
                .await,
            Err(MutationError::NotFound)
        );
        assert_eq!(persistence.list_albums().await.unwrap()[0].members.len(), 1);
        assert_eq!(
            persistence
                .mutate_album_membership(AlbumMembershipMutation::Add {
                    album_id,
                    photo_ids: vec![ids[0].clone(), ids[0].clone()],
                })
                .await,
            Err(MutationError::Invalid)
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn folder_members_mutation_is_atomic_and_reports_idempotent_counts() {
        let (_base, library, state, name, _path) = fixture();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![
                    discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0),
                    discovered("two.JPG", OriginalKind::Jpeg, 2, 2.0),
                    discovered("three.JPG", OriginalKind::Jpeg, 3, 3.0),
                ],
                Vec::new(),
            )
            .await
            .unwrap();
        let ids = photo_ids(&snapshot);
        let album_id = persistence
            .mutate_album(AlbumMutation::Create {
                name: "Folder".to_owned(),
            })
            .await
            .unwrap()
            .album_id;

        let first = persistence
            .mutate_album(AlbumMutation::AddFolderMembers {
                album_id: album_id.clone(),
                photo_ids: vec![ids[0].clone(), ids[1].clone()],
            })
            .await
            .unwrap();
        assert_eq!(first.added_count, 2);
        assert_eq!(first.already_member_count, 0);

        let repeated = persistence
            .mutate_album(AlbumMutation::AddFolderMembers {
                album_id: album_id.clone(),
                photo_ids: vec![ids[1].clone(), ids[2].clone(), ids[0].clone()],
            })
            .await
            .unwrap();
        assert_eq!(repeated.added_count, 1);
        assert_eq!(repeated.already_member_count, 2);
        let members = &persistence.list_albums().await.unwrap()[0].members;
        assert_eq!(
            members
                .iter()
                .map(|member| member.photo_id.clone())
                .collect::<Vec<_>>(),
            vec![ids[0].clone(), ids[1].clone(), ids[2].clone()]
        );
        assert_eq!(
            persistence
                .mutate_album(AlbumMutation::AddFolderMembers {
                    album_id,
                    photo_ids: vec![ids[0].clone(), ids[0].clone()],
                })
                .await,
            Err(MutationError::Conflict)
        );
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn album_name_uniqueness_folds_ascii_case_only() {
        let (_base, library, state, name, _path) = fixture();
        fs::write(library.canonical_path().join("one.JPG"), b"bytes").unwrap();
        let photo = discovered("one.JPG", OriginalKind::Jpeg, 4, 1000.0);
        let persistence = Persistence::open(
            state,
            name.clone(),
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        persistence
            .apply_scan(vec![photo], Vec::new())
            .await
            .unwrap();

        // Exact duplicates conflict in every script, including caseless ones.
        persistence
            .mutate_album(AlbumMutation::Create {
                name: "春节".to_owned(),
            })
            .await
            .unwrap();
        assert!(matches!(
            persistence
                .mutate_album(AlbumMutation::Create {
                    name: "春节".to_owned(),
                })
                .await,
            Err(MutationError::Conflict)
        ));

        // ASCII letter case folds: Trip and trip are the same Album name.
        persistence
            .mutate_album(AlbumMutation::Create {
                name: "Trip".to_owned(),
            })
            .await
            .unwrap();
        assert!(matches!(
            persistence
                .mutate_album(AlbumMutation::Create {
                    name: "trip".to_owned(),
                })
                .await,
            Err(MutationError::Conflict)
        ));

        // Documented boundary: folding applies to ASCII letters only. A name
        // whose case difference is carried by a non-ASCII letter (É vs é)
        // does not fold and stays a distinct Album name.
        persistence
            .mutate_album(AlbumMutation::Create {
                name: "Éclair".to_owned(),
            })
            .await
            .unwrap();
        persistence
            .mutate_album(AlbumMutation::Create {
                name: "éclair".to_owned(),
            })
            .await
            .unwrap();
        let names = persistence
            .list_albums()
            .await
            .unwrap()
            .into_iter()
            .map(|album| album.name)
            .collect::<Vec<_>>();
        assert!(names.contains(&"Éclair".to_owned()));
        assert!(names.contains(&"éclair".to_owned()));
    }

    #[tokio::test]
    async fn album_crud_normalizes_names_and_preserves_rows_across_restart() {
        let (_base, library, state, name, path) = fixture();
        let original_path = library.canonical_path().join("one.JPG");
        fs::write(&original_path, b"original bytes").unwrap();
        let original_bytes = fs::read(&original_path).unwrap();
        let photo = discovered("one.JPG", OriginalKind::Jpeg, 4, 1000.0);
        let persistence = Persistence::open(
            state,
            name.clone(),
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(vec![photo], Vec::new())
            .await
            .unwrap();
        let photo_id = snapshot.photos[0].id.clone();
        let created = persistence
            .mutate_album(AlbumMutation::Create {
                name: "  Picks  ".to_owned(),
            })
            .await
            .unwrap();
        let album_id = created.album_id;
        persistence
            .mutate_album(AlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: vec![photo_id.clone()],
            })
            .await
            .unwrap();
        assert_eq!(
            persistence
                .mutate_album(AlbumMutation::Create {
                    name: "pIcKs".to_owned(),
                })
                .await,
            Err(MutationError::Conflict)
        );
        persistence
            .mutate_album(AlbumMutation::Rename {
                album_id: album_id.clone(),
                name: " Renamed ".to_owned(),
            })
            .await
            .unwrap();
        let albums = persistence.list_albums().await.unwrap();
        assert_eq!(albums[0].name, "Renamed");
        persistence.shutdown().unwrap();

        let state = StateDirectory::open_or_create(&library, path.parent().unwrap()).unwrap();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let albums = persistence.list_albums().await.unwrap();
        assert_eq!(albums[0].id, album_id);
        assert_eq!(albums[0].members[0].photo_id, photo_id);
        persistence
            .mutate_album(AlbumMutation::Delete { album_id })
            .await
            .unwrap();
        assert!(persistence.list_albums().await.unwrap().is_empty());
        let connection = Connection::open(path).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT count(*) FROM photos", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(fs::read(&original_path).unwrap(), original_bytes);
        persistence.shutdown().unwrap();
    }

    #[tokio::test]
    async fn membership_batches_are_atomic_and_order_operations_are_dense() {
        let (_base, library, state, name, path) = fixture();
        let persistence = Persistence::open(
            state,
            name.clone(),
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        let snapshot = persistence
            .apply_scan(
                vec![
                    discovered("one.JPG", OriginalKind::Jpeg, 1, 1.0),
                    discovered("two.JPG", OriginalKind::Jpeg, 2, 2.0),
                    discovered("three.JPG", OriginalKind::Jpeg, 3, 3.0),
                ],
                Vec::new(),
            )
            .await
            .unwrap();
        let ids = photo_ids(&snapshot);
        let album_id = persistence
            .mutate_album(AlbumMutation::Create {
                name: "Order".to_owned(),
            })
            .await
            .unwrap()
            .album_id;
        assert_eq!(
            persistence
                .mutate_album(AlbumMutation::AddMembers {
                    album_id: album_id.clone(),
                    photo_ids: vec![ids[0].clone(), ids[0].clone()],
                })
                .await,
            Err(MutationError::Conflict)
        );
        assert_eq!(persistence.list_albums().await.unwrap()[0].members.len(), 0);
        assert_eq!(
            persistence
                .mutate_album(AlbumMutation::AddMembers {
                    album_id: album_id.clone(),
                    photo_ids: vec![ids[0].clone(), "unknown".to_owned()],
                })
                .await,
            Err(MutationError::NotFound)
        );
        assert_eq!(persistence.list_albums().await.unwrap()[0].members.len(), 0);
        assert_eq!(
            persistence
                .mutate_album(AlbumMutation::AddMembers {
                    album_id: album_id.clone(),
                    photo_ids: (0..=100).map(|index| format!("unknown-{index}")).collect(),
                })
                .await,
            Err(MutationError::Conflict)
        );
        persistence
            .mutate_album(AlbumMutation::AddMembers {
                album_id: album_id.clone(),
                photo_ids: ids.clone(),
            })
            .await
            .unwrap();
        assert_eq!(
            persistence
                .mutate_album(AlbumMutation::Reorder {
                    album_id: album_id.clone(),
                    photo_ids: vec![ids[2].clone(), ids[0].clone(), ids[1].clone()],
                })
                .await
                .unwrap()
                .album_id,
            album_id
        );
        assert_eq!(
            persistence.list_albums().await.unwrap()[0]
                .members
                .iter()
                .map(|member| member.photo_id.clone())
                .collect::<Vec<_>>(),
            vec![ids[2].clone(), ids[0].clone(), ids[1].clone()]
        );
        assert_eq!(
            persistence
                .mutate_album(AlbumMutation::Reorder {
                    album_id: album_id.clone(),
                    photo_ids: vec![ids[0].clone(), ids[0].clone(), ids[1].clone()],
                })
                .await,
            Err(MutationError::Conflict)
        );
        persistence
            .mutate_album(AlbumMutation::RemoveMember {
                album_id: album_id.clone(),
                photo_id: ids[0].clone(),
            })
            .await
            .unwrap();
        let members = &persistence.list_albums().await.unwrap()[0].members;
        assert_eq!(
            members
                .iter()
                .map(|member| member.position)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
        persistence.shutdown().unwrap();

        let connection = Connection::open(&path).unwrap();
        connection
            .execute(
                "UPDATE album_members SET position=9 WHERE album_id=? AND photo_id=?",
                params![album_id, ids[1]],
            )
            .unwrap();
        drop(connection);
        let state = StateDirectory::open_or_create(&library, path.parent().unwrap()).unwrap();
        let persistence = Persistence::open(
            state,
            name,
            library.canonical_path().to_string_lossy().into_owned(),
        )
        .unwrap();
        assert_eq!(
            persistence
                .mutate_album(AlbumMutation::Reorder {
                    album_id: album_id.clone(),
                    photo_ids: vec![ids[1].clone(), ids[2].clone()],
                })
                .await,
            Err(MutationError::Conflict)
        );
        persistence.shutdown().unwrap();
    }
}
