//! Album write contract: the write error surface and mutation normalization
//! shared by the Album SQL module and the persistence owner.

use super::owner::{MutationError, PersistenceError};
use crate::{
    ALBUM_MEMBERSHIP_BATCH_MAX, AlbumMembershipMutation, AlbumMutation, CheckedAlbumMutation,
    MAXIMUM_FOLDER_ALBUM_PHOTOS,
};
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
