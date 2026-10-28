use super::*;
use crate::{
    ExplicitPhotoRemovalMutation, ExplicitPhotoRestoreMutation, ExplicitPhotoRestoreResult,
    PhotoOperationRemainder, PhotoRemovalMutation, PhotoRemovalResult, PhotoRestoration,
    PhotoRestorationResult, RemovedPhotoRecord,
};

impl Library {
    /// Removes one reviewed result's Photos from the Library in one
    /// transaction. Each requested Photo reports exactly one outcome, and the
    /// operation id groups what Undo restores.
    pub async fn remove_photos(
        &self,
        mutation: PhotoRemovalMutation,
    ) -> Result<PhotoRemovalResult, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.remove_photos_receiver(mutation)
        }
        .map_err(LibraryError::from)?;
        receive
            .await
            .unwrap_or(Err(MutationError::Persistence))
            .map_err(Into::into)
    }
    /// Removes explicit, caller-reviewed Photos with decision evidence.
    pub async fn remove_photos_explicit(
        &self,
        mutation: ExplicitPhotoRemovalMutation,
    ) -> Result<PhotoRemovalResult, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.remove_photos_explicit_receiver(mutation)
        }
        .map_err(LibraryError::from)?;
        receive
            .await
            .unwrap_or(Err(MutationError::Persistence))
            .map_err(Into::into)
    }
    /// Reads the durable historical result of one removal operation without
    /// changing Library state.
    pub async fn photo_removal_operation(
        &self,
        operation_id: String,
    ) -> Result<Option<PhotoRemovalResult>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .photo_removal_operation_receiver(operation_id)
        }
        .map_err(LibraryError::from)?;
        receive
            .await
            .unwrap_or(Err(MutationError::Persistence))
            .map_err(Into::into)
    }
    /// Restores explicit, caller-reviewed Photos and retains the
    /// operation result for replay and read-only outcome inspection.
    pub async fn restore_photos_explicit(
        &self,
        mutation: ExplicitPhotoRestoreMutation,
    ) -> Result<ExplicitPhotoRestoreResult, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.restore_photos_explicit_receiver(mutation)
        }
        .map_err(LibraryError::from)?;
        receive
            .await
            .unwrap_or(Err(MutationError::Persistence))
            .map_err(Into::into)
    }

    /// Restores an admission-time Trash selection and retains its result.
    pub async fn restore_trash(
        &self,
        operation_id: String,
        selection: PermanentDeletionSelection,
    ) -> Result<ExplicitPhotoRestoreResult, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .restore_trash_receiver(operation_id, selection)
        }
        .map_err(LibraryError::from)?;
        receive
            .await
            .unwrap_or(Err(MutationError::Persistence))
            .map_err(Into::into)
    }

    /// Reads the durable historical result of one explicit restore attempt
    /// without changing Library state.
    pub async fn photo_restore_operation(
        &self,
        operation_id: String,
    ) -> Result<Option<ExplicitPhotoRestoreResult>, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence
                .photo_restore_operation_receiver(operation_id)
        }
        .map_err(LibraryError::from)?;
        receive
            .await
            .unwrap_or(Err(MutationError::Persistence))
            .map_err(Into::into)
    }

    /// Restores every Photo one operation still owns, or an explicit set,
    /// through the same compare-and-set rule.
    pub async fn restore_photos(
        &self,
        restoration: PhotoRestoration,
    ) -> Result<PhotoRestorationResult, LibraryError> {
        let receive = {
            let _admission = self.admit()?;
            self.persistence.restore_photos_receiver(restoration)
        }
        .map_err(LibraryError::from)?;
        receive
            .await
            .unwrap_or(Err(MutationError::Persistence))
            .map_err(Into::into)
    }

    /// One bounded page of removed Photos, newest removal first.
    pub async fn removed_photos(
        &self,
        start: usize,
        limit: usize,
    ) -> Result<
        (
            Vec<RemovedPhotoRecord>,
            usize,
            Option<PhotoOperationRemainder>,
        ),
        LibraryError,
    > {
        self.admitted(|persistence| persistence.removed_photos_receiver(start, limit))
            .await
    }
}
