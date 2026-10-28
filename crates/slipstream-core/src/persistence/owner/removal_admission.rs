use super::*;
use crate::{ExplicitPhotoRemovalMutation, ExplicitPhotoRestoreMutation, PhotoRemovalMutation};

impl Persistence {
    pub(crate) fn remove_photos_receiver(
        &self,
        mutation: PhotoRemovalMutation,
    ) -> Result<oneshot::Receiver<Result<PhotoRemovalResult, MutationError>>, MutationError> {
        if mutation.photo_ids.is_empty()
            || mutation.operation_id.is_empty()
            || mutation.photo_ids.iter().collect::<HashSet<_>>().len() != mutation.photo_ids.len()
        {
            return Err(MutationError::Invalid);
        }
        let (send, receive) = oneshot::channel();
        self.submit(Command::RemovePhotos(
            removal::PhotoRemovalRequest::Browse(mutation),
            send,
        ))
        .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }
    pub(crate) fn remove_photos_explicit_receiver(
        &self,
        mutation: ExplicitPhotoRemovalMutation,
    ) -> Result<oneshot::Receiver<Result<PhotoRemovalResult, MutationError>>, MutationError> {
        if mutation.operation_id.is_empty()
            || mutation.photos.is_empty()
            || mutation.photos.len() > crate::PHOTO_REMOVAL_MAX
            || mutation
                .photos
                .iter()
                .map(|photo| &photo.photo_id)
                .collect::<HashSet<_>>()
                .len()
                != mutation.photos.len()
            || mutation.photos.iter().any(|photo| {
                photo.photo_id.is_empty()
                    || photo.expected_decision_version.is_empty()
                    || photo.expected_selection_state != SelectionState::Rejected
                    || photo.expected_removed_at_ms.is_some_and(|value| value < 0)
            })
        {
            return Err(MutationError::Invalid);
        }
        let (send, receive) = oneshot::channel();
        self.submit(Command::RemovePhotos(
            removal::PhotoRemovalRequest::Explicit(mutation),
            send,
        ))
        .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }
    pub(crate) fn photo_removal_operation_receiver(
        &self,
        operation_id: String,
    ) -> Result<oneshot::Receiver<Result<Option<PhotoRemovalResult>, MutationError>>, MutationError>
    {
        if operation_id.is_empty() {
            return Err(MutationError::Invalid);
        }
        let (send, receive) = oneshot::channel();
        self.submit(Command::ReadPhotoRemovalOperation {
            operation_id,
            reply: send,
        })
        .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }

    pub(crate) fn restore_photos_explicit_receiver(
        &self,
        mutation: ExplicitPhotoRestoreMutation,
    ) -> Result<oneshot::Receiver<Result<ExplicitPhotoRestoreResult, MutationError>>, MutationError>
    {
        if mutation.operation_id.is_empty()
            || mutation.photos.is_empty()
            || mutation.photos.len() > crate::PHOTO_REMOVAL_MAX
            || mutation
                .photos
                .iter()
                .map(|photo| &photo.photo_id)
                .collect::<HashSet<_>>()
                .len()
                != mutation.photos.len()
            || mutation
                .photos
                .iter()
                .any(|photo| photo.photo_id.is_empty() || photo.removed_at_ms < 0)
        {
            return Err(MutationError::Invalid);
        }
        let (send, receive) = oneshot::channel();
        self.submit(Command::RestorePhotosExplicit(
            removal::PhotoRestoreRequest::Explicit(mutation),
            send,
        ))
        .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }

    pub(crate) fn restore_trash_receiver(
        &self,
        operation_id: String,
        selection: PermanentDeletionSelection,
    ) -> Result<oneshot::Receiver<Result<ExplicitPhotoRestoreResult, MutationError>>, MutationError>
    {
        let (ids, valid_count) = match &selection {
            PermanentDeletionSelection::Photos(ids) => (
                ids,
                !ids.is_empty() && ids.len() <= crate::PHOTO_REMOVAL_MAX,
            ),
            PermanentDeletionSelection::All { exclude_photo_ids } => (
                exclude_photo_ids,
                exclude_photo_ids.len() <= crate::PERMANENT_DELETION_MAX,
            ),
        };
        if operation_id.is_empty()
            || !valid_count
            || ids.iter().any(|id| id.is_empty())
            || ids.iter().collect::<HashSet<_>>().len() != ids.len()
        {
            return Err(MutationError::Invalid);
        }
        let (send, receive) = oneshot::channel();
        self.submit(Command::RestorePhotosExplicit(
            removal::PhotoRestoreRequest::Trash {
                operation_id,
                selection,
            },
            send,
        ))
        .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }

    pub(crate) fn photo_restore_operation_receiver(
        &self,
        operation_id: String,
    ) -> Result<
        oneshot::Receiver<Result<Option<ExplicitPhotoRestoreResult>, MutationError>>,
        MutationError,
    > {
        if operation_id.is_empty() {
            return Err(MutationError::Invalid);
        }
        let (send, receive) = oneshot::channel();
        self.submit(Command::ReadPhotoRestoreOperation {
            operation_id,
            reply: send,
        })
        .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }

    pub(crate) fn restore_photos_receiver(
        &self,
        restoration: PhotoRestoration,
    ) -> Result<oneshot::Receiver<Result<PhotoRestorationResult, MutationError>>, MutationError>
    {
        if let PhotoRestoration::Photos(markers) = &restoration
            && markers.is_empty()
        {
            return Err(MutationError::Invalid);
        }
        let (send, receive) = oneshot::channel();
        self.submit(Command::RestorePhotos(restoration, send))
            .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }

    pub(crate) fn removed_photos_receiver(
        &self,
        start: usize,
        limit: usize,
    ) -> Result<RemovedPhotoPageReceiver, PersistenceError> {
        self.submit_persistence_receiver(|reply| Command::RemovedPhotos {
            start,
            limit,
            reply,
        })
    }

    pub(crate) fn trash_candidates_receiver(
        &self,
        selection: PermanentDeletionSelection,
    ) -> Result<oneshot::Receiver<removal::TrashCandidateResult>, MutationError> {
        let valid = match &selection {
            PermanentDeletionSelection::Photos(photo_ids) => {
                !photo_ids.is_empty()
                    && photo_ids.len() <= crate::PERMANENT_DELETION_MAX
                    && photo_ids.iter().all(|id| !id.is_empty())
                    && photo_ids.iter().collect::<HashSet<_>>().len() == photo_ids.len()
            }
            PermanentDeletionSelection::All { exclude_photo_ids } => {
                exclude_photo_ids.len() <= crate::PERMANENT_DELETION_MAX
                    && exclude_photo_ids.iter().all(|id| !id.is_empty())
                    && exclude_photo_ids.iter().collect::<HashSet<_>>().len()
                        == exclude_photo_ids.len()
            }
        };
        if !valid {
            return Err(MutationError::Invalid);
        }
        let (send, receive) = oneshot::channel();
        self.submit(Command::TrashCandidates {
            selection,
            reply: send,
        })
        .map_err(mutation::mutation_error_from_persistence)?;
        Ok(receive)
    }
}
