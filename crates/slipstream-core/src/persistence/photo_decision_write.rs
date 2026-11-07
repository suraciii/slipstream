//! Photo decision write contract: the write error surface and request
//! normalization shared by the Photo decision SQL module and the persistence
//! owner.

use super::owner::{MutationError, PersistenceError};
use crate::{
    CheckedPhotoDecisionMutation, MAXIMUM_PHOTO_RATING, PhotoStateBatchMutation, PhotoStateField,
    PhotoStateMutation, PhotoStateValue, SelectionState,
};
use std::fmt;

/// Detailed refusal or storage outcome for checked Photo decision batches.
/// Conflicting and missing Photos are per-item domain results, so only
/// request validation and storage failures appear here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PhotoDecisionWriteError {
    Invalid,
    LimitExceeded { limit: usize, actual: usize },
    Persistence,
    Saturated,
    Closed,
}

impl fmt::Display for PhotoDecisionWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid => formatter.write_str("Photo decision write request is not valid"),
            Self::LimitExceeded { .. } => {
                formatter.write_str("Photo decision write exceeds its limit")
            }
            Self::Persistence => formatter.write_str("Photo decision write could not be persisted"),
            Self::Saturated => formatter.write_str("SQLite persistence queue is saturated"),
            Self::Closed => formatter.write_str("SQLite persistence is closed"),
        }
    }
}

impl std::error::Error for PhotoDecisionWriteError {}

pub(super) fn photo_decision_write_error_from_persistence(
    error: PersistenceError,
) -> PhotoDecisionWriteError {
    match error {
        PersistenceError::Saturated => PhotoDecisionWriteError::Saturated,
        PersistenceError::Closed => PhotoDecisionWriteError::Closed,
        _ => PhotoDecisionWriteError::Persistence,
    }
}

pub(super) fn photo_decision_write_error_from_mutation(
    error: MutationError,
) -> PhotoDecisionWriteError {
    match error {
        MutationError::Saturated => PhotoDecisionWriteError::Saturated,
        MutationError::Closed => PhotoDecisionWriteError::Closed,
        _ => PhotoDecisionWriteError::Persistence,
    }
}

pub(super) fn normalize_checked_photo_decision_mutation(
    mutation: CheckedPhotoDecisionMutation,
) -> Result<CheckedPhotoDecisionMutation, PhotoDecisionWriteError> {
    if std::mem::discriminant(&mutation.value)
        != match mutation.field {
            PhotoStateField::SelectionState => {
                std::mem::discriminant(&PhotoStateValue::Selection(SelectionState::Unflagged))
            }
            PhotoStateField::Rating => std::mem::discriminant(&PhotoStateValue::Rating(0)),
        }
        || matches!(mutation.value, PhotoStateValue::Rating(value) if value > MAXIMUM_PHOTO_RATING)
    {
        return Err(PhotoDecisionWriteError::Invalid);
    }

    if mutation.photos.len() > crate::PHOTO_STATE_BATCH_MAX {
        return Err(PhotoDecisionWriteError::LimitExceeded {
            limit: crate::PHOTO_STATE_BATCH_MAX,
            actual: mutation.photos.len(),
        });
    }
    if mutation.photos.is_empty()
        || mutation
            .photos
            .iter()
            .any(|photo| photo.photo_id.trim().is_empty() || photo.expected_version.is_empty())
        || mutation
            .photos
            .iter()
            .map(|photo| &photo.photo_id)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != mutation.photos.len()
    {
        return Err(PhotoDecisionWriteError::Invalid);
    }
    Ok(mutation)
}

pub(super) fn validate_photo_state_mutation(
    mutation: &PhotoStateMutation,
) -> Result<(), MutationError> {
    if mutation.expected_current.is_some_and(|expected| {
        std::mem::discriminant(&expected) != std::mem::discriminant(&mutation.value)
    }) {
        return Err(MutationError::Conflict);
    }
    match (mutation.field, mutation.value) {
        (PhotoStateField::SelectionState, PhotoStateValue::Selection(_))
        | (PhotoStateField::Rating, PhotoStateValue::Rating(_)) => Ok(()),
        _ => Err(MutationError::Conflict),
    }
}

pub(super) fn validate_photo_state_batch_mutation(
    mutation: &PhotoStateBatchMutation,
) -> Result<(), MutationError> {
    if mutation.value == SelectionState::Unflagged
        || mutation.photos.is_empty()
        || mutation.photos.len() > crate::PHOTO_STATE_BATCH_MAX
        || mutation
            .photos
            .iter()
            .map(|photo| &photo.photo_id)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != mutation.photos.len()
        || mutation
            .photos
            .iter()
            .any(|photo| photo.photo_id.is_empty())
    {
        return Err(MutationError::Invalid);
    }
    Ok(())
}
